//! Server-side channel ticks.
//!
//! While a [`ServerChannel`] component is on a player entity, the
//! tick system fires the channel's [`ChannelEffect`] every
//! `tick_interval` until `remaining <= 0`. Each tick:
//!  - resolves enemies inside the effect's hit volume,
//!  - applies `damage_per_tick * IncomingDamageMult`,
//!  - applies the optional `apply_debuff`,
//!  - emits a [`WorldEvent::ChannelTick`] for client visuals.
//!
//! On expiry we drop the component and emit
//! [`WorldEvent::ChannelEnd`].
//!
//! Adding a new channel pattern: extend
//! [`rift_game::abilities::ChannelEffect`] and add a match arm in
//! [`hits_for_effect`].

use glam::Vec3;
use hecs::Entity;
use rift_game::abilities::ChannelEffect;
use rift_net::{messages::WorldEvent, NetId, NetTick};

use super::player::ServerPlayer;
use super::projectile::{apply_hits_to_enemies, Hit};

/// Component added to a player entity while a channel is active.
#[derive(Clone, Debug)]
pub struct ServerChannel {
    pub ability_id: u8,
    pub remaining: f32,
    pub tick_interval: f32,
    pub tick_acc: f32,
    pub effect: ChannelEffect,
    /// Caster's crit chance at the time of cast (0..1). Frozen
    /// for the duration of the channel; equipping a fresh ring
    /// mid-cast won't retroactively boost crit.
    pub crit_chance: f32,
    pub crit_damage: f32,
    pub apply_debuff: Option<u8>,
    /// Direction the caster is aiming. Refreshed every server tick
    /// from the player's current `aim_yaw` so the beam follows the
    /// cursor while channeling.
    pub aim: Vec3,
    /// If `true`, any horizontal movement input cancels the
    /// channel. Mirrors the ability's flag.
    pub cancel_on_move: bool,
}

/// Tick every active channel and queue damage / debuff
/// applications. Borrows are split: we collect the `(caster_pos,
/// caster_net_id, channel)` tuples up front, then dispatch hits
/// against the enemy world afterwards so we can mutate enemy
/// state freely.
pub fn tick(
    world: &mut hecs::World,
    enemies: &[(Entity, Vec3, NetId, f32)],
    ctx: &mut super::loot::DeathCtx<'_>,
    tick_now: NetTick,
    dt: f32,
) {
    // 1. Walk channels: advance clocks, refresh aim from the
    //    player's current aim_yaw, decide which ones tick this
    //    frame, and queue the hits + visual events. Mark expired
    //    or movement-cancelled channels for stripping.
    let mut hits: Vec<Hit> = Vec::new();
    let mut to_strip: Vec<Entity> = Vec::new();
    for (entity, (player, channel)) in
        world.query_mut::<(&ServerPlayer, &mut ServerChannel)>()
    {
        // Movement-cancel: if the caster is moving and the
        // ability says so, end the channel immediately.
        if channel.cancel_on_move
            && player.k.velocity.length_squared() > 0.05 * 0.05
        {
            channel.remaining = 0.0;
        }

        channel.remaining -= dt;
        channel.tick_acc += dt;

        // Re-derive the caster's aim each tick so beams sweep
        // with the cursor. `aim_yaw` is updated by
        // `player::apply_inputs` from the latest InputCmd.
        let yaw = player.k.aim_yaw;
        channel.aim = Vec3::new(yaw.sin(), 0.0, yaw.cos());

        let caster_pos = player.k.position;
        let caster_net_id = player.net_id;

        while channel.tick_acc >= channel.tick_interval && channel.remaining > -dt {
            channel.tick_acc -= channel.tick_interval;
            // Visual event for this tick.
            ctx.events.push(WorldEvent::ChannelTick {
                caster: caster_net_id,
                ability: channel.ability_id as u16,
                position: caster_pos.to_array(),
                dir: [channel.aim.x, channel.aim.z],
                tick: tick_now,
            });
            collect_hits_for_effect(
                channel,
                caster_pos,
                caster_net_id,
                tick_now,
                enemies,
                &mut hits,
            );
        }

        if channel.remaining <= 0.0 {
            to_strip.push(entity);
            ctx.events.push(WorldEvent::ChannelEnd {
                caster: caster_net_id,
                ability: channel.ability_id as u16,
            });
        }
    }

    // 2. Apply queued hits to the enemy world.
    apply_hits_to_enemies(world, hits, ctx);

    // 3. Strip expired channels.
    for entity in to_strip {
        let _ = world.remove_one::<ServerChannel>(entity);
    }
}

/// Resolve a channel's per-tick hit set against the enemy snapshot.
fn collect_hits_for_effect(
    channel: &ServerChannel,
    caster_pos: Vec3,
    caster_net_id: NetId,
    tick_now: NetTick,
    enemies: &[(Entity, Vec3, NetId, f32)],
    hits: &mut Vec<Hit>,
) {
    let crit_chance = channel.crit_chance;
    let crit_damage = channel.crit_damage;
    let salt = (channel.ability_id as u64) ^ (channel.tick_acc.to_bits() as u64);
    match channel.effect {
        ChannelEffect::AuraAroundCaster { radius, damage_per_tick } => {
            let r2 = radius * radius;
            for (en, en_pos, nid, _r) in enemies {
                let dx = en_pos.x - caster_pos.x;
                let dz = en_pos.z - caster_pos.z;
                if dx * dx + dz * dz <= r2 {
                    hits.push(Hit {
                        enemy: *en,
                        enemy_net_id: *nid,
                        enemy_pos: *en_pos,
                        damage: damage_per_tick,
                        crit_chance,
                        crit_damage,
                        crit_seed: super::projectile::mix64(
                            (tick_now.0 as u64)
                                ^ ((nid.0 as u64) << 8)
                                ^ ((caster_net_id.0 as u64) << 24)
                                ^ salt.rotate_left(7),
                        ),
                        apply_debuff: channel.apply_debuff,
                    });
                }
            }
        }
        ChannelEffect::Beam { range, width, damage_per_tick, pierce_targets } => {
            // Project each enemy onto the aim axis. In-range if
            // forward distance is in [0, range] and lateral
            // distance is in [0, width].
            let aim = channel.aim.normalize_or_zero();
            if aim.length_squared() < 1.0e-4 {
                return;
            }
            // Right vector in XZ plane (rotate aim 90°).
            let right = Vec3::new(aim.z, 0.0, -aim.x);
            // First pass: collect (along, hit-row) for every enemy
            // inside the beam corridor.
            let mut candidates: Vec<(f32, Hit)> = Vec::new();
            for (en, en_pos, nid, _r) in enemies {
                let to = Vec3::new(en_pos.x - caster_pos.x, 0.0, en_pos.z - caster_pos.z);
                let along = to.dot(aim);
                if along < 0.0 || along > range {
                    continue;
                }
                let lateral = to.dot(right).abs();
                if lateral > width {
                    continue;
                }
                candidates.push((
                    along,
                    Hit {
                        enemy: *en,
                        enemy_net_id: *nid,
                        enemy_pos: *en_pos,
                        damage: damage_per_tick,
                        crit_chance,
                        crit_damage,
                        crit_seed: super::projectile::mix64(
                            (tick_now.0 as u64)
                                ^ ((nid.0 as u64) << 8)
                                ^ ((caster_net_id.0 as u64) << 24)
                                ^ salt.rotate_left(7),
                        ),
                        apply_debuff: channel.apply_debuff,
                    },
                ));
            }
            // Sort nearest-first, then truncate to `pierce_targets + 1`
            // (the beam always hits the first target; `pierce_targets`
            // is *additional* enemies it can pass through).
            candidates.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let cap = (pierce_targets as usize).saturating_add(1);
            for (_along, hit) in candidates.into_iter().take(cap) {
                hits.push(hit);
            }
        }
    }
}

/// Strip every active channel. Called on floor change so we don't
/// trail per-player state across worlds.
pub fn clear_all(world: &mut hecs::World) {
    let stale: Vec<Entity> = world
        .query::<&ServerChannel>()
        .iter()
        .map(|(e, _)| e)
        .collect();
    for e in stale {
        let _ = world.remove_one::<ServerChannel>(e);
    }
}

/// Cancel one player's currently-active channel (if it matches
/// `ability_id`). Emits a `ChannelEnd` event so clients tear
/// their visual down immediately.
pub fn cancel(
    world: &mut hecs::World,
    entity: Entity,
    ability_id: u8,
    events: &mut Vec<WorldEvent>,
) {
    let active = world
        .get::<&ServerChannel>(entity)
        .ok()
        .map(|c| (c.ability_id, c.ability_id == ability_id));
    if let Some((_existing_id, matches)) = active {
        if matches {
            // Pull caster's net id for the ChannelEnd event before
            // dropping the component.
            let caster_net_id = world
                .get::<&ServerPlayer>(entity)
                .ok()
                .map(|p| p.net_id);
            let _ = world.remove_one::<ServerChannel>(entity);
            if let Some(nid) = caster_net_id {
                events.push(WorldEvent::ChannelEnd {
                    caster: nid,
                    ability: ability_id as u16,
                });
            }
        }
    }
}
