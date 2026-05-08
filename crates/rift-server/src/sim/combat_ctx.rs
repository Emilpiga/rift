//! Shared mutable context threaded through every damage
//! subsystem (projectile hits, AoE zone ticks, channel beams,
//! debuff-driven DoT, on-death effects).
//!
//! Replaces the old `loot::DeathCtx` whose name was a
//! misnomer: the struct is touched on *every* hit, not just
//! killing blows. Lives in its own module so it isn't
//! conceptually owned by either the loot subsystem or the
//! projectile subsystem — both are consumers.
//!
//! Conceptually two phases share this context:
//!
//! * **Hit-time** — fired on every successful damage application,
//!   killing or not. [`CombatCtx::events`] receives `Damage` /
//!   `Hit` events; [`CombatCtx::player_damage_back`] receives
//!   thorns reflect rows.
//! * **Kill-time** — fired only when the hit drops the target.
//!   [`CombatCtx::kills`] receives a [`KillInfo`] for XP /
//!   progress bookkeeping; [`CombatCtx::next_loot_net_id`] is
//!   the loot allocator; [`CombatCtx::death_aoe_zones`]
//!   receives EXPLODER death pops; [`CombatCtx::floor_index`]
//!   pollutes the loot RNG seed.
//!
//! The two phases share `events` and `tick` so we keep them in
//! one struct rather than splitting into two arguments threaded
//! through five call sites — one borrow per damage call wins
//! over symmetry here.

use hecs::Entity;
use rift_net::{messages::WorldEvent, NetTick};

/// One context per `Sim::step` damage pass — see module docs.
pub struct CombatCtx<'a> {
    // ---- Shared (hit-time + kill-time) --------------------

    /// Wire-event sink. Damage / Hit / Death / LootDropped all
    /// land here in arrival order so the per-tick snapshot
    /// builder can drain them into reliable client messages.
    pub events: &'a mut Vec<WorldEvent>,
    /// Authoritative tick the damage is being applied on. Used
    /// to seed the loot RNG and to stamp `Hit.start_tick` so
    /// clients align hit-react clips.
    pub tick: NetTick,

    // ---- Hit-time -----------------------------------------

    /// Damage rows queued *back at the attacker* by elite
    /// `THORNS` mod when an enemy is hit (any hit, killing or
    /// not). Drained by the caller into the same player-damage
    /// queue used by enemy melee + cast resolves so the
    /// death-on-thorns path runs through one chokepoint. Empty
    /// for hits on non-thorns enemies.
    pub player_damage_back: &'a mut Vec<PlayerHit>,
    /// Combat-meter sink. Damage subsystems push one
    /// [`MeterEvent`] per attributable hit so [`super::Sim::step`]
    /// can fold them into the per-instance `Meters` table after
    /// the world borrow drops. Decoupled from `events` because
    /// these are server-internal bookkeeping, not wire traffic.
    pub meter_events: &'a mut Vec<MeterEvent>,

    // ---- Kill-time ----------------------------------------

    /// Loot net-id allocator. Bumped once per dropped item.
    pub next_loot_net_id: &'a mut u32,
    /// Floor depth — pollutes the loot RNG seed so re-entering
    /// a floor produces different drops, and scales item-level.
    pub floor_index: u32,
    /// One row per kill produced this tick. Drained by
    /// [`super::Sim::step`] to bump rift progress, grant XP,
    /// and detect the boss kill.
    pub kills: &'a mut Vec<KillInfo>,
    /// AoE zones queued by elite `EXPLODER` mod when an enemy
    /// dies. Drained by the caller into [`super::Sim::aoe_zones`]
    /// so the post-mortem pop ticks alongside player AoE the
    /// same frame.
    pub death_aoe_zones: &'a mut Vec<super::projectile::ServerAoeZone>,
    /// Net-id allocator for `death_aoe_zones`. Same allocator
    /// the projectile pipeline uses so wire ids are unique.
    pub next_projectile_net_id: &'a mut u32,
}

/// One row pushed to [`CombatCtx::meter_events`] per damage / heal
/// hit that can be attributed to a player. Drained at the end of
/// [`super::Sim::step`] and folded into the per-instance
/// [`super::Meters`] table.
#[derive(Clone, Copy, Debug)]
pub enum MeterEvent {
    /// Player → enemy damage. `attacker` is the player entity
    /// that landed the hit; `ability_id` is the wire-stable u8
    /// from `rift_game::abilities::id::*`, or `255` ("Other")
    /// when the source can't be attributed (DoT ticks today).
    DamageDealt {
        attacker: Entity,
        ability_id: u8,
        amount: f32,
    },
    /// Player → player healing (effective HP restored, post
    /// healing-received-mult and post-overheal). `caster` is
    /// the player entity that originated the heal — used to
    /// credit HoT ticks whose application site is several
    /// frames removed from the cast that produced them.
    HealingDone {
        caster: Entity,
        ability_id: u8,
        amount: f32,
    },
}

/// Per-kill information collected during damage subsystems and
/// drained at the end of [`super::Sim::step`] for XP / progress
/// bookkeeping.
#[derive(Clone, Copy, Debug)]
pub struct KillInfo {
    /// Role of the slain enemy.
    pub role: rift_game::monsters::MonsterRole,
}

/// One queued enemy-→-player damage row. Threaded through every
/// hostile damage source (melee swings, projectiles, AoE
/// pulses, channel beams, debuff DoT ticks, thorns reflect)
/// so [`super::Sim::apply_player_damage`] can credit the
/// receiving player's two-level meter breakdown
/// (`attacker_kind` → `ability_id` → amount).
///
/// `ability_id` is the wire-stable u8 from
/// `rift_game::abilities::id::*`, or
/// [`super::meters::ABILITY_ID_OTHER`] (`255`) for sources we
/// can't attribute to one (thorns reflect, environmental,
/// anonymous DoTs).
///
/// `attacker_kind` is `MonsterRole::to_wire_byte()` for hits
/// produced by a known enemy, or
/// [`super::meters::ATTACKER_KIND_OTHER`] (`255`) for sources
/// without an enemy origin (thorns reflect / environmental).
#[derive(Clone, Copy, Debug)]
pub struct PlayerHit {
    pub target: Entity,
    pub attacker_kind: u8,
    pub ability_id: u8,
    pub amount: f32,
}
