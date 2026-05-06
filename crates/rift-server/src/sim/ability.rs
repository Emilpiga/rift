//! Server-side ability dispatch.
//!
//! The static ability table itself lives in `rift-game` so the
//! client can share it (cooldown UI / button → ability mapping).
//! This module turns a `ClientMsg::CastAbility` into the actual
//! mutation of the simulation: spawn projectiles, queue an AoE
//! zone, or just emit the cast event for client-only effects.

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

/// Resolve a `ClientMsg::CastAbility` into authoritative effects.
///
/// Silently no-ops when the ability is on cooldown, the caster
/// isn't connected, or the ability id is unknown — the client
/// shouldn't have asked, and there's nothing useful for us to do.
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
    let Some(ability) = lookup(ability_id) else {
        return;
    };
    let cds = cooldowns.entry(client_id).or_insert([0.0; COOLDOWN_SLOTS]);
    let slot = (ability_id as usize).min(COOLDOWN_SLOTS - 1);
    if cds[slot] > 0.0 {
        return;
    }
    cds[slot] = ability.cooldown;

    let Some(&entity) = sessions.get(&client_id) else {
        return;
    };
    let (origin, caster_net_id, dmg_scalar, crit_chance, crit_damage) =
        match world.get::<&ServerPlayer>(entity) {
            Ok(p) => (
                p.k.position,
                p.net_id,
                p.damage_scalar(),
                p.stats.crit_chance,
                p.stats.crit_damage,
            ),
            Err(_) => return,
        };
    // Pre-scale the ability's authored base damage by the caster's
    // gear / attribute multiplier. Crit gets rolled per-hit on the
    // damage-application path using the values stamped below.
    let scaled_damage = ability.base_damage * dmg_scalar;
    let aim = {
        let v = glam::Vec2::from(aim_dir).normalize_or_zero();
        Vec3::new(v.x, 0.0, v.y)
    };

    // Trust the client's hand-position origin within a sanity
    // radius of the simulated player position (~2 m). This lets
    // projectiles visibly emerge from the casting hand on every
    // observer's screen without enabling a teleport-the-spawn
    // exploit. Out-of-range or zero origins fall back to the
    // simulated body position.
    let client_origin = Vec3::from_array(client_origin);
    let trusted_origin = if client_origin.distance_squared(origin) <= 2.0 * 2.0 {
        client_origin
    } else {
        origin + Vec3::Y * 1.25
    };

    events.push(WorldEvent::AbilityCast {
        caster: caster_net_id,
        ability: ability_id as u16,
        origin: origin.to_array(),
        dir: [aim.x, aim.z],
        target: placed_target,
        start_tick: tick,
    });

    match ability.kind {
        AbilityKind::Projectiles {
            count,
            spread,
            speed,
            ttl,
            pierce,
            apply_debuff,
        } => {
            let spawn_pos = trusted_origin + aim * 0.25;
            for i in 0..count {
                let angle_offset = if count > 1 {
                    let t = i as f32 / (count - 1) as f32 - 0.5;
                    t * spread
                } else {
                    0.0
                };
                let dir = Quat::from_rotation_y(angle_offset) * aim;
                let net_id = NetId(*next_projectile_net_id);
                *next_projectile_net_id = next_projectile_net_id
                    .wrapping_add(1)
                    .max(0x4000_0000);
                world.spawn((ServerProjectile {
                    net_id,
                    ability_id,
                    owner: caster_net_id,
                    position: spawn_pos,
                    velocity: dir * speed,
                    ttl,
                    damage: scaled_damage,
                    crit_chance,
                    crit_damage,
                    pierce_remaining: pierce,
                    size: 0.6,
                    apply_debuff,
                },));
            }
        }
        AbilityKind::AoeZone {
            radius,
            duration,
            tick_interval,
            apply_debuff,
        } => {
            let pos = placed_target
                .map(Vec3::from)
                .unwrap_or(origin + aim * 5.0);
            aoe_zones.push(ServerAoeZone {
                owner: caster_net_id,
                position: Vec3::new(pos.x, 0.0, pos.z),
                radius,
                damage_per_tick: scaled_damage,
                crit_chance,
                crit_damage,
                tick_interval,
                duration,
                elapsed: 0.0,
                tick_timer: 0.0,
                apply_debuff,
            });
        }
        AbilityKind::Channel {
            duration,
            tick_interval,
            effect,
            apply_debuff,
            cancel_on_move,
        } => {
            // Stamp a channel instance on the caster. The channel
            // tick system advances it every step; on duration
            // expiry the component is removed and a `ChannelEnd`
            // event fired.
            let _ = world.insert_one(
                entity,
                super::channel::ServerChannel {
                    ability_id,
                    remaining: duration,
                    tick_interval,
                    tick_acc: 0.0,
                    effect,
                    crit_chance,
                    crit_damage,
                    apply_debuff,
                    aim,
                    cancel_on_move,
                },
            );
        }
        AbilityKind::ClientOnly => {
            // A handful of "client-only" abilities still have a
            // kinematic side-effect on the caster. Evasive Roll is
            // the canonical example: pure visual on most clients,
            // but the server has to drive the actual translation
            // so prediction stays consistent and other players see
            // the dodge happen authoritatively.
            if ability_id == id::EVASIVE_ROLL {
                if let Ok(mut p) = world.get::<&mut ServerPlayer>(entity) {
                    rift_game::kinematic::start_roll(&mut p.k, aim);
                }
            }
        }
    }
}
