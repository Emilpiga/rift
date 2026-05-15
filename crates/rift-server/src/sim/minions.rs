//! Player-owned minion simulation.
//!
//! Minions are server-authoritative actors like enemies, but they
//! are not enemies: they snapshot as `EntityKind::Minion`, carry an
//! owning player net id, follow that owner, and fire player-team
//! projectiles at hostile `ServerEnemy` rows.

use glam::Vec3;
use hecs::Entity;
use rift_dungeon::Floor;
use rift_game::abilities::{AbilityWireId, MinionAttackKind};
use rift_game::kinematic::{self, loco, Kinematic};
use rift_game::monsters::MonsterRole;
use rift_net::NetId;

use super::actor::{NetIdentity, Vitals};
use super::enemies::enemy_anim;
use super::player::ServerPlayer;
use super::projectile::{ServerProjectile, Team};

const MINION_SPEED: f32 = 4.4;
const OWNER_RECALL_DISTANCE: f32 = 18.0;
const OWNER_SNAP_DISTANCE: f32 = 32.0;
const SPAWN_SIDE_OFFSET: f32 = 1.4;
const PROJECTILE_SIZE: f32 = 0.36;
const HOVER_ARRIVAL_RADIUS: f32 = 0.22;
const HOVER_ORBIT_SPEED: f32 = 0.75;
const MINION_SEPARATION_RADIUS: f32 = 0.62;
const MINION_SEPARATION_STRENGTH: f32 = 1.2;
const PATH_RECOMPUTE_INTERVAL: f32 = 0.35;

#[derive(Clone, Copy, Debug)]
pub struct MinionSpawnRequest {
    pub owner: Entity,
    pub owner_net_id: NetId,
    pub origin: Vec3,
    pub role: MonsterRole,
    pub formation_index: u32,
    pub duration: f32,
    pub hp: f32,
    pub follow_distance: f32,
    pub attack_range: f32,
    pub attack_interval: f32,
    pub attack_damage: f32,
    pub attack_kind: MinionAttackKind,
    pub crit_chance: f32,
    pub crit_damage: f32,
    pub projectile_speed: f32,
    pub projectile_ttl: f32,
}

#[derive(Clone, Debug)]
pub struct ServerMinion {
    pub owner: Entity,
    pub owner_net_id: NetId,
    pub role: MonsterRole,
    pub formation_index: u32,
    pub lifetime_remaining: f32,
    pub lifetime_max: f32,
    pub follow_distance: f32,
    pub attack_range: f32,
    pub attack_interval: f32,
    pub attack_cooldown: f32,
    pub attack_damage: f32,
    pub attack_kind: MinionAttackKind,
    pub crit_chance: f32,
    pub crit_damage: f32,
    pub projectile_speed: f32,
    pub projectile_ttl: f32,
    pub attack_anim_remaining: f32,
    pub target_lock: Option<NetId>,
    pub hover_phase: f32,
    pub path: Vec<(i32, i32)>,
    pub path_target_tile: Option<(i32, i32)>,
    pub path_recompute_in: f32,
}

impl ServerMinion {
    fn refresh(&mut self, request: &MinionSpawnRequest) {
        self.lifetime_remaining = request.duration;
        self.lifetime_max = request.duration.max(0.001);
        self.follow_distance = request.follow_distance;
        self.attack_range = request.attack_range;
        self.attack_interval = request.attack_interval;
        self.attack_damage = request.attack_damage;
        self.attack_kind = request.attack_kind;
        self.crit_chance = request.crit_chance;
        self.crit_damage = request.crit_damage;
        self.projectile_speed = request.projectile_speed;
        self.projectile_ttl = request.projectile_ttl;
        self.attack_cooldown = self.attack_cooldown.min(0.25);
        self.target_lock = None;
        self.path.clear();
        self.path_target_tile = None;
        self.path_recompute_in = 0.0;
    }
}

pub fn spawn_or_refresh(
    world: &mut hecs::World,
    request: MinionSpawnRequest,
    next_net_id: &mut u32,
) {
    for (_entity, (minion, vitals, kinematic)) in
        world.query_mut::<(&mut ServerMinion, &mut Vitals, &mut Kinematic)>()
    {
        if minion.owner == request.owner
            && minion.role == request.role
            && minion.formation_index == request.formation_index
        {
            minion.refresh(&request);
            vitals.rescale_max(request.hp);
            vitals.fill();
            if (kinematic.position - request.origin).length_squared()
                > OWNER_RECALL_DISTANCE * OWNER_RECALL_DISTANCE
            {
                kinematic.position = spawn_position(
                    request.origin,
                    request.owner_net_id,
                    request.formation_index,
                );
            }
            return;
        }
    }

    let net_id = NetId(*next_net_id);
    *next_net_id = next_net_id.wrapping_add(1).max(1);
    let position = spawn_position(
        request.origin,
        request.owner_net_id,
        request.formation_index,
    );
    let kinematic = Kinematic {
        position,
        velocity: Vec3::ZERO,
        yaw: 0.0,
        aim_yaw: 0.0,
        locomotion: loco::IDLE,
        vy: 0.0,
        airborne: false,
        ..Default::default()
    };
    world.spawn((
        ServerMinion {
            owner: request.owner,
            owner_net_id: request.owner_net_id,
            role: request.role,
            formation_index: request.formation_index,
            lifetime_remaining: request.duration,
            lifetime_max: request.duration.max(0.001),
            follow_distance: request.follow_distance,
            attack_range: request.attack_range,
            attack_interval: request.attack_interval,
            attack_cooldown: 0.2,
            attack_damage: request.attack_damage,
            attack_kind: request.attack_kind,
            crit_chance: request.crit_chance,
            crit_damage: request.crit_damage,
            projectile_speed: request.projectile_speed,
            projectile_ttl: request.projectile_ttl,
            attack_anim_remaining: 0.0,
            target_lock: None,
            hover_phase: initial_hover_phase(request.owner_net_id),
            path: Vec::new(),
            path_target_tile: None,
            path_recompute_in: 0.0,
        },
        NetIdentity::new(net_id),
        Vitals::new(request.hp),
        kinematic,
        super::effect::EffectStack::default(),
    ));
}

pub fn tick_ai(
    world: &mut hecs::World,
    floor: &Floor,
    enemies: &[(Entity, Vec3, NetId, f32, Option<Entity>)],
    next_projectile_net_id: &mut u32,
    dt: f32,
) {
    let mut owners = Vec::new();
    for (entity, (player, identity, vitals, kinematic)) in world
        .query::<(&ServerPlayer, &NetIdentity, &Vitals, &Kinematic)>()
        .iter()
    {
        owners.push((
            entity,
            identity.net_id,
            kinematic.position,
            vitals.is_dead() || player.is_ghost,
        ));
    }

    let neighbours: Vec<(NetId, Vec3)> = world
        .query::<(&ServerMinion, &NetIdentity, &Vitals, &Kinematic)>()
        .iter()
        .filter(|(_, (_minion, _identity, vitals, _kinematic))| !vitals.is_dead())
        .map(|(_, (_minion, identity, _vitals, kinematic))| (identity.net_id, kinematic.position))
        .collect();
    let grid =
        rift_math::spatial::SpatialGrid::build(&neighbours, MINION_SEPARATION_RADIUS, |&(_, p)| p);

    let mut to_despawn = Vec::new();
    let mut projectiles = Vec::new();
    for (entity, (minion, identity, vitals, kinematic)) in world
        .query::<(&mut ServerMinion, &NetIdentity, &Vitals, &mut Kinematic)>()
        .iter()
    {
        minion.lifetime_remaining -= dt;
        minion.attack_cooldown = (minion.attack_cooldown - dt).max(0.0);
        minion.attack_anim_remaining = (minion.attack_anim_remaining - dt).max(0.0);
        minion.path_recompute_in = (minion.path_recompute_in - dt).max(0.0);
        minion.hover_phase = (minion.hover_phase + HOVER_ORBIT_SPEED * dt) % std::f32::consts::TAU;
        if minion.lifetime_remaining <= 0.0 || vitals.is_dead() {
            to_despawn.push(entity);
            continue;
        }

        let Some((_owner_entity, _owner_net, owner_pos, owner_unavailable)) = owners
            .iter()
            .find(|(owner_entity, owner_net, _pos, _dead)| {
                *owner_entity == minion.owner && *owner_net == minion.owner_net_id
            })
            .copied()
        else {
            to_despawn.push(entity);
            continue;
        };
        if owner_unavailable {
            to_despawn.push(entity);
            continue;
        }

        let to_owner = owner_pos - kinematic.position;
        let owner_dist = xz_len(to_owner);
        if owner_dist > OWNER_SNAP_DISTANCE {
            kinematic.position =
                spawn_position(owner_pos, minion.owner_net_id, minion.formation_index);
            kinematic.velocity = Vec3::ZERO;
            continue;
        }

        let seek_range = match minion.attack_kind {
            MinionAttackKind::Projectile { .. } => minion.attack_range,
            MinionAttackKind::Melee { .. } => minion.attack_range.max(8.0),
        };
        let require_target_los = matches!(minion.attack_kind, MinionAttackKind::Projectile { .. });
        let target = locked_or_nearest_visible_enemy(
            kinematic.position,
            floor,
            enemies,
            seek_range,
            minion.target_lock,
            minion.owner,
            require_target_los,
        );
        if let Some((_enemy_entity, enemy_pos, enemy_net, _enemy_radius)) = target {
            minion.target_lock = Some(enemy_net);
            let aim = enemy_pos - kinematic.position;
            let dist = xz_len(aim).max(0.001);
            let dir = Vec3::new(aim.x / dist, 0.0, aim.z / dist);
            kinematic.yaw = dir.x.atan2(dir.z);
            kinematic.aim_yaw = kinematic.yaw;
            match minion.attack_kind {
                MinionAttackKind::Projectile { .. } => {
                    apply_hover_velocity(minion, kinematic, floor, owner_pos)
                }
                MinionAttackKind::Melee { .. } => {
                    apply_chase_velocity(minion, kinematic, floor, enemy_pos, dist)
                }
            }
            let attack_los_clear = floor.line_of_sight(kinematic.position, enemy_pos);
            if minion.attack_cooldown <= 0.0 && dist <= minion.attack_range && attack_los_clear {
                minion.attack_cooldown = minion.attack_interval;
                minion.attack_anim_remaining = 0.35;
                let net_id = NetId(*next_projectile_net_id);
                *next_projectile_net_id = next_projectile_net_id.wrapping_add(1).max(0x4000_0000);
                match minion.attack_kind {
                    MinionAttackKind::Projectile { ability_id } => {
                        let spawn = kinematic.position + Vec3::new(0.0, 0.85, 0.0) + dir * 0.45;
                        projectiles.push(ServerProjectile {
                            net_id,
                            ability_id,
                            owner: minion.owner_net_id,
                            team: Team::Player,
                            attacker_kind: super::meters::ATTACKER_KIND_OTHER,
                            position: spawn,
                            velocity: dir * minion.projectile_speed,
                            ttl: minion.projectile_ttl,
                            damage: minion.attack_damage,
                            crit_chance: minion.crit_chance,
                            crit_damage: minion.crit_damage,
                            pierce_remaining: 0,
                            size: PROJECTILE_SIZE,
                            apply_debuff: None,
                            from_minion: true,
                        });
                    }
                    MinionAttackKind::Melee { ability_id, radius } => {
                        projectiles.push(ServerProjectile {
                            net_id,
                            ability_id,
                            owner: minion.owner_net_id,
                            team: Team::Player,
                            attacker_kind: super::meters::ATTACKER_KIND_OTHER,
                            position: enemy_pos + Vec3::Y * 0.45,
                            velocity: Vec3::ZERO,
                            ttl: 0.1,
                            damage: minion.attack_damage,
                            crit_chance: minion.crit_chance,
                            crit_damage: minion.crit_damage,
                            pierce_remaining: 0,
                            size: radius * 2.0,
                            apply_debuff: None,
                            from_minion: true,
                        });
                    }
                }
            }
        } else {
            minion.target_lock = None;
            apply_hover_velocity(minion, kinematic, floor, owner_pos);
        }

        let push = rift_math::spatial::separation_push(
            &grid,
            kinematic.position,
            MINION_SEPARATION_RADIUS,
            &neighbours,
            |&(_, p)| p,
            |&(nid, _)| nid == identity.net_id,
        );
        if push.length_squared() > 1.0e-6 {
            kinematic.velocity += push * MINION_SPEED * MINION_SEPARATION_STRENGTH;
        }
    }

    for projectile in projectiles {
        world.spawn((projectile,));
    }
    for entity in to_despawn {
        let _ = world.despawn(entity);
    }
}

pub fn target_positions(world: &hecs::World) -> Vec<(Entity, Vec3)> {
    world
        .query::<(&ServerMinion, &Vitals, &Kinematic)>()
        .iter()
        .filter(|(_, (_minion, vitals, _kinematic))| !vitals.is_dead())
        .map(|(e, (_minion, _vitals, kinematic))| (e, kinematic.position))
        .collect()
}

pub fn integrate_motion(world: &mut hecs::World, floor: &Floor, dt: f32) {
    for (_entity, (_minion, kinematic)) in world.query_mut::<(&ServerMinion, &mut Kinematic)>() {
        kinematic::integrate(kinematic, floor, dt);
    }
}

pub fn anim_byte(minion: &ServerMinion, kinematic: &Kinematic) -> u8 {
    if minion.attack_anim_remaining > 0.0 {
        enemy_anim::ATTACK
    } else if kinematic.velocity.length_squared() > 0.05 * 0.05 {
        enemy_anim::WALK
    } else {
        enemy_anim::IDLE
    }
}

fn locked_or_nearest_visible_enemy(
    origin: Vec3,
    floor: &Floor,
    enemies: &[(Entity, Vec3, NetId, f32, Option<Entity>)],
    range: f32,
    target_lock: Option<NetId>,
    owner: Entity,
    require_los: bool,
) -> Option<(Entity, Vec3, NetId, f32)> {
    let range_sq = range * range;
    if let Some(locked_id) = target_lock {
        if let Some((entity, pos, net_id, radius, _target)) = enemies
            .iter()
            .find(|(_, _, net_id, _, _)| *net_id == locked_id)
        {
            let dx = pos.x - origin.x;
            let dz = pos.z - origin.z;
            let d2 = dx * dx + dz * dz;
            if d2 <= range_sq * 1.15 && (!require_los || floor.line_of_sight(origin, *pos)) {
                return Some((*entity, *pos, *net_id, *radius));
            }
        }
    }
    if let Some((_d2, entity, pos, net_id, radius)) = enemies
        .iter()
        .filter_map(|(entity, pos, net_id, radius, target)| {
            if *target != Some(owner) {
                return None;
            }
            let dx = pos.x - origin.x;
            let dz = pos.z - origin.z;
            let d2 = dx * dx + dz * dz;
            if d2 > range_sq || (require_los && !floor.line_of_sight(origin, *pos)) {
                None
            } else {
                Some((d2, *entity, *pos, *net_id, *radius))
            }
        })
        .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
    {
        return Some((entity, pos, net_id, radius));
    }
    enemies
        .iter()
        .filter_map(|(entity, pos, net_id, radius, _target)| {
            let dx = pos.x - origin.x;
            let dz = pos.z - origin.z;
            let d2 = dx * dx + dz * dz;
            if d2 > range_sq || (require_los && !floor.line_of_sight(origin, *pos)) {
                None
            } else {
                Some((d2, *entity, *pos, *net_id, *radius))
            }
        })
        .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(_d2, entity, pos, net_id, radius)| (entity, pos, net_id, radius))
}

fn apply_hover_velocity(
    minion: &mut ServerMinion,
    kinematic: &mut Kinematic,
    floor: &Floor,
    owner_pos: Vec3,
) {
    let radius = minion.follow_distance.max(1.2);
    let desired = owner_pos
        + Vec3::new(
            minion.hover_phase.cos() * radius,
            0.0,
            minion.hover_phase.sin() * radius,
        );
    let to_desired = desired - kinematic.position;
    let dist = xz_len(to_desired);
    if dist <= HOVER_ARRIVAL_RADIUS {
        kinematic.velocity = Vec3::ZERO;
        kinematic.locomotion = loco::IDLE;
        minion.path.clear();
        minion.path_target_tile = None;
        return;
    }
    let dir = navigation_dir(minion, kinematic.position, desired, floor).unwrap_or_else(|| {
        Vec3::new(
            to_desired.x / dist.max(0.001),
            0.0,
            to_desired.z / dist.max(0.001),
        )
    });
    let speed = (dist * 2.6).clamp(0.35, MINION_SPEED);
    kinematic.velocity = dir * speed;
    kinematic.locomotion = loco::IDLE;
}

fn apply_chase_velocity(
    minion: &mut ServerMinion,
    kinematic: &mut Kinematic,
    floor: &Floor,
    target_pos: Vec3,
    dist: f32,
) {
    if dist <= 0.95 {
        kinematic.velocity = Vec3::ZERO;
        kinematic.locomotion = loco::IDLE;
        minion.path.clear();
        minion.path_target_tile = None;
        return;
    }
    let to_target = target_pos - kinematic.position;
    let dir = navigation_dir(minion, kinematic.position, target_pos, floor).unwrap_or_else(|| {
        Vec3::new(
            to_target.x / dist.max(0.001),
            0.0,
            to_target.z / dist.max(0.001),
        )
    });
    kinematic.velocity = dir * MINION_SPEED;
    kinematic.locomotion = loco::RUN;
}

fn navigation_dir(
    minion: &mut ServerMinion,
    from_pos: Vec3,
    target_pos: Vec3,
    floor: &Floor,
) -> Option<Vec3> {
    if floor.line_of_sight(from_pos, target_pos) {
        minion.path.clear();
        minion.path_target_tile = None;
        let to_target = target_pos - from_pos;
        return Some(Vec3::new(to_target.x, 0.0, to_target.z).normalize_or_zero());
    }

    let target_tile = world_to_tile(target_pos);
    let need_recompute = minion.path.is_empty()
        || minion.path_target_tile != Some(target_tile)
        || minion.path_recompute_in <= 0.0;
    if need_recompute {
        let from_tile = world_to_tile(from_pos);
        minion.path = floor.path(from_tile, target_tile, 1024).unwrap_or_default();
        minion.path_target_tile = Some(target_tile);
        minion.path_recompute_in = PATH_RECOMPUTE_INTERVAL;
    }

    while let Some(&(wx, wz)) = minion.path.first() {
        let dx = wx as f32 - from_pos.x;
        let dz = wz as f32 - from_pos.z;
        if dx * dx + dz * dz < 0.25 {
            minion.path.remove(0);
        } else {
            break;
        }
    }

    minion.path.first().map(|&(wx, wz)| {
        Vec3::new(wx as f32 - from_pos.x, 0.0, wz as f32 - from_pos.z).normalize_or_zero()
    })
}

fn world_to_tile(p: Vec3) -> (i32, i32) {
    ((p.x + 0.5).floor() as i32, (p.z + 0.5).floor() as i32)
}

fn spawn_position(origin: Vec3, owner_net_id: NetId, formation_index: u32) -> Vec3 {
    let side = if (owner_net_id.0.wrapping_add(formation_index)) & 1 == 0 {
        1.0
    } else {
        -1.0
    };
    let row = formation_index / 2;
    Vec3::new(
        origin.x + SPAWN_SIDE_OFFSET * side,
        0.0,
        origin.z + 0.6 + row as f32 * 0.55,
    )
}

fn initial_hover_phase(owner_net_id: NetId) -> f32 {
    if owner_net_id.0 & 1 == 0 {
        0.35
    } else {
        std::f32::consts::PI - 0.35
    }
}

fn xz_len(v: Vec3) -> f32 {
    (v.x * v.x + v.z * v.z).sqrt()
}

#[allow(dead_code)]
fn _ability_marker(_: AbilityWireId) {}
