//! Server-driven enemy state, AI, and floor-pack spawning.
//!
//! Enemies share the kinematic substrate with players (`Kinematic`)
//! so the same wall-aware integrator handles their motion.
//!
//! ## Layout
//!
//! Per-role behaviour lives in its own sibling module so a new
//! enemy archetype is a single new file:
//!
//! * [`brute`] — chase + melee + flank + A* fallback. Powers
//!   brutes, elites, and the boss-fallback.
//! * [`stalker`] — approach → wind-up → dash → recover loop.
//! * [`caster`] — kite ring + lateral strafe + LOS-gated bolts.
//! * [`boss`] — 3-phase HP-gated multi-ability scheduler.
//!
//! This file owns the shared surface: the [`ServerEnemy`]
//! component, the wire-stable enums ([`role`], [`enemy_anim`],
//! [`elite_mod`]), the cross-role helpers (target resolve,
//! threat decay, wind-up timer, separation), the [`tick_ai`]
//! dispatcher, and the floor-pack / boss-summon spawn paths.
//!
//! Adding a new role: write a `tick` fn + `Spec` struct in a
//! new sibling file, append a `pub mod` + a dispatch arm in
//! [`tick_ai`], and register the wire byte under [`role`] +
//! `MonsterRole`.

use glam::Vec3;
use hecs::Entity;
use rift_dungeon::{Floor, FloorConfig};
use rift_game::abilities::AbilityWireId;
use rift_game::kinematic::{self, loco, Kinematic};
use rift_game::monsters::MonsterRole;
use rift_net::NetId;

pub mod boss;
pub mod brute;
pub mod caster;
pub mod stalker;

pub use boss::BossState;

/// Wire animation ids. Clients map these to clip names locally.
pub mod enemy_anim {
    pub const IDLE: u8 = 0;
    pub const WALK: u8 = 1;
    pub const ATTACK: u8 = 2;
    /// Corpse pose. Set in [`super::super::snapshot::build`] for
    /// any enemy whose `dying_remaining > 0.0` so the client
    /// engine plays the `Death` clip and the per-enemy fade
    /// tick runs.
    pub const DEATH: u8 = 3;
}

/// Elite affix bitfield — picked once at spawn (one or two
/// per elite, deterministic from the floor seed). Each bit is
/// independent and stacks; combining `JUGGERNAUT | SWIFT`
/// produces a stagger-immune fast-mover, etc. Wire-stable —
/// new modifiers append at the next free bit, never reorder.
///
/// Effects:
/// * `JUGGERNAUT` — immune to stagger; +20 % hp at spawn.
/// * `SWIFT` — +25 % move speed.
/// * `EXPLODER` — on death, spawn a small AoE damage zone
///   (see [`ELITE_EXPLODER_*`] tuning).
/// * `THORNS` — reflects [`ELITE_THORNS_FRAC`] of every hit
///   back to the attacker as raw player damage.
/// * `VAMPIRIC` — heals the elite for [`ELITE_VAMPIRE_FRAC`]
///   of every melee hit it lands. Combined with `JUGGERNAUT`
///   this is a real wall.
pub mod elite_mod {
    pub const JUGGERNAUT: u8 = 1 << 0;
    pub const SWIFT: u8 = 1 << 1;
    pub const EXPLODER: u8 = 1 << 2;
    pub const THORNS: u8 = 1 << 3;
    pub const VAMPIRIC: u8 = 1 << 4;
}

// ---- Shared tuning constants ---------------------------------

/// How long a killed enemy hangs around (HP=0, AI off, collision
/// off, snapshot still includes the row) so the client gets to
/// play its `Death` clip + corpse fade. Slightly longer than the
/// engine's own `Dying.duration` for skinned monsters (1.4 s) so
/// the server doesn't yank the row out from under the animation.
pub const DEATH_FADE_DUR: f32 = 1.6;

/// Aggro pickup range — within this distance an *unengaged*
/// enemy will lock onto the closest player. Tuned roughly to one
/// room-and-a-bit so a player walking down a corridor doesn't
/// instantly aggro every monster in the next two rooms the
/// moment a rift floor finishes loading.
pub const AGGRO_RANGE: f32 = 9.0;
/// Leash drop range — once an enemy is engaged with a target it
/// will keep chasing until the target is at least this far away,
/// at which point the enemy resets to idle (and may re-pick a
/// different nearby player). Larger than [`AGGRO_RANGE`] so a
/// brief duck behind a wall doesn't make the pack forget you,
/// but small enough that fleeing across the dungeon does.
pub const LEASH_RANGE: f32 = 28.0;

/// Aggro-spread radius. When an enemy takes damage from a
/// player, every other enemy within this distance of the
/// victim also locks onto the same attacker (provided they
/// don't already have a target). Models a "scream / signal"
/// reaction — wake your packmates when you get hit. Tuned
/// shorter than [`AGGRO_RANGE`] so it complements rather than
/// replaces line-of-sight pickup; pulling roughly one-room
/// radius of neighbours into the fight without alerting a
/// whole wing of the dungeon. Wall-gated: the line from victim
/// to packmate must clear the floor's tile grid, so enemies in
/// adjacent rooms / behind walls do not get pulled into the
/// fight by a fight they cannot see.
pub const AGGRO_SPREAD_RADIUS: f32 = 7.0;
/// Maximum delay (s) before a packmate fully responds to an
/// aggro spread. Distance-scaled: an enemy right next to the
/// victim reacts almost instantly, one at the spread radius
/// takes the full delay. Reads as a wave of heads turning
/// instead of an instantaneous pack-pivot.
pub const AGGRO_SPREAD_MAX_DELAY: f32 = 0.6;

/// Hit-flinch duration, in seconds. Set on
/// [`ServerEnemy::stagger_remaining`] when a stagger-eligible
/// hit lands; while non-zero the AI tick freezes velocity and
/// skips role logic. Brief on purpose — staggers should
/// punctuate combat rhythm, not pause it.
pub const STAGGER_DUR: f32 = 0.18;
/// Damage fraction of `hp_max` above which a non-crit hit
/// triggers a stagger. Tuned so chip damage from DoTs / weak
/// abilities doesn't lock enemies in place, but a single big
/// swing from a charged ability or weapon-special does.
pub const STAGGER_THRESHOLD: f32 = 0.12;

/// Threat decay constant (e-folding time, seconds). Each tick
/// every threat entry is multiplied by `exp(-dt / TAU)`, so
/// after `TAU` seconds without further damage a stale
/// attacker drops to ~37 % weight. Tuned long enough that
/// brief dodges don't drop a player off the threat list, but
/// short enough that a fleeing player gets de-prioritised.
pub const THREAT_DECAY_TAU: f32 = 8.0;
/// Multiplier on the leading attacker's threat below which the
/// AI will switch targets mid-fight. Bigger than 1.0 to add
/// hysteresis: a tied threat doesn't ping-pong locks every
/// tick, but a clearly-louder attacker steals aggro.
pub const THREAT_SWITCH_HYSTERESIS: f32 = 1.4;

/// How long after committing a swing the attack-anim flag stays
/// true on the wire — clients use it to play the attack clip.
pub const ATTACK_ANIM_DUR: f32 = 0.45;

/// Sphere radius used for projectile↔enemy XZ collision.
/// Tuned to roughly match the shrunken visual scales in
/// [`rift_game::monsters::MonsterRole::scale`] so projectiles
/// don't whiff visibly past small models.
pub const ENEMY_HIT_RADIUS: f32 = 0.45;

/// Personal-space radius used for separation steering between
/// enemies in the same pack. Below this distance an enemy steers
/// away from its neighbour so packs don't melt into a single dot
/// when they all converge on the player. Tightened along with
/// the visual shrink so dense packs still have enough room to
/// breathe but read as a swarm, not a parade.
pub const SEPARATION_RADIUS: f32 = 0.9;
/// Strength of the separation push relative to walking speed.
/// Tuned so two enemies brushing shoulders just barely shove each
/// other apart without breaking forward locomotion.
pub const SEPARATION_STRENGTH: f32 = 1.1;

/// Cadence (seconds) for refreshing the cached `ServerEnemy::
/// los_blocked_cached` flag. The role tick reads the cache
/// instead of calling `Floor::line_of_sight` every frame —
/// at swarm sizes (floor 40+) the grid sampler otherwise
/// dominates the AI budget. Short enough that a player ducking
/// behind cover still flips the AI to A*-pathing within ~one
/// dodge window.
pub const LOS_RECHECK_INTERVAL: f32 = 0.15;

/// Damage of the EXPLODER death AoE, as a multiplier on the
/// floor's enemy_damage_mult (so the pop scales with floor
/// difficulty the same way as a bolt).
pub const ELITE_EXPLODER_DAMAGE: f32 = 18.0;
/// Radius of the EXPLODER death AoE.
pub const ELITE_EXPLODER_RADIUS: f32 = 3.0;
/// Fraction of incoming damage THORNS elites reflect back at
/// the attacker.
pub const ELITE_THORNS_FRAC: f32 = 0.20;
/// Fraction of melee damage VAMPIRIC elites heal themselves
/// for.
pub const ELITE_VAMPIRE_FRAC: f32 = 0.10;
/// HP multiplier added on top of the elite base for JUGGERNAUT.
pub const ELITE_JUGGERNAUT_HP_MULT: f32 = 1.20;
/// Speed multiplier added on top of the elite base for SWIFT.
pub const ELITE_SWIFT_SPEED_MULT: f32 = 1.25;

// ---- Wind-up + AI phase --------------------------------------

/// Wind-up *kind* — distinguishes the three structurally
/// identical timer-and-freeze attack telegraphs (brute swing,
/// caster bolt, stalker dash). Each kind maps 1:1 to a wire
/// [`rift_net::messages::telegraph_kind`] byte and to the
/// resolve action the role tick runs when the timer expires.
///
/// Centralising the kind here means the wind-up timer ticking
/// + telegraph emission live in one place ([`enter_windup`] /
/// [`tick_windup`]); only the *resolve* step stays per-role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindupKind {
    /// Brute / elite melee swing — resolves to a melee damage
    /// row if the target is still in range.
    BruteMelee,
    /// Caster bolt — resolves to an `EnemyCast::Resolve` for
    /// the caster's [`caster::Spec::ability_id`].
    CasterBolt,
    /// Stalker dash — resolves to a phase swap into
    /// [`AiPhase::StalkerDash`] with the snapshotted aim.
    StalkerDash,
}

impl WindupKind {
    /// Wire byte for the matching [`WorldEvent::EnemyTelegraph`]
    /// cue.
    pub fn telegraph_byte(self) -> u8 {
        use rift_net::messages::telegraph_kind;
        match self {
            WindupKind::BruteMelee => telegraph_kind::MELEE_WINDUP,
            WindupKind::CasterBolt => telegraph_kind::RANGED_WINDUP,
            WindupKind::StalkerDash => telegraph_kind::DASH_WINDUP,
        }
    }
}

/// Per-enemy AI phase. Most roles only ever live in
/// [`AiPhase::Idle`]; the stalker dash cycle uses the
/// stalker-specific variants and any windup attack (brute
/// swing, caster bolt, stalker dash telegraph) uses
/// [`AiPhase::Windup`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AiPhase {
    /// Default state — apply role's baseline movement + attack.
    Idle,
    /// A wind-up attack is in progress. `kind` selects the
    /// resolve dispatch the role tick runs when `remaining`
    /// reaches zero. Centralised so timer-tick + telegraph
    /// emission don't get duplicated three times across the
    /// per-role tick functions.
    Windup { kind: WindupKind, remaining: f32 },
    /// Stalker is closing the distance toward its target. No
    /// timer; promoted to a `Windup { StalkerDash, .. }` once
    /// inside trigger range.
    StalkerApproach,
    /// Stalker is mid-dash toward the locked-in dash direction.
    /// First field is the remaining timer, second is the unit
    /// dash direction snapshotted at wind-up start (so the player
    /// can side-step the lunge).
    StalkerDash {
        remaining: f32,
        dir: Vec3,
        hit_landed: bool,
    },
    /// Post-dash retreat / cooldown. Counts down to zero, then
    /// flips back to `StalkerApproach`.
    StalkerRecover(f32),
}

impl Default for AiPhase {
    fn default() -> Self {
        Self::Idle
    }
}

// ---- ServerEnemy component -----------------------------------

/// Component bundle for one server-driven enemy.
#[derive(Clone, Debug)]
pub struct ServerEnemy {
    pub net_id: NetId,
    pub role: rift_game::monsters::MonsterRole,
    pub k: Kinematic,
    pub speed: f32,
    pub hp_max: f32,
    pub hp: f32,
    pub attack_cooldown: f32,
    pub attack_anim_remaining: f32,
    /// Seconds left in the death-fade window. `0.0` for live
    /// enemies. While `> 0.0`: AI is suppressed, velocity is
    /// zeroed, projectile/AoE/channel collision skips the row,
    /// snapshot ships the corpse with `flags::DEAD` and
    /// `enemy_anim::DEATH` so the client plays the death clip.
    /// On reaching `0.0`, [`tick_dying`] despawns the entity.
    pub dying_remaining: f32,
    /// Per-role behaviour state. Idle for brutes/elites; drives
    /// the stalker dash cycle and caster wind-up timers.
    pub ai_phase: AiPhase,
    /// Currently-engaged target, set when the enemy first picks
    /// up aggro and held until the target leaves
    /// [`LEASH_RANGE`] (or dies / despawns). Lets us run a tight
    /// [`AGGRO_RANGE`] for fresh pickups without having packs
    /// drop chase the moment the player ducks past the limit.
    pub target_lock: Option<Entity>,
    /// Crit roll chance for outgoing damage (0..1). Default
    /// `0.0` — enemies don't crit unless something tunes them
    /// to. Plumbed end-to-end so tuning a boss / elite to crit
    /// is one literal in the spawn block, not a refactor.
    pub crit_chance: f32,
    /// Crit damage multiplier added on top of `1.0` when the
    /// roll succeeds (e.g. `0.5` ⇒ +50 %).
    pub crit_damage: f32,
    /// Hit-flinch timer. Set to [`STAGGER_DUR`] when a hit
    /// crosses the [`STAGGER_THRESHOLD`] HP fraction (or any
    /// crit lands). While `> 0.0` the AI tick zeroes velocity
    /// and skips role logic, producing a brief "interrupted"
    /// pose. Tuned short so chip damage doesn't lock enemies in
    /// place; only meaty hits read as a real stagger. Crit hits
    /// always stagger to make crits feel weighty regardless of
    /// the damage roll. Suppressed on
    /// [`elite_mod::JUGGERNAUT`] enemies.
    pub stagger_remaining: f32,
    /// Pending aggro signal queued by [`notify_attacked`].
    /// `Some((attacker, delay))` — the enemy promotes
    /// `target_lock` to `attacker` once `delay` ticks down to
    /// zero. Modelled as a per-enemy reaction time so a pack-
    /// alert reads as a wave of heads turning rather than an
    /// instantaneous mind-meld pivot. Cleared on promotion.
    pub pending_aggro: Option<(Entity, f32)>,
    /// Per-attacker accumulated threat. Increments by raw
    /// damage taken on hit, decays multiplicatively each tick
    /// via [`THREAT_DECAY_TAU`]. The AI prefers the
    /// highest-threat attacker over the nearest player — so
    /// healers / casters naturally pull aggro the way they do
    /// in any tab-target RPG. Empty for fresh enemies; the
    /// first hit creates the entry.
    pub threat: std::collections::HashMap<Entity, f32>,
    /// Bitfield of [`elite_mod`] flags. `0` for normal enemies
    /// and bosses. Picked once at spawn from the floor seed for
    /// elite-role rows so the same floor regen always rolls the
    /// same affixes.
    pub elite_mods: u8,
    /// Deterministic flanking-slot seed. The brute / elite
    /// chase code projects an angular offset around the target
    /// from this so packs surround instead of dogpiling the
    /// front. Picked at spawn (currently from `net_id`) and
    /// never changes.
    pub flank_slot: u8,
    /// Cached A* path from the enemy's position to its current
    /// target, in tile coordinates. Recomputed every
    /// [`brute::PATH_RECOMPUTE_INTERVAL`] seconds (or sooner if
    /// the target moves to a different tile). Empty when the
    /// enemy has straight-line LOS to the target — the
    /// cheaper line-walk path takes over in that case.
    pub path: Vec<(i32, i32)>,
    /// Tile the path was computed against. `None` invalidates
    /// the cache and forces a recompute next tick.
    pub path_target_tile: Option<(i32, i32)>,
    /// Seconds until the next path recompute is allowed. Caps
    /// A* invocations to a few per second per enemy.
    pub path_recompute_in: f32,
    /// Cached "is line-of-sight to current target blocked?"
    /// flag. The role tick reads this to choose bee-line vs
    /// A* approach. Updated only when [`Self::los_recheck_in`]
    /// expires so we don't call the (relatively expensive)
    /// `Floor::line_of_sight` grid sampler on every enemy on
    /// every tick — at floor 40 swarm sizes that single check
    /// otherwise dominates the AI tick budget.
    pub los_blocked_cached: bool,
    /// Seconds until the next LOS recheck. Jittered at spawn
    /// (via `net_id`) so a freshly-spawned pack doesn't
    /// LOS-check synchronously on the same tick.
    pub los_recheck_in: f32,
}

impl ServerEnemy {
    /// `true` once the enemy has been killed — used by every
    /// damage subsystem to avoid hitting the same corpse twice and
    /// by the AI tick to skip dead bodies.
    pub fn is_dying(&self) -> bool {
        self.dying_remaining > 0.0
    }
}

/// One enemy ability cast emitted by an AI tick. The AI either
/// emits a `Start` (wind-up beginning, drives the client
/// telegraph visual) or a `Resolve` (wind-up expired, runs
/// authoritative effects). The caller (`Sim::step`) translates
/// these into `WorldEvent::AbilityCast` events + dispatches
/// resolves through the kernel pipeline
/// (`super::super::ability::submit` +
/// `super::super::ability::dispatch`).
///
/// Unifying every enemy attack onto this single channel means
/// adding a new attack is a registry-entry append + one
/// selection arm in `boss::tick` — no new bespoke `EnemyShot` /
/// `summons` plumbing.
#[derive(Clone, Copy, Debug)]
pub enum EnemyCast {
    /// Wind-up just started. `dir_x` / `dir_y` pack ability-
    /// specific scalars onto the wire `AbilityCast.dir` field
    /// (e.g. slam radius + wind-up duration).
    Start {
        owner: NetId,
        ability_id: AbilityWireId,
        origin: Vec3,
        target: Vec3,
        dir_x: f32,
        dir_y: f32,
    },
    /// Wind-up expired. Server runs the ability's effect (spawn
    /// projectiles / damage / summon).
    Resolve {
        owner: NetId,
        /// Casting enemy's `MonsterRole::to_wire_byte()`.
        /// Threaded through dispatch so spawned projectiles /
        /// zones / channels carry the kind for the receiving
        /// player's TAKEN-tab attribution.
        attacker_kind: u8,
        ability_id: AbilityWireId,
        origin: Vec3,
        aim: Vec3,
        damage_mult: f32,
        /// Caster's crit chance (0..1) at resolve time —
        /// snapshot here rather than re-read from the
        /// `ServerEnemy` so dispatch is source-agnostic.
        crit_chance: f32,
        crit_damage: f32,
        /// Optional ability-specific scalar override applied at
        /// resolve. For [`AbilityKind::DelayedAoe`] this is the
        /// effective radius (m), letting the AI bake in
        /// per-cast scaling (e.g. boss slam enrage multiplier)
        /// without having to author multiple registry entries.
        /// `0.0` falls back to the registry's authored value.
        param_a: f32,
    },
}

/// Output bundle from one AI tick. The dispatcher walks every
/// enemy, applies role-specific steering / attack logic, and
/// returns the queued damage + ability-cast rows for the caller
/// to apply once the world borrow ends.
#[derive(Default)]
pub struct AiOutcome {
    pub melee_damage: Vec<super::combat_ctx::PlayerHit>,
    /// Ability cast events (start-of-windup + resolve). Lifted
    /// into `WorldEvent::AbilityCast` and / or dispatched
    /// through the kernel pipeline by `Sim::step`.
    pub casts: Vec<EnemyCast>,
    /// Lightweight wire events emitted by the AI tick — currently
    /// just [`WorldEvent::EnemyTelegraph`] cues for SFX. Lifted
    /// straight into `Sim::pending_events` after the AI tick.
    pub events: Vec<rift_net::messages::WorldEvent>,
    /// Heal-back rows for elite [`elite_mod::VAMPIRIC`] enemies.
    /// `(enemy_entity, amount)` — the caller adds `amount` to
    /// the enemy's `hp` after the AI borrow ends. Routed
    /// through `AiOutcome` rather than mutating `hp` inline so
    /// the post-tick consumer can clamp against `hp_max` and
    /// emit a `Heal` event in one place.
    pub vampiric_heals: Vec<(Entity, f32)>,
}

// ---- AI tick dispatcher --------------------------------------

/// One AI tick for every enemy in the world.
///
/// Dispatches to a per-role tick ([`brute::tick`] /
/// [`stalker::tick`] / [`caster::tick`] / [`boss::tick`]) so
/// each archetype lives in its own self-contained file. All
/// roles share two common behaviours wired up here:
///
/// 1. **Target selection** — nearest player within
///    [`AGGRO_RANGE`] becomes the AI target (gated by LOS).
///    Threat hysteresis can re-target an in-leash player who
///    dealt enough damage.
/// 2. **Separation steering** — every enemy nudges away from
///    in-pack neighbours within [`SEPARATION_RADIUS`] so big
///    packs don't melt into one entity-overlapping dot when they
///    converge on a player.
pub fn tick_ai(
    world: &mut hecs::World,
    floor: &Floor,
    player_positions: &[(Entity, Vec3)],
    damage_mult: f32,
    dt: f32,
) -> AiOutcome {
    // Snapshot every live enemy's (net_id, position) and bucket
    // them into a coarse spatial grid keyed on `SEPARATION_RADIUS`
    // cells. The separation pass then queries only the 3×3 cell
    // neighbourhood around each enemy instead of the full live-
    // enemy list — O(N) per tick instead of O(N²). At floor 40
    // swarm sizes (hundreds of mobs) this is the difference
    // between a CPU-bound 30 Hz tick and a smooth one.
    let neighbours: Vec<(NetId, Vec3)> = world
        .query::<&ServerEnemy>()
        .iter()
        .filter(|(_, en)| !en.is_dying())
        .map(|(_, en)| (en.net_id, en.k.position))
        .collect();
    let grid = rift_math::spatial::SpatialGrid::build(&neighbours, SEPARATION_RADIUS, |&(_, p)| p);

    let mut outcome = AiOutcome::default();
    for (_e, (en, stack, boss_state)) in world.query_mut::<(
        &mut ServerEnemy,
        Option<&super::effect::EffectStack>,
        Option<&mut BossState>,
    )>() {
        // Skip dying enemies — their AI is frozen until the
        // death-fade timer expires and they're despawned.
        if en.is_dying() {
            en.k.velocity = Vec3::ZERO;
            continue;
        }
        // Hit-flinch: if a recent hit set `stagger_remaining`,
        // tick it down and freeze the AI for this frame. Skips
        // role logic and separation so a staggered enemy reads
        // as briefly stunned. Juggernauts can't ever enter
        // this branch — `stagger_remaining` is gated on the
        // `JUGGERNAUT` mod at write time in
        // `apply_hits_to_enemies`.
        if en.stagger_remaining > 0.0 {
            en.stagger_remaining = (en.stagger_remaining - dt).max(0.0);
            en.k.velocity = Vec3::ZERO;
            en.k.locomotion = loco::IDLE;
            // Decay threat too even while staggered so a
            // stagger-locked mob doesn't accumulate a stale
            // target list.
            decay_threat(en, dt);
            continue;
        }
        // Threat decay — `e^(-dt/TAU)` per entry. Cheap; runs
        // on every live enemy whether engaged or not so the
        // table doesn't grow unbounded after a kite.
        decay_threat(en, dt);
        // Promote any queued pack-alert that's run its
        // reaction-delay clock down. Bypasses the normal
        // visual-aggro path so packs alerted by a hit on a
        // sibling kick into the fight even if the alerted
        // enemy never sees the player itself.
        if let Some((attacker, delay)) = en.pending_aggro {
            let next = delay - dt;
            if next <= 0.0 {
                en.pending_aggro = None;
                if en.target_lock.is_none() {
                    en.target_lock = Some(attacker);
                }
            } else {
                en.pending_aggro = Some((attacker, next));
            }
        }
        // Apply speed-altering debuffs (Slow, Chill, ...).
        let speed_mult = stack.map(|s| s.move_speed_mult()).unwrap_or(1.0);
        // Tick cooldowns shared by every role.
        if en.attack_cooldown > 0.0 {
            en.attack_cooldown = (en.attack_cooldown - dt).max(0.0);
        }
        if en.attack_anim_remaining > 0.0 {
            en.attack_anim_remaining = (en.attack_anim_remaining - dt).max(0.0);
        }
        if en.path_recompute_in > 0.0 {
            en.path_recompute_in = (en.path_recompute_in - dt).max(0.0);
        }
        if en.los_recheck_in > 0.0 {
            en.los_recheck_in = (en.los_recheck_in - dt).max(0.0);
        }
        // Find or refresh the engaged target. Two-phase logic:
        // honour an existing lock until it leaves leash range,
        // otherwise pick a fresh nearest within AGGRO_RANGE
        // gated by LOS. Threat hysteresis can re-target.
        let target = resolve_target(en, floor, player_positions);

        // Per-role steering + attack. Adding a new role is one
        // arm here + a new sibling module file.
        match en.role {
            MonsterRole::Stalker => stalker::tick(
                en,
                &stalker::SPEC,
                floor,
                target,
                speed_mult,
                damage_mult,
                dt,
                &mut outcome,
            ),
            MonsterRole::Caster => caster::tick(
                en,
                &caster::SPEC,
                floor,
                target,
                speed_mult,
                damage_mult,
                dt,
                &mut outcome,
            ),
            MonsterRole::Boss => {
                if let Some(b) = boss_state {
                    boss::tick(
                        en,
                        b,
                        target,
                        player_positions,
                        speed_mult,
                        damage_mult,
                        dt,
                        &mut outcome,
                    );
                } else {
                    // Boss without its `BossState` companion —
                    // fall back to a brute-shaped melee so the
                    // fight stays functional even if the
                    // component slot ever fails to attach.
                    brute::tick(
                        en,
                        &brute::BOSS_MELEE_SPEC,
                        floor,
                        target,
                        speed_mult,
                        damage_mult,
                        dt,
                        &mut outcome,
                    );
                }
            }
            // Brute, Elite, and unknowns share `brute::tick`.
            _ => brute::tick(
                en,
                &brute::SPEC,
                floor,
                target,
                speed_mult,
                damage_mult,
                dt,
                &mut outcome,
            ),
        }

        // Separation: shove away from any neighbour inside
        // SEPARATION_RADIUS so packs spread out instead of
        // stacking. Skipped for the boss — there's only ever
        // one of him and the body is huge, so neighbour pushes
        // would just jitter him off his attack mark.
        if en.role != MonsterRole::Boss {
            let self_id = en.net_id;
            let push = rift_math::spatial::separation_push(
                &grid,
                en.k.position,
                SEPARATION_RADIUS,
                &neighbours,
                |&(_, p)| p,
                |&(nid, _)| nid == self_id,
            );
            if push.length_squared() > 1.0e-6 {
                // Scale by base speed so the shove feels uniform
                // across slow / fast roles. Applied additively so
                // forward locomotion still wins when it's set; in
                // pure-Idle states the push is what unjams the clump.
                en.k.velocity += push * en.speed * SEPARATION_STRENGTH * speed_mult;
            }
        }
    }
    outcome
}

// ---- Cross-role helpers --------------------------------------

/// Resolve the active target for `en`, honouring the leash:
/// keep `target_lock` while it's within [`LEASH_RANGE`], else
/// pick a fresh nearest within [`AGGRO_RANGE`] (and update the
/// lock). Returns `None` and clears the lock when no eligible
/// target exists.
///
/// The lock can also be *stolen* mid-fight by a player whose
/// accumulated [`ServerEnemy::threat`] exceeds the locked
/// player's by [`THREAT_SWITCH_HYSTERESIS`]× — so a back-line
/// caster nuking the pack pulls aggro off a tank-y front-liner
/// the same way they would in a tab-target RPG. Hysteresis
/// keeps the lock from ping-ponging between near-equal
/// attackers.
fn resolve_target(
    en: &mut ServerEnemy,
    floor: &Floor,
    players: &[(Entity, Vec3)],
) -> Option<(Entity, Vec3, f32)> {
    // 1. Honour an existing lock as long as that player is
    //    still around and within leash distance. We don't
    //    re-check LOS for an already-locked target — once an
    //    enemy is engaged it'll chase until the leash drops,
    //    which matches what players expect when they kite
    //    around a pillar mid-fight.
    if let Some(locked) = en.target_lock {
        if let Some(&(pe, pp)) = players.iter().find(|(e, _)| *e == locked) {
            let dx = pp.x - en.k.position.x;
            let dz = pp.z - en.k.position.z;
            let d2 = dx * dx + dz * dz;
            if d2 <= LEASH_RANGE * LEASH_RANGE {
                // Threat steal check: any other in-leash player
                // whose threat outweighs the locked player by
                // the hysteresis multiplier wins the lock.
                let locked_threat = en.threat.get(&locked).copied().unwrap_or(0.0);
                let mut best_threat = locked_threat;
                let mut best_target: Option<(Entity, Vec3, f32)> = Some((pe, pp, d2));
                for (cand_e, cand_p) in players {
                    if *cand_e == locked {
                        continue;
                    }
                    let cdx = cand_p.x - en.k.position.x;
                    let cdz = cand_p.z - en.k.position.z;
                    let cd2 = cdx * cdx + cdz * cdz;
                    if cd2 > LEASH_RANGE * LEASH_RANGE {
                        continue;
                    }
                    let t = en.threat.get(cand_e).copied().unwrap_or(0.0);
                    if t > best_threat * THREAT_SWITCH_HYSTERESIS {
                        best_threat = t;
                        best_target = Some((*cand_e, *cand_p, cd2));
                    }
                }
                if let Some((te, tp, td2)) = best_target {
                    if te != locked {
                        en.target_lock = Some(te);
                    }
                    return Some((te, tp, td2));
                }
            }
        }
        // Player gone or out of leash — drop the lock and
        // fall through to the fresh-pickup path.
        en.target_lock = None;
    }

    // 2. Fresh aggro: nearest *visible* player within
    //    `AGGRO_RANGE`. LOS gate prevents aggro through walls.
    let picked = nearest_visible_player(en.k.position, floor, players);
    if let Some((pe, _, _)) = picked {
        en.target_lock = Some(pe);
    }
    picked
}

/// Multiplicative threat decay applied every AI tick. Iterates
/// the table once, scales each entry by `e^(-dt / TAU)`, and
/// drops entries that fall below an epsilon so the map doesn't
/// keep zombie keys for players that left the floor.
fn decay_threat(en: &mut ServerEnemy, dt: f32) {
    if en.threat.is_empty() {
        return;
    }
    let scale = (-dt / THREAT_DECAY_TAU).exp();
    en.threat.retain(|_, v| {
        *v *= scale;
        *v > 0.05
    });
}

/// Find the nearest entry in `players` within [`AGGRO_RANGE`] of
/// `pos` that has a clear line of sight against the tile grid.
/// Returns `None` if every player is out of range or hidden.
fn nearest_visible_player(
    pos: Vec3,
    floor: &Floor,
    players: &[(Entity, Vec3)],
) -> Option<(Entity, Vec3, f32)> {
    let mut best: Option<(Entity, Vec3, f32)> = None;
    for (pe, pp) in players {
        let dx = pp.x - pos.x;
        let dz = pp.z - pos.z;
        let d2 = dx * dx + dz * dz;
        if d2 > AGGRO_RANGE * AGGRO_RANGE {
            continue;
        }
        if best.map_or(false, |(_, _, bd2)| d2 >= bd2) {
            continue;
        }
        // Cheapest test (range) ran first; LOS is the most
        // expensive (tile-grid sampling), only run on the
        // candidate that would actually win.
        if !floor.line_of_sight(pos, *pp) {
            continue;
        }
        best = Some((*pe, *pp, d2));
    }
    best
}

/// React to a player attack on `victim`. Forces the victim to
/// retaliate against `attacker` immediately (overriding any
/// prior target — getting shot trumps everything), and queues
/// a delayed pack-alert on every other live enemy within
/// [`AGGRO_SPREAD_RADIUS`].
///
/// The spread uses [`ServerEnemy::pending_aggro`] with a
/// distance-scaled delay (up to [`AGGRO_SPREAD_MAX_DELAY`]) so
/// nearby packmates react instantly while the edge of the
/// radius takes a beat to wake up — visually a wave of heads
/// turning toward the victim. Already-engaged enemies are
/// skipped so we don't yank a pack off the player they're
/// chasing.
///
/// Wall-gated: spread is dropped for any packmate whose
/// straight-line path from the victim is broken by a wall, so
/// enemies in adjacent rooms or behind closed corridors do not
/// get pulled into a fight they cannot see. Visual
/// `AGGRO_RANGE` aggro also respects LOS via
/// [`nearest_visible_player`].
pub fn notify_attacked(world: &mut hecs::World, floor: &Floor, victim: Entity, attacker: Entity) {
    // 1. Force-aggro the victim onto the attacker. Bypasses the
    //    `pending_aggro` queue — the victim *knows* who hit it.
    let victim_pos = match world.get::<&mut ServerEnemy>(victim) {
        Ok(mut en) => {
            if en.is_dying() {
                return;
            }
            en.target_lock = Some(attacker);
            en.pending_aggro = None;
            en.k.position
        }
        Err(_) => return,
    };

    // 2. Spread to packmates: queue a `pending_aggro` on every
    //    live, currently-unengaged enemy within radius. Delay
    //    scales linearly with distance so the closest packmates
    //    react first.
    let r2 = AGGRO_SPREAD_RADIUS * AGGRO_SPREAD_RADIUS;
    for (e, en) in world.query_mut::<&mut ServerEnemy>() {
        if e == victim || en.is_dying() || en.target_lock.is_some() {
            continue;
        }
        // Skip if there's already a queued alert — first signal
        // wins, second signals would just reset the timer.
        if en.pending_aggro.is_some() {
            continue;
        }
        let dx = en.k.position.x - victim_pos.x;
        let dz = en.k.position.z - victim_pos.z;
        let d2 = dx * dx + dz * dz;
        if d2 > r2 {
            continue;
        }
        // Wall LOS gate: a victim's shout doesn't reach a
        // packmate that's on the far side of a wall, even if
        // they're inside the radius. Keeps adjacent-room
        // enemies asleep until the player crosses their
        // sight line.
        if !floor.line_of_sight(victim_pos, en.k.position) {
            continue;
        }
        let frac = (d2 / r2).sqrt(); // 0 at victim, 1 at edge
        let delay = AGGRO_SPREAD_MAX_DELAY * frac;
        en.pending_aggro = Some((attacker, delay));
    }
}

/// Sum repulsion vectors from every neighbour inside
/// [`SEPARATION_RADIUS`]. Each push is scaled by `(R - d) / R` so
/// neighbours that are touching produce the strongest shove and
/// neighbours just inside the boundary contribute almost nothing.
/// Refresh the cached LOS-blocked flag if its timer has expired,
/// otherwise return the cached value. Centralises the
/// `Floor::line_of_sight` rate-limit so every role tick gets the
/// same throttling for free. See [`LOS_RECHECK_INTERVAL`] for the
/// cadence rationale.
pub(super) fn cached_los_blocked(en: &mut ServerEnemy, floor: &Floor, target_pos: Vec3) -> bool {
    if en.los_recheck_in <= 0.0 {
        en.los_blocked_cached = !floor.line_of_sight(en.k.position, target_pos);
        en.los_recheck_in = LOS_RECHECK_INTERVAL;
    }
    en.los_blocked_cached
}

/// Begin a wind-up attack: freeze the enemy by setting the
/// phase, arm `attack_anim_remaining` so the snapshot reports
/// it as attacking for the right duration, and emit the
/// telegraph cue exactly once.
///
/// Centralised so all three wind-up entry points (brute swing,
/// caster bolt, stalker dash) use the same emit-once contract
/// — without this, the telegraph event was duplicated three
/// times across the per-role tick functions and risked
/// drifting out of sync.
pub(crate) fn enter_windup(
    en: &mut ServerEnemy,
    kind: WindupKind,
    duration: f32,
    outcome: &mut AiOutcome,
) {
    use rift_net::messages::WorldEvent;
    en.ai_phase = AiPhase::Windup {
        kind,
        remaining: duration,
    };
    en.attack_anim_remaining = duration;
    outcome.events.push(WorldEvent::EnemyTelegraph {
        source: en.net_id,
        kind: kind.telegraph_byte(),
        position: en.k.position.to_array(),
    });
}

/// Tick the [`AiPhase::Windup`] timer. Returns `Some(kind)` on
/// the frame the wind-up expires (caller dispatches the
/// resolve), `None` while still counting down or if the enemy
/// isn't in a wind-up phase. Velocity is zeroed while
/// counting down so the freeze reads consistently.
///
/// The phase is reset to [`AiPhase::Idle`] on expiry — the
/// caller may immediately swap it back into a follow-up phase
/// (e.g. stalker enters [`AiPhase::StalkerDash`] after its
/// wind-up) before any other code observes it.
pub(crate) fn tick_windup(en: &mut ServerEnemy, dt: f32) -> Option<WindupKind> {
    let AiPhase::Windup { kind, remaining } = en.ai_phase else {
        return None;
    };
    let next = remaining - dt;
    en.k.velocity = Vec3::ZERO;
    en.k.locomotion = loco::IDLE;
    if next <= 0.0 {
        en.ai_phase = AiPhase::Idle;
        Some(kind)
    } else {
        en.ai_phase = AiPhase::Windup {
            kind,
            remaining: next,
        };
        None
    }
}

// ---- Spawn / lifecycle ---------------------------------------

/// Spawn one ad-hoc enemy at `pos` with the given role byte and
/// HP multiplier (relative to floor base HP). Used by the boss
/// summon path; mirrors the construction in [`spawn_for_floor`]
/// but with caller-provided HP scaling.
pub fn spawn_summon(
    world: &mut hecs::World,
    pos: Vec3,
    role: MonsterRole,
    hp_mult: f32,
    floor_index: u32,
    next_enemy_net_id: &mut u32,
) {
    let cfg = FloorConfig::for_floor(floor_index);
    // Per-role spawn stats live on `MonsterRole::stats()` so a
    // new role only needs an entry there.
    let role_stats = role.stats();
    // Caller-provided `hp_mult` overrides the role's spawn HP
    // multiplier for summons so the boss can scale brute pack
    // size to its own floor curve.
    let hp = cfg.enemy_health * hp_mult;
    let speed = cfg.enemy_speed * role_stats.speed_mult;
    let net_id = NetId(*next_enemy_net_id);
    *next_enemy_net_id = next_enemy_net_id.wrapping_add(1).max(1);
    let enemy = ServerEnemy {
        net_id,
        role,
        k: Kinematic {
            position: Vec3::new(pos.x, 0.0, pos.z),
            velocity: Vec3::ZERO,
            yaw: 0.0,
            aim_yaw: 0.0,
            locomotion: loco::IDLE,
            vy: 0.0,
            airborne: false,
            ..Default::default()
        },
        speed,
        hp_max: hp,
        hp,
        attack_cooldown: 0.0,
        attack_anim_remaining: 0.0,
        dying_remaining: 0.0,
        ai_phase: AiPhase::default(),
        target_lock: None,
        crit_chance: 0.0,
        crit_damage: 0.0,
        stagger_remaining: 0.0,
        pending_aggro: None,
        threat: std::collections::HashMap::new(),
        elite_mods: 0,
        flank_slot: (net_id.0 % brute::FLANK_SLOTS as u32) as u8,
        path: Vec::new(),
        path_target_tile: None,
        path_recompute_in: 0.0,
        los_blocked_cached: false,
        // Jitter the first LOS check across enemies so a freshly-
        // spawned pack doesn't all hit `line_of_sight` on the same
        // tick. Spread over the recheck interval.
        los_recheck_in: ((net_id.0 % 13) as f32) * (LOS_RECHECK_INTERVAL / 13.0),
    };
    world.spawn((enemy, super::effect::EffectStack::default()));
}

/// Integrate every enemy's velocity against the floor's wall grid.
pub fn integrate_motion(world: &mut hecs::World, floor: &Floor, dt: f32) {
    for (_e, en) in world.query_mut::<&mut ServerEnemy>() {
        if en.is_dying() {
            continue;
        }
        kinematic::integrate(&mut en.k, floor, dt);
    }
}

/// Snapshot every enemy's `(entity, position, net_id, hit_radius)`
/// into a Vec — used by the projectile/AoE collision step which
/// needs to read enemies while it mutates them.
pub fn snapshot_for_collision(world: &hecs::World) -> Vec<(Entity, Vec3, NetId, f32)> {
    world
        .query::<&ServerEnemy>()
        .iter()
        .filter(|(_, en)| !en.is_dying())
        .map(|(e, en)| (e, en.k.position, en.net_id, ENEMY_HIT_RADIUS))
        .collect()
}

/// Tick the death-fade timer on every dying enemy. Despawns rows
/// whose timer has expired so the snapshot stops shipping them.
pub fn tick_dying(world: &mut hecs::World, dt: f32) {
    let mut to_despawn: Vec<Entity> = Vec::new();
    for (e, en) in world.query_mut::<&mut ServerEnemy>() {
        if !en.is_dying() {
            continue;
        }
        en.dying_remaining -= dt;
        en.k.velocity = Vec3::ZERO;
        if en.dying_remaining <= 0.0 {
            to_despawn.push(e);
        }
    }
    for e in to_despawn {
        let _ = world.despawn(e);
    }
}

/// Despawn every `ServerEnemy` in the world. Called on floor change.
pub fn despawn_all(world: &mut hecs::World) {
    let stale: Vec<Entity> = world
        .query::<&ServerEnemy>()
        .iter()
        .map(|(e, _)| e)
        .collect();
    for e in stale {
        let _ = world.despawn(e);
    }
}

/// Deterministically place enemies for the current floor. Uses the
/// same room iteration + pack RNG the SP code used so the layout is
/// reproducible across server restarts.
pub fn spawn_for_floor(
    world: &mut hecs::World,
    floor: &Floor,
    floor_index: u32,
    next_enemy_net_id: &mut u32,
) {
    if floor_index == 0 {
        // Hub has no enemies.
        return;
    }
    let cfg = FloorConfig::for_floor(floor_index);
    let spawn = Vec3::new(floor.spawn_pos.x, 0.0, floor.spawn_pos.z);
    const SAFE_SPAWN_DIST: f32 = 13.5;
    let safe_dist_sq = SAFE_SPAWN_DIST * SAFE_SPAWN_DIST;
    let safe_from_player = |p: Vec3| -> bool {
        let dx = p.x - spawn.x;
        let dz = p.z - spawn.z;
        (dx * dx + dz * dz) >= safe_dist_sq
    };
    let mut enemy_seed = 1000_u64 + floor_index as u64;
    let arena_rooms = floor.arena_rooms();
    let mut spawned = 0u32;
    for room in arena_rooms {
        let packs = room.spawn_packs(cfg.packs_per_room, cfg.mobs_per_pack, enemy_seed);
        enemy_seed = enemy_seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        for (pack_center, positions) in &packs {
            if !safe_from_player(*pack_center) {
                continue;
            }
            let elite_roll = ((enemy_seed >> 16) as f32) / (u32::MAX as f32);
            enemy_seed = enemy_seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let has_elite = elite_roll < cfg.elite_chance;
            for (i, pos) in positions.iter().enumerate() {
                if !safe_from_player(*pos) {
                    continue;
                }
                let is_elite = has_elite && i == 0;
                let role = if is_elite {
                    MonsterRole::Elite
                } else {
                    match i % 3 {
                        0 => MonsterRole::Caster,
                        1 => MonsterRole::Stalker,
                        _ => MonsterRole::Brute,
                    }
                };
                // Roll elite affixes deterministically from the
                // pack RNG. Picks 1-2 modifiers; never the same
                // bit twice. Normal enemies get 0.
                let mut elite_mods: u8 = 0;
                if is_elite {
                    let roll1 = ((enemy_seed >> 24) as u8) % 5;
                    elite_mods |= 1u8 << roll1;
                    enemy_seed = enemy_seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                    // ~50 % chance of a second affix.
                    if (enemy_seed & 0x1) == 0 {
                        let mut roll2 = ((enemy_seed >> 24) as u8) % 5;
                        if (1u8 << roll2) == elite_mods {
                            roll2 = (roll2 + 1) % 5;
                        }
                        elite_mods |= 1u8 << roll2;
                        enemy_seed = enemy_seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                    }
                }
                // Look up role stats through the central
                // table. Elites use `cfg.elite_hp_mult` /
                // `0.8` speed instead of the role's own
                // numbers (the elite tier is its own tuning
                // dimension); affixes layer on top of either.
                let role_stats = role.stats();
                let mut hp = if is_elite {
                    cfg.enemy_health * cfg.elite_hp_mult
                } else {
                    cfg.enemy_health * role_stats.hp_mult
                };
                if (elite_mods & elite_mod::JUGGERNAUT) != 0 {
                    hp *= ELITE_JUGGERNAUT_HP_MULT;
                }
                let mut speed = if is_elite {
                    cfg.enemy_speed * 0.8
                } else {
                    cfg.enemy_speed * role_stats.speed_mult
                };
                if (elite_mods & elite_mod::SWIFT) != 0 {
                    speed *= ELITE_SWIFT_SPEED_MULT;
                }
                let net_id = NetId(*next_enemy_net_id);
                *next_enemy_net_id = next_enemy_net_id.wrapping_add(1).max(1);
                let enemy = ServerEnemy {
                    net_id,
                    role,
                    k: Kinematic {
                        position: Vec3::new(pos.x, 0.0, pos.z),
                        velocity: Vec3::ZERO,
                        yaw: 0.0,
                        aim_yaw: 0.0,
                        locomotion: loco::IDLE,
                        vy: 0.0,
                        airborne: false,
                        ..Default::default()
                    },
                    speed,
                    hp_max: hp,
                    hp,
                    attack_cooldown: 0.0,
                    attack_anim_remaining: 0.0,
                    dying_remaining: 0.0,
                    ai_phase: AiPhase::default(),
                    target_lock: None,
                    crit_chance: 0.0,
                    crit_damage: 0.0,
                    stagger_remaining: 0.0,
                    pending_aggro: None,
                    threat: std::collections::HashMap::new(),
                    elite_mods,
                    flank_slot: (net_id.0 % brute::FLANK_SLOTS as u32) as u8,
                    path: Vec::new(),
                    path_target_tile: None,
                    path_recompute_in: 0.0,
                    los_blocked_cached: false,
                    los_recheck_in: ((net_id.0 % 13) as f32) * (LOS_RECHECK_INTERVAL / 13.0),
                };
                world.spawn((enemy, super::effect::EffectStack::default()));
                spawned += 1;
            }
        }
    }
    log::info!("sim: spawned {spawned} enemies on floor {floor_index}");
}
