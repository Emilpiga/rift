//! Server-driven enemy state, AI, and floor-pack spawning.
//!
//! Enemies share the kinematic substrate with players (`Kinematic`)
//! so the same wall-aware integrator handles their motion.

use glam::Vec3;
use hecs::Entity;
use rift_dungeon::{Floor, FloorConfig};
use rift_net::NetId;
use rift_game::kinematic::{self, loco, Kinematic};

/// Wire role ids for replicated enemies. Stable, picked once and
/// never reordered — clients use the byte directly to index their
/// `MonsterCache`.
#[allow(dead_code)] // BOSS is part of the wire contract.
pub mod role {
    pub const BRUTE: u8 = 0;
    pub const STALKER: u8 = 1;
    pub const CASTER: u8 = 2;
    pub const ELITE: u8 = 3;
    pub const BOSS: u8 = 4;
}

/// Wire animation ids. Clients map these to clip names locally.
pub mod enemy_anim {
    pub const IDLE: u8 = 0;
    pub const WALK: u8 = 1;
    pub const ATTACK: u8 = 2;
    /// Corpse pose. Set in [`super::snapshot::build`] for any enemy
    /// whose `dying_remaining > 0.0` so the client engine plays the
    /// `Death` clip and the per-enemy fade tick runs.
    pub const DEATH: u8 = 3;
}

/// How long a killed enemy hangs around (HP=0, AI off, collision
/// off, snapshot still includes the row) so the client gets to
/// play its `Death` clip + corpse fade. Slightly longer than the
/// engine's own `Dying.duration` for skinned monsters (1.4 s) so
/// the server doesn't yank the row out from under the animation.
pub const DEATH_FADE_DUR: f32 = 1.6;

/// Aggro pickup range — within this distance the closest player is
/// chosen as the target. Outside this, the enemy idles in place.
/// Sized to comfortably exceed the typical viewport so enemies
/// you can see are already reacting to you, instead of standing
/// like statues until you're nearly on top of them.
pub const AGGRO_RANGE: f32 = 22.0;
/// How close the enemy must be before it stops moving and swings.
pub const ATTACK_RANGE: f32 = 1.6;
/// Damage dealt per successful melee hit.
pub const ATTACK_DAMAGE: f32 = 8.0;
/// Cooldown between consecutive melee swings, in seconds.
pub const ATTACK_COOLDOWN: f32 = 1.4;
/// How long after committing a swing the attack-anim flag stays
/// true on the wire — clients use it to play the attack clip.
pub const ATTACK_ANIM_DUR: f32 = 0.45;

/// Sphere radius used for projectile↔enemy XZ collision.
pub const ENEMY_HIT_RADIUS: f32 = 0.6;

/// Personal-space radius used for separation steering between
/// enemies in the same pack. Below this distance an enemy steers
/// away from its neighbour so packs don't melt into a single dot
/// when they all converge on the player.
pub const SEPARATION_RADIUS: f32 = 1.3;
/// Strength of the separation push relative to walking speed.
/// Tuned so two enemies brushing shoulders just barely shove each
/// other apart without breaking forward locomotion.
pub const SEPARATION_STRENGTH: f32 = 1.1;

/// Caster ranged-attack tuning.
pub mod caster {
    /// Preferred kite distance — caster tries to stay near this
    /// from its target, backing off if the player closes inside
    /// `MIN_RANGE` and approaching if outside `MAX_RANGE`.
    pub const KITE_DISTANCE: f32 = 11.0;
    /// Below this distance the caster actively retreats.
    pub const MIN_RANGE: f32 = 6.0;
    /// Above this distance the caster moves toward the player to
    /// stay in firing range.
    pub const MAX_RANGE: f32 = 14.0;
    /// Cooldown between consecutive bolt shots, in seconds.
    pub const SHOT_COOLDOWN: f32 = 2.4;
    /// How long the wind-up freeze lasts before the bolt actually
    /// fires. Doubles as the wire `ATTACK` anim window so clients
    /// see a telegraphed cast.
    pub const WINDUP_DUR: f32 = 0.55;
    /// Bolt projectile speed (units / s).
    pub const BOLT_SPEED: f32 = 14.0;
    /// Bolt projectile lifetime — auto-expires after this many
    /// seconds even with no impact.
    pub const BOLT_TTL: f32 = 1.5;
    /// Damage dealt to the player on direct hit.
    pub const BOLT_DAMAGE: f32 = 9.0;
}

/// Stalker dash-attack tuning.
pub mod stalker {
    /// Distance at which the stalker stops approaching and starts
    /// its dash wind-up. Tuned to be close enough that the dash
    /// reliably overshoots the player even after they shuffle a
    /// bit during the wind-up — `DASH_SPEED_MULT * base * DASH_DUR`
    /// has to be comfortably greater than this.
    pub const TRIGGER_RANGE: f32 = 3.5;
    /// Telegraph crouch before the dash — stalker freezes briefly
    /// so players can react.
    pub const WINDUP_DUR: f32 = 0.35;
    /// How long the dash itself lasts. Combined with the speed
    /// multiplier this needs to overshoot `TRIGGER_RANGE` by a
    /// healthy margin so the stalker reads as passing *through*
    /// the player rather than stopping at them.
    pub const DASH_DUR: f32 = 0.55;
    /// Speed multiplier applied to base `enemy.speed` during the
    /// dash. Stalkers are already 1.35× base; with this 4.5×
    /// multiplier on top the dash travels roughly 6.7m on floor 0
    /// — well past the trigger distance.
    pub const DASH_SPEED_MULT: f32 = 4.5;
    /// Damage applied if the stalker passes within
    /// `super::ATTACK_RANGE` of its target during the dash.
    pub const DASH_DAMAGE: f32 = 12.0;
    /// Recovery period after the dash ends — stalker drifts
    /// backward at half-speed and can't dash again.
    pub const RECOVER_DUR: f32 = 1.1;
    /// Multiplier on `enemy.speed` during recovery (negative-ish:
    /// applied to the *away-from-target* direction).
    pub const RECOVER_SPEED_MULT: f32 = 0.8;
}

/// Per-enemy AI phase. Most roles only ever live in
/// [`AiPhase::Idle`]; the stalker dash cycle uses the timed
/// variants and the caster's wind-up uses
/// [`AiPhase::CasterWindup`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AiPhase {
    /// Default state — apply role's baseline movement + attack.
    Idle,
    /// Stalker is closing the distance toward its target. No
    /// timer; promoted to `StalkerWindup` once inside trigger
    /// range.
    StalkerApproach,
    /// Brief telegraph freeze before the dash. `f32` counts down
    /// to zero, then promotes to `StalkerDash`.
    StalkerWindup(f32),
    /// Stalker is mid-dash toward the locked-in dash direction.
    /// First field is the remaining timer, second is the unit
    /// dash direction snapshotted at wind-up start (so the player
    /// can side-step the lunge).
    StalkerDash { remaining: f32, dir: Vec3, hit_landed: bool },
    /// Post-dash retreat / cooldown. Counts down to zero, then
    /// flips back to `StalkerApproach`.
    StalkerRecover(f32),
    /// Caster wind-up before a bolt. Fires when the timer hits
    /// zero and immediately drops back to `Idle`.
    CasterWindup(f32),
}

impl Default for AiPhase {
    fn default() -> Self {
        Self::Idle
    }
}

/// Component bundle for one server-driven enemy.
#[derive(Clone, Debug)]
pub struct ServerEnemy {
    pub net_id: NetId,
    pub role: u8,
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
}

impl ServerEnemy {
    /// `true` once the enemy has been killed — used by every
    /// damage subsystem to avoid hitting the same corpse twice and
    /// by the AI tick to skip dead bodies.
    pub fn is_dying(&self) -> bool {
        self.dying_remaining > 0.0
    }
}

/// One in-flight ranged-attack request emitted by the AI tick.
/// The caller (`Sim::step`) consumes these to spawn enemy
/// projectile entities once the AI's mutable borrow ends.
#[derive(Clone, Debug)]
pub struct EnemyShot {
    /// Net id of the casting enemy (so the spawned projectile
    /// can record its owner for replication).
    pub owner: NetId,
    /// Bolt origin (caster body + slight forward offset).
    pub origin: Vec3,
    /// Unit aim direction in XZ.
    pub aim: Vec3,
}

/// Output bundle from one AI tick. The dispatcher walks every
/// enemy, applies role-specific steering / attack logic, and
/// returns the queued damage + ranged-shot rows for the caller
/// to apply once the world borrow ends.
#[derive(Default)]
pub struct AiOutcome {
    pub melee_damage: Vec<(Entity, f32)>,
    pub shots: Vec<EnemyShot>,
}

/// One AI tick for every enemy in the world.
///
/// Dispatches to a per-role tick (`tick_brute` / `tick_stalker` /
/// `tick_caster`) so each archetype can have its own movement /
/// attack pattern. All roles share two common behaviours wired
/// up here:
///
/// 1. **Target selection** — nearest player within
///    [`AGGRO_RANGE`] becomes the AI target. Out of range the
///    enemy holds its position.
/// 2. **Separation steering** — every enemy nudges away from
///    in-pack neighbours within [`SEPARATION_RADIUS`] so big
///    packs don't melt into one entity-overlapping dot when they
///    converge on a player.
pub fn tick_ai(
    world: &mut hecs::World,
    player_positions: &[(Entity, Vec3)],
    dt: f32,
) -> AiOutcome {
    // Snapshot every live enemy's (net_id, position) so the
    // separation pass below can read neighbour positions while
    // each row is borrowed mutably for steering. Net id is
    // included in the key so we can skip self when summing
    // repulsions. Dying enemies don't count — they're frozen and
    // shouldn't shove their neighbours around.
    let neighbours: Vec<(NetId, Vec3)> = world
        .query::<&ServerEnemy>()
        .iter()
        .filter(|(_, en)| !en.is_dying())
        .map(|(_, en)| (en.net_id, en.k.position))
        .collect();

    let mut outcome = AiOutcome::default();
    for (_e, (en, stack)) in world
        .query_mut::<(&mut ServerEnemy, Option<&super::debuff::DebuffStack>)>()
    {
        // Skip dying enemies — their AI is frozen until the
        // death-fade timer expires and they're despawned.
        if en.is_dying() {
            en.k.velocity = Vec3::ZERO;
            continue;
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
        // Find nearest player within aggro range.
        let target = nearest_player(en.k.position, player_positions);

        // Per-role steering + attack.
        match en.role {
            role::STALKER => tick_stalker(en, target, speed_mult, dt, &mut outcome),
            role::CASTER => tick_caster(en, target, speed_mult, dt, &mut outcome),
            // Brute, Elite, Boss, and any unknown role: classic
            // chase-and-melee behaviour.
            _ => tick_brute(en, target, speed_mult, dt, &mut outcome),
        }

        // Separation: shove away from any neighbour inside
        // SEPARATION_RADIUS so packs spread out instead of
        // stacking. Skips dying enemies (filtered above) and
        // self (matched by net_id).
        let push = separation_steering(en.net_id, en.k.position, &neighbours);
        if push.length_squared() > 1.0e-6 {
            // Scale by base speed so the shove feels uniform
            // across slow / fast roles. Applied additively so
            // forward locomotion still wins when it's set; in
            // pure-Idle states the push is what unjams the clump.
            en.k.velocity += push * en.speed * SEPARATION_STRENGTH * speed_mult;
        }
    }
    outcome
}

/// Find the nearest entry in `players` within [`AGGRO_RANGE`] of
/// `pos`. Returns `None` if every player is out of range.
fn nearest_player(
    pos: Vec3,
    players: &[(Entity, Vec3)],
) -> Option<(Entity, Vec3, f32)> {
    let mut best: Option<(Entity, Vec3, f32)> = None;
    for (pe, pp) in players {
        let dx = pp.x - pos.x;
        let dz = pp.z - pos.z;
        let d2 = dx * dx + dz * dz;
        if d2 <= AGGRO_RANGE * AGGRO_RANGE
            && best.map_or(true, |(_, _, bd2)| d2 < bd2)
        {
            best = Some((*pe, *pp, d2));
        }
    }
    best
}

/// Sum repulsion vectors from every neighbour inside
/// [`SEPARATION_RADIUS`]. Each push is scaled by `(R - d) / R` so
/// neighbours that are touching produce the strongest shove and
/// neighbours just inside the boundary contribute almost nothing.
fn separation_steering(self_id: NetId, pos: Vec3, neighbours: &[(NetId, Vec3)]) -> Vec3 {
    let mut push = Vec3::ZERO;
    for (nid, npos) in neighbours {
        if *nid == self_id {
            continue;
        }
        let dx = pos.x - npos.x;
        let dz = pos.z - npos.z;
        let d2 = dx * dx + dz * dz;
        if d2 >= SEPARATION_RADIUS * SEPARATION_RADIUS || d2 < 1.0e-6 {
            continue;
        }
        let d = d2.sqrt();
        let weight = (SEPARATION_RADIUS - d) / SEPARATION_RADIUS;
        // Normalize the offset and scale by the weight. y is
        // intentionally zero — separation is purely horizontal.
        push += Vec3::new(dx / d, 0.0, dz / d) * weight;
    }
    push
}

/// Brute / Elite / Boss / fallback behaviour: walk in a straight
/// line to the target and swing in melee range. Same as the
/// pre-refactor monolithic `tick_ai`, just per-row.
fn tick_brute(
    en: &mut ServerEnemy,
    target: Option<(Entity, Vec3, f32)>,
    speed_mult: f32,
    _dt: f32,
    outcome: &mut AiOutcome,
) {
    let Some((target_entity, target_pos, d2)) = target else {
        en.k.velocity = Vec3::ZERO;
        en.k.locomotion = loco::IDLE;
        return;
    };
    let dist = d2.sqrt();
    let to_target = Vec3::new(
        target_pos.x - en.k.position.x,
        0.0,
        target_pos.z - en.k.position.z,
    );
    if to_target.length_squared() > 1.0e-4 {
        en.k.yaw = to_target.x.atan2(to_target.z);
        en.k.aim_yaw = en.k.yaw;
    }
    if dist > ATTACK_RANGE {
        let dir = to_target.normalize_or_zero();
        en.k.velocity = dir * en.speed * speed_mult;
        en.k.locomotion = loco::RUN;
    } else {
        en.k.velocity = Vec3::ZERO;
        en.k.locomotion = loco::IDLE;
        if en.attack_cooldown <= 0.0 {
            en.attack_cooldown = ATTACK_COOLDOWN;
            en.attack_anim_remaining = ATTACK_ANIM_DUR;
            outcome
                .melee_damage
                .push((target_entity, ATTACK_DAMAGE));
        }
    }
}

/// Stalker behaviour: approach until inside [`stalker::TRIGGER_RANGE`],
/// telegraph briefly, dash through the target, then retreat. The
/// dash applies a one-shot melee hit if the stalker passes inside
/// [`ATTACK_RANGE`] of its target during the dash window.
fn tick_stalker(
    en: &mut ServerEnemy,
    target: Option<(Entity, Vec3, f32)>,
    speed_mult: f32,
    dt: f32,
    outcome: &mut AiOutcome,
) {
    let Some((target_entity, target_pos, d2)) = target else {
        en.k.velocity = Vec3::ZERO;
        en.k.locomotion = loco::IDLE;
        en.ai_phase = AiPhase::StalkerApproach;
        return;
    };
    let dist = d2.sqrt();
    let to_target = Vec3::new(
        target_pos.x - en.k.position.x,
        0.0,
        target_pos.z - en.k.position.z,
    );
    // Faces the target unless we're mid-dash with a locked dir.
    if to_target.length_squared() > 1.0e-4 {
        en.k.yaw = to_target.x.atan2(to_target.z);
        en.k.aim_yaw = en.k.yaw;
    }

    // Promote `Idle` to `Approach` so brand-new stalkers don't
    // wedge in their initial Idle state.
    if matches!(en.ai_phase, AiPhase::Idle) {
        en.ai_phase = AiPhase::StalkerApproach;
    }

    match en.ai_phase {
        AiPhase::StalkerApproach => {
            if dist <= stalker::TRIGGER_RANGE {
                // Lock in the approach by dropping into wind-up.
                en.ai_phase = AiPhase::StalkerWindup(stalker::WINDUP_DUR);
                en.k.velocity = Vec3::ZERO;
                en.k.locomotion = loco::IDLE;
                en.attack_anim_remaining = stalker::WINDUP_DUR + stalker::DASH_DUR;
                return;
            }
            let dir = to_target.normalize_or_zero();
            en.k.velocity = dir * en.speed * speed_mult;
            en.k.locomotion = loco::RUN;
        }
        AiPhase::StalkerWindup(t) => {
            let next = t - dt;
            en.k.velocity = Vec3::ZERO;
            en.k.locomotion = loco::IDLE;
            if next <= 0.0 {
                // Snapshot the dash direction now so the player
                // can side-step the lunge after the telegraph.
                let dir = to_target.normalize_or_zero();
                en.ai_phase = AiPhase::StalkerDash {
                    remaining: stalker::DASH_DUR,
                    dir,
                    hit_landed: false,
                };
            } else {
                en.ai_phase = AiPhase::StalkerWindup(next);
            }
        }
        AiPhase::StalkerDash {
            remaining,
            dir,
            hit_landed,
        } => {
            let next = remaining - dt;
            // Dash drives motion regardless of player range. The
            // dash velocity is uniform — no separation easing,
            // no slowdown near target — so the lunge feels
            // committal.
            en.k.velocity = dir * en.speed * stalker::DASH_SPEED_MULT * speed_mult;
            en.k.locomotion = loco::RUN;
            // One-shot damage: applied the first frame the
            // dash crosses inside ATTACK_RANGE of the target.
            let mut landed = hit_landed;
            if !landed && dist <= ATTACK_RANGE {
                outcome
                    .melee_damage
                    .push((target_entity, stalker::DASH_DAMAGE));
                landed = true;
            }
            if next <= 0.0 {
                en.ai_phase = AiPhase::StalkerRecover(stalker::RECOVER_DUR);
            } else {
                en.ai_phase = AiPhase::StalkerDash {
                    remaining: next,
                    dir,
                    hit_landed: landed,
                };
            }
        }
        AiPhase::StalkerRecover(t) => {
            let next = t - dt;
            // Drift backward at a fraction of base speed so the
            // stalker reads as winded after the lunge.
            let away = -to_target.normalize_or_zero();
            en.k.velocity = away * en.speed * stalker::RECOVER_SPEED_MULT * speed_mult;
            en.k.locomotion = loco::RUN;
            if next <= 0.0 {
                en.ai_phase = AiPhase::StalkerApproach;
            } else {
                en.ai_phase = AiPhase::StalkerRecover(next);
            }
        }
        // Caster phases shouldn't occur on a stalker; if they
        // ever do (component shuffle, save load), reset.
        AiPhase::CasterWindup(_) | AiPhase::Idle => {
            en.ai_phase = AiPhase::StalkerApproach;
        }
    }
}

/// Caster behaviour: kite at [`caster::KITE_DISTANCE`] from the
/// target while firing bolts on cooldown. Approach if too far,
/// retreat if too close. Wind-up freeze before each bolt
/// telegraphs the attack so players can break line of sight.
fn tick_caster(
    en: &mut ServerEnemy,
    target: Option<(Entity, Vec3, f32)>,
    speed_mult: f32,
    dt: f32,
    outcome: &mut AiOutcome,
) {
    let Some((_target_entity, target_pos, d2)) = target else {
        en.k.velocity = Vec3::ZERO;
        en.k.locomotion = loco::IDLE;
        en.ai_phase = AiPhase::Idle;
        return;
    };
    let dist = d2.sqrt();
    let to_target = Vec3::new(
        target_pos.x - en.k.position.x,
        0.0,
        target_pos.z - en.k.position.z,
    );
    let dir_to = to_target.normalize_or_zero();
    if to_target.length_squared() > 1.0e-4 {
        en.k.yaw = to_target.x.atan2(to_target.z);
        en.k.aim_yaw = en.k.yaw;
    }

    // Mid-windup: freeze in place, fire when the timer hits zero.
    if let AiPhase::CasterWindup(t) = en.ai_phase {
        let next = t - dt;
        en.k.velocity = Vec3::ZERO;
        en.k.locomotion = loco::IDLE;
        if next <= 0.0 {
            // Fire bolt from a slightly elevated body offset so
            // the visual emerges from the caster's chest, not
            // its feet. Direction is freshly recomputed at fire
            // time so very-late side-steps still get tracked.
            let origin = en.k.position + Vec3::Y * 1.1 + dir_to * 0.4;
            outcome.shots.push(EnemyShot {
                owner: en.net_id,
                origin,
                aim: dir_to,
            });
            en.attack_cooldown = caster::SHOT_COOLDOWN;
            en.ai_phase = AiPhase::Idle;
        } else {
            en.ai_phase = AiPhase::CasterWindup(next);
        }
        return;
    }

    // Distance-based kiting movement.
    if dist > caster::MAX_RANGE {
        en.k.velocity = dir_to * en.speed * speed_mult;
        en.k.locomotion = loco::RUN;
    } else if dist < caster::MIN_RANGE {
        en.k.velocity = -dir_to * en.speed * speed_mult;
        en.k.locomotion = loco::RUN;
    } else {
        // In the kite ring — strafe to nudge toward the ideal
        // distance, but slowly. Pulls toward `KITE_DISTANCE` so
        // the caster gravitates to the sweet spot instead of
        // ping-ponging between MIN and MAX.
        let drift = (dist - caster::KITE_DISTANCE) * 0.3;
        en.k.velocity = dir_to * drift * speed_mult;
        en.k.locomotion = if drift.abs() > 0.05 { loco::RUN } else { loco::IDLE };
    }

    // Try to fire if cooldown is up and we have line-of-distance.
    // The actual line-of-sight check is implicit in the bolt's
    // wall-collision step — telegraphing now even through walls
    // would reveal positions, which is fine for a PvE game.
    if en.attack_cooldown <= 0.0 && dist <= caster::MAX_RANGE {
        en.ai_phase = AiPhase::CasterWindup(caster::WINDUP_DUR);
        en.attack_anim_remaining = caster::WINDUP_DUR;
    }
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
                let role_byte = if is_elite {
                    role::ELITE
                } else {
                    match i % 3 {
                        0 => role::CASTER,
                        1 => role::STALKER,
                        _ => role::BRUTE,
                    }
                };
                let hp = if is_elite {
                    cfg.enemy_health * cfg.elite_hp_mult
                } else {
                    match role_byte {
                        role::BRUTE => cfg.enemy_health * 1.15,
                        role::STALKER => cfg.enemy_health * 0.75,
                        role::CASTER => cfg.enemy_health * 0.65,
                        _ => cfg.enemy_health,
                    }
                };
                let speed = if is_elite {
                    cfg.enemy_speed * 0.8
                } else {
                    match role_byte {
                        role::BRUTE => cfg.enemy_speed * 0.85,
                        role::STALKER => cfg.enemy_speed * 1.35,
                        role::CASTER => cfg.enemy_speed * 0.95,
                        _ => cfg.enemy_speed,
                    }
                };
                let net_id = NetId(*next_enemy_net_id);
                *next_enemy_net_id = next_enemy_net_id.wrapping_add(1).max(1);
                let enemy = ServerEnemy {
                    net_id,
                    role: role_byte,
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
                };
                world.spawn((enemy, super::debuff::DebuffStack::default()));
                spawned += 1;
            }
        }
    }
    log::info!("sim: spawned {spawned} enemies on floor {floor_index}");
}
