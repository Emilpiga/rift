pub mod rift_state;
pub mod player;
pub mod loot_manager;
pub mod floor;
pub mod hud;
pub mod equipment_visuals;
pub mod enemy_attacks;
pub mod monsters;
pub mod props;
pub mod environment;
pub mod abilities;
pub mod talents;
pub mod classes;
pub mod character;
pub mod character_select;


use glam::Vec3;
use rift_engine::ai;
use rift_engine::combat::ability::TargetingMode;
use rift_engine::ecs::components::{Health, Player, Transform};
use rift_engine::ui::CombatTextSystem;
use rift_engine::renderer::decals::DecalSystem;
use equipment_visuals::EquipmentVisuals;
use rift_engine::ecs::systems::{
    camera_follow_system, cast_advance_system, collision_system, contact_damage_system, despawn_system,
    enemy_anim_system, movement_system, player_action_post_system, player_action_pre_system, player_input_system,
    render_sync_system, skinning_system, locomotion_anim_system, PlayerActionConfig,
};
use rift_engine::loot::{Equipment, Inventory};
use rift_engine::ui::InventoryUI;
use rift_engine::{EmitterConfig, Input, LoadStatus, PointLight, Renderer};

use floor::FloorManager;
use loot_manager::LootManager;
use player::PlayerState;
use rift_engine::combat::ProjectilePool;
use rift_state::RiftState;
use enemy_attacks::EnemyAttackSystem;

/// Top-level game state — the single struct that orchestrates all gameplay.
pub struct GameState {
    pub world: hecs::World,
    pub rift: RiftState,
    pub player_state: PlayerState,
    pub floor_mgr: FloorManager,
    pub loot_mgr: LootManager,
    pub projectile_mgr: ProjectilePool,
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
    /// Where we are in the per-frame staged init. See `load_step`.
    load_phase: LoadPhase,
    /// Index into `monsters::ALL_ROLES` of the next role to load during
    /// the `LoadPhase::Monsters` stage.
    monster_load_index: usize,
    /// Eases from 1 -> 0 over ~0.5 s after the player takes damage.
    /// Drives a red screen-edge vignette so hits read clearly without
    /// blocking the action.
    damage_flash: f32,
    /// `true` once we've triggered the player's death animation, so we
    /// don't keep re-triggering it every frame they sit at 0 HP.
    player_dying: bool,
    /// True while the player is in the safe hub zone. Suppresses rift
    /// progress UI, boss spawn, and uses the hub portal interaction.
    in_hub: bool,
    /// Portal that returns the player from the hub into the rift loop
    /// (floor 1).  Only present while `in_hub` is true.
    hub_portal: Option<HubPortal>,
    /// Pending floor transition request resolved at the top of `update`.
    pending_transition: Option<Transition>,
    /// Active death→hub fade-to-black sequence.
    death_fade: Option<DeathFade>,
    /// Top-level state (character-select vs playing).
    app_state: AppState,
    /// Owns the character-select screen UI + preview avatar.
    character_select: character_select::CharacterSelect,
}

/// What the next floor regeneration should produce.
#[derive(Clone, Copy, Debug)]
enum Transition {
    /// Currently in the rift, advance to the next dungeon floor.
    AdvanceRift,
    /// Spawn / respawn the safe hub zone.
    ToHub,
    /// Leave the hub and start a fresh rift run at floor 1.
    StartRift,
}

/// Returning-from-rift portal placed in the hub.  Click + walk-near to
/// confirm the press-F prompt and start a new rift run.
struct HubPortal {
    position: Vec3,
    obj_idx: usize,
    emitter_idx: usize,
    age: f32,
}

/// Two-stage fade triggered when the player dies: hold while the death
/// animation plays, then fade to black, swap to the hub, and fade back
/// in.  See `update_death_fade` for the state machine.
struct DeathFade {
    phase: FadePhase,
    /// Time spent in the current phase.
    t: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FadePhase {
    /// Death animation is playing; alpha = 0.
    Hold,
    /// Screen ramps from clear to black.
    Out,
    /// Hub has been generated underneath; ramps back to clear.
    In,
}

/// Stages of `GameState::load_step`. Each frame the engine calls
/// `load_step` once; we run one stage's worth of work and advance.
/// Stages of `GameState::load_step`. Each frame the engine calls
/// `load_step` once; we run one stage's worth of work and advance.
///
/// The initial loader only prepares assets that the character-select
/// screen needs (monster glTFs, projectile / enemy-attack pools).
/// Floor + outfits + walls happen later, once the player has picked a
/// character and we know the gender/class to spawn.
enum LoadPhase {
    /// Pre-load skinned monster glTFs (one role per call). Runs before
    /// floor generation so spawn can attach skinned components.
    Monsters,
    /// Set up the projectile object pool.
    Projectiles,
    /// Set up the enemy attack object pool.
    EnemyAttacks,
    /// Loading complete; subsequent calls return `Done` immediately.
    Done,
}

/// Top-level app state. Drives whether `update` runs the in-game loop
/// or the character-select screen.
#[derive(Clone, Debug, PartialEq, Eq)]
enum AppState {
    /// Showing the roster / create / delete screen.
    CharacterSelect,
    /// User picked Play. Run the heavy world setup (floor, outfits,
    /// walls) one chunk per frame so the screen stays responsive and
    /// shows a progress bar.
    EnteringWorld(EnterPhase),
    /// Player is in-game (hub or rift).
    Playing,
}

/// One step of the character-select → in-game transition. Each variant
/// runs in a single frame; the next frame advances the phase and
/// renders an updated progress bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EnterPhase {
    /// Tear down the preview avatar's render slot, switch camera back
    /// to gameplay defaults.
    PrepareScene,
    /// `floor_mgr.generate_hub` + spawn hub portal. One big chunk; same
    /// cost as a hub respawn.
    GenerateHub,
    /// `attach_outfit_pieces` (no I/O, just creates the component).
    AttachOutfits,
    /// Stream outfit pieces in one at a time, like the initial loader.
    LoadOutfits,
    /// Compute wall collision caches.
    RebuildWalls,
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
            player_state: PlayerState::new(classes::HUNTER),
            floor_mgr: FloorManager::new(),
            loot_mgr: LootManager::new(),
            projectile_mgr: ProjectilePool::new(),
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
            load_phase: LoadPhase::Monsters,
            monster_load_index: 0,
            damage_flash: 0.0,
            player_dying: false,
            in_hub: true,
            hub_portal: None,
            pending_transition: None,
            death_fade: None,
            app_state: AppState::CharacterSelect,
            character_select: character_select::CharacterSelect::new(),
        }
    }

    /// Drive one stage of staged initialization. Called every frame by
    /// the engine while a loading screen is shown; once we return
    /// `LoadStatus::Done`, the engine begins the normal `update` loop.
    ///
    /// The initial loader only prepares assets that work without a
    /// player entity. Floor / outfit / wall init runs synchronously
    /// when the player picks a character (see [`enter_world`]).
    pub fn load_step(&mut self, renderer: &mut Renderer) -> anyhow::Result<LoadStatus> {
        let monster_total = monsters::ALL_ROLES.len();
        // 3 stages after monsters: projectiles, enemy_attacks, done.
        let total_steps = (monster_total + 2) as f32;

        let monster_done = self.monster_load_index;
        let done_before = match self.load_phase {
            LoadPhase::Monsters => monster_done,
            LoadPhase::Projectiles => monster_total,
            LoadPhase::EnemyAttacks => monster_total + 1,
            LoadPhase::Done => return Ok(LoadStatus::Done),
        };

        let label = match self.load_phase {
            LoadPhase::Monsters => {
                let role = monsters::ALL_ROLES[self.monster_load_index];
                let asset = monsters::load_role(role);
                *self.floor_mgr.monsters.slot_mut(role) = asset;
                self.monster_load_index += 1;
                if self.monster_load_index >= monsters::ALL_ROLES.len() {
                    self.load_phase = LoadPhase::Projectiles;
                }
                format!("Loading monster: {:?}", role)
            }
            LoadPhase::Projectiles => {
                self.projectile_mgr.init_pool(renderer);
                self.load_phase = LoadPhase::EnemyAttacks;
                "Preparing projectiles…".to_string()
            }
            LoadPhase::EnemyAttacks => {
                self.enemy_attacks.init_pool(renderer);
                self.load_phase = LoadPhase::Done;
                "Preparing enemy attacks…".to_string()
            }
            LoadPhase::Done => return Ok(LoadStatus::Done),
        };

        let done_after = (done_before + 1) as f32;
        let progress = (done_after / total_steps).min(1.0);

        if matches!(self.load_phase, LoadPhase::Done) {
            Ok(LoadStatus::Done)
        } else {
            Ok(LoadStatus::Loading { progress, label })
        }
    }

    /// Build the modular outfit attachments and insert them onto the
    /// player entity. Must run after the player's `Skinned` component
    /// exists, since we read its skeleton's joint table.
    fn attach_outfit_pieces(&mut self, renderer: &mut Renderer) {
        let Some(player_id) = self.player_id() else { return };
        let host_table = match self.world.get::<&rift_engine::ecs::components::Skinned>(player_id) {
            Ok(s) => s.mesh.joint_index_by_name.clone(),
            Err(_) => return,
        };
        self.equip_visuals.clear();
        let atts = self.equip_visuals.build_attachments(renderer, &host_table);
        self.world.insert_one(player_id, atts).ok();
    }

    /// Synchronously stream every outfit piece into the player's
    /// `SkinnedAttachments`.  Used on floor transitions / deaths /
    /// rift entry, since the staged loader only runs once at startup.
    fn load_all_outfit_pieces(&mut self, renderer: &mut Renderer) {
        let Some(player_id) = self.player_id() else { return };
        if let Ok(mut atts) = self.world.get::<&mut rift_engine::ecs::components::SkinnedAttachments>(player_id) {
            // step_load returns Some until everything's loaded; loop until None.
            while self.equip_visuals.step_load(renderer, &mut atts).is_some() {}
        }
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

    /// Spawn a glowing rift entry portal in the hub at `pos`.
    fn spawn_hub_portal(&mut self, renderer: &mut Renderer, pos: Vec3) {
        let portal_mesh = rift_engine::Mesh::portal();
        if renderer.add_mesh(&portal_mesh, glam::Mat4::from_translation(pos)).is_ok() {
            let obj_idx = renderer.objects.len() - 1;
            let emitter = rift_engine::Emitter::new(
                pos + Vec3::new(0.0, 0.9, 0.0),
                EmitterConfig::portal_vortex(),
            );
            let emitter_idx = renderer.particle_system.add_emitter(emitter);
            self.hub_portal = Some(HubPortal {
                position: pos,
                obj_idx,
                emitter_idx,
                age: 0.0,
            });
        }
    }

    /// Tear down per-floor transient state (loot, projectiles, particles)
    /// shared by every floor regeneration path.
    fn reset_for_regeneration(&mut self, renderer: &mut Renderer) {
        self.player_dying = false;
        self.damage_flash = 0.0;
        self.portal = None;
        self.hub_portal = None;
        self.targeting = None;
        self.decals.clear();
        self.loot_mgr.clear();
        self.projectile_mgr.clear(renderer);
        self.enemy_attacks.clear(renderer);
        renderer.particle_system.clear_emitters();
    }

    /// Carry out a queued transition: regenerate the world for hub /
    /// rift advance / start-of-rift, then re-init pools and caches.
    fn perform_transition(&mut self, renderer: &mut Renderer, transition: Transition) {
        self.reset_for_regeneration(renderer);
        let result = match transition {
            Transition::AdvanceRift => {
                self.in_hub = false;
                self.rift = RiftState::new(self.rift.floor + 1);
                self.floor_mgr
                    .generate(&mut self.world, renderer, &self.rift, &self.player_state)
                    .map(|_| ())
            }
            Transition::StartRift => {
                self.in_hub = false;
                self.rift = RiftState::new(1);
                self.floor_mgr
                    .generate(&mut self.world, renderer, &self.rift, &self.player_state)
                    .map(|_| ())
            }
            Transition::ToHub => {
                self.in_hub = true;
                self.rift = RiftState::new(1);
                match self.floor_mgr.generate_hub(
                    &mut self.world,
                    renderer,
                    &self.player_state,
                ) {
                    Ok(portal_pos) => {
                        self.spawn_hub_portal(renderer, portal_pos);
                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            }
        };
        if let Err(e) = result {
            log::error!("Floor regeneration failed: {}", e);
        }
        self.projectile_mgr.init_pool(renderer);
        self.attach_outfit_pieces(renderer);
        self.load_all_outfit_pieces(renderer);
        self.enemy_attacks.init_pool(renderer);
        self.rebuild_wall_caches();
    }

    /// Tick the death\u2192hub fade. Returns the current black-screen alpha
    /// (0\u20131) so the HUD pass can blit a fullscreen quad on top.
    fn advance_death_fade(&mut self, renderer: &mut Renderer, dt: f32) {
        const HOLD_SECS: f32 = 2.4;     // length of death anim hold
        const OUT_SECS: f32 = 0.55;     // fade-out duration
        const IN_SECS: f32 = 0.55;      // fade-in duration

        let Some(fade) = self.death_fade.as_mut() else { return };
        fade.t += dt;
        match fade.phase {
            FadePhase::Hold => {
                if fade.t >= HOLD_SECS {
                    fade.phase = FadePhase::Out;
                    fade.t = 0.0;
                }
            }
            FadePhase::Out => {
                if fade.t >= OUT_SECS {
                    // Screen is fully black; regenerate the hub now and
                    // restore full HP for the next run.
                    self.pending_transition = Some(Transition::ToHub);
                    if let Some(player_id) = self.player_id() {
                        if let Ok(mut h) = self.world.get::<&mut Health>(player_id) {
                            h.current = h.max;
                        }
                    }
                    if let Some(fade) = self.death_fade.as_mut() {
                        fade.phase = FadePhase::In;
                        fade.t = 0.0;
                    }
                }
            }
            FadePhase::In => {
                if fade.t >= IN_SECS {
                    self.death_fade = None;
                }
            }
        }
        let _ = renderer;
    }

    /// Tick the hub return portal (rotation + press-F interaction).
    /// Returns true when the player is standing close enough to it for
    /// the prompt to be shown.  When F is pressed inside that range we
    /// queue a `StartRift` transition.
    fn tick_hub_portal(&mut self, renderer: &mut Renderer, input: &Input, dt: f32) -> bool {
        use winit::keyboard::KeyCode;
        let Some(portal) = self.hub_portal.as_mut() else { return false };
        portal.age += dt;
        if portal.obj_idx < renderer.objects.len() {
            let rot = glam::Mat4::from_rotation_y(portal.age * 1.5);
            renderer.objects[portal.obj_idx].model_matrix =
                glam::Mat4::from_translation(portal.position) * rot;
        }
        let portal_pos = portal.position;
        let player_pos: Option<Vec3> = self
            .world
            .query::<(&Transform, &Player)>()
            .iter()
            .map(|(_, (t, _))| t.position)
            .next();
        let Some(pp) = player_pos else { return false };
        let dist = Vec3::new(pp.x - portal_pos.x, 0.0, pp.z - portal_pos.z).length();
        let in_range = dist < 2.6;
        if in_range && input.key_just_pressed(KeyCode::KeyF) {
            log::info!("Entering rift from hub.");
            self.pending_transition = Some(Transition::StartRift);
        }
        in_range
    }

    /// Current fade alpha [0,1] for the death-to-hub transition.
    fn death_fade_alpha(&self) -> f32 {
        let Some(fade) = self.death_fade.as_ref() else { return 0.0 };
        const OUT_SECS: f32 = 0.55;
        const IN_SECS: f32 = 0.55;
        match fade.phase {
            FadePhase::Hold => 0.0,
            FadePhase::Out => (fade.t / OUT_SECS).clamp(0.0, 1.0),
            FadePhase::In => (1.0 - fade.t / IN_SECS).clamp(0.0, 1.0),
        }
    }

    /// Free GPU resources owned outside the renderer (shared monster
    /// + outfit textures).  Called once before the renderer drops so
    /// validation doesn't flag leaked images at `vkDestroyDevice`.
    pub fn shutdown(&mut self, renderer: &mut Renderer) {
        unsafe { renderer.ash_device().device_wait_idle().ok(); }
        let device = renderer.ash_device().clone();
        let allocator = renderer.allocator_arc();
        self.equip_visuals.cleanup_gpu(&device, &allocator);
        self.floor_mgr.monsters.cleanup_gpu(&device, &allocator);
        self.floor_mgr.props.cleanup_gpu(&device, &allocator);
        self.floor_mgr.env.cleanup_gpu(&device, &allocator);
    }

    pub fn update(&mut self, renderer: &mut Renderer, input: &Input, dt: f32) {
        // Top-level state gate. While the character-select screen is
        // up, we run only the bare minimum: animator advancing for the
        // preview avatar and the screen's own input handling.
        match self.app_state.clone() {
            AppState::CharacterSelect => {
                self.update_character_select(renderer, input, dt);
                return;
            }
            AppState::EnteringWorld(phase) => {
                self.tick_entering_world(renderer, phase);
                return;
            }
            AppState::Playing => {}
        }

        // Floor transition (rift advance / hub <-> rift / death respawn).
        if let Some(transition) = self.pending_transition.take() {
            self.perform_transition(renderer, transition);
            return;
        }
        if self.needs_new_floor {
            self.needs_new_floor = false;
            self.perform_transition(renderer, Transition::AdvanceRift);
            return;
        }

        // Death\u2192hub fade: hold during anim, fade out, swap world, fade in.
        if self.death_fade.is_some() {
            self.advance_death_fade(renderer, dt);
        }

        self.rift.timer += if self.in_hub { 0.0 } else { dt };
        let stats = self.equipment.total_stats();

        // ECS systems
        let action_cfg = PlayerActionConfig::default();
        let accept_input = !(self.player_dying || self.death_fade.is_some());
        player_action_pre_system(&mut self.world, input, dt, &action_cfg, accept_input);
        player_input_system(&mut self.world, input, dt);
        ai::ai_system(&mut self.world, dt, &self.floor_mgr.nav_grid, &self.wall_aabbs);
        movement_system(&mut self.world, dt);
        player_action_post_system(&mut self.world, &action_cfg);
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
            self.trigger_hit_reaction(player_dmg_taken);
        }
        // Crossed the death threshold this frame? Play the full-body
        // death animation once and lock it.  (Note: most player damage
        // arrives later in the frame via projectiles / enemy_attacks /
        // AoE zones, so the canonical check happens at the end of
        // `update` — this branch just covers contact damage.)
        if hp_before > 0.0 && hp_after <= 0.0 && !self.player_dying {
            self.trigger_player_death();
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
                        // Play the upper-body pickup animation so the
                        // gesture reads on the character without
                        // interrupting movement.
                        if let Some(player_id) = self.player_id() {
                            let clip = self.world
                                .get::<&rift_engine::ecs::components::AnimationSet>(player_id)
                                .ok()
                                .and_then(|s| s.find_any(&[
                                    "PickUp_Table", "PickUp", "Pick_Up", "Pickup",
                                ]));
                            if let Some(clip) = clip {
                                if let Ok(mut cast) = self.world
                                    .get::<&mut rift_engine::ecs::components::SpellCast>(player_id)
                                {
                                    cast.play_oneshot(clip);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Boss-room exit portal: press F while near it to advance to
        // the next rift floor.  Same interaction model as the hub.
        let mut near_exit_portal = false;
        if let Some(portal) = &mut self.portal {
            use winit::keyboard::KeyCode;
            portal.age += dt;
            if portal.obj_idx < renderer.objects.len() {
                let rot = glam::Mat4::from_rotation_y(portal.age * 1.5);
                renderer.objects[portal.obj_idx].model_matrix =
                    glam::Mat4::from_translation(portal.position) * rot;
            }
            let portal_pos = portal.position;
            if let Some(pp) = self.world.query::<(&Transform, &Player)>().iter()
                .map(|(_, (t, _))| t.position).next()
            {
                let dist = Vec3::new(pp.x - portal_pos.x, 0.0, pp.z - portal_pos.z).length();
                near_exit_portal = dist < 2.6;
                if near_exit_portal && input.key_just_pressed(KeyCode::KeyF) {
                    self.needs_new_floor = true;
                    log::info!(
                        "Descended to next floor. Run time: {:.1}s",
                        self.rift.timer,
                    );
                }
            }
        }

        // Hub portal: press F when standing near it to start a fresh
        // rift run (floor 1).  We expose `near_hub_portal` so the HUD
        // can paint the prompt.
        let near_hub_portal = self.tick_hub_portal(renderer, input, dt);

        // Ability-based combat (only fires if click was not consumed by loot or portal)
        self.player_state.abilities.tick_all(dt);
        if !loot_clicked && !self.player_dying && !self.in_hub {
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

        // Tick active debuffs (poison/burn/mark/slow): drives DoT
        // damage and follows each enemy with its visual aura emitter.
        let debuff_hits = rift_engine::combat::debuff_tick_system(
            &mut self.world,
            renderer,
            dt,
        );
        for (pos, damage) in debuff_hits {
            self.combat_text.spawn_damage(pos, damage, false);
        }

        // Drain AI pending actions (ranged shots, slams, leaps) and tick them.
        self.enemy_attacks.drain_pending(&mut self.world, renderer);
        let player_hits = self.enemy_attacks.tick(&mut self.world, renderer, dt);
        for (pos, damage) in &player_hits {
            self.combat_text.spawn_player_damage(*pos, *damage);
        }
        if let Some((_, dmg)) = player_hits.iter().max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)) {
            if *dmg > 0.5 && !self.player_dying {
                self.trigger_hit_reaction(*dmg);
            }
        }

        // Catch-all death detection: any damage path (projectiles, AoE,
        // enemy_attacks, contact) eventually winds up here. The first
        // frame that the player's HP actually hits zero we kick the
        // death animation and freeze input.
        if !self.player_dying {
            let dead = self.world.query::<(&Health, &Player)>().iter()
                .any(|(_, (h, _))| h.is_dead());
            if dead {
                self.trigger_player_death();
            }
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
        if !self.in_hub && self.rift.progress_percent() >= 100.0 && !self.rift.boss_spawned {
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
        // Layer monster reaction one-shots (Death / HitRecieve / Bite_Front)
        // on top of locomotion. Must run AFTER `locomotion_anim_system` so
        // it can override the locomotion clip with the reaction.
        enemy_anim_system(&mut self.world, dt);

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
            rift_engine::combat::execute_ability_instant(
                &ability,
                origin,
                aim_dir,
                damage,
                Some(&self.player_state.talents),
                &mut self.projectile_mgr,
                &mut self.world,
                renderer,
            );
        }

        // Sync modular outfit visibility to the equipment state. Must
        // run BEFORE skinning so newly lazy-loaded pieces get skinned on
        // the same frame they become visible (otherwise we'd hide the
        // base body but not yet draw the outfit).
        if let Some(player_id) = self.player_id() {
            if let Ok(mut atts) = self.world.get::<&mut rift_engine::ecs::components::SkinnedAttachments>(player_id) {
                let hide_base = self.equip_visuals.sync(&self.equipment, &mut atts, renderer);
                atts.hide_base = hide_base;
            }
        }

        // Skeletal animation: advance animators and CPU-skin meshes into the
        // renderer's per-frame dynamic vertex buffers. Must run after
        // prepare_frame (engine ensures that) and before draw_frame.
        skinning_system(&mut self.world, renderer, dt);

        // Animate any blood splatter decals that are still spreading.
        self.decals.update(dt, renderer);

        // Equipment visual sync (other gameplay state, like the held
        // weapon's world position) still happens after skinning.
        let player_pos = self.world
            .query::<(&Transform, &Player)>()
            .iter()
            .map(|(_, (t, _))| t.position)
            .next()
            .unwrap_or(Vec3::ZERO);

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

        // Decay damage-flash overlay (≈0.5 s fade).
        if self.damage_flash > 0.0 {
            self.damage_flash = (self.damage_flash - dt * 2.2).max(0.0);
        }

        // HUD
        renderer.overlay_batch.clear();
        let (sw, sh) = renderer.screen_size();
        if self.damage_flash > 0.001 {
            hud::render_damage_flash(&mut renderer.overlay_batch, self.damage_flash, sw, sh);
        }
        if near_hub_portal && !self.player_dying {
            hud::render_portal_prompt(
                &mut renderer.overlay_batch,
                "PRESS [F] TO ENTER THE RIFT",
                sw,
                sh,
            );
        } else if near_exit_portal && !self.player_dying {
            hud::render_portal_prompt(
                &mut renderer.overlay_batch,
                "PRESS [F] TO DESCEND DEEPER",
                sw,
                sh,
            );
        }
        hud::render_hud(
            &mut renderer.overlay_batch,
            &self.world,
            &self.rift,
            &self.player_state,
            &self.equipment,
            sw,
            sh,
            stats.max_hp_bonus,
            self.in_hub,
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
        if !self.in_hub {
            hud::render_boss_arrow(
                &mut renderer.overlay_batch,
                &self.world,
                renderer.camera.view_projection(),
                sw,
                sh,
            );
        }
        // Minimap (top-right). Active portal differs per mode: hub
        // shows the rift-entry portal, dungeon shows the boss exit.
        let portal_pos = if self.in_hub {
            self.hub_portal.as_ref().map(|p| p.position)
        } else {
            self.portal.as_ref().map(|p| p.position)
        };
        let player_facing = self
            .world
            .query::<(&Transform, &Player)>()
            .iter()
            .map(|(_, (t, _))| t.rotation * Vec3::Z)
            .next()
            .unwrap_or(Vec3::Z);
        hud::render_minimap(
            &mut renderer.overlay_batch,
            &self.world,
            &self.floor_mgr.nav_grid,
            player_facing,
            portal_pos,
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
        // Fade-to-black overlay for the death\u2192hub transition is drawn
        // last so it covers every other HUD element.
        let fade_alpha = self.death_fade_alpha();
        if fade_alpha > 0.001 {
            hud::render_fade_to_black(&mut renderer.overlay_batch, fade_alpha, sw, sh);
        }
    }

    /// Tick the character-select screen. Drives only the animator on the
    /// preview avatar plus the screen's UI logic, then renders both.
    fn update_character_select(&mut self, renderer: &mut Renderer, input: &Input, dt: f32) {
        use rift_engine::ecs::systems::skinning_system;
        // Reset the overlay batch each frame; nothing else writes to it
        // while we're on this screen.
        renderer.overlay_batch.clear();
        // Run the screen update — may set `pending_transition` and
        // flip `app_state` if the user clicked Play.
        let action = self
            .character_select
            .update(&mut self.world, renderer, input, dt);

        // Advance the avatar's animator and CPU-skin it so the preview
        // moves. `skinning_system` hits any entity with the right
        // components (no Player needed).
        skinning_system(&mut self.world, renderer, dt);

        // Render the screen overlay last so it sits on top of the 3D
        // preview.
        let (sw, sh) = renderer.screen_size();
        self.character_select.render(&mut renderer.overlay_batch, sw, sh);

        match action {
            character_select::SelectAction::None => {}
            character_select::SelectAction::Play(profile) => {
                self.start_with_profile(profile);
            }
            character_select::SelectAction::Quit => {
                // The engine doesn't expose a programmatic exit hook;
                // for now just log. A clean shutdown will be added when
                // we wire a main-menu button to actually quit.
                log::info!("Quit requested from character select");
            }
        }
    }

    /// Build a `PlayerState` for the chosen profile and kick off the
    /// phased world-entry sequence. The next several frames each run
    /// one chunk of work and render a progress bar; once everything is
    /// loaded we flip to `AppState::Playing`.
    fn start_with_profile(&mut self, profile: character::CharacterProfile) {
        log::info!(
            "Entering world as '{}' ({:?} {:?})",
            profile.name, profile.gender, profile.class,
        );
        self.player_state = player::PlayerState::with_profile(
            profile.class,
            profile.gender,
            profile.name.clone(),
        );
        self.app_state = AppState::EnteringWorld(EnterPhase::PrepareScene);
    }

    /// Run one phase of the character-select → in-game transition and
    /// render a progress bar. Called every frame while `app_state` is
    /// `EnteringWorld(_)`.
    fn tick_entering_world(&mut self, renderer: &mut Renderer, phase: EnterPhase) {
        let (label, next): (&'static str, Option<EnterPhase>) = match phase {
            EnterPhase::PrepareScene => {
                // Drop the preview avatar (and its dynamic mesh slot)
                // before we wipe the world, so we don't leak a
                // hub-stale render object.
                self.character_select.teardown_preview(&mut self.world, renderer);
                renderer.point_lights.clear();
                ("Preparing world…", Some(EnterPhase::GenerateHub))
            }
            EnterPhase::GenerateHub => {
                self.in_hub = true;
                self.rift = RiftState::new(1);
                match self.floor_mgr.generate_hub(
                    &mut self.world,
                    renderer,
                    &self.player_state,
                ) {
                    Ok(portal_pos) => {
                        self.spawn_hub_portal(renderer, portal_pos);
                    }
                    Err(e) => log::error!("Hub generation failed: {}", e),
                }
                ("Generating hub…", Some(EnterPhase::AttachOutfits))
            }
            EnterPhase::AttachOutfits => {
                self.attach_outfit_pieces(renderer);
                ("Preparing outfits…", Some(EnterPhase::LoadOutfits))
            }
            EnterPhase::LoadOutfits => {
                // Stream pieces in batches of 2 per frame so the
                // progress bar moves visibly without dragging the
                // entrance out for too long.
                let player_id = self.player_id();
                let mut still_loading = false;
                if let Some(pid) = player_id {
                    if let Ok(mut atts) = self
                        .world
                        .get::<&mut rift_engine::ecs::components::SkinnedAttachments>(pid)
                    {
                        for _ in 0..2 {
                            if self.equip_visuals.step_load(renderer, &mut atts).is_none() {
                                break;
                            }
                            still_loading = true;
                        }
                        // Probe one more time without consuming the
                        // step to see if there's still work left.
                        if !still_loading
                            && self.equip_visuals.loaded_pieces()
                                < self.equip_visuals.total_pieces()
                        {
                            still_loading = true;
                        }
                    }
                }
                let next = if still_loading {
                    Some(EnterPhase::LoadOutfits)
                } else {
                    Some(EnterPhase::RebuildWalls)
                };
                ("Loading outfits…", next)
            }
            EnterPhase::RebuildWalls => {
                self.rebuild_wall_caches();
                ("Finalizing…", None)
            }
        };

        // Compute progress for the bar. Use phase ordinal + outfit
        // sub-progress for the LoadOutfits stretch, which dominates.
        let progress = compute_enter_progress(phase, &self.equip_visuals);
        draw_world_loading_overlay(renderer, progress, label);

        match next {
            Some(p) => self.app_state = AppState::EnteringWorld(p),
            None => self.app_state = AppState::Playing,
        }
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
                    rift_engine::combat::execute_ability_placed(
                        &targeting.ability,
                        cursor_pos,
                        targeting.damage,
                        Some(&self.player_state.talents),
                        &mut self.projectile_mgr,
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

                rift_engine::combat::execute_ability_instant(
                    &ability_clone,
                    player_pos,
                    aim_dir,
                    damage,
                    Some(&self.player_state.talents),
                    &mut self.projectile_mgr,
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

    /// Trigger an upper-body hit reaction whose clip is chosen based on
    /// the direction of the closest enemy relative to the player's
    /// facing.  Hit_Head occasionally substitutes for variety on big
    /// hits.  Falls back gracefully when an entry is missing from the
    /// animation library.
    fn trigger_hit_reaction(&mut self, damage: f32) {
        use rift_engine::ecs::components::{AnimationSet, Enemy, SpellCast};

        // Subtle red vignette pulse, scaled by damage but capped low
        // so even big hits don't fill the screen.
        let flash_strength = (damage * 0.04 + 0.30).min(0.7);
        if flash_strength > self.damage_flash {
            self.damage_flash = flash_strength;
        }

        let Some(player_id) = self.player_id() else { return };
        let (player_pos, player_fwd) = match self.world.get::<&Transform>(player_id) {
            Ok(t) => (t.position, t.rotation * Vec3::Z),
            Err(_) => return,
        };

        // Find the nearest non-dying enemy as the hit source.
        let mut nearest: Option<(f32, Vec3)> = None;
        for (_id, (t, _e)) in self.world
            .query::<(&Transform, &Enemy)>()
            .without::<&rift_engine::ecs::components::Dying>()
            .iter()
        {
            let to = t.position - player_pos;
            let d2 = to.x * to.x + to.z * to.z;
            if nearest.map_or(true, |(best, _)| d2 < best) {
                nearest = Some((d2, t.position));
            }
        }
        let Some((_, src_pos)) = nearest else { return };

        // Direction from player to attacker, projected on XZ.
        let to_src = Vec3::new(src_pos.x - player_pos.x, 0.0, src_pos.z - player_pos.z);
        if to_src.length_squared() < 1e-4 { return }
        let to_src = to_src.normalize();
        let fwd = Vec3::new(player_fwd.x, 0.0, player_fwd.z).normalize_or_zero();
        if fwd.length_squared() < 1e-4 { return }
        let right = Vec3::new(fwd.z, 0.0, -fwd.x); // 90° clockwise
        // Local-space components: front = dot(fwd), side = dot(right). +side = right of player.
        let front = to_src.dot(fwd);
        let side = to_src.dot(right);
        let angle = side.atan2(front); // 0 = front, +π/2 = right, -π/2 = left, ±π = back

        // Roll a small chance of Hit_Head for variety on bigger blows.
        let rng = (player_pos.x.to_bits() ^ player_pos.z.to_bits() ^ damage.to_bits()) as u32;
        let head_chance = if damage > 12.0 { 5 } else { 9 };
        let prefer_head = (rng % head_chance as u32) == 0;

        let candidates: &[&str] = if prefer_head {
            &["Hit_Head", "Hit_Chest"]
        } else if angle.abs() < std::f32::consts::FRAC_PI_4 {
            // Mostly front: alternate chest/stomach by parity of rng.
            if rng & 1 == 0 {
                &["Hit_Chest", "Hit_Stomach", "Hit_Head"]
            } else {
                &["Hit_Stomach", "Hit_Chest", "Hit_Head"]
            }
        } else if angle > 0.0 && angle < std::f32::consts::PI - 0.4 {
            &["Hit_Shoulder_R", "Hit_Chest", "Hit_Stomach"]
        } else if angle < 0.0 && angle > -(std::f32::consts::PI - 0.4) {
            &["Hit_Shoulder_L", "Hit_Chest", "Hit_Stomach"]
        } else {
            // Behind: no dedicated back animation, fall through to chest.
            &["Hit_Chest", "Hit_Stomach", "Hit_Head"]
        };

        let clip = match self.world.get::<&AnimationSet>(player_id) {
            Ok(set) => set.find_any(candidates),
            Err(_) => None,
        };
        if let Some(clip) = clip {
            if let Ok(mut cast) = self.world.get::<&mut SpellCast>(player_id) {
                cast.play_hit(clip);
            }
        }
    }

    /// Swap the player's base animator into Death01 / Death02 and
    /// freeze locomotion.  `Death01` is the falling-backwards variant
    /// (used when the killing blow lands from in front), `Death02` is
    /// the falling-forwards variant (when hit from behind).  The
    /// animation library may only ship `Death01`, in which case it's
    /// used in both cases via `find_any`.
    fn trigger_player_death(&mut self) {
        use rift_engine::animation::Animator;
        use rift_engine::ecs::components::{
            AnimationSet, Enemy, Player, PlayerAction, SpellCast, Velocity,
        };

        self.player_dying = true;
        self.damage_flash = (self.damage_flash + 0.45).min(0.85);
        // Kick off the fade-to-hub sequence; held while the death anim
        // plays, then the screen fades through black before the hub
        // regenerates underneath.
        if self.death_fade.is_none() {
            self.death_fade = Some(DeathFade { phase: FadePhase::Hold, t: 0.0 });
        }
        log::info!("Player death triggered (rift floor {}).", self.rift.floor);

        let Some(player_id) = self.player_id() else { return };
        let (player_pos, player_fwd) = match self.world.get::<&Transform>(player_id) {
            Ok(t) => (t.position, t.rotation * Vec3::Z),
            Err(_) => return,
        };

        // Find the closest enemy that could be the killing blow source.
        let mut nearest: Option<(f32, Vec3)> = None;
        for (_id, (t, _e)) in self.world.query::<(&Transform, &Enemy)>().iter() {
            let to = t.position - player_pos;
            let d2 = to.x * to.x + to.z * to.z;
            if nearest.map_or(true, |(best, _)| d2 < best) {
                nearest = Some((d2, t.position));
            }
        }

        // Default to "fall backwards" when no attacker can be found.
        let from_front = match nearest {
            Some((_, src_pos)) => {
                let to_src = Vec3::new(src_pos.x - player_pos.x, 0.0, src_pos.z - player_pos.z);
                let fwd = Vec3::new(player_fwd.x, 0.0, player_fwd.z).normalize_or_zero();
                if to_src.length_squared() < 1e-4 || fwd.length_squared() < 1e-4 {
                    true
                } else {
                    to_src.normalize().dot(fwd) >= 0.0
                }
            }
            None => true,
        };

        // Falling backwards (Death01) when hit from front; falling forward
        // (Death02) when hit from behind. Both fall back to whichever clip
        // exists in the library.
        let candidates: &[&str] = if from_front {
            &["Death01", "Death_01", "Death", "Death02", "Death_02"]
        } else {
            &["Death02", "Death_02", "Death", "Death01", "Death_01"]
        };

        let clip = match self.world.get::<&AnimationSet>(player_id) {
            Ok(set) => set.find_any(candidates),
            Err(_) => None,
        };
        let Some(clip) = clip else {
            log::warn!("Death animation not found in player's clip set");
            return;
        };

        // Cancel any active upper-body cast/hit reaction so the death
        // pose plays cleanly across the whole body.
        if let Ok(mut cast) = self.world.get::<&mut SpellCast>(player_id) {
            cast.phase = rift_engine::ecs::components::SpellPhase::Idle;
            cast.layer_animator = None;
            cast.weight = 0.0;
            cast.pending_oneshot = None;
            cast.oneshot_is_hit = false;
        }
        if let Ok(mut anim) = self.world.get::<&mut Animator>(player_id) {
            anim.cross_fade(clip, false, 0.18);
            anim.speed = 1.0;
        }
        if let Ok(mut vel) = self.world.get::<&mut Velocity>(player_id) {
            vel.linear = Vec3::ZERO;
        }
        // Drop any in-flight full-body action (Roll / Jump*) so the
        // post-system can't snap us into JumpLand on touchdown and
        // clobber the death pose. Also zero the jump vertical velocity
        // and clear the airborne flag so gravity stops yanking the
        // corpse around.
        if let Ok(mut p) = self.world.get::<&mut Player>(player_id) {
            p.action = PlayerAction::None;
            p.action_timer = 0.0;
            p.vy = 0.0;
            p.airborne = false;
        }
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

/// Map an `EnterPhase` (with sub-progress for the outfit-loading
/// stretch) to a 0..=1 fraction for the loading bar.
fn compute_enter_progress(
    phase: EnterPhase,
    equip: &equipment_visuals::EquipmentVisuals,
) -> f32 {
    // Phase weights — roughly proportional to wall-clock cost.
    const PREP_END: f32 = 0.05;
    const HUB_END: f32 = 0.45;
    const ATTACH_END: f32 = 0.50;
    const OUTFITS_END: f32 = 0.95;
    const WALLS_END: f32 = 1.0;

    match phase {
        EnterPhase::PrepareScene => PREP_END * 0.5,
        EnterPhase::GenerateHub => (PREP_END + HUB_END) * 0.5,
        EnterPhase::AttachOutfits => HUB_END + (ATTACH_END - HUB_END) * 0.5,
        EnterPhase::LoadOutfits => {
            let total = equip.total_pieces().max(1) as f32;
            let done = equip.loaded_pieces() as f32;
            ATTACH_END + (OUTFITS_END - ATTACH_END) * (done / total)
        }
        EnterPhase::RebuildWalls => OUTFITS_END + (WALLS_END - OUTFITS_END) * 0.5,
    }
}

/// Draw a centered "Entering world" loading overlay on top of whatever
/// the renderer is currently showing. Mirrors the engine's startup
/// loading screen but is drawn from the game side because the engine
/// only owns the boot loader.
fn draw_world_loading_overlay(renderer: &mut Renderer, progress: f32, label: &str) {
    let (sw, sh) = renderer.screen_size();
    let batch = &mut renderer.overlay_batch;

    // Full-screen darken so the in-progress hub geometry doesn't bleed
    // through (only relevant after `GenerateHub` has run).
    batch.rect_px(0.0, 0.0, sw, sh, [0.02, 0.02, 0.03, 0.92], sw, sh);

    let title = "Entering World";
    let title_size = 30.0;
    let title_w = batch.measure_text(title, title_size);
    batch.text(
        title,
        (sw - title_w) * 0.5,
        sh * 0.40 - title_size,
        title_size,
        [0.85, 0.80, 0.65, 1.0],
        sw,
        sh,
    );

    let bar_w = (sw * 0.45).max(240.0);
    let bar_h = 18.0;
    let bar_x = (sw - bar_w) * 0.5;
    let bar_y = sh * 0.50;
    batch.rect_px(bar_x, bar_y, bar_w, bar_h, [0.10, 0.10, 0.14, 1.0], sw, sh);
    let fill_w = bar_w * progress.clamp(0.0, 1.0);
    if fill_w > 0.5 {
        batch.rect_px(bar_x, bar_y, fill_w, bar_h, [0.55, 0.45, 0.20, 1.0], sw, sh);
    }
    let border = [0.30, 0.28, 0.22, 1.0];
    let t = 1.5;
    batch.rect_px(bar_x, bar_y, bar_w, t, border, sw, sh);
    batch.rect_px(bar_x, bar_y + bar_h - t, bar_w, t, border, sw, sh);
    batch.rect_px(bar_x, bar_y, t, bar_h, border, sw, sh);
    batch.rect_px(bar_x + bar_w - t, bar_y, t, bar_h, border, sw, sh);

    let label_size = 14.0;
    let label_w = batch.measure_text(label, label_size);
    batch.text(
        label,
        (sw - label_w) * 0.5,
        bar_y + bar_h + 16.0,
        label_size,
        [0.65, 0.62, 0.55, 1.0],
        sw,
        sh,
    );
}
