use glam::{Mat4, Vec3};
use hecs::World;
use rift_engine::ai::{self, boss_behavior, enemy_behavior, NavGrid};
use rift_engine::ai::systems::AiAgent;
use rift_engine::combat::{
    Ability, AbilitySlot, Attributes, AttributeScaling, Class, Experience, Projectile, TalentTree,
};
use rift_engine::ecs::components::{
    Boss, Collider, Enemy, Health, Player, Renderable, Static, Transform, Velocity,
};
use rift_engine::ecs::systems::{
    camera_follow_system, collision_system, contact_damage_system, despawn_system,
    movement_system, player_input_system,
    render_sync_system,
};
use rift_engine::loot::{DropTable, Equipment, Inventory, LootDrop};
use rift_engine::loot::item::{ItemKind, PotionType};
use rift_engine::ui::InventoryUI;
use rift_engine::{App, Emitter, EmitterConfig, Floor, FloorConfig, Input, Mesh, Renderer, Window};

/// Rift progression state.
struct RiftState {
    floor: u32,
    progress: f32,
    progress_required: f32,
    boss_spawned: bool,
    boss_killed: bool,
    timer: f32,
    floor_complete: bool,
    loot_timer: f32,
}

impl RiftState {
    fn new(floor: u32) -> Self {
        let progress_required = 80.0 + floor as f32 * 20.0;
        Self {
            floor,
            progress: 0.0,
            progress_required,
            boss_spawned: false,
            boss_killed: false,
            timer: 0.0,
            floor_complete: false,
            loot_timer: 0.0,
        }
    }

    fn progress_percent(&self) -> f32 {
        (self.progress / self.progress_required * 100.0).min(100.0)
    }
}

struct RiftGame {
    world: World,
    rift: RiftState,
    boss_room_center: Vec3,
    needs_new_floor: bool,
    nav_grid: NavGrid,
    // Loot
    inventory: Inventory,
    equipment: Equipment,
    ground_loot: Vec<LootDrop>,
    loot_seed: u64,
    // UI
    inventory_ui: InventoryUI,
    // Combat
    class: Class,
    attributes: Attributes,
    attribute_scaling: AttributeScaling,
    experience: Experience,
    abilities: AbilitySlot,
    talents: TalentTree,
    projectiles: Vec<Projectile>,
    /// Render object indices for active projectiles.
    projectile_obj_indices: Vec<Option<usize>>,
}

impl RiftGame {
    fn new() -> Self {
        let class = Class::Hunter;
        let config = class.config();
        let attributes = Attributes::for_class(config.primary_attribute);
        let attribute_scaling = AttributeScaling::new(config.primary_attribute);

        // Set up ability bar: Steady Shot on LMB, others on 1-5
        let mut abilities = AbilitySlot::new();
        abilities.set(0, Ability::steady_shot());
        abilities.set(1, Ability::multi_shot());
        abilities.set(2, Ability::evasive_roll());
        abilities.set(3, Ability::rapid_fire());
        abilities.set(4, Ability::mark_for_death());
        abilities.set(5, Ability::rain_of_arrows());

        Self {
            world: World::new(),
            rift: RiftState::new(1),
            boss_room_center: Vec3::ZERO,
            needs_new_floor: false,
            nav_grid: NavGrid::from_floor(&Floor::generate(FloorConfig::for_floor(1), 42)),
            inventory: Inventory::new(),
            equipment: Equipment::new(),
            ground_loot: Vec::new(),
            loot_seed: 12345,
            inventory_ui: InventoryUI::new(),
            class,
            attributes,
            attribute_scaling,
            experience: Experience::new(),
            abilities,
            talents: TalentTree::hunter(),
            projectiles: Vec::new(),
            projectile_obj_indices: Vec::new(),
        }
    }

    fn generate_floor(&mut self, renderer: &mut Renderer) -> anyhow::Result<()> {
        // Clear world (keep nothing)
        self.world = World::new();
        renderer.clear_objects();

        let config = FloorConfig::for_floor(self.rift.floor);
        let seed = 42 + self.rift.floor as u64 * 7;
        let floor = Floor::generate(config, seed);

        self.boss_room_center = floor.boss_room_center;
        self.nav_grid = NavGrid::from_floor(&floor);

        // Floor mesh
        let floor_mesh = Mesh::grid(floor.width as f32, floor.width as u32 / 2);
        renderer.add_mesh(
            &floor_mesh,
            Mat4::from_translation(Vec3::new(
                floor.width as f32 / 2.0,
                0.0,
                floor.depth as f32 / 2.0,
            )),
        )?;

        // Walls
        let wall_mesh = Mesh::wall();
        let wall_positions = floor.wall_positions();

        for pos in &wall_positions {
            renderer.add_mesh(&wall_mesh, Mat4::from_translation(*pos))?;
            let obj_index = renderer.objects.len() - 1;

            self.world.spawn((
                Transform::from_position(*pos + Vec3::new(0.0, 1.5, 0.0)),
                Collider::new(0.5, 1.5, 0.5),
                Renderable { object_index: obj_index },
                Static,
            ));
        }

        // Player
        let cube = Mesh::cube();
        let spawn = floor.spawn_pos;
        renderer.add_mesh(&cube, Mat4::from_translation(spawn))?;
        let player_obj_index = renderer.objects.len() - 1;

        let class_cfg = self.class.config();
        self.world.spawn((
            Transform::from_position(spawn),
            Velocity::default(),
            Player { speed: class_cfg.base_move_speed, aim_dir: glam::Vec3::Z, spine_joint: u32::MAX },
            Collider::new(0.3, 0.5, 0.3),
            Health::new(class_cfg.base_hp + class_cfg.hp_per_level * self.experience.level as f32),
            Renderable {
                object_index: player_obj_index,
            },
        ));

        // Enemies in arena rooms
        let enemy_mesh = Mesh::enemy();
        let config = &floor.config;
        let arena_rooms = floor.arena_rooms();
        let enemies_per_room =
            (config.enemy_count() as usize).max(1) / arena_rooms.len().max(1);

        let progress_per_enemy = self.rift.progress_required
            / (config.enemy_count() as f32).max(1.0);

        let mut enemy_seed = 1000_u64 + self.rift.floor as u64;
        for room in &arena_rooms {
            let positions = room.spawn_positions(enemies_per_room.max(1), enemy_seed);
            enemy_seed += 1;

            for pos in positions {
                renderer.add_mesh(&enemy_mesh, Mat4::from_translation(pos))?;
                let obj_index = renderer.objects.len() - 1;

                self.world.spawn((
                    Transform::from_position(pos),
                    Velocity::default(),
                    Enemy {
                        speed: config.enemy_speed,
                        progress_value: progress_per_enemy,
                    },
                    Collider::new(0.4, 0.45, 0.4),
                    Health::new(config.enemy_health),
                    Renderable { object_index: obj_index },
                    AiAgent::new(enemy_behavior(), pos),
                ));
            }
        }

        log::info!(
            "=== RIFT LEVEL {} === | {} rooms | {} enemies | Kill progress needed: {:.0}",
            self.rift.floor,
            floor.rooms.len(),
            config.enemy_count(),
            self.rift.progress_required
        );

        Ok(())
    }

    fn spawn_boss(&mut self, renderer: &mut Renderer) {
        let boss_mesh = Mesh::boss();
        let pos = self.boss_room_center + Vec3::new(0.0, 0.0, 0.0);

        if let Ok(()) = renderer.add_mesh(&boss_mesh, Mat4::from_translation(pos)) {
            let obj_index = renderer.objects.len() - 1;
            let boss_health = 100.0 + self.rift.floor as f32 * 50.0;

            self.world.spawn((
                Transform::from_position(pos),
                Velocity::default(),
                Enemy {
                    speed: self.rift.rift_boss_speed(),
                    progress_value: 0.0,
                },
                Boss,
                Collider::new(0.8, 0.9, 0.8),
                Health::new(boss_health),
                Renderable { object_index: obj_index },
                AiAgent::new(boss_behavior(), pos),
            ));

            log::info!(
                ">>> BOSS SPAWNED! HP: {:.0} | Location: boss room <<<",
                boss_health
            );
        }
    }

    /// Compute final damage for a weapon attack.
    fn compute_attack_damage(&self) -> f32 {
        let class_cfg = self.class.config();
        let equip_stats = self.equipment.total_stats();
        let talent_bonuses = self.talents.compute_bonuses();

        let base = class_cfg.base_damage + equip_stats.flat_damage;
        let attr_bonus = self.attribute_scaling.damage_bonus(&self.attributes);
        let talent_bonus = talent_bonuses.damage_pct;
        let equip_pct = equip_stats.percent_damage;

        base * (1.0 + attr_bonus + talent_bonus + equip_pct)
    }

    /// Find the nearest enemy position to the player (for auto-aim).
    fn nearest_enemy_dir(&self, player_pos: Vec3, range: f32) -> Option<Vec3> {
        let mut best: Option<(f32, Vec3)> = None;
        for (_, (t, _)) in self.world.query::<(&Transform, &Enemy)>().iter() {
            let delta = t.position - player_pos;
            let dist = delta.length();
            if dist < range && dist > 0.1 {
                if best.is_none() || dist < best.unwrap().0 {
                    best = Some((dist, delta.normalize()));
                }
            }
        }
        best.map(|(_, dir)| dir)
    }

    /// Tick the ability-based combat system.
    fn tick_combat(&mut self, input: &Input, renderer: &mut Renderer, dt: f32) {
        use winit::keyboard::KeyCode;

        let player_data: Option<(Vec3, glam::Quat)> = self.world
            .query::<(&Transform, &Player)>()
            .iter()
            .map(|(_, (t, _))| (t.position, t.rotation))
            .next();

        let Some((player_pos, player_rot)) = player_data else { return };

        let class_cfg = self.class.config();
        let range = class_cfg.base_range;

        // Determine aim direction (towards nearest enemy, or facing direction)
        let aim_dir = self.nearest_enemy_dir(player_pos, range)
            .unwrap_or_else(|| {
                // Use player facing direction
                let forward = player_rot * Vec3::new(0.0, 0.0, -1.0);
                Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero()
            });

        // Ability keybinds: LMB = slot 0 (Steady Shot), 1-5 = slots 1-5
        let ability_inputs = [
            input.left_clicked(),                            // Slot 0: LMB
            input.key_just_pressed(KeyCode::Digit1),         // Slot 1
            input.key_just_pressed(KeyCode::Digit2),         // Slot 2
            input.key_just_pressed(KeyCode::Digit3),         // Slot 3
            input.key_just_pressed(KeyCode::Digit4),         // Slot 4
            input.key_just_pressed(KeyCode::Digit5),         // Slot 5
        ];

        for (i, &pressed) in ability_inputs.iter().enumerate() {
            if !pressed { continue; }
            if let Some(ability) = self.abilities.try_use(i) {
                let ability_clone = ability.clone();
                self.fire_ability(&ability_clone, player_pos, aim_dir, renderer);
            }
        }

        // Tick projectiles — collect enemy data for collision
        let mut enemy_data: Vec<(hecs::Entity, Vec3, f32)> = self.world
            .query::<(&Transform, &Enemy, &Collider)>()
            .iter()
            .map(|(e, (t, _, c))| (e, t.position, c.half_extents.x))
            .collect();

        // Move projectiles and check collisions
        let mut hits_to_apply: Vec<(hecs::Entity, f32)> = Vec::new();
        for proj in &mut self.projectiles {
            if !proj.alive() { continue; }
            proj.tick(dt);

            // Check collision with enemies
            for (entity, pos, radius) in &enemy_data {
                let dist = (proj.position - *pos).length();
                if dist < *radius + proj.size * 0.5 {
                    hits_to_apply.push((*entity, proj.damage));

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
        for (entity, damage) in &hits_to_apply {
            if let Ok(mut health) = self.world.get::<&mut Health>(*entity) {
                health.current -= damage;
            }
            // Spawn hit spark particles
            if let Some((_, pos, _)) = enemy_data.iter().find(|(e, _, _)| e == entity) {
                let emitter = Emitter::new(*pos, EmitterConfig::hit_spark([1.0, 0.8, 0.3]));
                renderer.particle_system.add_emitter(emitter);
            }
        }

        // Remove dead projectiles and hide their render objects
        let mut i = 0;
        while i < self.projectiles.len() {
            if !self.projectiles[i].alive() {
                self.projectiles.swap_remove(i);
                if let Some(obj_idx) = self.projectile_obj_indices.swap_remove(i) {
                    if obj_idx < renderer.objects.len() {
                        renderer.objects[obj_idx].model_matrix = Mat4::ZERO;
                    }
                }
            } else {
                // Update projectile render position
                if let Some(Some(obj_idx)) = self.projectile_obj_indices.get(i) {
                    if *obj_idx < renderer.objects.len() {
                        let proj = &self.projectiles[i];
                        let rot_y = (-proj.direction.x).atan2(-proj.direction.z);
                        renderer.objects[*obj_idx].model_matrix =
                            Mat4::from_translation(proj.position)
                            * Mat4::from_rotation_y(rot_y)
                            * Mat4::from_scale(Vec3::splat(proj.size));
                    }
                }
                i += 1;
            }
        }
    }

    /// Fire an ability: spawn projectiles.
    fn fire_ability(&mut self, ability: &Ability, origin: Vec3, aim_dir: Vec3, renderer: &mut Renderer) {
        use rift_engine::combat::ability::AbilityId;

        let damage = self.compute_attack_damage() * ability.damage_mult;

        match ability.id {
            AbilityId::SteadyShot | AbilityId::MultiShot | AbilityId::RapidFire => {
                let count = ability.projectile_count;
                let spread = ability.spread_angle;

                for i in 0..count {
                    // Spread arrows evenly across the spread angle
                    let angle_offset = if count > 1 {
                        let t = i as f32 / (count - 1) as f32 - 0.5;
                        t * spread
                    } else {
                        0.0
                    };

                    let rot = glam::Quat::from_rotation_y(angle_offset);
                    let dir = rot * aim_dir;

                    let spawn_pos = origin + Vec3::new(0.0, 0.8, 0.0) + dir * 0.5;
                    let mut proj = Projectile::arrow(spawn_pos, dir, damage);

                    // Apply talent pierce bonus
                    let talent_bonuses = self.talents.compute_bonuses();
                    // Check for pierce talent on steady shot
                    for node in &self.talents.nodes {
                        if node.current_rank > 0 {
                            if let rift_engine::combat::talent::TalentEffect::AbilityMod {
                                ability: mod_ability,
                                modifier: rift_engine::combat::talent::AbilityModifier::Pierce(n),
                            } = &node.effect {
                                if *mod_ability == ability.id {
                                    proj.pierce_remaining += n * node.current_rank as u32;
                                }
                            }
                        }
                    }

                    self.projectiles.push(proj);

                    // Add a small arrow mesh for rendering
                    let arrow_mesh = Mesh::arrow();
                    if renderer.add_mesh(&arrow_mesh, Mat4::ZERO).is_ok() {
                        self.projectile_obj_indices.push(Some(renderer.objects.len() - 1));
                    } else {
                        self.projectile_obj_indices.push(None);
                    }
                }
            }
            AbilityId::EvasiveRoll => {
                // Dash in movement direction — handled as instant position offset
                let move_dir = aim_dir;
                let dash_dist = 4.0;
                for (_, (t, _, _)) in self.world.query_mut::<(&mut Transform, &Player, &mut Velocity)>() {
                    t.position += move_dir * dash_dist;
                }
            }
            AbilityId::RainOfArrows | AbilityId::MarkForDeath => {
                // TODO: AoE and debuff systems (placeholder — just deal instant damage)
                let target_pos = origin + aim_dir * 5.0;
                for (_, (t, _, health)) in self.world.query_mut::<(&Transform, &Enemy, &mut Health)>() {
                    let dist = (t.position - target_pos).length();
                    if dist < 3.0 {
                        health.current -= damage;
                    }
                }
            }
        }
    }
}

impl RiftState {
    fn rift_boss_speed(&self) -> f32 {
        3.0 + self.floor as f32 * 0.5
    }
}

impl App for RiftGame {
    fn init(&mut self, renderer: &mut Renderer) -> anyhow::Result<()> {
        self.generate_floor(renderer)?;
        Ok(())
    }

    fn update(&mut self, renderer: &mut Renderer, input: &Input, dt: f32) {
        // Floor transition
        if self.needs_new_floor {
            self.rift = RiftState::new(self.rift.floor + 1);
            self.needs_new_floor = false;
            self.ground_loot.clear();
            self.projectiles.clear();
            self.projectile_obj_indices.clear();
            renderer.particle_system.clear_emitters();
            if let Err(e) = self.generate_floor(renderer) {
                log::error!("Failed to generate floor: {}", e);
            }
            return;
        }

        self.rift.timer += dt;

        // Apply equipment stats to player
        let stats = self.equipment.total_stats();

        // Game systems
        player_input_system(&mut self.world, input, dt);
        ai::ai_system(&mut self.world, dt, &self.nav_grid);
        movement_system(&mut self.world, dt);
        collision_system(&mut self.world);
        contact_damage_system(&mut self.world, dt);

        // Ability-based combat
        self.abilities.tick_all(dt);
        self.tick_combat(input, renderer, dt);

        let kills = despawn_system(&mut self.world, renderer);

        // Process kills: track progress + spawn loot + grant XP
        let mut progress_earned = 0.0_f32;
        let mut boss_killed = false;
        for kill in &kills {
            progress_earned += kill.progress_value;
            if kill.is_boss {
                boss_killed = true;
            }

            // Grant XP
            let monster_level = self.rift.floor;
            let xp = Experience::xp_for_kill(monster_level, self.experience.level);
            let rewards = self.experience.grant_xp(xp);
            for reward in &rewards {
                self.attributes.unspent_points += reward.attribute_points;
                self.talents.unspent_points += reward.talent_points;
                log::info!(
                    ">>> LEVEL UP! Now level {} | +{} attr pts, +{} talent pts <<<",
                    reward.new_level, reward.attribute_points, reward.talent_points
                );
            }

            // Roll drop table — items burst out with physics
            self.loot_seed = self.loot_seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let table = if kill.is_boss { DropTable::boss() } else { DropTable::enemy() };
            let drops = table.roll(self.rift.floor, kill.position, self.loot_seed);
            for drop in drops {
                log::info!(
                    "  LOOT: {} ({:?})",
                    drop.item.display_name, drop.item.rarity
                );
                self.ground_loot.push(drop);
            }
        }

        // Track rift progress
        if progress_earned > 0.0 {
            self.rift.progress += progress_earned;
            let pct = self.rift.progress_percent();
            log::info!("Rift progress: {:.0}%", pct);

            if pct >= 100.0 && !self.rift.boss_spawned {
                self.rift.boss_spawned = true;
                self.spawn_boss(renderer);
            }
        }

        // Boss killed — start loot timer before advancing
        if boss_killed {
            self.rift.boss_killed = true;
            self.rift.floor_complete = true;
            self.rift.loot_timer = 5.0; // 5 seconds to loot before next floor
            log::info!(
                "=== RIFT LEVEL {} COMPLETE! Looting for 5s... ===",
                self.rift.floor,
            );
        }

        // Countdown loot timer after boss kill
        if self.rift.floor_complete && !self.needs_new_floor {
            self.rift.loot_timer -= dt;
            if self.rift.loot_timer <= 0.0 {
                self.needs_new_floor = true;
                log::info!("Floor transition! Time: {:.1}s", self.rift.timer);
            }
        }

        // Pick up loot on left-click (nearest grounded item in range)
        // Only if the inventory UI didn't consume the click
        let (sw, sh) = renderer.screen_size();
        let ui_consumed = self.inventory_ui.update(input, &mut self.inventory, &mut self.equipment, sw, sh);

        let player_pos: Option<Vec3> = self.world
            .query::<(&Transform, &Player)>()
            .iter()
            .map(|(_, (t, _))| t.position)
            .next();

        if let Some(player_pos) = player_pos {
            if !ui_consumed && input.left_clicked() {
                let pickup_radius = 2.0_f32;
                // Find nearest grounded item in range
                let nearest = self.ground_loot.iter().enumerate()
                    .filter(|(_, d)| d.grounded)
                    .map(|(i, d)| (i, (d.position - player_pos).length()))
                    .filter(|(_, dist)| *dist < pickup_radius)
                    .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

                if let Some((i, _)) = nearest {
                    let drop = self.ground_loot.remove(i);
                    let item_name = drop.item.display_name.clone();
                    let rarity = drop.item.rarity;

                    // Hide render objects
                    if let Some(idx) = drop.orb_obj_index {
                        if idx < renderer.objects.len() {
                            renderer.objects[idx].model_matrix = Mat4::ZERO;
                        }
                    }
                    if let Some(idx) = drop.beam_obj_index {
                        if idx < renderer.objects.len() {
                            renderer.objects[idx].model_matrix = Mat4::ZERO;
                        }
                    }
                    // Remove particle emitter
                    if let Some(idx) = drop.emitter_index {
                        renderer.particle_system.deactivate_emitter(idx);
                    }

                    // Auto-equip if it's better, otherwise add to inventory
                    if let Some(slot) = drop.item.slot() {
                        let should_equip = match self.equipment.get(slot) {
                            None => true,
                            Some(current) => {
                                let current_power = current.total_damage() + current.total_defense();
                                let new_power = drop.item.total_damage() + drop.item.total_defense();
                                new_power > current_power
                            }
                        };

                        if should_equip {
                            let old = self.equipment.equip(drop.item);
                            log::info!("  EQUIPPED: {} ({:?})", item_name, rarity);
                            if let Some(old_item) = old {
                                self.inventory.add_item(old_item);
                            }
                        } else {
                            self.inventory.add_item(drop.item);
                            log::info!("  PICKED UP: {} ({:?})", item_name, rarity);
                        }
                    } else {
                        // Potion — use immediately
                        if let ItemKind::Potion(potion_type) = drop.item.base.kind {
                            match potion_type {
                                PotionType::Health => {
                                    for (_id, (health, _player)) in self.world.query_mut::<(&mut Health, &Player)>() {
                                        let heal = drop.item.base.base_value;
                                        health.current = (health.current + heal).min(health.max + stats.max_hp_bonus);
                                        log::info!("  HEALED: +{:.0} HP", heal);
                                    }
                                }
                                _ => {
                                    log::info!("  USED: {}", item_name);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Tick loot physics + create/update render objects
        for drop in &mut self.ground_loot {
            drop.tick_physics(dt);
            drop.lifetime -= dt;

            // Create render objects for new drops that don't have them yet
            if drop.orb_obj_index.is_none() {
                let color = drop.item.rarity.color();
                let orb_mesh = Mesh::loot_orb(color);
                if renderer.add_mesh(&orb_mesh, Mat4::from_translation(drop.position)).is_ok() {
                    drop.orb_obj_index = Some(renderer.objects.len() - 1);
                }
            }
            if drop.beam_obj_index.is_none() && drop.grounded {
                let color = drop.item.rarity.color();
                let beam_mesh = Mesh::light_beam(color);
                if renderer.add_mesh(&beam_mesh, Mat4::ZERO).is_ok() {
                    drop.beam_obj_index = Some(renderer.objects.len() - 1);
                }
            }

            // Spawn particle emitter when grounded
            if drop.emitter_index.is_none() && drop.grounded {
                let color = drop.item.rarity.color();
                let emitter = Emitter::new(drop.position, EmitterConfig::loot_beam(color));
                let idx = renderer.particle_system.add_emitter(emitter);
                drop.emitter_index = Some(idx);
            }

            // Update orb position (follows physics + bob)
            if let Some(idx) = drop.orb_obj_index {
                if idx < renderer.objects.len() {
                    let bob = drop.bob_offset();
                    let pos = drop.position + Vec3::new(0.0, bob, 0.0);
                    // Spin the orb
                    let spin = Mat4::from_rotation_y(drop.age * 3.0);
                    renderer.objects[idx].model_matrix =
                        Mat4::from_translation(pos) * spin;
                }
            }

            // Update beam (only when grounded, scales with rarity)
            if let Some(idx) = drop.beam_obj_index {
                if idx < renderer.objects.len() {
                    let beam_h = drop.beam_height();
                    if beam_h > 0.1 {
                        let scale = Mat4::from_scale(Vec3::new(1.0, beam_h, 1.0));
                        renderer.objects[idx].model_matrix =
                            Mat4::from_translation(drop.position) * scale;
                    } else {
                        renderer.objects[idx].model_matrix = Mat4::ZERO;
                    }
                }
            }
        }

        // Remove expired loot (hide their render objects)
        self.ground_loot.retain(|d| {
            if d.lifetime <= 0.0 {
                if let Some(idx) = d.emitter_index {
                    renderer.particle_system.deactivate_emitter(idx);
                }
                false
            } else {
                true
            }
        });

        // HP regen from equipment
        if stats.hp_regen > 0.0 {
            if let Some(player_pos) = player_pos {
                let _ = player_pos;
                for (_id, (health, _player)) in self.world.query_mut::<(&mut Health, &Player)>() {
                    health.current = (health.current + stats.hp_regen * dt).min(health.max + stats.max_hp_bonus);
                }
            }
        }

        render_sync_system(&self.world, renderer);
        camera_follow_system(&self.world, renderer, input);

        // Tick particles
        renderer.particle_system.tick(dt);

        // === HUD Overlay ===
        renderer.overlay_batch.clear();
        let (sw, sh) = renderer.screen_size();

        // HP bar (top-left, 200x20 px)
        let hp_pct = self.world
            .query::<(&Health, &Player)>()
            .iter()
            .map(|(_, (h, _))| h.current / (h.max + stats.max_hp_bonus))
            .next()
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);

        let bar_x = 10.0;
        let bar_y = 10.0;
        let bar_w = 200.0;
        let bar_h = 20.0;
        // Background
        renderer.overlay_batch.rect_px(bar_x, bar_y, bar_w, bar_h, [0.1, 0.1, 0.1, 0.8], sw, sh);
        // HP fill (green → red)
        let hp_color = if hp_pct > 0.5 {
            [0.1, 0.8, 0.1, 0.9]
        } else if hp_pct > 0.25 {
            [0.9, 0.7, 0.0, 0.9]
        } else {
            [0.9, 0.1, 0.1, 0.9]
        };
        renderer.overlay_batch.rect_px(bar_x, bar_y, bar_w * hp_pct, bar_h, hp_color, sw, sh);
        // Border
        renderer.overlay_batch.rect_px(bar_x, bar_y, bar_w, 2.0, [0.4, 0.4, 0.4, 0.9], sw, sh);
        renderer.overlay_batch.rect_px(bar_x, bar_y + bar_h - 2.0, bar_w, 2.0, [0.4, 0.4, 0.4, 0.9], sw, sh);
        renderer.overlay_batch.rect_px(bar_x, bar_y, 2.0, bar_h, [0.4, 0.4, 0.4, 0.9], sw, sh);
        renderer.overlay_batch.rect_px(bar_x + bar_w - 2.0, bar_y, 2.0, bar_h, [0.4, 0.4, 0.4, 0.9], sw, sh);

        // Rift progress bar (top-center, 300x16 px)
        let prog_pct = self.rift.progress_percent() / 100.0;
        let prog_w = 300.0;
        let prog_h = 16.0;
        let prog_x = (sw - prog_w) / 2.0;
        let prog_y = 10.0;
        renderer.overlay_batch.rect_px(prog_x, prog_y, prog_w, prog_h, [0.1, 0.1, 0.1, 0.8], sw, sh);
        renderer.overlay_batch.rect_px(prog_x, prog_y, prog_w * prog_pct, prog_h, [0.3, 0.5, 0.9, 0.9], sw, sh);

        // Floor indicator (small box top-right)
        let floor_w = 40.0;
        let floor_h = 20.0;
        renderer.overlay_batch.rect_px(sw - floor_w - 10.0, 10.0, floor_w, floor_h, [0.2, 0.2, 0.3, 0.8], sw, sh);
        // Show floor number as bars (1 bar per floor, up to 10)
        let bars = (self.rift.floor as f32).min(10.0);
        let bar_unit_w = (floor_w - 6.0) / 10.0;
        for i in 0..bars as u32 {
            renderer.overlay_batch.rect_px(
                sw - floor_w - 10.0 + 3.0 + i as f32 * bar_unit_w,
                14.0,
                bar_unit_w - 1.0,
                floor_h - 8.0,
                [0.8, 0.7, 0.2, 0.9],
                sw, sh,
            );
        }

        // Equipment slots (bottom-left, 6 slots: 32x32 each)
        let slot_size = 32.0;
        let slot_gap = 4.0;
        let eq_x = 10.0;
        let eq_y = sh - slot_size - 10.0;
        let slots = [
            self.equipment.get(rift_engine::loot::item::ItemSlot::Weapon),
            self.equipment.get(rift_engine::loot::item::ItemSlot::Helmet),
            self.equipment.get(rift_engine::loot::item::ItemSlot::Chest),
            self.equipment.get(rift_engine::loot::item::ItemSlot::Boots),
            self.equipment.get(rift_engine::loot::item::ItemSlot::Ring),
            self.equipment.get(rift_engine::loot::item::ItemSlot::Amulet),
        ];
        for (i, slot) in slots.iter().enumerate() {
            let sx = eq_x + i as f32 * (slot_size + slot_gap);
            // Slot background
            renderer.overlay_batch.rect_px(sx, eq_y, slot_size, slot_size, [0.15, 0.15, 0.2, 0.8], sw, sh);
            // If equipped, fill with rarity color
            if let Some(item) = slot {
                let [r, g, b] = item.rarity.color();
                renderer.overlay_batch.rect_px(
                    sx + 3.0, eq_y + 3.0,
                    slot_size - 6.0, slot_size - 6.0,
                    [r, g, b, 0.9],
                    sw, sh,
                );
            }
        }

        // Inventory panel (managed by InventoryUI — Tab to toggle)
        self.inventory_ui.render(&mut renderer.overlay_batch, &self.inventory, &self.equipment, input, sw, sh);

        // XP bar (below HP bar, 200x10 px)
        let xp_pct = self.experience.progress();
        let xp_x = 10.0;
        let xp_y = 34.0;
        let xp_w = 200.0;
        let xp_h = 10.0;
        renderer.overlay_batch.rect_px(xp_x, xp_y, xp_w, xp_h, [0.1, 0.1, 0.1, 0.7], sw, sh);
        renderer.overlay_batch.rect_px(xp_x, xp_y, xp_w * xp_pct, xp_h, [0.4, 0.2, 0.9, 0.9], sw, sh);
        // Level text
        let level_text = format!("Lv.{}", self.experience.level);
        renderer.overlay_batch.text(&level_text, xp_x + xp_w + 6.0, xp_y, 14.0, [0.9, 0.9, 0.9, 1.0], sw, sh);

        // Ability bar (bottom-center, 6 slots: 40x40 each)
        let ab_size = 40.0;
        let ab_gap = 4.0;
        let ab_total_w = 6.0 * ab_size + 5.0 * ab_gap;
        let ab_x = (sw - ab_total_w) / 2.0;
        let ab_y = sh - ab_size - 10.0;
        let ab_keys = ["LMB", "1", "2", "3", "4", "5"];

        for (i, slot) in self.abilities.slots.iter().enumerate() {
            let sx = ab_x + i as f32 * (ab_size + ab_gap);

            // Slot background
            renderer.overlay_batch.rect_px(sx, ab_y, ab_size, ab_size, [0.12, 0.12, 0.18, 0.85], sw, sh);

            if let Some(state) = slot {
                // Fill with ability color (dimmed if on cooldown)
                let ready = state.ready();
                let color = if ready {
                    [0.3, 0.6, 0.9, 0.9]
                } else {
                    [0.15, 0.2, 0.3, 0.7]
                };
                renderer.overlay_batch.rect_px(sx + 2.0, ab_y + 2.0, ab_size - 4.0, ab_size - 4.0, color, sw, sh);

                // Cooldown sweep (dark overlay from top)
                if !ready {
                    let cd_pct = 1.0 - state.cooldown_progress();
                    let cd_h = (ab_size - 4.0) * cd_pct;
                    renderer.overlay_batch.rect_px(sx + 2.0, ab_y + 2.0, ab_size - 4.0, cd_h, [0.0, 0.0, 0.0, 0.6], sw, sh);
                }
            }

            // Keybind label
            renderer.overlay_batch.text(ab_keys[i], sx + 2.0, ab_y + ab_size - 12.0, 10.0, [0.7, 0.7, 0.7, 0.8], sw, sh);
        }

        // Loot timer bar (if floor complete, shows countdown)
        if self.rift.floor_complete && !self.needs_new_floor {
            let timer_pct = (self.rift.loot_timer / 5.0).clamp(0.0, 1.0);
            let tw = 250.0;
            let th = 12.0;
            let tx = (sw - tw) / 2.0;
            let ty = 35.0;
            renderer.overlay_batch.rect_px(tx, ty, tw, th, [0.1, 0.1, 0.1, 0.8], sw, sh);
            renderer.overlay_batch.rect_px(tx, ty, tw * timer_pct, th, [0.9, 0.6, 0.1, 0.9], sw, sh);
        }
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let window = Window::new("Rift Crawler", 1280, 720);
    window.run(RiftGame::new())
}
