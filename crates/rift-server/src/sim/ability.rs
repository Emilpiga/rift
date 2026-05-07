//! Server-side ability kernel.
//!
//! The static ability table itself lives in `rift-game` so the
//! client can share it (cooldown UI / button → ability mapping).
//! This module turns a [`CombatIntent`] (player input or AI
//! decision) into authoritative simulation effects through a
//! single pipeline:
//!
//! ```text
//! CombatIntent ──► submit() ──► AcceptedCast ──► dispatch() ──► effects
//! ```
//!
//! [`submit`] runs the per-source validation (loadout / cooldown
//! / origin sanity for players; trivial for AI) and produces a
//! normalised [`AcceptedCast`]. [`dispatch`] then matches on the
//! ability's [`AbilityKind`] and runs the effects (spawn
//! projectiles, place AoE zones, attach channels, queue summons,
//! …) — one match arm per kind, shared by every caster type.
//!
//! Public wrappers [`cast`] (player) and [`resolve_enemy_cast`]
//! (AI) compose `submit` + `dispatch` for the two existing call
//! sites; they exist so `Sim` can keep its high-level method
//! shapes stable.

use std::collections::HashMap;

use glam::{Quat, Vec3};
use hecs::Entity;
use rift_net::{
    messages::WorldEvent,
    ClientId, NetId, NetTick,
};

pub use rift_game::abilities::id;
pub use rift_game::abilities::{lookup, AbilityKind};

use super::player::ServerPlayer;
use super::projectile::{ServerAoeZone, ServerProjectile};

/// Number of cooldown slots tracked per player. Plenty of headroom
/// over the 6 ability ids in use today; bumping this is free.
pub const COOLDOWN_SLOTS: usize = 16;

/// Per-player cooldown state. Indexed by ability id.
pub type CooldownTable = HashMap<ClientId, [f32; COOLDOWN_SLOTS]>;

/// Decay every active cooldown by `dt`.
pub fn tick_cooldowns(cooldowns: &mut CooldownTable, dt: f32) {
    for cds in cooldowns.values_mut() {
        for cd in cds.iter_mut() {
            if *cd > 0.0 {
                *cd = (*cd - dt).max(0.0);
            }
        }
    }
}

/// Reset every cooldown for every player. Used on floor change.
pub fn clear_cooldowns(cooldowns: &mut CooldownTable) {
    for cds in cooldowns.values_mut() {
        *cds = [0.0; COOLDOWN_SLOTS];
    }
}

// ─── Kernel types ────────────────────────────────────────────────────────
//
// `CombatIntent` is the un-validated input — what a player asked
// to do, or what an AI decided to do. `AcceptedCast` is the
// post-validation, normalised payload that flows into
// [`dispatch`]. Two doors, one room: every cast resolves
// through the same effect pipeline regardless of source, but
// each source runs through its own validation gate so the
// player path's anti-cheat (loadout / cooldown / sanity-origin)
// stays out of the AI path's hot loop.

/// One cast request, before validation. Built by the caller
/// (network handler for players, AI tick for enemies) and fed
/// through [`submit`].
#[derive(Clone, Copy, Debug)]
pub enum CombatIntent {
    /// Player input. `client_origin` is the client-authored
    /// hand position (sanity-checked in [`submit`] against the
    /// authoritative body position so a malicious client can't
    /// teleport the projectile spawn).
    Player {
        client_id: ClientId,
        ability_id: u8,
        client_origin: Vec3,
        aim: Vec3,
        placed_target: Option<Vec3>,
    },
    /// AI-driven cast (e.g. enemy bolt at end of wind-up). The
    /// AI is trusted — there's no validation beyond the kind
    /// dispatch in [`dispatch`].
    Ai {
        caster: NetId,
        ability_id: u8,
        origin: Vec3,
        aim: Vec3,
        damage_mult: f32,
        /// Ability-specific scalar override (e.g. boss slam
        /// enrage radius). `0.0` falls back to the registry
        /// value at dispatch time.
        param_a: f32,
    },
}

/// One validated cast, ready for [`dispatch`]. Carries
/// everything dispatch needs and nothing it doesn't — by the
/// time we hit dispatch, all source-specific concerns
/// (sessions, cooldown tables, client trust) are out of scope.
#[derive(Clone, Copy, Debug)]
pub struct AcceptedCast {
    pub caster: NetId,
    /// Caster's ECS entity, when known. Player casts always
    /// have one (used for Channel attachment + Evasive Roll).
    /// AI casts pass `None` — enemies don't currently target
    /// effects at their own entity.
    pub caster_entity: Option<Entity>,
    pub ability_id: u8,
    /// Authoritative body position of the caster.
    pub origin: Vec3,
    /// Spawn position for projectile-shaped abilities. For
    /// player casts this is the (validated) client-authored
    /// hand position so visuals emerge from the casting hand
    /// on every observer's screen; for AI casts it's a fixed
    /// chest-height offset of `origin`. Pre-baked here so
    /// [`dispatch`] doesn't carry per-source spawn logic.
    pub spawn_origin: Vec3,
    /// Unit XZ aim direction.
    pub aim: Vec3,
    /// Ground-targeted spot for placed-AoE abilities.
    pub placed_target: Option<Vec3>,
    /// Total scalar applied to `ability.base_damage` at effect
    /// time. Player path bakes in the gear / attribute /
    /// element / archetype multipliers here so dispatch is
    /// source-agnostic; AI path passes the floor difficulty
    /// scalar.
    pub damage_scalar: f32,
    pub crit_chance: f32,
    pub crit_damage: f32,
    /// Ability-specific scalar override (e.g. effective slam
    /// radius for `DelayedAoe`). `0.0` means "use the
    /// registry value".
    pub param_a: f32,
}

/// Validate a [`CombatIntent`] and produce an
/// [`AcceptedCast`], or return `None` if the intent should be
/// silently dropped (cooldown, missing session, unknown
/// ability id, off-loadout cast …).
///
/// Side effect: on a successful Player intent, the cooldown
/// table is updated. AI intents have no per-caster cooldown
/// table at this layer — the AI ticks its own per-attack
/// cooldowns inside `super::enemy`.
pub fn submit(
    world: &hecs::World,
    sessions: &HashMap<ClientId, Entity>,
    cooldowns: &mut CooldownTable,
    intent: CombatIntent,
) -> Option<AcceptedCast> {
    match intent {
        CombatIntent::Player {
            client_id,
            ability_id,
            client_origin,
            aim,
            placed_target,
        } => {
            let ability = lookup(ability_id)?;
            let &entity = sessions.get(&client_id)?;
            // Reject casts that don't match the player's
            // persisted loadout. A misbehaving client that
            // asks to fire an ability they haven't slotted
            // gets silently dropped here rather than burning
            // the cooldown — checked before we touch the
            // cooldown table so a rejected cast leaves no
            // residue.
            let p_ref = world.get::<&ServerPlayer>(entity).ok()?;
            if !p_ref.loadout.contains(ability_id) {
                return None;
            }
            // Snapshot the caster's authoritative state. The
            // borrow drops at end of statement so the
            // cooldown-table mutation below is safe.
            let body = p_ref.k.position;
            let net_id = p_ref.net_id;
            let dmg_scalar = p_ref.damage_scalar();
            let crit_chance = p_ref.stats.crit_chance;
            let crit_damage = p_ref.stats.crit_damage;
            let ability_mult = p_ref.stats.ability_damage_mult(ability);
            drop(p_ref);

            let cds = cooldowns
                .entry(client_id)
                .or_insert([0.0; COOLDOWN_SLOTS]);
            let slot = (ability_id as usize).min(COOLDOWN_SLOTS - 1);
            if cds[slot] > 0.0 {
                return None;
            }
            cds[slot] = ability.cooldown;

            // Trust the client's hand-position origin within a
            // sanity radius of the simulated body (~2 m). This
            // lets projectiles visibly emerge from the casting
            // hand on every observer's screen without enabling
            // a teleport-the-spawn exploit. Out-of-range or
            // zero origins fall back to a chest-height offset
            // of the body. Aim-forward nudge matches the old
            // behaviour so the projectile starts just past the
            // hand instead of inside it.
            let trusted_origin = if client_origin.distance_squared(body) <= 2.0 * 2.0 {
                client_origin
            } else {
                body + Vec3::Y * 1.25
            };
            let spawn_origin = trusted_origin + aim * 0.25;

            Some(AcceptedCast {
                caster: net_id,
                caster_entity: Some(entity),
                ability_id,
                origin: body,
                spawn_origin,
                aim,
                placed_target,
                // Pre-bake gear / attribute / element /
                // archetype scaling. Dispatch only knows
                // `ability.base_damage * damage_scalar`.
                damage_scalar: dmg_scalar * ability_mult,
                crit_chance,
                crit_damage,
                param_a: 0.0,
            })
        }
        CombatIntent::Ai {
            caster,
            ability_id,
            origin,
            aim,
            damage_mult,
            param_a,
        } => {
            // AI is trusted — no validation beyond the
            // existence of a registry entry. Still gate on it
            // so a misauthored ability id is silently dropped.
            lookup(ability_id)?;
            // Caster bolts emerge ~1.1 m up + slight forward
            // offset so the visual reads as coming from the
            // chest, not the feet.
            let spawn_origin = origin + Vec3::Y * 1.1 + aim * 0.4;
            Some(AcceptedCast {
                caster,
                caster_entity: None,
                ability_id,
                origin,
                spawn_origin,
                aim,
                placed_target: None,
                damage_scalar: damage_mult,
                // Enemies don't crit today — leave the roll
                // weights at zero so the damage-application
                // path treats every hit as non-crit.
                crit_chance: 0.0,
                crit_damage: 0.0,
                param_a,
            })
        }
    }
}

/// Optional sinks for [`dispatch`]. Each effect produced by
/// the kind match writes into one of these; the caller
/// decides which to pass in. Decoupling from `Sim` directly
/// means dispatch is pure-ish and easy to reason about — every
/// mutation it performs is visible in this struct.
pub struct DispatchSinks<'a> {
    pub aoe_zones: &'a mut Vec<ServerAoeZone>,
    pub events: &'a mut Vec<WorldEvent>,
    pub next_projectile_net_id: &'a mut u32,
    /// Damage rows targeted at players (used by `DelayedAoe`).
    pub player_damage: &'a mut Vec<(Entity, f32)>,
    /// Summon spawn requests `(pos, role, hp_mult)` queued for
    /// `Sim::step` to drain into entities. Net-id allocation
    /// stays in `Sim`.
    pub summons: &'a mut Vec<(Vec3, u8, f32)>,
    /// Live `(entity, position)` rows for every player. Read
    /// by `DelayedAoe` to find who's inside the slam disc.
    pub player_targets: &'a [(Entity, Vec3)],
}

/// Run the effect pipeline for an [`AcceptedCast`]. One match
/// arm per [`AbilityKind`]; both player and AI casts flow
/// through here, so adding a new ability shape is a single
/// arm and (optionally) a registry entry.
pub fn dispatch(
    world: &mut hecs::World,
    accepted: AcceptedCast,
    sinks: &mut DispatchSinks<'_>,
    tick: NetTick,
) {
    let Some(ability) = lookup(accepted.ability_id) else {
        return;
    };
    let scaled_damage = ability.base_damage * accepted.damage_scalar;

    match ability.kind {
        // ── Player-shaped kinds ─────────────────────────────────
        AbilityKind::Projectiles {
            count, spread, speed, ttl, pierce, apply_debuff,
        } => {
            for i in 0..count {
                let angle_offset = if count > 1 {
                    let t = i as f32 / (count - 1) as f32 - 0.5;
                    t * spread
                } else {
                    0.0
                };
                let dir = Quat::from_rotation_y(angle_offset) * accepted.aim;
                let net_id = NetId(*sinks.next_projectile_net_id);
                *sinks.next_projectile_net_id = sinks
                    .next_projectile_net_id
                    .wrapping_add(1)
                    .max(0x4000_0000);
                world.spawn((ServerProjectile {
                    net_id,
                    ability_id: accepted.ability_id,
                    owner: accepted.caster,
                    team: Team::Player,
                    position: accepted.spawn_origin,
                    velocity: dir * speed,
                    ttl,
                    damage: scaled_damage,
                    crit_chance: accepted.crit_chance,
                    crit_damage: accepted.crit_damage,
                    pierce_remaining: pierce,
                    size: 0.6,
                    apply_debuff,
                },));
            }
        }
        AbilityKind::AoeZone {
            radius, duration, tick_interval, apply_debuff,
        } => {
            let pos = accepted
                .placed_target
                .unwrap_or(accepted.origin + accepted.aim * 5.0);
            sinks.aoe_zones.push(ServerAoeZone {
                owner: accepted.caster,
                position: Vec3::new(pos.x, 0.0, pos.z),
                radius,
                damage_per_tick: scaled_damage,
                crit_chance: accepted.crit_chance,
                crit_damage: accepted.crit_damage,
                tick_interval,
                duration,
                elapsed: 0.0,
                tick_timer: 0.0,
                apply_debuff,
            });
        }
        AbilityKind::Channel {
            duration, tick_interval, effect, apply_debuff, cancel_on_move,
        } => {
            // Channels need a caster entity to attach to. AI
            // casts pass `None`; if a future enemy ever wants
            // to channel we'll add an enemy-side channel
            // component, but for now it's a no-op.
            if let Some(entity) = accepted.caster_entity {
                let _ = world.insert_one(
                    entity,
                    super::channel::ServerChannel {
                        ability_id: accepted.ability_id,
                        remaining: duration,
                        tick_interval,
                        tick_acc: 0.0,
                        effect,
                        crit_chance: accepted.crit_chance,
                        crit_damage: accepted.crit_damage,
                        apply_debuff,
                        aim: accepted.aim,
                        cancel_on_move,
                    },
                );
            }
        }
        AbilityKind::ClientOnly => {
            // A handful of "client-only" abilities still have
            // a kinematic side-effect on the caster. Evasive
            // Roll is the canonical example: pure visual on
            // most clients, but the server has to drive the
            // actual translation so prediction stays
            // consistent and other players see the dodge
            // happen authoritatively.
            if accepted.ability_id == id::EVASIVE_ROLL {
                if let Some(entity) = accepted.caster_entity {
                    if let Ok(mut p) = world.get::<&mut ServerPlayer>(entity) {
                        rift_game::kinematic::start_roll(&mut p.k, accepted.aim);
                    }
                }
            }
        }
        // ── AI-shaped kinds ─────────────────────────────────────
        AbilityKind::EnemyProjectiles {
            count, spread, speed, ttl, windup: _, size,
        } => {
            for i in 0..count {
                let angle_offset = if count > 1 {
                    let t = i as f32 / (count - 1) as f32 - 0.5;
                    t * spread
                } else {
                    0.0
                };
                let dir = Quat::from_rotation_y(angle_offset) * accepted.aim;
                let net_id = NetId(*sinks.next_projectile_net_id);
                *sinks.next_projectile_net_id = sinks
                    .next_projectile_net_id
                    .wrapping_add(1)
                    .max(0x4000_0000);
                world.spawn((ServerProjectile {
                    net_id,
                    ability_id: accepted.ability_id,
                    owner: accepted.caster,
                    team: Team::Enemy,
                    position: accepted.spawn_origin,
                    velocity: dir * speed,
                    ttl,
                    damage: scaled_damage,
                    crit_chance: 0.0,
                    crit_damage: 0.0,
                    pierce_remaining: 0,
                    size,
                    apply_debuff: None,
                },));
            }
        }
        AbilityKind::DelayedAoe { radius, windup: _ } => {
            // `param_a > 0.0` overrides the registry radius
            // for this cast (e.g. boss slam enrage scaling
            // captured at wind-up start).
            let effective_radius = if accepted.param_a > 0.0 {
                accepted.param_a
            } else {
                radius
            };
            let r2 = effective_radius * effective_radius;
            for (pe, pp) in sinks.player_targets {
                let dx = pp.x - accepted.origin.x;
                let dz = pp.z - accepted.origin.z;
                if dx * dx + dz * dz <= r2 {
                    sinks.player_damage.push((*pe, scaled_damage));
                }
            }
            // Paired impact visual — sustained ground ring
            // is already up from the wind-up event; this
            // fires the shockwave.
            sinks.events.push(WorldEvent::AbilityCast {
                caster: accepted.caster,
                ability: id::GROUND_SLAM_IMPACT as u16,
                origin: accepted.origin.to_array(),
                dir: [effective_radius, 0.0],
                target: Some(accepted.origin.to_array()),
                start_tick: tick,
            });
        }
        AbilityKind::Summon {
            count, role, hp_mult, ring_radius, windup: _,
        } => {
            // Spawn enemies in a ring at evenly-spaced
            // angles. Net-id allocation stays in `Sim::step`
            // so we route through `summons` instead of
            // inserting entities directly.
            let n = count.max(1) as i32;
            for i in 0..n {
                let theta = std::f32::consts::TAU * (i as f32) / (n as f32);
                let pos = accepted.origin
                    + Vec3::new(theta.cos(), 0.0, theta.sin()) * ring_radius;
                sinks.summons.push((pos, role, hp_mult));
            }
        }
    }
}

// ─── Public wrappers ─────────────────────────────────────────────────────

/// Resolve a `ClientMsg::CastAbility` into authoritative
/// effects. Thin wrapper around [`submit`] + [`dispatch`] —
/// kept so the network handler call site stays unchanged.
///
/// Silently no-ops when the ability is on cooldown, the caster
/// isn't connected, or the ability id is unknown — the client
/// shouldn't have asked, and there's nothing useful for us to
/// do.
pub fn cast(
    world: &mut hecs::World,
    sessions: &HashMap<ClientId, Entity>,
    cooldowns: &mut CooldownTable,
    aoe_zones: &mut Vec<ServerAoeZone>,
    events: &mut Vec<WorldEvent>,
    next_projectile_net_id: &mut u32,
    client_id: ClientId,
    ability_id: u8,
    client_origin: [f32; 3],
    aim_dir: [f32; 2],
    placed_target: Option<[f32; 3]>,
    tick: NetTick,
) {
    let aim = {
        let v = glam::Vec2::from(aim_dir).normalize_or_zero();
        Vec3::new(v.x, 0.0, v.y)
    };
    let intent = CombatIntent::Player {
        client_id,
        ability_id,
        client_origin: Vec3::from_array(client_origin),
        aim,
        placed_target: placed_target.map(Vec3::from),
    };
    let Some(accepted) = submit(world, sessions, cooldowns, intent) else {
        return;
    };
    // Player casts emit the AbilityCast wire event right at
    // cast time — there's no separate windup/resolve split for
    // the player path today. AI casts emit their own
    // `EnemyCast::Start` event up in the AI tick before this
    // function ever runs, so dispatch never has to.
    events.push(WorldEvent::AbilityCast {
        caster: accepted.caster,
        ability: accepted.ability_id as u16,
        origin: accepted.origin.to_array(),
        dir: [accepted.aim.x, accepted.aim.z],
        target: accepted.placed_target.map(|t| t.to_array()),
        start_tick: tick,
    });
    // Player casts don't currently produce summons or
    // player→player damage rows, but the kernel sinks need
    // valid references regardless.
    let mut summons: Vec<(Vec3, u8, f32)> = Vec::new();
    let mut player_damage: Vec<(Entity, f32)> = Vec::new();
    let no_targets: [(Entity, Vec3); 0] = [];
    let mut sinks = DispatchSinks {
        aoe_zones,
        events,
        next_projectile_net_id,
        player_damage: &mut player_damage,
        summons: &mut summons,
        player_targets: &no_targets,
    };
    dispatch(world, accepted, &mut sinks, tick);
    debug_assert!(
        summons.is_empty() && player_damage.is_empty(),
        "player cast() emitted enemy-shaped effects",
    );
}

// ─── Enemy-side wrapper ──────────────────────────────────────────────────
//
// Enemies share the [`rift_game::abilities::REGISTRY`] table
// with players: every projectile, slam, and summon has a single
// authoritative entry holding its tuning. The AI in
// `super::enemy` ticks its own per-attack cooldowns + wind-up
// timers — there's no entity-keyed cooldown table — and emits
// two kinds of events through `AiOutcome`:
//
//  * `EnemyCast::Start` — wind-up has begun. Lifted by
//    `Sim::step` into a `WorldEvent::AbilityCast` so clients
//    can play the telegraph (cast pose, ground ring, …).
//  * `EnemyCast::Resolve` — wind-up has expired. Lifted by
//    `Sim::step` through [`resolve_enemy_cast`], which builds
//    a [`CombatIntent::Ai`] and runs it through the shared
//    [`submit`] + [`dispatch`] pipeline.
//
// The split lets the AI control *when* the wind-up ends without
// the kernel ever caring about role-specific state.

use super::projectile::Team;

/// One in-flight enemy ability resolution emitted by the AI tick
/// after a wind-up expires. Kept as a distinct struct (rather
/// than building a [`CombatIntent::Ai`] in the AI tick) so the
/// existing `EnemyCast::Resolve` payload doesn't have to leak
/// the kernel types into [`super::enemy`].
#[derive(Clone, Copy, Debug)]
pub struct EnemyCastResolve {
    pub caster: NetId,
    /// Caster body position at resolve time.
    pub origin: Vec3,
    /// Aim direction (XZ-plane unit). Centre of the fan for
    /// `EnemyProjectiles`; ignored for `DelayedAoe` / `Summon`.
    pub aim: Vec3,
    /// One of `rift_game::abilities::id::*`.
    pub ability_id: u8,
    /// Floor damage scalar captured at cast start so any
    /// floor-mid-cast difficulty change doesn't retroactively
    /// rescale damage.
    pub damage_mult: f32,
    /// Optional ability-specific scalar override. For
    /// [`AbilityKind::DelayedAoe`] this overrides the registry
    /// radius (m) when non-zero, letting the AI bake in
    /// per-cast scaling (e.g. boss slam enrage). `0.0` means
    /// "use the registry value".
    pub param_a: f32,
}

/// Resolve one [`EnemyCastResolve`] through the kernel. Thin
/// wrapper that builds a [`CombatIntent::Ai`], runs [`submit`]
/// (trivial for AI), and feeds the result through [`dispatch`].
///
/// `players` is the live `(entity, position)` snapshot used by
/// `DelayedAoe` to find who's inside the radius. `melee_damage`
/// and `summon_queue` are the same `AiOutcome` channels the AI
/// uses, reused here so resolves go through one damage / spawn
/// path with the rest of the tick.
pub fn resolve_enemy_cast(
    cast: EnemyCastResolve,
    players: &[(Entity, Vec3)],
    world: &mut hecs::World,
    next_projectile_net_id: &mut u32,
    melee_damage: &mut Vec<(Entity, f32)>,
    summon_queue: &mut Vec<(Vec3, u8, f32)>,
    events: &mut Vec<WorldEvent>,
    tick: NetTick,
) {
    // AI submit needs a sessions / cooldowns reference but
    // doesn't read either; pass empty stand-ins so the kernel
    // signature stays uniform.
    let sessions: HashMap<ClientId, Entity> = HashMap::new();
    let mut cooldowns: CooldownTable = HashMap::new();
    let intent = CombatIntent::Ai {
        caster: cast.caster,
        ability_id: cast.ability_id,
        origin: cast.origin,
        aim: cast.aim,
        damage_mult: cast.damage_mult,
        param_a: cast.param_a,
    };
    let Some(accepted) = submit(world, &sessions, &mut cooldowns, intent) else {
        return;
    };
    // Dummy sink for `aoe_zones` — enemy resolves never queue
    // persistent AoE zones today (their slam is a single-tick
    // `DelayedAoe`). Future enemy abilities that do can wire
    // through `Sim::step`'s real `aoe_zones` slot.
    let mut aoe_zones: Vec<ServerAoeZone> = Vec::new();
    let mut sinks = DispatchSinks {
        aoe_zones: &mut aoe_zones,
        events,
        next_projectile_net_id,
        player_damage: melee_damage,
        summons: summon_queue,
        player_targets: players,
    };
    dispatch(world, accepted, &mut sinks, tick);
    debug_assert!(
        aoe_zones.is_empty(),
        "enemy cast queued a persistent AoE zone (kind not supported on AI path)",
    );
}
