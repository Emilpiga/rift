//! Connected-player state, input ingestion, and movement integration.
//!
//! Players are pure data: no AI, no scripted state machines. Every
//! tick the latest coalesced `InputCmd` is fed through the shared
//! `rift-game` integrator (which the client mirrors verbatim for
//! prediction).

use std::collections::HashMap;

use hecs::Entity;
use rift_dungeon::Floor;
use rift_net::{
    messages::{button_bits, InputCmd},
    ClientId, NetId,
};
use rift_game::attributes::Attributes;
use rift_game::experience::{Experience, LevelUpReward};
use rift_game::hero::HERO;
use rift_game::kinematic::{self, loco, Kinematic};
use rift_game::loadout::Loadout;
use rift_game::stats::CharacterStats;

/// Default per-player level until the persisted level field is
/// wired through. Drives `CharacterStats::compute`.
pub const DEFAULT_LEVEL: u32 = 1;

/// Component bundle for a connected player.
#[derive(Clone, Debug)]
pub struct ServerPlayer {
    pub client_id: ClientId,
    pub net_id: NetId,
    pub k: Kinematic,
    pub hp_max: f32,
    pub hp: f32,
    /// Last input `seq` we successfully applied. Echoed back in
    /// snapshots so the client can prune its prediction buffer.
    pub last_input_seq: u32,
    /// In-memory inventory of items the player has picked up this
    /// session. Authoritative on the server. Persisted via the
    /// `inventory_items` table; rows live across sessions and
    /// floor transitions. The bag mirror only \u2014 anything
    /// currently equipped sits in [`ServerPlayer::equipment`].
    ///
    /// **Sparse**: an `inventory[i] == None` is an empty slot the
    /// player has carved out via drag-and-drop. The vector is
    /// trimmed of trailing `None`s after every mutation so its
    /// length tracks the highest occupied slot + 1.
    pub inventory: Vec<Option<rift_game::loot::Item>>,
    /// Currently-equipped items, keyed by
    /// [`rift_game::loot::EquipSlot`]. Authoritative on the
    /// server. Stat / damage formulas read from this set; the
    /// bag never contributes affixes.
    pub equipment: rift_game::loot::Equipment,
    /// Per-character private stash. Lives in its own DB table
    /// (`stash_items`) and is hydrated alongside `inventory` at
    /// Hello time. Items here are pure storage — they never
    /// contribute to stats and aren't visible from the bag UI
    /// unless the player explicitly opens the chest. Sparse like
    /// [`Self::inventory`].
    pub stash: Vec<Option<rift_game::loot::Item>>,
    /// Whether the owner currently has an active stash session
    /// (i.e. is interacting with the chest in the hub). Gates
    /// `DepositToStash` / `WithdrawFromStash` so out-of-band
    /// transfer requests are dropped server-side.
    pub stash_open: bool,
    /// Character level. Used by [`CharacterStats::compute`] for
    /// HP-per-level scaling.
    pub level: u32,
    /// Authoritative XP / level state. Source of truth for the
    /// `ServerMsg::CharacterStats` reply pushed to the owning
    /// client. `level` is mirrored in the dedicated `level`
    /// field above so `CharacterStats::compute` can stay
    /// pure-arg.
    pub experience: Experience,
    /// `Attributes::for_class` allocation until per-character
    /// attribute points are persisted.
    pub attrs: Attributes,
    /// Cached snapshot of the derived combat stats. Recomputed
    /// whenever inputs change ([`Self::recompute_stats`]); read
    /// by the cast / projectile / channel pipelines so client
    /// and server agree on the formulas.
    pub stats: CharacterStats,
    /// Authoritative ability bar. Casts are gated against this so
    /// a client can't fire an ability they haven't slotted.
    /// Persisted via the `characters.loadout` column; mutated
    /// through `ClientMsg::SetLoadoutSlot`.
    pub loadout: Loadout,
    /// `true` once a dead player has finished their down-pose
    /// timer and entered ghost mode — they can move freely,
    /// trigger the rift exit-vote portal, but can't cast,
    /// loot, or be targeted by AI. Cleared on heal_all().
    pub is_ghost: bool,
    /// Countdown (seconds) from death until ghost rise. `None`
    /// while alive or already a ghost. Set to
    /// `GHOST_RISE_DELAY` the tick HP first crosses 0; ticked
    /// down in `Sim::step`. When it reaches 0 we flip
    /// `is_ghost = true` and clear the timer.
    pub ghost_rise_timer: Option<f32>,
}

impl ServerPlayer {
    /// Build a fresh player record. Stats are computed from the
    /// hero config + default attributes + empty equipment, so a
    /// freshly-spawned character matches `CharacterStats::baseline`.
    pub fn fresh(client_id: ClientId, net_id: NetId, spawn: glam::Vec3) -> Self {
        let attrs = Attributes::for_class(HERO.primary_attribute);
        let equipment = rift_game::loot::Equipment::new();
        let stats = CharacterStats::compute(
            &attrs,
            DEFAULT_LEVEL,
            &equipment.active_affix_sum(),
            &rift_game::stats::StatModifiers::new(),
        );
        let hp_max = stats.max_hp;
        Self {
            client_id,
            net_id,
            k: Kinematic {
                position: spawn,
                velocity: glam::Vec3::ZERO,
                yaw: 0.0,
                aim_yaw: 0.0,
                locomotion: loco::IDLE,
                vy: 0.0,
                airborne: false,
                ..Default::default()
            },
            hp_max,
            hp: hp_max,
            last_input_seq: 0,
            inventory: Vec::new(),
            equipment,
            stash: Vec::new(),
            stash_open: false,
            level: DEFAULT_LEVEL,
            attrs,
            stats,
            experience: Experience::new(),
            loadout: Loadout::default_hero(),
            is_ghost: false,
            ghost_rise_timer: None,
        }
    }

    /// Recompute [`Self::stats`] from the current equipment /
    /// attributes / level. Rescales `hp` so the same percentage
    /// of max HP is preserved across an `hp_max` change (e.g.
    /// equipping a +Health item heals to the same fraction of
    /// the new pool).
    ///
    /// Call after any mutation that changes a `compute` input:
    /// equip / unequip, attribute respec (TBD), level up (TBD).
    pub fn recompute_stats(&mut self) {
        let new_stats = CharacterStats::compute(
            &self.attrs,
            self.level,
            &self.equipment.active_affix_sum(),
            &rift_game::stats::StatModifiers::new(),
        );
        let hp_pct = if self.hp_max > 0.0 {
            (self.hp / self.hp_max).clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.hp_max = new_stats.max_hp;
        self.hp = new_stats.max_hp * hp_pct;
        self.stats = new_stats;
    }

    /// Per-cast damage scalar — `stats.damage / class.base_damage`.
    /// Multiplied into each ability's authored `base_damage` so a
    /// freshly-spawned (no gear, default attrs) character deals
    /// the authored numbers, and gear / attributes scale every
    /// ability uniformly.
    pub fn damage_scalar(&self) -> f32 {
        if HERO.base_damage <= 0.0 {
            1.0
        } else {
            self.stats.damage / HERO.base_damage
        }
    }

    /// Grant XP. If one or more level-ups happen, the cached
    /// `level` field is bumped, [`recompute_stats`] is called so
    /// the HP pool reflects the new tier, and the (possibly
    /// empty) reward list is returned for the caller to act on
    /// (granting attribute / talent points lives a layer up).
    ///
    /// Dead-or-ghosting players are short-circuited entirely:
    /// the heal-to-full on level-up would otherwise resurrect a
    /// player who took a killing blow and earned XP in the same
    /// tick (e.g. their ability finished a kill the same frame
    /// it killed them) — server flips `hp = 0` and arms
    /// `ghost_rise_timer`, but `is_ghost` doesn't latch until
    /// the rise delay elapses, so we can't gate purely on that
    /// flag. Treating any of {hp<=0, is_ghost, ghost timer
    /// armed} as "not earning XP this tick" closes the window.
    pub fn grant_xp(&mut self, amount: u64) -> Vec<LevelUpReward> {
        if self.is_dead_or_ghosting() {
            return Vec::new();
        }
        let rewards = self.experience.grant_xp(amount);
        if !rewards.is_empty() {
            self.level = self.experience.level;
            // Heal-to-full feel on level up: keep current % then
            // top off the gained HP. We just call recompute and
            // then refill so a fresh-level character isn't stuck
            // at the pre-level-up HP fraction.
            self.recompute_stats();
            self.hp = self.hp_max;
        }
        rewards
    }

    /// `true` if this player is in any "not actively playing"
    /// state — dead (hp≤0), risen ghost, or in the down-pose
    /// waiting to rise. Used to gate XP / heal / vote-init paths
    /// that would otherwise have inconsistent behaviour during
    /// the death→ghost transition window.
    pub fn is_dead_or_ghosting(&self) -> bool {
        self.hp <= 0.0 || self.is_ghost || self.ghost_rise_timer.is_some()
    }
}

/// Edge-triggered button bits we forward across input coalescing so a
/// brief press never gets dropped between server ticks.
const STICKY_BUTTONS: u16 = button_bits::JUMP
    | button_bits::ROLL
    | button_bits::INTERACT
    | button_bits::ATTACK
    | button_bits::ABILITY_1
    | button_bits::ABILITY_2
    | button_bits::ABILITY_3
    | button_bits::ABILITY_4
    | button_bits::ABILITY_5
    | button_bits::ABILITY_6;

/// Merge a fresh input into a possibly-already-pending one for the
/// same client. Drops out-of-order packets and OR-folds sticky
/// buttons forward.
pub fn merge_pending(pending: &mut HashMap<ClientId, InputCmd>, client_id: ClientId, cmd: InputCmd) {
    if let Some(existing) = pending.get(&client_id) {
        if cmd.seq.wrapping_sub(existing.seq) as i32 <= 0 {
            return;
        }
    }
    let mut merged = cmd;
    if let Some(existing) = pending.get(&client_id) {
        merged.buttons |= existing.buttons & STICKY_BUTTONS;
    }
    pending.insert(client_id, merged);
}

/// Apply the latest pending input for each connected player. Drains
/// `pending`. Records the applied `seq` on each `ServerPlayer` so
/// the next snapshot's `ack_seq` is correct.
pub fn apply_inputs(
    world: &mut hecs::World,
    sessions: &HashMap<ClientId, Entity>,
    pending: &mut HashMap<ClientId, InputCmd>,
) {
    let inputs: Vec<(ClientId, InputCmd)> = pending.drain().collect();
    for (client_id, cmd) in inputs {
        if let Some(&entity) = sessions.get(&client_id) {
            if let Ok(mut p) = world.get::<&mut ServerPlayer>(entity) {
                p.last_input_seq = cmd.seq;
                // Dead-but-not-yet-risen players are pinned in
                // the down pose: zero velocity, drop input. Once
                // they've risen as a ghost they regain movement
                // (but `cast_ability` still rejects them, so the
                // attack/ability button bits below are harmless).
                if p.hp <= 0.0 && !p.is_ghost {
                    p.k.velocity = glam::Vec3::ZERO;
                    continue;
                }
                kinematic::apply_input(&mut p.k, cmd.move_dir, cmd.aim_dir, cmd.buttons);
            }
        }
    }
}

/// Integrate every player's velocity against the floor's wall grid.
pub fn integrate_motion(world: &mut hecs::World, floor: &Floor, dt: f32) {
    for (_e, p) in world.query_mut::<&mut ServerPlayer>() {
        kinematic::integrate(&mut p.k, floor, dt);
    }
}

/// Snapshot every *living* player's `(entity, position)` into a
/// Vec, suitable for use as the AI target list during the enemy
/// tick. Ghosts (`hp <= 0.0` while in a rift floor) are filtered
/// out so AI / enemy projectiles don't aim at corpses.
pub fn target_positions(world: &hecs::World) -> Vec<(Entity, glam::Vec3)> {
    world
        .query::<&ServerPlayer>()
        .iter()
        .filter(|(_, p)| p.hp > 0.0)
        .map(|(e, p)| (e, p.k.position))
        .collect()
}

/// Reset every player's kinematic state to a fresh spawn pose.
/// Called from the floor-change path so a held key doesn't slide
/// the freshly-loaded floor's start position.
pub fn snap_all_to(world: &mut hecs::World, spawn: glam::Vec3) {
    for (_e, p) in world.query_mut::<&mut ServerPlayer>() {
        p.k.position = spawn;
        p.k.velocity = glam::Vec3::ZERO;
        p.k.vy = 0.0;
        p.k.airborne = false;
        p.k.locomotion = loco::IDLE;
    }
}

/// Restore every player to full HP. Called from the floor-change
/// path so a player respawning at the hub after a death (or
/// completing a rift) arrives alive.
pub fn heal_all(world: &mut hecs::World) {
    for (_e, p) in world.query_mut::<&mut ServerPlayer>() {
        p.hp = p.hp_max;
        p.is_ghost = false;
        p.ghost_rise_timer = None;
    }
}
