use glam::{Mat4, Vec3};
use crate::combat::Projectile;
use crate::ecs::components::{Collider, Enemy, Static, Transform};
use crate::physics::{Aabb, Ray, raycast};
use crate::renderer::particles::{Emitter, EmitterConfig};
use crate::renderer::mesh::Mesh;
use crate::renderer::Renderer;

const MAX_PROJECTILES: usize = 64;

/// Manages active projectiles: spawning, physics, collision, rendering.
/// Uses a fixed pool of pre-allocated render objects to avoid GPU buffer churn.
pub struct ProjectilePool {
    pub projectiles: Vec<Projectile>,
    /// Maps each active projectile to its pool slot index.
    pub pool_slots: Vec<usize>,
    /// Render object indices for the pre-allocated arrow pool.
    pool_obj_indices: Vec<usize>,
    /// Which pool slots are currently free.
    free_slots: Vec<usize>,
    /// Whether the pool has been initialized.
    initialized: bool,
    /// Cached wall AABBs (rebuilt on floor change, walls are static).
    wall_cache: Vec<Aabb>,
    /// Active area-of-effect damage zones (e.g., Rain of Arrows).
    pub aoe_zones: Vec<AoeZone>,
}

/// A persistent area-of-effect damage zone.
pub struct AoeZone {
    pub position: Vec3,
    pub radius: f32,
    pub damage_per_tick: f32,
    pub tick_interval: f32,
    pub duration: f32,
    pub elapsed: f32,
    pub tick_timer: f32,
}

impl ProjectilePool {
    pub fn new() -> Self {
        Self {
            projectiles: Vec::new(),
            pool_slots: Vec::new(),
            pool_obj_indices: Vec::new(),
            free_slots: Vec::new(),
            initialized: false,
            wall_cache: Vec::new(),
            aoe_zones: Vec::new(),
        }
    }

    /// Pre-allocate arrow render objects. Call once after floor generation.
    pub fn init_pool(&mut self, renderer: &mut Renderer) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        // Use a fireball mesh for player projectiles (gameplay still uses the
        // existing Projectile::arrow physics — only the visual changes).
        let proj_mesh = Mesh::fireball();
        for _ in 0..MAX_PROJECTILES {
            if renderer.add_mesh(&proj_mesh, Mat4::ZERO).is_ok() {
                self.pool_obj_indices.push(renderer.objects.len() - 1);
            }
        }
        self.free_slots = (0..self.pool_obj_indices.len()).rev().collect();
    }

    /// Reset the pool entirely. Call on floor transition (after clear_objects).
    pub fn clear(&mut self, _renderer: &mut Renderer) {
        self.projectiles.clear();
        self.pool_slots.clear();
        self.pool_obj_indices.clear();
        self.free_slots.clear();
        self.initialized = false;
        self.wall_cache.clear();
        self.aoe_zones.clear();
    }

    /// Rebuild cached wall AABBs from the ECS world (call after floor generation).
    pub fn rebuild_wall_cache(&mut self, world: &hecs::World) {
        self.wall_cache = world
            .query::<(&Transform, &Collider, &Static)>()
            .iter()
            .map(|(_, (t, c, _))| Aabb::from_center(t.position, c.half_extents))
            .collect();
    }

    /// Allocate a pool slot, returning the render object index. None if pool exhausted.
    fn alloc_slot(&mut self) -> Option<usize> {
        let slot = self.free_slots.pop()?;
        Some(slot)
    }

    /// Return a pool slot to the free list and hide its render object.
    fn free_slot(&mut self, slot: usize, renderer: &mut Renderer) {
        if let Some(&obj_idx) = self.pool_obj_indices.get(slot) {
            if obj_idx < renderer.objects.len() {
                renderer.objects[obj_idx].model_matrix = Mat4::ZERO;
            }
        }
        self.free_slots.push(slot);
    }

    /// Fire an ability, spawning projectiles.
    /// Spawn a single projectile if the pool has a free slot.  Returns
    /// `true` on success, `false` if the pool is exhausted (the
    /// projectile is then silently dropped — visuals only).
    pub fn queue_projectile(&mut self, proj: Projectile) -> bool {
        if let Some(slot) = self.alloc_slot() {
            self.projectiles.push(proj);
            self.pool_slots.push(slot);
            true
        } else {
            false
        }
    }

    /// Spawn an active AoE damage zone (e.g. Rain of Arrows).
    pub fn queue_aoe(&mut self, zone: AoeZone) {
        self.aoe_zones.push(zone);
    }

    /// Tick projectiles: move, collide with enemies, clean up dead ones.
    /// Tick all projectiles. Returns list of (world_position, damage) for hits this frame.
    pub fn tick(
        &mut self,
        world: &mut hecs::World,
        renderer: &mut Renderer,
        dt: f32,
    ) -> Vec<(Vec3, f32)> {
        // Collect enemy data for collision
        let enemy_data: Vec<(hecs::Entity, Vec3, f32)> = world
            .query::<(&Transform, &Enemy, &Collider)>()
            .iter()
            .map(|(e, (t, _, c))| (e, t.position, c.half_extents.x))
            .collect();

        // Use cached wall AABBs (static geometry, rebuilt on floor change)
        let wall_aabbs = &self.wall_cache;

        // Move projectiles and check collisions
        let mut hits_to_apply: Vec<(hecs::Entity, f32, Vec3)> = Vec::new();
        for proj in &mut self.projectiles {
            if !proj.alive() {
                continue;
            }

            let prev_pos = proj.position;
            proj.tick(dt);
            let travel_dist = (proj.position - prev_pos).length();

            // Wall collision: raycast from previous to current position
            if travel_dist > 0.001 {
                let ray = Ray::new(prev_pos, proj.direction);
                if let Some(hit) = raycast(&ray, travel_dist, &wall_aabbs) {
                    proj.position = hit.point;
                    proj.lifetime = 0.0;
                    continue;
                }
            }

            // Enemy collision (sphere/cylinder check, XZ only)
            for (entity, pos, radius) in &enemy_data {
                let delta = proj.position - *pos;
                let dist_xz = Vec3::new(delta.x, 0.0, delta.z).length();
                if dist_xz < *radius + proj.size * 0.5 {
                    hits_to_apply.push((*entity, proj.damage, *pos));

                    if proj.pierce_remaining > 0 {
                        proj.pierce_remaining -= 1;
                    } else {
                        proj.lifetime = 0.0;
                        break;
                    }
                }
            }
        }

        // Apply damage from hits
        let mut damage_events: Vec<(Vec3, f32)> = Vec::new();
        for (entity, damage, pos) in &hits_to_apply {
            let dealt = crate::combat::debuff::apply_damage(world, *entity, *damage);
            if dealt > 0.0 {
                damage_events.push((*pos, dealt));
            }
            // Spawn hit spark particles
            let emitter = Emitter::new(*pos, EmitterConfig::hit_spark([1.0, 0.8, 0.3]));
            renderer.particle_system.add_emitter(emitter);
        }

        // Remove dead projectiles and return their pool slots
        let mut i = 0;
        while i < self.projectiles.len() {
            if !self.projectiles[i].alive() {
                self.projectiles.swap_remove(i);
                let slot = self.pool_slots.swap_remove(i);
                self.free_slot(slot, renderer);
            } else {
                // Update render object transform
                let slot = self.pool_slots[i];
                if let Some(&obj_idx) = self.pool_obj_indices.get(slot) {
                    if obj_idx < renderer.objects.len() {
                        let proj = &self.projectiles[i];
                        let rot_y = (-proj.direction.x).atan2(-proj.direction.z);
                        renderer.objects[obj_idx].model_matrix =
                            Mat4::from_translation(proj.position)
                                * Mat4::from_rotation_y(rot_y)
                                * Mat4::from_scale(Vec3::splat(proj.size));
                    }
                }
                // Arrow trail particles (every other frame to reduce cost)
                let proj = &self.projectiles[i];
                if proj.lifetime > 0.05 {
                    let trail_emitter = Emitter::new(proj.position, EmitterConfig::arrow_trail());
                    renderer.particle_system.add_emitter(trail_emitter);
                }
                i += 1;
            }
        }

        damage_events
    }

    /// Tick AoE damage zones. Returns (position, damage) for each hit this frame.
    pub fn tick_aoe(
        &mut self,
        world: &mut hecs::World,
        dt: f32,
    ) -> Vec<(Vec3, f32)> {
        let mut damage_events = Vec::new();

        self.aoe_zones.retain_mut(|zone| {
            zone.elapsed += dt;
            zone.tick_timer += dt;

            // Apply damage on each tick interval
            if zone.tick_timer >= zone.tick_interval {
                zone.tick_timer -= zone.tick_interval;

                // Snapshot enemies in radius first, then damage them
                // through the centralised debuff-aware path.
                let targets: Vec<(hecs::Entity, Vec3)> = world
                    .query::<(&Transform, &Enemy)>()
                    .iter()
                    .filter(|(_, (t, _))| (t.position - zone.position).length() < zone.radius)
                    .map(|(e, (t, _))| (e, t.position))
                    .collect();
                for (entity, pos) in targets {
                    let dealt = crate::combat::debuff::apply_damage(
                        world, entity, zone.damage_per_tick,
                    );
                    if dealt > 0.0 {
                        damage_events.push((pos, dealt));
                    }
                }
            }

            zone.elapsed < zone.duration
        });

        damage_events
    }
}
