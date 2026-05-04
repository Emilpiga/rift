use glam::{Mat4, Vec3};
use rift_engine::combat::ability::AbilityId;
use rift_engine::combat::talent::{AbilityModifier, TalentEffect};
use rift_engine::combat::{Ability, Projectile};
use rift_engine::ecs::components::{Collider, Enemy, Health, Player, Static, Transform, Velocity};
use rift_engine::physics::{Aabb, Ray, raycast};
use rift_engine::{Emitter, EmitterConfig, Mesh, Renderer};

use crate::player::PlayerState;

const MAX_PROJECTILES: usize = 64;

/// Manages active projectiles: spawning, physics, collision, rendering.
/// Uses a fixed pool of pre-allocated render objects to avoid GPU buffer churn.
pub struct ProjectileManager {
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

impl ProjectileManager {
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
    pub fn fire_ability(
        &mut self,
        ability: &Ability,
        origin: Vec3,
        aim_dir: Vec3,
        damage: f32,
        player_state: &PlayerState,
        world: &mut hecs::World,
        _renderer: &mut Renderer,
    ) {
        match ability.id {
            AbilityId::SteadyShot | AbilityId::MultiShot | AbilityId::RapidFire => {
                let count = ability.projectile_count;
                let spread = ability.spread_angle;

                for i in 0..count {
                    let angle_offset = if count > 1 {
                        let t = i as f32 / (count - 1) as f32 - 0.5;
                        t * spread
                    } else {
                        0.0
                    };

                    let rot = glam::Quat::from_rotation_y(angle_offset);
                    let dir = rot * aim_dir;

                    // Spawn from approximately the player's right-hand /
                    // raised-arm position so projectiles appear to come out
                    // of the casting hand. Offset is in world-space along
                    // `aim_dir` plus a fixed shoulder height; this is a
                    // visual approximation since we don't query the live
                    // hand bone position here.
                    let yaw = aim_dir.x.atan2(aim_dir.z);
                    let right = glam::Quat::from_rotation_y(yaw) * glam::Vec3::new(0.30, 0.0, 0.0);
                    let spawn_pos = origin
                        + glam::Vec3::Y * 1.25
                        + right
                        + aim_dir * 0.55;
                    let mut proj = Projectile::arrow(spawn_pos, dir, damage);

                    // Apply talent pierce bonus
                    for node in &player_state.talents.nodes {
                        if node.current_rank > 0 {
                            if let TalentEffect::AbilityMod {
                                ability: mod_ability,
                                modifier: AbilityModifier::Pierce(n),
                            } = &node.effect
                            {
                                if *mod_ability == ability.id {
                                    proj.pierce_remaining += n * node.current_rank as u32;
                                }
                            }
                        }
                    }

                    // Allocate a pool slot for rendering
                    if let Some(slot) = self.alloc_slot() {
                        self.projectiles.push(proj);
                        self.pool_slots.push(slot);
                    }
                    // If pool exhausted, projectile is simply not rendered (dropped)
                }
            }
            AbilityId::EvasiveRoll => {
                let dash_dist = 4.0;
                for (_, (t, _, _)) in
                    world.query_mut::<(&mut Transform, &Player, &mut Velocity)>()
                {
                    // Spawn afterimage puff at start position
                    let puff = Emitter::new(t.position + Vec3::new(0.0, 0.5, 0.0), EmitterConfig::dodge_puff());
                    _renderer.particle_system.add_emitter(puff);
                    t.position += aim_dir * dash_dist;
                }
            }
            AbilityId::RainOfArrows | AbilityId::MarkForDeath => {
                let target_pos = origin + aim_dir * 5.0;
                self.execute_placed(ability, target_pos, damage, world, _renderer);
            }
        }
    }

    /// Fire a placed ability at a specific world position (used by targeting system).
    pub fn fire_ability_at(
        &mut self,
        ability: &Ability,
        _origin: Vec3,
        _aim_dir: Vec3,
        target_pos: Vec3,
        damage: f32,
        _player_state: &PlayerState,
        world: &mut hecs::World,
        renderer: &mut Renderer,
    ) {
        self.execute_placed(ability, target_pos, damage, world, renderer);
    }

    /// Execute a placed ability at the given world position.
    fn execute_placed(
        &mut self,
        ability: &Ability,
        target_pos: Vec3,
        damage: f32,
        world: &mut hecs::World,
        renderer: &mut Renderer,
    ) {
        match ability.id {
            AbilityId::RainOfArrows => {
                // Visual: rain of arrows particle effect at target area
                let rain_emitter = Emitter::new(
                    target_pos + Vec3::new(0.0, 5.0, 0.0),
                    EmitterConfig::rain_of_arrows([1.0, 0.8, 0.3]),
                );
                renderer.particle_system.add_emitter(rain_emitter);

                // Create AoE damage zone (ticks 4 times over 2 seconds)
                self.aoe_zones.push(AoeZone {
                    position: target_pos,
                    radius: 3.0,
                    damage_per_tick: damage,
                    tick_interval: 0.5,
                    duration: 2.0,
                    elapsed: 0.0,
                    tick_timer: 0.0,
                });
            }
            AbilityId::MarkForDeath => {
                // MarkForDeath: instant damage
                for (_, (t, _, health)) in
                    world.query_mut::<(&Transform, &Enemy, &mut Health)>()
                {
                    let dist = (t.position - target_pos).length();
                    if dist < 3.0 {
                        health.current -= damage;
                    }
                }
            }
            _ => {}
        }
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
            if let Ok(mut health) = world.get::<&mut Health>(*entity) {
                health.current -= damage;
                damage_events.push((*pos, *damage));
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

                // Damage all enemies in radius
                for (_, (t, _, health)) in
                    world.query_mut::<(&Transform, &Enemy, &mut Health)>()
                {
                    let dist = (t.position - zone.position).length();
                    if dist < zone.radius {
                        health.current -= zone.damage_per_tick;
                        damage_events.push((t.position, zone.damage_per_tick));
                    }
                }
            }

            zone.elapsed < zone.duration
        });

        damage_events
    }
}
