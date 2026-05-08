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

use super::enemies::ServerEnemy;

/// Which side a projectile belongs to. Drives target-list
/// filtering in [`tick`]: `Player`-team bolts collide with
/// enemies, `Enemy`-team bolts collide with players. Both
/// share the same `ServerProjectile` component, snapshot
/// shape, and net-id allocator — the client doesn't need to
/// know which team a bolt is on because the visual is keyed
/// off `ability_id`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Team {
    Player,
    Enemy,
}

/// One in-flight server-side projectile.
#[derive(Clone, Debug)]
pub struct ServerProjectile {
    pub net_id: NetId,
    pub ability_id: u8,
    pub owner: NetId,
    pub team: Team,
    pub position: Vec3,
    pub velocity: Vec3,
    pub ttl: f32,
    pub damage: f32,
    /// Caster's crit chance at the time of cast (0..1).
    /// `Team::Enemy` projectiles leave this at `0.0` since
    /// enemies don't crit today.
    pub crit_chance: f32,
    /// Caster's crit damage multiplier at the time of cast
    /// (e.g. `0.5` = +50 %). Ignored when `crit_chance == 0.0`.
    pub crit_damage: f32,
    /// Remaining pierce count. `Team::Enemy` projectiles use
    /// `0` (first-hit-wins).
    pub pierce_remaining: u32,
    pub size: f32,
    /// Debuff to apply on hit (if any). Wire id from
    /// `rift_game::effects::id::*`. `None` for `Team::Enemy`
    /// bolts today.
    pub apply_debuff: Option<u8>,
}

/// Active server-side AoE damage zone.
#[derive(Clone, Debug)]
pub struct ServerAoeZone {
    pub owner: NetId,
    /// Which side the zone damages. `Player`-team zones hit
    /// enemies; `Enemy`-team zones hit players.
    pub team: Team,
    pub position: Vec3,
    pub radius: f32,
    pub damage_per_tick: f32,
    pub crit_chance: f32,
    pub crit_damage: f32,
    pub tick_interval: f32,
    pub duration: f32,
    pub elapsed: f32,
    pub tick_timer: f32,
    /// Debuff to apply on every target each tick hits. Wire id
    /// from `rift_game::effects::id::*`. `None` for `Team::Enemy`
    /// zones today (no player-side debuff stack yet).
    pub apply_debuff: Option<u8>,
}

/// One projectile↔enemy hit, queued for application after the
/// ECS borrow ends. Public-in-crate so sibling modules
/// (`channel`, ...) can reuse the damage-application path.
pub(super) struct Hit {
    pub enemy: Entity,
    pub enemy_net_id: NetId,
    pub enemy_pos: Vec3,
    /// Net id of the attacker that produced this hit (player
    /// for projectile / AoE / channel sources, enemy for
    /// friendly-fire which is currently never seen). Used by
    /// [`apply_hits_to_enemies`] to drive aggro-on-hit + the
    /// pack-alert spread; resolved to a player [`Entity`] via
    /// the ECS at apply time so we don't have to thread the
    /// entity through every hit construction site.
    pub attacker: NetId,
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

/// Integrate every projectile, run XZ collision against the
/// appropriate target list per team, and apply damage. Pushes
/// a `WorldEvent::Damage` per `Player`-team hit and a
/// `WorldEvent::Death` per kill into `events`. Despawns dead
/// projectiles and dead enemies.
///
/// Returns the queued `(player_entity, damage)` rows produced
/// by `Enemy`-team projectile hits — the caller (`Sim::step`)
/// applies them through `apply_player_damage` so the player-
/// damage event ordering stays consistent with the rest of the
/// tick.
pub fn tick(
    world: &mut hecs::World,
    floor: &Floor,
    enemies: &[(Entity, Vec3, NetId, f32)],
    players: &[(Entity, Vec3)],
    ctx: &mut super::combat_ctx::CombatCtx<'_>,
    dt: f32,
) -> Vec<(Entity, f32)> {
    let mut hits: Vec<Hit> = Vec::new();
    let mut player_damage: Vec<(Entity, f32)> = Vec::new();
    let mut player_debuffs: Vec<(Entity, u8)> = Vec::new();
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
        match proj.team {
            Team::Player => {
                // Player bolts hit enemies. Pierce drains per
                // hit; first frame past zero stops the bolt.
                for (en_entity, en_pos, en_net_id, en_radius) in enemies {
                    let dx = proj.position.x - en_pos.x;
                    let dz = proj.position.z - en_pos.z;
                    let dist_xz = (dx * dx + dz * dz).sqrt();
                    if dist_xz < *en_radius + proj.size * 0.5 {
                        hits.push(Hit {
                            enemy: *en_entity,
                            enemy_net_id: *en_net_id,
                            enemy_pos: *en_pos,
                            attacker: proj.owner,
                            damage: proj.damage,
                            crit_chance: proj.crit_chance,
                            crit_damage: proj.crit_damage,
                            crit_seed: hit_seed(
                                ctx.tick,
                                *en_net_id,
                                proj.owner,
                                proj.net_id.0 as u64,
                            ),
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
            }
            Team::Enemy => {
                // Enemy bolts hit players. First-hit-wins:
                // pierce / debuffs are not currently wired
                // for enemy projectiles. Crit roll happens
                // here (rather than at damage-application)
                // because the player-damage path receives
                // flat `(Entity, f32)` rows; baking the
                // multiplier in keeps it source-agnostic.
                for (player_entity, ppos) in players {
                    let dx = proj.position.x - ppos.x;
                    let dz = proj.position.z - ppos.z;
                    let dist_xz = (dx * dx + dz * dz).sqrt();
                    if dist_xz < PLAYER_HIT_RADIUS + proj.size * 0.5 {
                        let crit_mult = if proj.crit_chance > 0.0 {
                            let seed = hit_seed(
                                ctx.tick,
                                NetId(0),
                                proj.owner,
                                proj.net_id.0 as u64,
                            );
                            let roll =
                                (mix64(seed) >> 40) as f32 / (1u32 << 24) as f32;
                            if roll < proj.crit_chance {
                                1.0 + proj.crit_damage
                            } else {
                                1.0
                            }
                        } else {
                            1.0
                        };
                        player_damage
                            .push((*player_entity, proj.damage * crit_mult));
                        if let Some(debuff_id) = proj.apply_debuff {
                            player_debuffs.push((*player_entity, debuff_id));
                        }
                        consumed = true;
                        break;
                    }
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
    // Player debuff applications run after the projectile
    // borrow ends so we can mutably grab each player's
    // `EffectStack` row without aliasing the projectile query.
    for (player_entity, debuff_id) in player_debuffs {
        if let Ok(mut stack) =
            world.get::<&mut super::effect::EffectStack>(player_entity)
        {
            stack.apply(debuff_id, None);
        }
    }
    player_damage
}

/// Tick every AoE zone: advance its clock, apply damage on each
/// `tick_interval`, expire when the duration elapses.
///
/// Returns the queued `(player_entity, damage)` rows produced
/// by `Team::Enemy` zones — applied through `apply_player_damage`
/// by the caller, the same way enemy projectile rows are.
pub fn tick_aoe(
    world: &mut hecs::World,
    zones: &mut Vec<ServerAoeZone>,
    enemies: &[(Entity, Vec3, NetId, f32)],
    players: &[(Entity, Vec3)],
    ctx: &mut super::combat_ctx::CombatCtx<'_>,
    dt: f32,
) -> Vec<(Entity, f32)> {
    let mut hits: Vec<Hit> = Vec::new();
    let mut player_damage: Vec<(Entity, f32)> = Vec::new();
    let mut player_debuffs: Vec<(Entity, u8)> = Vec::new();
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
        let zone_team = zone.team;
        let expired = zone.elapsed >= zone.duration;
        if tick {
            match zone_team {
                Team::Player => {
                    for (en_entity, en_pos, en_net_id, _r) in enemies {
                        let dx = en_pos.x - zone_pos.x;
                        let dz = en_pos.z - zone_pos.z;
                        if dx * dx + dz * dz < zone_radius * zone_radius {
                            hits.push(Hit {
                                enemy: *en_entity,
                                enemy_net_id: *en_net_id,
                                enemy_pos: *en_pos,
                                attacker: zone_owner,
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
                Team::Enemy => {
                    // Enemy-team zones bake crit at hit-time
                    // since the player-damage path takes flat
                    // `(Entity, f32)` rows.
                    for (player_entity, ppos) in players {
                        let dx = ppos.x - zone_pos.x;
                        let dz = ppos.z - zone_pos.z;
                        if dx * dx + dz * dz < zone_radius * zone_radius {
                            let crit_mult = if zone_crit_chance > 0.0 {
                                let seed = hit_seed(
                                    ctx.tick,
                                    NetId(0),
                                    zone_owner,
                                    (zone.elapsed.to_bits() as u64) ^ 0xA0E5_BEEF,
                                );
                                let roll = (mix64(seed) >> 40) as f32
                                    / (1u32 << 24) as f32;
                                if roll < zone_crit_chance {
                                    1.0 + zone_crit_damage
                                } else {
                                    1.0
                                }
                            } else {
                                1.0
                            };
                            player_damage
                                .push((*player_entity, zone_dmg * crit_mult));
                            if let Some(debuff_id) = zone.apply_debuff {
                                player_debuffs.push((*player_entity, debuff_id));
                            }
                        }
                    }
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
    for (player_entity, debuff_id) in player_debuffs {
        if let Ok(mut stack) =
            world.get::<&mut super::effect::EffectStack>(player_entity)
        {
            stack.apply(debuff_id, None);
        }
    }
    player_damage
}

/// Apply a batch of hits to enemies: subtract HP (scaled by the
/// target's `IncomingDamageMult` debuffs), push `Damage` events,
/// `Death` + despawn for any enemy that crosses zero HP, and apply
/// any `apply_debuff` carried by the hit.
/// Apply a batch of hits to enemies: subtract HP (scaled by the
/// target's `IncomingDamageMult` debuffs), push `Damage` events,
/// `Death` + despawn for any enemy that crosses zero HP, and apply
/// any `apply_debuff` carried by the hit. Also drives:
/// * **Threat accumulation** — every hit adds `scaled` to the
///   victim's `threat[attacker]` so the AI can target whoever
///   is dealing the most damage rather than whoever is
///   closest. See [`super::enemies::THREAT_DECAY_TAU`].
/// * **Stagger** — non-juggernaut enemies hit for more than
///   [`super::enemies::STAGGER_THRESHOLD`] of `hp_max`, or by any
///   crit, get `stagger_remaining` set to interrupt their next
///   AI tick. Pushes a [`WorldEvent::Hit`] so the client can
///   start a hit-react clip without waiting for the next snapshot.
/// * **Thorns** — elites with [`super::enemies::elite_mod::THORNS`]
///   reflect a fraction of every incoming hit back at the
///   attacker, queued through `ctx.player_damage_back`.
/// * **Aggro spread** — see [`super::enemies::notify_attacked`].
pub(super) fn apply_hits_to_enemies(
    world: &mut hecs::World,
    hits: Vec<Hit>,
    ctx: &mut super::combat_ctx::CombatCtx<'_>,
) {
    // Build a one-shot NetId → Entity map of live players so
    // the aggro-on-hit notify below can resolve the attacker
    // without hitting the ECS query path once per hit. Done
    // inline (cheap; player count is small) so callers don't
    // have to thread it through.
    let attacker_lookup: Vec<(NetId, Entity)> = world
        .query::<&super::player::ServerPlayer>()
        .iter()
        .map(|(e, p)| (p.net_id, e))
        .collect();

    let mut dead: Vec<(Entity, NetId)> = Vec::new();
    let mut aggro_alerts: Vec<(Entity, Entity)> = Vec::new();
    for hit in hits {
        // Read the incoming-damage multiplier off the target's
        // debuff stack (if any) before grabbing the enemy mutably.
        let dmg_mult = world
            .get::<&super::effect::EffectStack>(hit.enemy)
            .map(|s| s.incoming_damage_mult())
            .unwrap_or(1.0);
        // Roll crit using the per-hit deterministic seed. A
        // mixed `crit_seed` gives a uniform `[0, 1)` float; we
        // crit when it lands under the caster's chance.
        let roll = (mix64(hit.crit_seed) >> 40) as f32 / (1u32 << 24) as f32;
        let crit = hit.crit_chance > 0.0 && roll < hit.crit_chance;
        let crit_mult = if crit { 1.0 + hit.crit_damage } else { 1.0 };
        let scaled = hit.damage * dmg_mult * crit_mult;
        // Resolve attacker entity once per hit — used by threat,
        // aggro spread, and thorns reflection.
        let attacker_entity = attacker_lookup
            .iter()
            .find(|(nid, _)| *nid == hit.attacker)
            .map(|(_, e)| *e);
        if let Ok(mut en) = world.get::<&mut ServerEnemy>(hit.enemy) {
            // Already dying \u2014 ignore further hits so we don't
            // double-emit Damage events on the same corpse.
            if en.is_dying() {
                continue;
            }
            en.hp = (en.hp - scaled).max(0.0);
            let died = en.hp <= 0.0;
            // Stagger: any crit, or any hit > threshold of hp_max,
            // and the enemy isn't a juggernaut. Skipped on the
            // killing blow — the death anim takes over there.
            let juggernaut = (en.elite_mods
                & super::enemies::elite_mod::JUGGERNAUT)
                != 0;
            let stagger_eligible = !died
                && !juggernaut
                && (crit || scaled > en.hp_max * super::enemies::STAGGER_THRESHOLD);
            if stagger_eligible {
                en.stagger_remaining = en
                    .stagger_remaining
                    .max(super::enemies::STAGGER_DUR);
            }
            // Threat accumulation: by raw scaled damage so big
            // hits weigh proportionally. Skipped on death (the
            // corpse won't aggro anyone).
            if !died {
                if let Some(ae) = attacker_entity {
                    *en.threat.entry(ae).or_insert(0.0) += scaled;
                }
            }
            let thorns = (en.elite_mods & super::enemies::elite_mod::THORNS) != 0;
            drop(en);
            ctx.events.push(WorldEvent::Damage {
                target: hit.enemy_net_id,
                amount: scaled,
                crit,
                position: hit.enemy_pos.to_array(),
            });
            if stagger_eligible {
                ctx.events.push(WorldEvent::Hit {
                    target: hit.enemy_net_id,
                    start_tick: ctx.tick,
                });
            }
            // Apply any debuff carried by the source ability. We
            // do this *after* the damage write so DoT clocks
            // start from now.
            if let Some(debuff_id) = hit.apply_debuff {
                if let Ok(mut stack) =
                    world.get::<&mut super::effect::EffectStack>(hit.enemy)
                {
                    stack.apply(debuff_id, None);
                }
            }
            // Thorns reflect: drain a fraction of `scaled` back
            // to the attacker. Routed through the player-damage
            // queue so the death-on-thorns path uses the same
            // chokepoint as a normal melee swing — emits
            // Damage / Death events and arms the rise timer.
            if thorns && !died {
                if let Some(ae) = attacker_entity {
                    let reflect = scaled * super::enemies::ELITE_THORNS_FRAC;
                    if reflect > 0.0 {
                        ctx.player_damage_back.push((ae, reflect));
                    }
                }
            }
            if died {
                dead.push((hit.enemy, hit.enemy_net_id));
            } else {
                // Live victim — queue an aggro notify so it
                // (and nearby packmates) lock onto the
                // attacker. Skipped for kills since a corpse
                // doesn't need a target. Resolved here while
                // we still have the attacker NetId in scope;
                // the actual mutation runs after the loop so
                // we don't double-borrow `world`.
                if let Some(ae) = attacker_entity {
                    aggro_alerts.push((hit.enemy, ae));
                }
            }
        }
    }
    // Apply queued aggro alerts. Done after the hit loop so we
    // don't conflict with the per-victim mutable borrows above.
    // De-duplicate on victim — multiple hits in the same tick
    // (pierce, AoE tick) only need one notify.
    aggro_alerts.sort_by_key(|(v, _)| v.id());
    aggro_alerts.dedup_by_key(|(v, _)| v.id());
    for (victim, attacker) in aggro_alerts {
        super::enemies::notify_attacked(world, victim, attacker);
    }
    super::loot::finalise_kills(world, ctx, dead);
}

// ── Enemy → player projectiles ────────────────────────────────
//
// Enemy projectiles share the [`ServerProjectile`] component
// with player projectiles, distinguished by `team: Team::Enemy`.
// The unified [`tick`] above handles both — this section only
// keeps the player-side hit radius as a public constant so AoE
// / channel code can reuse the player-target overlap rule.

/// Sphere radius used for enemy-projectile↔player XZ collision.
/// Slightly bigger than the enemy hit radius — players are
/// taller and the bolt should connect on glancing blows so the
/// telegraph reads as a real threat instead of a free dodge.
pub const PLAYER_HIT_RADIUS: f32 = 0.5;

