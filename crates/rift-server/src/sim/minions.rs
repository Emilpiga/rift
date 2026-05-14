//! Player-owned minion simulation.
//!
//! Minions are server-authoritative actors like enemies, but they
//! are not enemies: they snapshot as `EntityKind::Minion`, carry an
//! owning player net id, follow that owner, and fire player-team
//! projectiles at hostile `ServerEnemy` rows.

use glam::Vec3;
use hecs::Entity;
use rift_dungeon::Floor;
use rift_game::abilities::{self, AbilityWireId};
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

#[derive(Clone, Copy, Debug)]
pub struct MinionSpawnRequest {
    pub owner: Entity,
    pub owner_net_id: NetId,
    pub origin: Vec3,
    pub role: MonsterRole,
    pub duration: f32,
    pub hp: f32,
    pub follow_distance: f32,
    pub attack_range: f32,
    pub attack_interval: f32,
    pub attack_damage: f32,
    pub projectile_speed: f32,
    pub projectile_ttl: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct ServerMinion {
    pub owner: Entity,
    pub owner_net_id: NetId,
    pub role: MonsterRole,
    pub lifetime_remaining: f32,
    pub follow_distance: f32,
    pub attack_range: f32,
    pub attack_interval: f32,
    pub attack_cooldown: f32,
    pub attack_damage: f32,
    pub projectile_speed: f32,
    pub projectile_ttl: f32,
    pub attack_anim_remaining: f32,
    pub target_lock: Option<NetId>,
    pub hover_phase: f32,
}

impl ServerMinion {
    fn refresh(&mut self, request: &MinionSpawnRequest) {
        self.lifetime_remaining = request.duration;
        self.follow_distance = request.follow_distance;
        self.attack_range = request.attack_range;
        self.attack_interval = request.attack_interval;
        self.attack_damage = request.attack_damage;
        self.projectile_speed = request.projectile_speed;
        self.projectile_ttl = request.projectile_ttl;
        self.attack_cooldown = self.attack_cooldown.min(0.25);
        self.target_lock = None;
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
        if minion.owner == request.owner && minion.role == request.role {
            minion.refresh(&request);
            vitals.rescale_max(request.hp);
            vitals.fill();
            if (kinematic.position - request.origin).length_squared()
                > OWNER_RECALL_DISTANCE * OWNER_RECALL_DISTANCE
            {
                kinematic.position = spawn_position(request.origin, request.owner_net_id);
            }
            return;
        }
    }

    let net_id = NetId(*next_net_id);
    *next_net_id = next_net_id.wrapping_add(1).max(1);
    let position = spawn_position(request.origin, request.owner_net_id);
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
            lifetime_remaining: request.duration,
            follow_distance: request.follow_distance,
            attack_range: request.attack_range,
            attack_interval: request.attack_interval,
            attack_cooldown: 0.2,
            attack_damage: request.attack_damage,
            projectile_speed: request.projectile_speed,
            projectile_ttl: request.projectile_ttl,
            attack_anim_remaining: 0.0,
            target_lock: None,
            hover_phase: initial_hover_phase(request.owner_net_id),
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
    enemies: &[(Entity, Vec3, NetId, f32)],
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

    let mut to_despawn = Vec::new();
    let mut projectiles = Vec::new();
    for (entity, (minion, _identity, vitals, kinematic)) in world
        .query::<(&mut ServerMinion, &NetIdentity, &Vitals, &mut Kinematic)>()
        .iter()
    {
        minion.lifetime_remaining -= dt;
        minion.attack_cooldown = (minion.attack_cooldown - dt).max(0.0);
        minion.attack_anim_remaining = (minion.attack_anim_remaining - dt).max(0.0);
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
            kinematic.position = spawn_position(owner_pos, minion.owner_net_id);
            kinematic.velocity = Vec3::ZERO;
            continue;
        }

        let target = locked_or_nearest_visible_enemy(
            kinematic.position,
            floor,
            enemies,
            minion.attack_range,
            minion.target_lock,
        );
        if let Some((_enemy_entity, enemy_pos, enemy_net, _enemy_radius)) = target {
            minion.target_lock = Some(enemy_net);
            let aim = enemy_pos - kinematic.position;
            let dist = xz_len(aim).max(0.001);
            let dir = Vec3::new(aim.x / dist, 0.0, aim.z / dist);
            kinematic.yaw = dir.x.atan2(dir.z);
            kinematic.aim_yaw = kinematic.yaw;
            apply_hover_velocity(
                kinematic,
                owner_pos,
                minion.follow_distance,
                minion.hover_phase,
            );
            if minion.attack_cooldown <= 0.0 {
                minion.attack_cooldown = minion.attack_interval;
                minion.attack_anim_remaining = 0.35;
                let net_id = NetId(*next_projectile_net_id);
                *next_projectile_net_id = next_projectile_net_id.wrapping_add(1).max(0x4000_0000);
                let spawn = kinematic.position + Vec3::new(0.0, 0.85, 0.0) + dir * 0.45;
                projectiles.push(ServerProjectile {
                    net_id,
                    ability_id: abilities::id::VOID_FAMILIAR_BOLT,
                    owner: minion.owner_net_id,
                    team: Team::Player,
                    attacker_kind: super::meters::ATTACKER_KIND_OTHER,
                    position: spawn,
                    velocity: dir * minion.projectile_speed,
                    ttl: minion.projectile_ttl,
                    damage: minion.attack_damage,
                    crit_chance: 0.0,
                    crit_damage: 0.0,
                    pierce_remaining: 0,
                    size: PROJECTILE_SIZE,
                    apply_debuff: None,
                });
            }
        } else {
            minion.target_lock = None;
            apply_hover_velocity(
                kinematic,
                owner_pos,
                minion.follow_distance,
                minion.hover_phase,
            );
        }
    }

    for projectile in projectiles {
        world.spawn((projectile,));
    }
    for entity in to_despawn {
        let _ = world.despawn(entity);
    }
}

pub fn integrate_motion(world: &mut hecs::World, floor: &Floor, dt: f32) {
    for (_entity, (_minion, kinematic)) in world.query_mut::<(&ServerMinion, &mut Kinematic)>() {
        kinematic::integrate(kinematic, floor, dt);
    }
}

pub fn anim_byte(_minion: &ServerMinion, _kinematic: &Kinematic) -> u8 {
    enemy_anim::IDLE
}

fn locked_or_nearest_visible_enemy(
    origin: Vec3,
    floor: &Floor,
    enemies: &[(Entity, Vec3, NetId, f32)],
    range: f32,
    target_lock: Option<NetId>,
) -> Option<(Entity, Vec3, NetId, f32)> {
    let range_sq = range * range;
    if let Some(locked_id) = target_lock {
        if let Some((entity, pos, net_id, radius)) = enemies
            .iter()
            .find(|(_, _, net_id, _)| *net_id == locked_id)
        {
            let dx = pos.x - origin.x;
            let dz = pos.z - origin.z;
            let d2 = dx * dx + dz * dz;
            if d2 <= range_sq * 1.15 && floor.line_of_sight(origin, *pos) {
                return Some((*entity, *pos, *net_id, *radius));
            }
        }
    }
    enemies
        .iter()
        .filter_map(|(entity, pos, net_id, radius)| {
            let dx = pos.x - origin.x;
            let dz = pos.z - origin.z;
            let d2 = dx * dx + dz * dz;
            if d2 > range_sq || !floor.line_of_sight(origin, *pos) {
                None
            } else {
                Some((d2, *entity, *pos, *net_id, *radius))
            }
        })
        .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(_d2, entity, pos, net_id, radius)| (entity, pos, net_id, radius))
}

fn apply_hover_velocity(
    kinematic: &mut Kinematic,
    owner_pos: Vec3,
    follow_distance: f32,
    hover_phase: f32,
) {
    let radius = follow_distance.max(1.2);
    let desired =
        owner_pos + Vec3::new(hover_phase.cos() * radius, 0.0, hover_phase.sin() * radius);
    let to_desired = desired - kinematic.position;
    let dist = xz_len(to_desired);
    if dist <= HOVER_ARRIVAL_RADIUS {
        kinematic.velocity = Vec3::ZERO;
        kinematic.locomotion = loco::IDLE;
        return;
    }
    let dir = Vec3::new(
        to_desired.x / dist.max(0.001),
        0.0,
        to_desired.z / dist.max(0.001),
    );
    let speed = (dist * 2.6).clamp(0.35, MINION_SPEED);
    kinematic.velocity = dir * speed;
    kinematic.locomotion = loco::IDLE;
}

fn spawn_position(origin: Vec3, owner_net_id: NetId) -> Vec3 {
    let side = if owner_net_id.0 & 1 == 0 { 1.0 } else { -1.0 };
    Vec3::new(origin.x + SPAWN_SIDE_OFFSET * side, 0.0, origin.z + 0.6)
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
