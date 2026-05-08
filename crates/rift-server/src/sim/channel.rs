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

use super::enemies::ServerEnemy;
use super::player::ServerPlayer;
use super::projectile::{apply_hits_to_enemies, mix64, Hit, Team, PLAYER_HIT_RADIUS};

/// Component added to a player or enemy entity while a channel
/// is active. `team` drives the target list and the damage
/// routing (enemies for `Player`-team channels, players for
/// `Enemy`-team channels).
#[derive(Clone, Debug)]
pub struct ServerChannel {
    pub ability_id: u8,
    pub team: Team,
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
    /// from the caster's current `aim_yaw` so the beam follows the
    /// cursor while channeling.
    pub aim: Vec3,
    /// If `true`, any horizontal movement input cancels the
    /// channel. Mirrors the ability's flag.
    pub cancel_on_move: bool,
}

/// Tick every active channel and queue damage / debuff
/// applications.
///
/// Player-team channels hit enemies and route through
/// `apply_hits_to_enemies` like projectile hits. Enemy-team
/// channels hit players and produce `(player, damage)` rows
/// returned to the caller for `apply_player_damage`.
///
/// `enemies` is the live snapshot used by player-team channels;
/// `players` is the equivalent for enemy-team channels.
///
/// Borrow strategy: walk player-attached channels first
/// (`(&ServerPlayer, &mut ServerChannel)`), then enemy-attached
/// channels (`(&ServerEnemy, &mut ServerChannel)`). Hits are
/// queued during the walk and applied after the borrows end.
pub fn tick(
    world: &mut hecs::World,
    enemies: &[(Entity, Vec3, NetId, f32)],
    players: &[(Entity, Vec3)],
    ctx: &mut super::combat_ctx::CombatCtx<'_>,
    tick_now: NetTick,
    dt: f32,
) -> Vec<(Entity, f32)> {
    let mut hits: Vec<Hit> = Vec::new();
    let mut player_damage: Vec<(Entity, f32)> = Vec::new();
    let mut player_debuffs: Vec<(Entity, u8, Option<Entity>, u8)> = Vec::new();
    let mut to_strip: Vec<Entity> = Vec::new();

    // 1. Player-attached channels.
    for (entity, (player, channel)) in
        world.query_mut::<(&ServerPlayer, &mut ServerChannel)>()
    {
        if channel.cancel_on_move
            && player.k.velocity.length_squared() > 0.05 * 0.05
        {
            channel.remaining = 0.0;
        }
        channel.remaining -= dt;
        channel.tick_acc += dt;
        let yaw = player.k.aim_yaw;
        channel.aim = Vec3::new(yaw.sin(), 0.0, yaw.cos());
        let caster_pos = player.k.position;
        let caster_net_id = player.net_id;
        while channel.tick_acc >= channel.tick_interval && channel.remaining > -dt {
            channel.tick_acc -= channel.tick_interval;
            ctx.events.push(WorldEvent::ChannelTick {
                caster: caster_net_id,
                ability: channel.ability_id as u16,
                position: caster_pos.to_array(),
                dir: [channel.aim.x, channel.aim.z],
                tick: tick_now,
            });
            collect_hits_for_effect(
                channel,
                Some(entity),
                caster_pos,
                caster_net_id,
                tick_now,
                enemies,
                players,
                &mut hits,
                &mut player_damage,
                &mut player_debuffs,
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

    // 2. Enemy-attached channels. Same shape as the player
    //    pass — refresh aim from the caster's `aim_yaw`,
    //    cancel-on-move from the caster's velocity, queue
    //    visual events + hits, mark expired channels.
    for (entity, (en, channel)) in
        world.query_mut::<(&ServerEnemy, &mut ServerChannel)>()
    {
        if en.is_dying() {
            // Treat death as channel cancel so the visual
            // doesn't trail off a corpse.
            channel.remaining = 0.0;
        }
        if channel.cancel_on_move
            && en.k.velocity.length_squared() > 0.05 * 0.05
        {
            channel.remaining = 0.0;
        }
        channel.remaining -= dt;
        channel.tick_acc += dt;
        let yaw = en.k.aim_yaw;
        channel.aim = Vec3::new(yaw.sin(), 0.0, yaw.cos());
        let caster_pos = en.k.position;
        let caster_net_id = en.net_id;
        while channel.tick_acc >= channel.tick_interval && channel.remaining > -dt {
            channel.tick_acc -= channel.tick_interval;
            ctx.events.push(WorldEvent::ChannelTick {
                caster: caster_net_id,
                ability: channel.ability_id as u16,
                position: caster_pos.to_array(),
                dir: [channel.aim.x, channel.aim.z],
                tick: tick_now,
            });
            collect_hits_for_effect(
                channel,
                Some(entity),
                caster_pos,
                caster_net_id,
                tick_now,
                enemies,
                players,
                &mut hits,
                &mut player_damage,
                &mut player_debuffs,
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

    // 3. Apply queued enemy-side hits.
    apply_hits_to_enemies(world, hits, ctx);

    // 4. Apply queued player-side debuffs (rare path; flag-gated
    //    on `apply_debuff = Some(_)` per channel).
    for (player_entity, debuff_id, caster, ability_id) in player_debuffs {
        if let Ok(mut stack) =
            world.get::<&mut super::effect::EffectStack>(player_entity)
        {
            stack.apply(debuff_id, None, caster, ability_id);
        }
    }

    // 5. Strip expired channels.
    for entity in to_strip {
        let _ = world.remove_one::<ServerChannel>(entity);
    }

    player_damage
}

/// Resolve a channel's per-tick hit set. For `Player`-team
/// channels, queue Hits against the enemy snapshot. For
/// `Enemy`-team channels, queue `(player, damage)` rows
/// against the player snapshot — crit is rolled at hit time so
/// the player-damage path receives flat damage values.
fn collect_hits_for_effect(
    channel: &ServerChannel,
    caster_entity: Option<Entity>,
    caster_pos: Vec3,
    caster_net_id: NetId,
    tick_now: NetTick,
    enemies: &[(Entity, Vec3, NetId, f32)],
    players: &[(Entity, Vec3)],
    hits: &mut Vec<Hit>,
    player_damage: &mut Vec<(Entity, f32)>,
    player_debuffs: &mut Vec<(Entity, u8, Option<Entity>, u8)>,
) {
    let crit_chance = channel.crit_chance;
    let crit_damage = channel.crit_damage;
    let salt = (channel.ability_id as u64) ^ (channel.tick_acc.to_bits() as u64);
    // Build the per-target seed once — every hit on this tick
    // shares the salt and caster, only the target id varies.
    let seed_for = |target_id: u64| -> u64 {
        mix64(
            (tick_now.0 as u64)
                ^ (target_id << 8)
                ^ ((caster_net_id.0 as u64) << 24)
                ^ salt.rotate_left(7),
        )
    };
    // Helper: roll crit-multiplier at hit time. Used by the
    // enemy-team path (player-damage rows are flat).
    let roll_crit_mult = |seed: u64| -> f32 {
        if crit_chance > 0.0 {
            let roll = (mix64(seed) >> 40) as f32 / (1u32 << 24) as f32;
            if roll < crit_chance {
                1.0 + crit_damage
            } else {
                1.0
            }
        } else {
            1.0
        }
    };
    match channel.effect {
        ChannelEffect::AuraAroundCaster { radius, damage_per_tick } => {
            let r2 = radius * radius;
            match channel.team {
                Team::Player => {
                    for (en, en_pos, nid, _r) in enemies {
                        let dx = en_pos.x - caster_pos.x;
                        let dz = en_pos.z - caster_pos.z;
                        if dx * dx + dz * dz <= r2 {
                            hits.push(Hit {
                                enemy: *en,
                                enemy_net_id: *nid,
                                enemy_pos: *en_pos,
                                attacker: caster_net_id,
                                ability_id: channel.ability_id,
                                damage: damage_per_tick,
                                crit_chance,
                                crit_damage,
                                crit_seed: seed_for(nid.0 as u64),
                                apply_debuff: channel.apply_debuff,
                            });
                        }
                    }
                }
                Team::Enemy => {
                    for (pe, ppos) in players {
                        let dx = ppos.x - caster_pos.x;
                        let dz = ppos.z - caster_pos.z;
                        if dx * dx + dz * dz <= r2 {
                            let mult = roll_crit_mult(seed_for(pe.id() as u64));
                            player_damage.push((*pe, damage_per_tick * mult));
                            if let Some(id) = channel.apply_debuff {
                                player_debuffs.push((
                                    *pe,
                                    id,
                                    caster_entity,
                                    channel.ability_id,
                                ));
                            }
                        }
                    }
                }
            }
        }
        ChannelEffect::Beam { range, width, damage_per_tick, pierce_targets } => {
            let aim = channel.aim.normalize_or_zero();
            if aim.length_squared() < 1.0e-4 {
                return;
            }
            let right = Vec3::new(aim.z, 0.0, -aim.x);
            let cap = (pierce_targets as usize).saturating_add(1);
            // Player-side vs enemy-side share the projection
            // math but route into different sinks.
            match channel.team {
                Team::Player => {
                    let mut candidates: Vec<(f32, Hit)> = Vec::new();
                    for (en, en_pos, nid, _r) in enemies {
                        let to = Vec3::new(
                            en_pos.x - caster_pos.x,
                            0.0,
                            en_pos.z - caster_pos.z,
                        );
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
                                attacker: caster_net_id,
                                ability_id: channel.ability_id,
                                damage: damage_per_tick,
                                crit_chance,
                                crit_damage,
                                crit_seed: seed_for(nid.0 as u64),
                                apply_debuff: channel.apply_debuff,
                            },
                        ));
                    }
                    candidates.sort_by(|a, b| {
                        a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal)
                    });
                    for (_along, hit) in candidates.into_iter().take(cap) {
                        hits.push(hit);
                    }
                }
                Team::Enemy => {
                    let player_width = width.max(PLAYER_HIT_RADIUS);
                    let mut candidates: Vec<(f32, Entity)> = Vec::new();
                    for (pe, ppos) in players {
                        let to = Vec3::new(
                            ppos.x - caster_pos.x,
                            0.0,
                            ppos.z - caster_pos.z,
                        );
                        let along = to.dot(aim);
                        if along < 0.0 || along > range {
                            continue;
                        }
                        let lateral = to.dot(right).abs();
                        if lateral > player_width {
                            continue;
                        }
                        candidates.push((along, *pe));
                    }
                    candidates.sort_by(|a, b| {
                        a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal)
                    });
                    for (_along, pe) in candidates.into_iter().take(cap) {
                        let mult = roll_crit_mult(seed_for(pe.id() as u64));
                        player_damage.push((pe, damage_per_tick * mult));
                        if let Some(id) = channel.apply_debuff {
                            player_debuffs.push((
                                pe,
                                id,
                                caster_entity,
                                channel.ability_id,
                            ));
                        }
                    }
                }
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
