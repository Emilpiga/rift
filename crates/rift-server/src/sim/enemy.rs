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
pub const AGGRO_RANGE: f32 = 12.0;
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
}

impl ServerEnemy {
    /// `true` once the enemy has been killed — used by every
    /// damage subsystem to avoid hitting the same corpse twice and
    /// by the AI tick to skip dead bodies.
    pub fn is_dying(&self) -> bool {
        self.dying_remaining > 0.0
    }
}

/// One AI tick for every enemy in the world.
///
/// Picks the nearest in-range player as the target, walks toward
/// them, and queues a melee hit when in attack range and off
/// cooldown. Returns `(target_player_entity, damage)` rows the
/// caller applies once the enemy borrow ends.
pub fn tick_ai(
    world: &mut hecs::World,
    player_positions: &[(Entity, Vec3)],
    dt: f32,
) -> Vec<(Entity, f32)> {
    let mut pending_damage: Vec<(Entity, f32)> = Vec::new();
    for (_e, (en, stack)) in world.query_mut::<(&mut ServerEnemy, Option<&super::debuff::DebuffStack>)>() {
        // Skip dying enemies — their AI is frozen until the
        // death-fade timer expires and they're despawned.
        if en.is_dying() {
            en.k.velocity = Vec3::ZERO;
            continue;
        }
        // Apply speed-altering debuffs (Slow, Chill, ...).
        let speed_mult = stack.map(|s| s.move_speed_mult()).unwrap_or(1.0);
        // Tick cooldowns.
        if en.attack_cooldown > 0.0 {
            en.attack_cooldown = (en.attack_cooldown - dt).max(0.0);
        }
        if en.attack_anim_remaining > 0.0 {
            en.attack_anim_remaining = (en.attack_anim_remaining - dt).max(0.0);
        }
        // Find nearest player within aggro range.
        let mut best: Option<(Entity, Vec3, f32)> = None;
        for (pe, pp) in player_positions {
            let dx = pp.x - en.k.position.x;
            let dz = pp.z - en.k.position.z;
            let d2 = dx * dx + dz * dz;
            if d2 <= AGGRO_RANGE * AGGRO_RANGE
                && best.map_or(true, |(_, _, bd2)| d2 < bd2)
            {
                best = Some((*pe, *pp, d2));
            }
        }
        if let Some((target_entity, target_pos, d2)) = best {
            let dist = d2.sqrt();
            let to_target = Vec3::new(
                target_pos.x - en.k.position.x,
                0.0,
                target_pos.z - en.k.position.z,
            );
            // Face the target so client spine yaw points right.
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
                    pending_damage.push((target_entity, ATTACK_DAMAGE));
                }
            }
        } else {
            en.k.velocity = Vec3::ZERO;
            en.k.locomotion = loco::IDLE;
        }
    }
    pending_damage
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
                };
                world.spawn((enemy, super::debuff::DebuffStack::default()));
                spawned += 1;
            }
        }
    }
    log::info!("sim: spawned {spawned} enemies on floor {floor_index}");
}
