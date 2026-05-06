//! Server-side projectiles and persistent AoE damage zones.
//!
//! Projectiles live as ECS entities so the snapshot builder can
//! iterate them with the same query pattern it uses for players and
//! enemies. AoE zones are short-lived and don't need ECS — they sit
//! in a Vec on `Sim`.

use glam::Vec3;
use hecs::Entity;
use rift_dungeon::{Floor, Tile};
use rift_net::{messages::WorldEvent, NetId};

use super::enemy::ServerEnemy;

/// One in-flight server-side projectile.
#[derive(Clone, Debug)]
pub struct ServerProjectile {
    pub net_id: NetId,
    pub ability_id: u8,
    pub owner: NetId,
    pub position: Vec3,
    pub velocity: Vec3,
    pub ttl: f32,
    pub damage: f32,
    /// Caster's crit chance at the time of cast (0..1).
    pub crit_chance: f32,
    /// Caster's crit damage multiplier at the time of cast
    /// (e.g. `0.5` = +50 %).
    pub crit_damage: f32,
    pub pierce_remaining: u32,
    pub size: f32,
    /// Debuff to apply on hit (if any). Wire id from
    /// `rift_game::debuffs::id::*`.
    pub apply_debuff: Option<u8>,
}

/// Active server-side AoE damage zone.
#[derive(Clone, Debug)]
pub struct ServerAoeZone {
    pub owner: NetId,
    pub position: Vec3,
    pub radius: f32,
    pub damage_per_tick: f32,
    pub crit_chance: f32,
    pub crit_damage: f32,
    pub tick_interval: f32,
    pub duration: f32,
    pub elapsed: f32,
    pub tick_timer: f32,
    /// Debuff to apply on every enemy each tick hits.
    pub apply_debuff: Option<u8>,
}

/// One projectile↔enemy hit, queued for application after the
/// ECS borrow ends. Public-in-crate so sibling modules
/// (`channel`, ...) can reuse the damage-application path.
pub(super) struct Hit {
    pub enemy: Entity,
    pub enemy_net_id: NetId,
    pub enemy_pos: Vec3,
    pub damage: f32,
    /// Crit roll inputs from the source caster's stats.
    /// `crit_chance` 0..1; `crit_damage` is the multiplier added
    /// on top of 1.0 when the roll succeeds.
    pub crit_chance: f32,
    pub crit_damage: f32,
    /// Stable seed for the crit roll. The same `(tick, enemy,
    /// owner, ability)` tuple yields the same outcome every
    /// replay, so server determinism is preserved.
    pub crit_seed: u64,
    pub apply_debuff: Option<u8>,
}

/// Despawn every `ServerProjectile` in the world. Called on floor
/// change.
pub fn despawn_all(world: &mut hecs::World) {
    let stale: Vec<Entity> = world
        .query::<&ServerProjectile>()
        .iter()
        .map(|(e, _)| e)
        .collect();
    for e in stale {
        let _ = world.despawn(e);
    }
}

/// 64-bit mixing function used to derive deterministic per-hit
/// crit seeds. Splitmix64 — cheap, well-distributed, good enough
/// for one boolean roll per hit.
pub(super) fn mix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn hit_seed(tick: rift_net::NetTick, enemy: NetId, owner: NetId, salt: u64) -> u64 {
    mix64(
        (tick.0 as u64)
            ^ ((enemy.0 as u64) << 8)
            ^ ((owner.0 as u64) << 24)
            ^ salt.rotate_left(7),
    )
}

/// Check whether the projectile's XZ centre lies inside a wall
/// tile. Out-of-bounds counts as a wall (matches
/// `kinematic::tile_at`). Y is ignored — projectiles travel at a
/// fixed cast height.
fn hits_wall(floor: &Floor, p: Vec3) -> bool {
    // Same world→grid convention as `kinematic::tile_at`: tile
    // (i, j) is centred at world (i, j), spanning [i-0.5, i+0.5].
    let gx = (p.x + 0.5).floor();
    let gz = (p.z + 0.5).floor();
    if gx < 0.0 || gz < 0.0 {
        return true;
    }
    floor.get(gx as usize, gz as usize) == Tile::Wall
}

/// Integrate every projectile, run XZ collision against the enemy
/// snapshot, and apply damage. Pushes a `WorldEvent::Damage` per hit
/// and a `WorldEvent::Death` per kill into `events`. Despawns dead
/// projectiles and dead enemies.
pub fn tick(
    world: &mut hecs::World,
    floor: &Floor,
    enemies: &[(Entity, Vec3, NetId, f32)],
    ctx: &mut super::loot::DeathCtx<'_>,
    dt: f32,
) {
    let mut hits: Vec<Hit> = Vec::new();
    let mut to_despawn: Vec<Entity> = Vec::new();
    for (pe, proj) in world.query_mut::<&mut ServerProjectile>() {
        proj.position += proj.velocity * dt;
        proj.ttl -= dt;
        if proj.ttl <= 0.0 {
            to_despawn.push(pe);
            continue;
        }
        // Wall collision: if the projectile's new XZ position
        // sits inside a wall tile, detonate immediately so it
        // doesn't sail through the dungeon geometry. Tile size
        // matches `kinematic::tile_at` (1 unit per tile, world
        // origin at (0,0)).
        if hits_wall(floor, proj.position) {
            to_despawn.push(pe);
            continue;
        }
        let mut consumed = false;
        for (en_entity, en_pos, en_net_id, en_radius) in enemies {
            let dx = proj.position.x - en_pos.x;
            let dz = proj.position.z - en_pos.z;
            let dist_xz = (dx * dx + dz * dz).sqrt();
            if dist_xz < *en_radius + proj.size * 0.5 {
                hits.push(Hit {
                    enemy: *en_entity,
                    enemy_net_id: *en_net_id,
                    enemy_pos: *en_pos,
                    damage: proj.damage,
                    crit_chance: proj.crit_chance,
                    crit_damage: proj.crit_damage,
                    crit_seed: hit_seed(ctx.tick, *en_net_id, proj.owner, proj.net_id.0 as u64),
                    apply_debuff: proj.apply_debuff,
                });
                if proj.pierce_remaining > 0 {
                    proj.pierce_remaining -= 1;
                } else {
                    consumed = true;
                    break;
                }
            }
        }
        if consumed {
            to_despawn.push(pe);
        }
    }
    for e in to_despawn {
        let _ = world.despawn(e);
    }
    apply_hits_to_enemies(world, hits, ctx);
}

/// Tick every AoE zone: advance its clock, apply damage on each
/// `tick_interval`, expire when the duration elapses.
pub fn tick_aoe(
    world: &mut hecs::World,
    zones: &mut Vec<ServerAoeZone>,
    enemies: &[(Entity, Vec3, NetId, f32)],
    ctx: &mut super::loot::DeathCtx<'_>,
    dt: f32,
) {
    let mut hits: Vec<Hit> = Vec::new();
    let mut idx = 0;
    while idx < zones.len() {
        let zone = &mut zones[idx];
        zone.elapsed += dt;
        zone.tick_timer += dt;
        let mut tick = false;
        if zone.tick_timer >= zone.tick_interval {
            zone.tick_timer -= zone.tick_interval;
            tick = true;
        }
        let zone_pos = zone.position;
        let zone_radius = zone.radius;
        let zone_dmg = zone.damage_per_tick;
        let zone_crit_chance = zone.crit_chance;
        let zone_crit_damage = zone.crit_damage;
        let zone_owner = zone.owner;
        let expired = zone.elapsed >= zone.duration;
        if tick {
            for (en_entity, en_pos, en_net_id, _r) in enemies {
                let dx = en_pos.x - zone_pos.x;
                let dz = en_pos.z - zone_pos.z;
                if dx * dx + dz * dz < zone_radius * zone_radius {
                    hits.push(Hit {
                        enemy: *en_entity,
                        enemy_net_id: *en_net_id,
                        enemy_pos: *en_pos,
                        damage: zone_dmg,
                        crit_chance: zone_crit_chance,
                        crit_damage: zone_crit_damage,
                        crit_seed: hit_seed(
                            ctx.tick,
                            *en_net_id,
                            zone_owner,
                            (zone.elapsed.to_bits() as u64) ^ 0xA0E5_BEEF,
                        ),
                        apply_debuff: zone.apply_debuff,
                    });
                }
            }
        }
        if expired {
            zones.swap_remove(idx);
        } else {
            idx += 1;
        }
    }
    apply_hits_to_enemies(world, hits, ctx);
}

/// Apply a batch of hits to enemies: subtract HP (scaled by the
/// target's `IncomingDamageMult` debuffs), push `Damage` events,
/// `Death` + despawn for any enemy that crosses zero HP, and apply
/// any `apply_debuff` carried by the hit.
pub(super) fn apply_hits_to_enemies(
    world: &mut hecs::World,
    hits: Vec<Hit>,
    ctx: &mut super::loot::DeathCtx<'_>,
) {
    let mut dead: Vec<(Entity, NetId)> = Vec::new();
    for hit in hits {
        // Read the incoming-damage multiplier off the target's
        // debuff stack (if any) before grabbing the enemy mutably.
        let dmg_mult = world
            .get::<&super::debuff::DebuffStack>(hit.enemy)
            .map(|s| s.incoming_damage_mult())
            .unwrap_or(1.0);
        // Roll crit using the per-hit deterministic seed. A
        // mixed `crit_seed` gives a uniform `[0, 1)` float; we
        // crit when it lands under the caster's chance.
        let roll = (mix64(hit.crit_seed) >> 40) as f32 / (1u32 << 24) as f32;
        let crit = hit.crit_chance > 0.0 && roll < hit.crit_chance;
        let crit_mult = if crit { 1.0 + hit.crit_damage } else { 1.0 };
        let scaled = hit.damage * dmg_mult * crit_mult;
        if let Ok(mut en) = world.get::<&mut ServerEnemy>(hit.enemy) {
            // Already dying \u2014 ignore further hits so we don't
            // double-emit Damage events on the same corpse.
            if en.is_dying() {
                continue;
            }
            en.hp = (en.hp - scaled).max(0.0);
            let died = en.hp <= 0.0;
            drop(en);
            ctx.events.push(WorldEvent::Damage {
                target: hit.enemy_net_id,
                amount: scaled,
                crit,
                position: hit.enemy_pos.to_array(),
            });
            // Apply any debuff carried by the source ability. We
            // do this *after* the damage write so DoT clocks
            // start from now.
            if let Some(debuff_id) = hit.apply_debuff {
                if let Ok(mut stack) =
                    world.get::<&mut super::debuff::DebuffStack>(hit.enemy)
                {
                    stack.apply(debuff_id, None);
                }
            }
            if died {
                dead.push((hit.enemy, hit.enemy_net_id));
            }
        }
    }
    super::loot::finalise_kills(world, ctx, dead);
}
