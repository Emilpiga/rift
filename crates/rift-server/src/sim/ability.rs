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
//! Both call sites (`Sim::cast_ability` for players, `Sim::step`
//! for AI cast resolves) compose `submit` + `dispatch` inline.

use std::collections::HashMap;

use glam::{Quat, Vec3};
use hecs::Entity;
use rift_dungeon::Floor;
use rift_net::{
    messages::WorldEvent,
    ClientId, NetId, NetTick,
};

pub use rift_game::abilities::id;
pub use rift_game::abilities::{lookup, AbilityKind, TargetingMode};

use super::player::ServerPlayer;
use super::projectile::{ServerAoeZone, ServerProjectile, Team};

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
        /// Friendly entity target for heal-style casts. `None`
        /// for any ability that doesn't use
        /// [`TargetingMode::TargetEntity`].
        target_net_id: Option<NetId>,
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
        /// Caster's crit chance (0..1). `0.0` means "no crit".
        crit_chance: f32,
        crit_damage: f32,
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
    /// Which side this cast belongs to. Drives team-aware
    /// downstream effects (projectile target list, AoE-zone
    /// target list).
    pub team: Team,
    /// Ability-specific scalar override (e.g. effective slam
    /// radius for `DelayedAoe`). `0.0` means "use the
    /// registry value".
    pub param_a: f32,
    /// Resolved entity target for heal-style casts. Set by
    /// [`submit`] from the client-supplied `target_net_id`
    /// after validation; dispatch arms that need it (Heal,
    /// HealOverTimeTarget) panic-on-`None` would be a bug, so
    /// they silently no-op instead.
    pub target_entity: Option<Entity>,
    /// Wire id of `target_entity`, mirrored here so dispatch
    /// can populate `WorldEvent::Heal` without re-querying
    /// the player row.
    pub target_net_id: Option<NetId>,
    /// World-space position of the heal target at submit
    /// time. Used as the `Heal` event position so the
    /// floating green number anchors on the right body even
    /// if the target moves between submit and dispatch.
    pub target_position: Option<Vec3>,
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
    floor: &Floor,
    intent: CombatIntent,
) -> Option<AcceptedCast> {
    match intent {
        CombatIntent::Player {
            client_id,
            ability_id,
            client_origin,
            aim,
            placed_target,
            target_net_id,
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

            // Friendly entity-target validation. For abilities
            // that use [`TargetingMode::TargetEntity`] we
            // require a live, in-range, line-of-sight target;
            // any failure silently drops the cast (no cooldown
            // burn, same shape as a loadout reject). A `None`
            // wire target falls back to the caster — Landing 1
            // ships before the client gains a hover-pick UI, so
            // self-cast is the implicit default.
            let (target_entity, target_position) =
                if matches!(ability.targeting, TargetingMode::TargetEntity) {
                    let want = target_net_id.unwrap_or(net_id);
                    // Find the player whose `net_id` matches.
                    // Self-cast is allowed (Shift+key) — the
                    // alive / range / LOS gates apply
                    // uniformly.
                    let mut found: Option<(Entity, Vec3)> = None;
                    for (e, p) in world.query::<&ServerPlayer>().iter() {
                        if p.net_id == want && !p.is_dead_or_ghosting() {
                            found = Some((e, p.k.position));
                            break;
                        }
                    }
                    let (te, tpos) = found?;
                    let d = tpos - body;
                    let dist2 = d.x * d.x + d.z * d.z;
                    if dist2 > ability.range * ability.range {
                        return None;
                    }
                    if !floor.line_of_sight(body, tpos) {
                        return None;
                    }
                    (Some(te), Some(tpos))
                } else {
                    (None, None)
                };

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
                team: Team::Player,
                param_a: 0.0,
                target_entity,
                // Resolved net id may differ from the request:
                // a `None` wire id was rewritten to the caster
                // for self-cast.
                target_net_id: target_entity.map(|_| {
                    target_net_id.unwrap_or(net_id)
                }),
                target_position,
            })
        }
        CombatIntent::Ai {
            caster,
            ability_id,
            origin,
            aim,
            damage_mult,
            crit_chance,
            crit_damage,
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
                crit_chance,
                crit_damage,
                team: Team::Enemy,
                param_a,
                target_entity: None,
                target_net_id: None,
                target_position: None,
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
    /// Healing rows targeted at players. Drained by the caller
    /// after dispatch the same way `player_damage` is — keeps
    /// dispatch from poking `ServerPlayer.hp` directly while
    /// the projectile / channel borrow is in scope (matters for
    /// the AI-tick path even though current heal sources are
    /// player-only, because future buffs may queue heals from
    /// inside the same world borrow).
    pub player_heals: &'a mut Vec<(Entity, f32)>,
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
                    team: accepted.team,
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
                team: accepted.team,
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
            // to channel we'll pipe its entity through
            // `AcceptedCast.caster_entity` (insert directly
            // with `team: Team::Enemy`).
            if let Some(entity) = accepted.caster_entity {
                let _ = world.insert_one(
                    entity,
                    super::channel::ServerChannel {
                        ability_id: accepted.ability_id,
                        team: accepted.team,
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
            count, spread, speed, ttl, windup: _, size, apply_debuff,
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
                    team: accepted.team,
                    position: accepted.spawn_origin,
                    velocity: dir * speed,
                    ttl,
                    damage: scaled_damage,
                    crit_chance: accepted.crit_chance,
                    crit_damage: accepted.crit_damage,
                    pierce_remaining: 0,
                    size,
                    apply_debuff,
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
        // ── Friendly support kinds ─────────────────────────────
        AbilityKind::HealTarget { amount } => {
            // Submit already validated alive / range / LOS, so
            // a missing target_entity here means the cast
            // shouldn't have made it past submit — silently
            // no-op rather than panic.
            let (Some(target), Some(target_net), Some(tpos)) = (
                accepted.target_entity,
                accepted.target_net_id,
                accepted.target_position,
            ) else {
                return;
            };
            sinks.player_heals.push((target, amount));
            sinks.events.push(WorldEvent::Heal {
                caster: accepted.caster,
                target: target_net,
                amount,
                over_time: false,
                position: tpos.to_array(),
            });
        }
        AbilityKind::HealOverTimeTarget { apply_buff } => {
            let Some(target) = accepted.target_entity else {
                return;
            };
            // The buff system keeps its own tick clock — we
            // just refresh / apply at the registry's default
            // duration (tooltip says 10 s, registry agrees).
            if let Ok(mut stack) =
                world.get::<&mut super::effect::EffectStack>(target)
            {
                stack.apply(apply_buff, None);
            }
        }
    }
}
