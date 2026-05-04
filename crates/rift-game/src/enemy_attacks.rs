//! Enemy attack execution: drains AI `pending_action`s into world effects
//! (enemy projectiles, leap FX, telegraphed AoE slams).
//!
//! This module is the bridge from the AI tick (which only sets intents) to
//! the game world (which spawns projectiles, particles, and damages the player).

use glam::{Mat4, Vec3};
use rift_engine::ai::systems::AiAgent;
use rift_engine::ai::PendingAction;
use rift_engine::ecs::components::{Collider, Health, Player, Transform};
use rift_engine::physics::{raycast, Aabb, Ray};
use rift_engine::{Emitter, EmitterConfig, Mesh, Renderer};

const MAX_ENEMY_PROJECTILES: usize = 64;
const ENEMY_BOLT_SPEED: f32 = 14.0;
const ENEMY_BOLT_RADIUS: f32 = 0.25;

/// One enemy projectile in flight.
struct EnemyBolt {
    pos: Vec3,
    dir: Vec3,
    speed: f32,
    damage: f32,
    lifetime: f32,
    pool_slot: usize,
}

/// Telegraphed AoE warning circle that ticks down before damaging.
struct PendingSlam {
    center: Vec3,
    radius: f32,
    damage: f32,
    delay_remaining: f32,
    indicator_obj: Option<usize>,
}

/// Manages all enemy-side attack effects.
pub struct EnemyAttackSystem {
    bolts: Vec<EnemyBolt>,
    slams: Vec<PendingSlam>,
    /// Render-object indices for the bolt mesh pool.
    pool_obj_indices: Vec<usize>,
    free_slots: Vec<usize>,
    initialized: bool,
    wall_cache: Vec<Aabb>,
}

impl EnemyAttackSystem {
    pub fn new() -> Self {
        Self {
            bolts: Vec::new(),
            slams: Vec::new(),
            pool_obj_indices: Vec::new(),
            free_slots: Vec::new(),
            initialized: false,
            wall_cache: Vec::new(),
        }
    }

    pub fn init_pool(&mut self, renderer: &mut Renderer) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        // Glowing toxic-green bolt mesh — clearly distinct from player's gold arrows.
        let mesh = Mesh::enemy_bolt([0.20, 1.20, 0.30]);
        for _ in 0..MAX_ENEMY_PROJECTILES {
            if renderer.add_mesh(&mesh, Mat4::ZERO).is_ok() {
                self.pool_obj_indices.push(renderer.objects.len() - 1);
            }
        }
        self.free_slots = (0..self.pool_obj_indices.len()).rev().collect();
        log::info!("EnemyAttackSystem: initialized with {} bolt slots", self.pool_obj_indices.len());
    }

    pub fn clear(&mut self, _renderer: &mut Renderer) {
        self.bolts.clear();
        self.slams.clear();
        self.pool_obj_indices.clear();
        self.free_slots.clear();
        self.initialized = false;
        self.wall_cache.clear();
    }

    pub fn rebuild_wall_cache(&mut self, world: &hecs::World) {
        use rift_engine::ecs::components::Static;
        self.wall_cache = world
            .query::<(&Transform, &Collider, &Static)>()
            .iter()
            .map(|(_, (t, c, _))| Aabb::from_center(t.position, c.half_extents))
            .collect();
    }

    /// Drain AI pending actions and spawn the corresponding game-world effects.
    pub fn drain_pending(&mut self, world: &mut hecs::World, renderer: &mut Renderer) {
        // Collect pending actions first to avoid borrow conflicts.
        let mut actions: Vec<PendingAction> = Vec::new();
        for (_e, agent) in world.query_mut::<&mut AiAgent>() {
            if let Some(a) = agent.blackboard.pending_action.take() {
                actions.push(a);
            }
        }
        for action in actions {
            match action {
                PendingAction::RangedShot { origin, target, damage } => {
                    self.spawn_bolt(origin, target, damage, renderer);
                }
                PendingAction::LeapStart { target } => {
                    // Visual feedback: a quick burst of dust at the takeoff point.
                    let emitter = Emitter::new(target, EmitterConfig::hit_spark([0.6, 0.4, 1.0]));
                    renderer.particle_system.add_emitter(emitter);
                }
                PendingAction::SlamTelegraph { center, radius, damage, delay } => {
                    self.spawn_slam(center, radius, damage, delay, renderer);
                }
            }
        }
    }

    fn spawn_bolt(&mut self, origin: Vec3, target: Vec3, damage: f32, renderer: &mut Renderer) {
        let to_target = target - origin;
        let dir = to_target.normalize_or_zero();
        if dir == Vec3::ZERO {
            return;
        }
        let Some(slot) = self.free_slots.pop() else {
            log::warn!("EnemyAttackSystem: bolt pool exhausted");
            return;
        };
        log::info!("Enemy bolt fired from {:?} -> {:?}", origin, target);
        self.bolts.push(EnemyBolt {
            pos: origin,
            dir,
            speed: ENEMY_BOLT_SPEED,
            damage,
            lifetime: 2.5,
            pool_slot: slot,
        });
        // Spawn a muzzle flash at the origin.
        let emitter = Emitter::new(origin, EmitterConfig::hit_spark([0.4, 1.0, 0.6]));
        renderer.particle_system.add_emitter(emitter);
    }

    fn spawn_slam(&mut self, center: Vec3, radius: f32, damage: f32, delay: f32, renderer: &mut Renderer) {
        // Use the targeting circle mesh in red as a warning indicator.
        let mesh = Mesh::targeting_circle([1.0, 0.15, 0.10]);
        let mat = Mat4::from_translation(center) * Mat4::from_scale(Vec3::splat(radius));
        let obj_index = match renderer.add_mesh(&mesh, mat) {
            Ok(()) => Some(renderer.objects.len() - 1),
            Err(_) => None,
        };
        self.slams.push(PendingSlam {
            center,
            radius,
            damage,
            delay_remaining: delay,
            indicator_obj: obj_index,
        });
    }

    /// Tick all enemy bolts and slams. Returns (position, damage) of player hits.
    pub fn tick(&mut self, world: &mut hecs::World, renderer: &mut Renderer, dt: f32) -> Vec<(Vec3, f32)> {
        let mut hits: Vec<(Vec3, f32)> = Vec::new();

        // Get player for collision tests.
        let player_data: Option<(hecs::Entity, Vec3, Collider)> = world
            .query::<(&Transform, &Collider, &Player)>()
            .iter()
            .map(|(e, (t, c, _))| (e, t.position, *c))
            .next();

        let walls = &self.wall_cache;

        // ─── Bolts ─────────────────────────────────────────────
        for bolt in &mut self.bolts {
            bolt.lifetime -= dt;
            if bolt.lifetime <= 0.0 {
                continue;
            }
            let prev = bolt.pos;
            bolt.pos += bolt.dir * bolt.speed * dt;
            let travel = (bolt.pos - prev).length();

            // Wall raycast.
            if travel > 0.001 {
                let ray = Ray::new(prev, bolt.dir);
                if let Some(hit) = raycast(&ray, travel, walls) {
                    bolt.pos = hit.point;
                    bolt.lifetime = 0.0;
                    let emitter = Emitter::new(hit.point, EmitterConfig::hit_spark([0.4, 1.0, 0.6]));
                    renderer.particle_system.add_emitter(emitter);
                    continue;
                }
            }

            // Player collision.
            if let Some((_, ppos, pcol)) = &player_data {
                let delta = bolt.pos - *ppos;
                let dist_xz = (delta.x * delta.x + delta.z * delta.z).sqrt();
                let player_radius = pcol.half_extents.x.max(pcol.half_extents.z);
                if dist_xz < player_radius + ENEMY_BOLT_RADIUS
                    && bolt.pos.y > ppos.y - 0.5
                    && bolt.pos.y < ppos.y + pcol.half_extents.y * 2.0 + 0.5
                {
                    hits.push((bolt.pos, bolt.damage));
                    bolt.lifetime = 0.0;
                    let emitter = Emitter::new(bolt.pos, EmitterConfig::hit_spark([1.0, 0.4, 0.4]));
                    renderer.particle_system.add_emitter(emitter);
                }
            }
        }

        // Apply bolt damage to the player.
        if let Some((player_entity, _, _)) = &player_data {
            let total: f32 = hits.iter().map(|(_, d)| *d).sum();
            if total > 0.0 {
                if let Ok(mut h) = world.get::<&mut Health>(*player_entity) {
                    h.current = (h.current - total).max(0.0);
                }
            }
        }

        // Reap dead bolts and update render transforms.
        let mut i = 0;
        while i < self.bolts.len() {
            if self.bolts[i].lifetime <= 0.0 {
                let bolt = self.bolts.swap_remove(i);
                if let Some(&obj_idx) = self.pool_obj_indices.get(bolt.pool_slot) {
                    if obj_idx < renderer.objects.len() {
                        renderer.objects[obj_idx].model_matrix = Mat4::ZERO;
                    }
                }
                self.free_slots.push(bolt.pool_slot);
            } else {
                let bolt = &self.bolts[i];
                if let Some(&obj_idx) = self.pool_obj_indices.get(bolt.pool_slot) {
                    if obj_idx < renderer.objects.len() {
                        let rot_y = (-bolt.dir.x).atan2(-bolt.dir.z);
                        renderer.objects[obj_idx].model_matrix =
                            Mat4::from_translation(bolt.pos)
                                * Mat4::from_rotation_y(rot_y)
                                * Mat4::from_scale(Vec3::splat(1.0));
                    }
                }
                // Trail particles.
                let emitter = Emitter::new(bolt.pos, EmitterConfig::arrow_trail());
                renderer.particle_system.add_emitter(emitter);
                i += 1;
            }
        }

        // ─── Slams ─────────────────────────────────────────────
        let mut slam_i = 0;
        while slam_i < self.slams.len() {
            let slam = &mut self.slams[slam_i];
            slam.delay_remaining -= dt;

            if slam.delay_remaining <= 0.0 {
                // Detonate: damage player if inside radius.
                if let Some((_, ppos, _)) = &player_data {
                    let dx = ppos.x - slam.center.x;
                    let dz = ppos.z - slam.center.z;
                    if dx * dx + dz * dz < slam.radius * slam.radius {
                        hits.push((*ppos, slam.damage));
                        if let Some((player_entity, _, _)) = &player_data {
                            if let Ok(mut h) = world.get::<&mut Health>(*player_entity) {
                                h.current = (h.current - slam.damage).max(0.0);
                            }
                        }
                    }
                }
                // Detonation FX.
                for _ in 0..3 {
                    let emitter = Emitter::new(slam.center, EmitterConfig::hit_spark([1.0, 0.3, 0.1]));
                    renderer.particle_system.add_emitter(emitter);
                }
                if let Some(idx) = slam.indicator_obj {
                    if idx < renderer.objects.len() {
                        renderer.objects[idx].model_matrix = Mat4::ZERO;
                    }
                }
                self.slams.swap_remove(slam_i);
            } else {
                // Pulse the indicator a bit to draw attention.
                if let Some(idx) = slam.indicator_obj {
                    if idx < renderer.objects.len() {
                        let pulse = 1.0 + 0.08 * (slam.delay_remaining * 14.0).sin();
                        renderer.objects[idx].model_matrix =
                            Mat4::from_translation(slam.center)
                                * Mat4::from_scale(Vec3::splat(slam.radius * pulse));
                    }
                }
                slam_i += 1;
            }
        }

        hits
    }
}

impl Default for EnemyAttackSystem {
    fn default() -> Self {
        Self::new()
    }
}
