pub mod rift_state;
pub mod player;
pub mod projectiles;
pub mod loot_manager;
pub mod floor;
pub mod hud;
pub mod combat_text;
pub mod equipment_visuals;
pub mod decals;
pub mod enemy_attacks;

use glam::Vec3;
use rift_engine::ai;
use rift_engine::combat::Class;
use rift_engine::combat::ability::TargetingMode;
use rift_engine::ecs::components::{Health, Player, Transform};
use combat_text::CombatTextSystem;
use decals::DecalSystem;
use equipment_visuals::EquipmentVisuals;
use rift_engine::ecs::systems::{
    camera_follow_system, cast_advance_system, collision_system, contact_damage_system, despawn_system,
    movement_system, player_input_system, render_sync_system, skinning_system, locomotion_anim_system,
};
use rift_engine::loot::{Equipment, Inventory};
use rift_engine::ui::InventoryUI;
use rift_engine::{EmitterConfig, Input, PointLight, Renderer};

use floor::FloorManager;
use loot_manager::LootManager;
use player::PlayerState;
use projectiles::ProjectileManager;
use rift_state::RiftState;
use enemy_attacks::EnemyAttackSystem;

/// Top-level game state — the single struct that orchestrates all gameplay.
pub struct GameState {
    pub world: hecs::World,
    pub rift: RiftState,
    pub player_state: PlayerState,
    pub floor_mgr: FloorManager,
    pub loot_mgr: LootManager,
    pub projectile_mgr: ProjectileManager,
    pub inventory: Inventory,
    pub equipment: Equipment,
    pub inventory_ui: InventoryUI,
    pub combat_text: CombatTextSystem,
    pub equip_visuals: EquipmentVisuals,
    pub decals: DecalSystem,
    pub enemy_attacks: EnemyAttackSystem,
    needs_new_floor: bool,
    /// Cached wall colliders for physics (rebuilt on floor change).
    wall_colliders: Vec<(Vec3, rift_engine::ecs::components::Collider)>,
    /// Cached wall AABBs for raycasting (rebuilt on floor change).
    wall_aabbs: Vec<rift_engine::physics::Aabb>,
    /// Exit portal state.
    portal: Option<Portal>,
    /// Active placed-ability targeting (if any).
    targeting: Option<PlacedTargeting>,
}

/// A floor exit portal that the player walks into.
struct Portal {
    position: Vec3,
    obj_idx: usize,
    emitter_idx: usize,
    age: f32,
}

/// Active placed-ability targeting state (player is choosing where to place an AoE).
struct PlacedTargeting {
    /// Which ability slot triggered this.
    slot_index: usize,
    /// The ability being placed (cloned).
    ability: rift_engine::combat::Ability,
    /// Pre-computed damage for the ability.
    damage: f32,
    /// Radius of the AoE indicator circle.
    radius: f32,
    /// Render object index for the ground indicator mesh.
    indicator_obj: Option<usize>,
}

impl GameState {
    pub fn new() -> Self {
        Self {
            world: hecs::World::new(),
            rift: RiftState::new(1),
            player_state: PlayerState::new(Class::Hunter),
            floor_mgr: FloorManager::new(),
            loot_mgr: LootManager::new(),
            projectile_mgr: ProjectileManager::new(),
            inventory: Inventory::new(),
            equipment: Equipment::new(),
            inventory_ui: InventoryUI::new(),
            combat_text: CombatTextSystem::new(),
            equip_visuals: EquipmentVisuals::new(),
            decals: DecalSystem::new(),
            enemy_attacks: EnemyAttackSystem::new(),
            needs_new_floor: false,
            wall_colliders: Vec::new(),
            wall_aabbs: Vec::new(),
            portal: None,
            targeting: None,
        }
    }

    pub fn init(&mut self, renderer: &mut Renderer) -> anyhow::Result<()> {
        self.floor_mgr.generate(
            &mut self.world,
            renderer,
            &self.rift,
            &self.player_state,
        )?;
        self.projectile_mgr.init_pool(renderer);
        self.equip_visuals.init(renderer);
        self.enemy_attacks.init_pool(renderer);
        self.rebuild_wall_caches();
        Ok(())
    }

    /// Rebuild cached wall collision data from ECS static entities.
    fn rebuild_wall_caches(&mut self) {
        use rift_engine::ecs::components::{Collider, Static};
        use rift_engine::physics::Aabb;

        self.wall_colliders = self.world
            .query::<(&Transform, &Collider, &Static)>()
            .iter()
            .map(|(_, (t, c, _))| (t.position, *c))
            .collect();

        self.wall_aabbs = self.wall_colliders
            .iter()
            .map(|(pos, col)| Aabb::from_center(*pos, col.half_extents))
            .collect();

        self.projectile_mgr.rebuild_wall_cache(&self.world);
        self.enemy_attacks.rebuild_wall_cache(&self.world);
    }

    pub fn update(&mut self, renderer: &mut Renderer, input: &Input, dt: f32) {
        // Floor transition
        if self.needs_new_floor {
            self.rift = RiftState::new(self.rift.floor + 1);
            self.needs_new_floor = false;
            self.portal = None;
            self.decals.clear();
            self.loot_mgr.clear();
            self.projectile_mgr.clear(renderer);
            self.enemy_attacks.clear(renderer);
            renderer.particle_system.clear_emitters();
            if let Err(e) = self.floor_mgr.generate(
                &mut self.world,
                renderer,
                &self.rift,
                &self.player_state,
            ) {
                log::error!("Failed to generate floor: {}", e);
            }
            self.projectile_mgr.init_pool(renderer);
            self.equip_visuals.init(renderer);
            self.enemy_attacks.init_pool(renderer);
            self.rebuild_wall_caches();
            return;
        }

        self.rift.timer += dt;
        let stats = self.equipment.total_stats();

        // ECS systems
        player_input_system(&mut self.world, input, dt);
        ai::ai_system(&mut self.world, dt, &self.floor_mgr.nav_grid, &self.wall_aabbs);
        movement_system(&mut self.world, dt);
        collision_system(&mut self.world, &self.wall_colliders);

        // Track player HP before contact damage for feedback
        let hp_before: f32 = self.world.query::<(&Health, &Player)>().iter()
            .map(|(_, (h, _))| h.current).next().unwrap_or(0.0);
        contact_damage_system(&mut self.world, dt);
        let hp_after: f32 = self.world.query::<(&Health, &Player)>().iter()
            .map(|(_, (h, _))| h.current).next().unwrap_or(0.0);
        let player_dmg_taken = hp_before - hp_after;
        if player_dmg_taken > 0.5 {
            if let Some(pos) = self.world.query::<(&Transform, &Player)>().iter()
                .map(|(_, (t, _))| t.position).next()
            {
                self.combat_text.spawn_player_damage(pos, player_dmg_taken);
            }
        }

        // Loot hover + click-to-pickup (BEFORE combat so it consumes the click first)
        let (sw, sh) = renderer.screen_size();
        let ui_consumed =
            self.inventory_ui
                .update(input, &mut self.inventory, &mut self.equipment, sw, sh);

        // Handle drag-to-drop: spawn item on ground near player
        if let Some(dropped) = self.inventory_ui.dropped_item.take() {
            if let Some(pp) = self
                .world
                .query::<(&Transform, &Player)>()
                .iter()
                .map(|(_, (t, _))| t.position)
                .next()
            {
                let drop_pos = Vec3::new(pp.x + 1.0, 0.3, pp.z + 1.0);
                self.loot_mgr.spawn_drop(dropped, drop_pos, renderer);
            }
        }

        let player_pos: Option<Vec3> = self
            .world
            .query::<(&Transform, &Player)>()
            .iter()
            .map(|(_, (t, _))| t.position)
            .next();

        let cursor_hit = Self::cursor_world_pos(input, renderer, 0.3); // ground loot at y≈0.3
        let hovered = cursor_hit.and_then(|pos| self.loot_mgr.item_under_cursor(pos, 0.8));
        self.loot_mgr.update_hover(hovered, renderer);

        let mut loot_clicked = false;
        if !ui_consumed {
            if let Some(idx) = hovered {
                if input.left_clicked() {
                    // Check pickup distance — must be within 3 units of the item
                    let in_range = player_pos
                        .and_then(|pp| self.loot_mgr.ground_loot.get(idx).map(|d| {
                            let delta = pp - d.position;
                            Vec3::new(delta.x, 0.0, delta.z).length() < 3.0
                        }))
                        .unwrap_or(false);

                    if in_range {
                        self.loot_mgr.pickup_at(
                            idx,
                            &mut self.inventory,
                            &mut self.equipment,
                            &stats,
                            &mut self.world,
                            renderer,
                        );
                        loot_clicked = true;
                    }
                }
            }
        }

        // Portal interaction: click to enter (check before combat so it consumes the click)
        let mut portal_clicked = false;
        if let Some(portal) = &mut self.portal {
            portal.age += dt;
            // Gentle rotation animation
            if portal.obj_idx < renderer.objects.len() {
                let rot = glam::Mat4::from_rotation_y(portal.age * 1.5);
                renderer.objects[portal.obj_idx].model_matrix =
                    glam::Mat4::from_translation(portal.position) * rot;
            }
            // Click-to-enter: player must click near portal and be within range
            if !loot_clicked && input.left_clicked() {
                if let Some(cursor_pos) = Self::cursor_world_pos(input, renderer, 0.0) {
                    let cursor_dist = Vec3::new(
                        cursor_pos.x - portal.position.x, 0.0, cursor_pos.z - portal.position.z
                    ).length();
                    if cursor_dist < 2.0 {
                        // Also check player is close enough
                        if let Some(pp) = self.world.query::<(&Transform, &Player)>().iter()
                            .map(|(_, (t, _))| t.position).next()
                        {
                            let player_dist = Vec3::new(
                                pp.x - portal.position.x, 0.0, pp.z - portal.position.z
                            ).length();
                            if player_dist < 4.0 {
                                self.needs_new_floor = true;
                                portal_clicked = true;
                                log::info!("Entered portal! Floor transition. Time: {:.1}s", self.rift.timer);
                            }
                        }
                    }
                }
            }
        }

        // Ability-based combat (only fires if click was not consumed by loot or portal)
        self.player_state.abilities.tick_all(dt);
        if !loot_clicked && !portal_clicked {
            self.tick_combat(input, renderer, dt);
        }
        // Always tick projectiles in flight
        let hits = self.projectile_mgr.tick(&mut self.world, renderer, dt);
        for (pos, damage) in hits {
            self.combat_text.spawn_damage(pos, damage, false);
        }

        // Tick AoE zones (Rain of Arrows damage over time)
        let aoe_hits = self.projectile_mgr.tick_aoe(&mut self.world, dt);
        for (pos, damage) in aoe_hits {
            self.combat_text.spawn_damage(pos, damage, false);
        }

        // Drain AI pending actions (ranged shots, slams, leaps) and tick them.
        self.enemy_attacks.drain_pending(&mut self.world, renderer);
        let player_hits = self.enemy_attacks.tick(&mut self.world, renderer, dt);
        for (pos, damage) in player_hits {
            self.combat_text.spawn_player_damage(pos, damage);
        }

        // Tick combat text
        self.combat_text.tick(dt);

        // Despawn dead entities
        let kills = despawn_system(&mut self.world, renderer);

        // Process kills
        for kill in &kills {
            self.rift.progress += kill.progress_value;

            // Death explosion particles
            let death_color = if kill.is_boss {
                [1.0, 0.3, 0.0]
            } else if kill.is_elite {
                [0.9, 0.7, 0.1]
            } else {
                [0.8, 0.2, 0.2]
            };
            let burst_pos = kill.position + Vec3::new(0.0, 0.5, 0.0);
            let emitter = rift_engine::Emitter::new(burst_pos, EmitterConfig::death_burst(death_color));
            renderer.particle_system.add_emitter(emitter);

            // Blood splatter decals on floor/walls
            self.decals.spawn_blood(kill.position, &self.wall_aabbs, renderer);

            // Grant XP
            let rewards = self.player_state.grant_kill_xp(self.rift.floor);
            for reward in &rewards {
                log::info!(
                    ">>> LEVEL UP! Now level {} | +{} attr pts, +{} talent pts <<<",
                    reward.new_level, reward.attribute_points, reward.talent_points
                );
            }

            // Spawn loot
            self.loot_mgr.spawn_drops(self.rift.floor, kill.position, kill.is_boss, kill.is_elite);

            if kill.is_boss {
                self.rift.boss_killed = true;
                self.rift.floor_complete = true;

                // Spawn exit portal where boss died
                let portal_pos = kill.position;
                let portal_mesh = rift_engine::Mesh::portal();
                if renderer.add_mesh(&portal_mesh, glam::Mat4::from_translation(portal_pos)).is_ok() {
                    let obj_idx = renderer.objects.len() - 1;
                    let emitter = rift_engine::Emitter::new(
                        portal_pos + Vec3::new(0.0, 0.9, 0.0),
                        EmitterConfig::portal_vortex(),
                    );
                    let emitter_idx = renderer.particle_system.add_emitter(emitter);
                    self.portal = Some(Portal { position: portal_pos, obj_idx, emitter_idx, age: 0.0 });
                }
                log::info!("=== RIFT LEVEL {} COMPLETE! Portal opened. ===", self.rift.floor);
            }
        }

        // Boss spawn check
        if self.rift.progress_percent() >= 100.0 && !self.rift.boss_spawned {
            self.rift.boss_spawned = true;
            self.floor_mgr.spawn_boss(&mut self.world, renderer, &self.rift);
        }

        // Loot physics + rendering
        self.loot_mgr.tick(renderer, dt);

        // Populate point lights from grounded loot + portal
        renderer.point_lights.clear();
        if let Some(portal) = &self.portal {
            renderer.point_lights.push(PointLight {
                position: portal.position + Vec3::new(0.0, 1.0, 0.0),
                color: Vec3::new(0.3, 0.6, 1.0),
                radius: 8.0,
                intensity: 2.5,
            });
        }
        for drop in &self.loot_mgr.ground_loot {
            if drop.grounded && renderer.point_lights.len() < 8 {
                let c = drop.item.rarity.color();
                renderer.point_lights.push(PointLight {
                    position: drop.position + Vec3::new(0.0, 0.8, 0.0),
                    color: Vec3::new(c[0], c[1], c[2]),
                    radius: 5.0,
                    intensity: 1.5,
                });
            }
        }

        // HP regen
        if stats.hp_regen > 0.0 {
            for (_id, (health, _player)) in self.world.query_mut::<(&mut Health, &Player)>() {
                health.current =
                    (health.current + stats.hp_regen * dt).min(health.max + stats.max_hp_bonus);
            }
        }

        // Render sync
        render_sync_system(&self.world, renderer);

        // Pick the right animation clip for each animated entity based on
        // gameplay state (idle vs. moving), then advance + skin.
        locomotion_anim_system(&mut self.world);

        // Spell-cast state machine: advances the upper-body cast layer
        // (Enter → Shoot → Exit) and emits a fire event the moment we
        // enter the Shoot phase, so the projectile leaves the hand in
        // sync with the wind-up animation rather than at click time.
        let cast_fires = cast_advance_system(&mut self.world, dt);
        for (entity, aim_dir, damage) in cast_fires {
            // Snapshot transform + ability for this caster.
            let (origin, ability) = {
                let pos = self.world.get::<&Transform>(entity)
                    .map(|t| t.position).ok();
                let ab = self.world.get::<&mut rift_engine::ecs::components::SpellCast>(entity)
                    .ok().and_then(|mut c| c.pending_ability.take());
                match (pos, ab) {
                    (Some(p), Some(a)) => (p, a),
                    _ => continue,
                }
            };
            self.projectile_mgr.fire_ability(
                &ability,
                origin,
                aim_dir,
                damage,
                &self.player_state,
                &mut self.world,
                renderer,
            );
        }

        // Skeletal animation: advance animators and CPU-skin meshes into the
        // renderer's per-frame dynamic vertex buffers. Must run after
        // prepare_frame (engine ensures that) and before draw_frame.
        skinning_system(&mut self.world, renderer, dt);

        // Sync equipment visuals to player position
        let player_pos = self.world
            .query::<(&Transform, &Player)>()
            .iter()
            .map(|(_, (t, _))| t.position)
            .next()
            .unwrap_or(Vec3::ZERO);
        self.equip_visuals.sync(&self.equipment, player_pos, renderer);

        // Aim direction from cursor — drives upper-body torso twist (in
        // the skinning system) without disturbing locomotion or movement
        // input direction. The body keeps facing where it's moving;
        // the spine rotates up to ~120° to point at the cursor.
        let arm_aim = Self::cursor_aim_dir(input, renderer, player_pos);
        if let Some(player_id) = self.player_id() {
            if let Ok(mut p) = self.world.get::<&mut rift_engine::ecs::components::Player>(player_id) {
                p.aim_dir = arm_aim;
            }
        }

        camera_follow_system(&self.world, renderer, input, &self.wall_aabbs);
        renderer.particle_system.tick(dt);

        // HUD
        renderer.overlay_batch.clear();
        let (sw, sh) = renderer.screen_size();
        hud::render_hud(
            &mut renderer.overlay_batch,
            &self.world,
            &self.rift,
            &self.player_state,
            &self.equipment,
            sw,
            sh,
            stats.max_hp_bonus,
        );
        hud::render_ability_bar(
            &mut renderer.overlay_batch,
            &self.player_state.abilities,
            input.mouse_pos(),
            sw,
            sh,
        );
        hud::render_enemy_health_bars(
            &mut renderer.overlay_batch,
            &self.world,
            renderer.camera.view_projection(),
            sw,
            sh,
        );
        // Floating damage numbers
        self.combat_text.render(
            &mut renderer.overlay_batch,
            renderer.camera.view_projection(),
            sw,
            sh,
        );
        self.inventory_ui.render(
            &mut renderer.overlay_batch,
            &self.inventory,
            &self.equipment,
            input,
            sw,
            sh,
        );
    }

    fn tick_combat(&mut self, input: &Input, renderer: &mut Renderer, _dt: f32) {
        use glam::Mat4;
        use winit::keyboard::KeyCode;

        let player_data: Option<(Vec3, glam::Quat)> = self
            .world
            .query::<(&Transform, &Player)>()
            .iter()
            .map(|(_, (t, _))| (t.position, t.rotation))
            .next();

        let Some((player_pos, _player_rot)) = player_data else {
            return;
        };

        // Compute aim direction from cursor → ground plane
        let aim_dir = Self::cursor_aim_dir(input, renderer, player_pos);

        // ─── Placed ability targeting mode ─────────────────────────────────
        if self.targeting.is_some() {
            // Update indicator position to follow cursor on ground plane
            if let Some(cursor_pos) = Self::cursor_world_pos(input, renderer, 0.0) {
                let targeting = self.targeting.as_ref().unwrap();
                let radius = targeting.radius;
                if let Some(obj_idx) = targeting.indicator_obj {
                    if obj_idx < renderer.objects.len() {
                        renderer.objects[obj_idx].model_matrix =
                            Mat4::from_translation(cursor_pos)
                                * Mat4::from_scale(Vec3::splat(radius));
                    }
                }
            }

            // Left-click: confirm placement
            if input.left_clicked() {
                if let Some(cursor_pos) = Self::cursor_world_pos(input, renderer, 0.0) {
                    let targeting = self.targeting.take().unwrap();
                    // Remove indicator mesh
                    if let Some(obj_idx) = targeting.indicator_obj {
                        if obj_idx < renderer.objects.len() {
                            renderer.objects[obj_idx].model_matrix = Mat4::ZERO;
                        }
                    }
                    // Fire the placed ability at the confirmed location
                    let aim_to_target = (cursor_pos - player_pos).normalize_or_zero();
                    self.projectile_mgr.fire_ability_at(
                        &targeting.ability,
                        player_pos,
                        aim_to_target,
                        cursor_pos,
                        targeting.damage,
                        &self.player_state,
                        &mut self.world,
                        renderer,
                    );
                }
                return;
            }

            // Right-click or Escape: cancel targeting
            if input.right_clicked() || input.key_just_pressed(KeyCode::Escape) {
                let targeting = self.targeting.take().unwrap();
                if let Some(obj_idx) = targeting.indicator_obj {
                    if obj_idx < renderer.objects.len() {
                        renderer.objects[obj_idx].model_matrix = Mat4::ZERO;
                    }
                }
                // Refund cooldown since ability wasn't used
                if let Some(state) = &mut self.player_state.abilities.slots[targeting.slot_index] {
                    state.cooldown_remaining = 0.0;
                }
                return;
            }

            return; // Consume all input while targeting
        }

        // ─── Normal ability keybinds ──────────────────────────────────────
        let ability_inputs = [
            input.left_clicked(),
            input.key_just_pressed(KeyCode::Digit1),
            input.key_just_pressed(KeyCode::Digit2),
            input.key_just_pressed(KeyCode::Digit3),
            input.key_just_pressed(KeyCode::Digit4),
            input.key_just_pressed(KeyCode::Digit5),
        ];

        let stats = self.equipment.total_stats();
        for (i, &pressed) in ability_inputs.iter().enumerate() {
            if !pressed {
                continue;
            }
            if let Some(ability) = self.player_state.abilities.try_use(i) {
                let ability_clone = ability.clone();
                let damage =
                    self.player_state.compute_attack_damage(&stats) * ability_clone.damage_mult;

                // Check if this is a placed ability → enter targeting mode
                if let TargetingMode::Placed { radius } = ability_clone.targeting {
                    // Spawn the ground indicator circle at cursor position (bright blue ring)
                    let indicator_mesh = rift_engine::Mesh::targeting_circle([0.2, 0.5, 1.0]);
                    let initial_pos = Self::cursor_world_pos(input, renderer, 0.0)
                        .unwrap_or(player_pos);
                    let initial_mat = Mat4::from_translation(initial_pos)
                        * Mat4::from_scale(Vec3::splat(radius));
                    let indicator_obj = if let Ok(()) = renderer.add_mesh(&indicator_mesh, initial_mat) {
                        Some(renderer.objects.len() - 1)
                    } else {
                        None
                    };

                    log::info!("Placed ability targeting: radius={}, pos={:?}, obj={:?}",
                        radius, initial_pos, indicator_obj);

                    self.targeting = Some(PlacedTargeting {
                        slot_index: i,
                        ability: ability_clone,
                        damage,
                        radius,
                        indicator_obj,
                    });
                    break;
                }

                // Slot 0 (left-click primary) routes through the spell-cast
                // state machine so the upper-body cast animation plays.
                // The actual projectile spawn is deferred until the Shoot
                // phase fires its event in `cast_advance_system`. Other
                // slots fire instantly.
                if i == 0 {
                    if let Some(player_id) = self.player_id() {
                        if let Ok(mut cast) = self.world.get::<&mut rift_engine::ecs::components::SpellCast>(player_id) {
                            cast.begin(ability_clone, aim_dir, damage);
                            continue;
                        }
                    }
                }

                self.projectile_mgr.fire_ability(
                    &ability_clone,
                    player_pos,
                    aim_dir,
                    damage,
                    &self.player_state,
                    &mut self.world,
                    renderer,
                );
            }
        }
    }

    /// Find the player's entity id (first entity with a `Player` component).
    fn player_id(&self) -> Option<hecs::Entity> {
        self.world
            .query::<&Player>()
            .iter()
            .map(|(e, _)| e)
            .next()
    }

    /// Compute the world position where the cursor ray hits a ground plane at the given Y.
    fn cursor_world_pos(input: &Input, renderer: &Renderer, ground_y: f32) -> Option<Vec3> {
        let (mx, my) = input.mouse_pos();
        let [w, h] = renderer.window_extent();
        if w == 0 || h == 0 {
            return None;
        }

        let ndc_x = (mx / w as f32) * 2.0 - 1.0;
        let ndc_y = (my / h as f32) * 2.0 - 1.0;

        let inv_vp = (renderer.camera.projection_matrix() * renderer.camera.view_matrix()).inverse();
        let near_point = inv_vp.project_point3(glam::Vec3::new(ndc_x, ndc_y, 0.0));
        let far_point = inv_vp.project_point3(glam::Vec3::new(ndc_x, ndc_y, 1.0));
        let ray_dir = (far_point - near_point).normalize();

        if ray_dir.y.abs() < 1e-6 {
            return None;
        }
        let t = (ground_y - near_point.y) / ray_dir.y;
        Some(near_point + ray_dir * t)
    }

    /// Compute a horizontal aim direction from the cursor position to the ground plane.
    fn cursor_aim_dir(input: &Input, renderer: &Renderer, player_pos: Vec3) -> Vec3 {
        if let Some(hit) = Self::cursor_world_pos(input, renderer, player_pos.y) {
            let delta = hit - player_pos;
            let flat = Vec3::new(delta.x, 0.0, delta.z);
            if flat.length_squared() > 0.01 {
                return flat.normalize();
            }
        }
        Vec3::NEG_Z
    }
}
