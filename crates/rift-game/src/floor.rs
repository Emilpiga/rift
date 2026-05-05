use glam::{Mat4, Vec3};
use rift_engine::ai::systems::AiAgent;
use rift_engine::ai::{boss_behavior, brute_behavior, caster_behavior, elite_behavior, stalker_behavior, NavGrid};
use rift_engine::ecs::components::{
    AnimationSet, Boss, Collider, Elite, Enemy, EnemyAnim, EnemyKind, Health, Player, Renderable, Skinned, Static, Transform, Velocity,
};
use rift_engine::renderer::mesh::SkinnedMesh;
use rift_engine::{Floor, FloorConfig, Mesh, Renderer};
use std::sync::Arc;

use crate::environment::EnvTextures;
use crate::monsters::{MonsterCache, MonsterRole};
use crate::player::PlayerState;
use crate::props::PropLibrary;
use crate::rift_state::RiftState;

/// Manages floor generation: creating the dungeon, spawning entities.
pub struct FloorManager {
    pub boss_room_center: Vec3,
    pub nav_grid: NavGrid,
    pub monsters: MonsterCache,
    pub props: PropLibrary,
    pub env: EnvTextures,
    /// Cache of the player's bound animation clips. Populated on the
    /// first `spawn_player` call and reused on every subsequent floor
    /// regeneration so we don't pay the glTF-load + bind cost again
    /// (and so death / hit-reaction lookups don't ever miss because
    /// the clip library failed to load mid-run).
    player_anim_cache: Option<AnimationSet>,
}

impl FloorManager {
    pub fn new() -> Self {
        let floor = Floor::generate(FloorConfig::for_floor(1), 42);
        Self {
            boss_room_center: Vec3::ZERO,
            nav_grid: NavGrid::from_floor(&floor),
            monsters: MonsterCache::default(),
            props: PropLibrary::new(),
            env: EnvTextures::default(),
            player_anim_cache: None,
        }
    }

    /// Generate a new floor: clear world, create dungeon, spawn player + enemies.
    pub fn generate(
        &mut self,
        world: &mut hecs::World,
        renderer: &mut Renderer,
        rift: &RiftState,
        player_state: &PlayerState,
    ) -> anyhow::Result<()> {
        *world = hecs::World::new();
        renderer.clear_objects();

        let config = FloorConfig::for_floor(rift.floor);
        let seed = 42 + rift.floor as u64 * 7;
        let floor = Floor::generate(config, seed);

        self.boss_room_center = floor.boss_room_center;
        self.nav_grid = NavGrid::from_floor(&floor);

        // Set floor theme clear color — moody Diablo-style ambience
        renderer.clear_color = match rift.floor % 4 {
            0 => [0.012, 0.008, 0.006, 1.0], // dark stone dungeon (warm shadow)
            1 => [0.008, 0.014, 0.008, 1.0], // mossy crypts (cold dampness)
            2 => [0.030, 0.008, 0.005, 1.0], // infernal red tint (hellish glow)
            _ => [0.006, 0.010, 0.020, 1.0], // icy depths (deep cold blue)
        };
        // Fog color slightly warmer than clear (distant haze tint)
        renderer.fog_color = [
            renderer.clear_color[0] * 1.4 + 0.004,
            renderer.clear_color[1] * 1.2 + 0.002,
            renderer.clear_color[2] * 1.1 + 0.001,
        ];

        // Floor mesh — only walkable tiles, batched into one draw
        let floor_positions = floor.floor_positions();
        let floor_mesh = Mesh::dungeon_floor(&floor_positions, rift.floor);
        renderer.add_mesh(&floor_mesh, Mat4::IDENTITY)?;
        let floor_obj_idx = renderer.objects.len() - 1;

        // Walls — batched into a single draw call, themed per floor (darker, more saturated)
        let wall_color = match rift.floor % 4 {
            0 => Vec3::new(0.32, 0.28, 0.24), // dark weathered stone
            1 => Vec3::new(0.22, 0.32, 0.18), // deep mossy green
            2 => Vec3::new(0.42, 0.18, 0.14), // dried-blood crimson
            _ => Vec3::new(0.20, 0.26, 0.36), // glacial blue-gray
        };
        let wall_mesh = Mesh::wall_colored(wall_color);
        let wall_positions = floor.wall_positions();

        // Batch all walls into one big mesh for rendering
        let batched_walls = Mesh::batch_at_positions(&wall_mesh, &wall_positions);
        renderer.add_mesh(&batched_walls, Mat4::IDENTITY)?;
        let wall_obj_idx = renderer.objects.len() - 1;

        // Bind procedural stone textures to floor and walls.
        self.env.ensure(renderer);
        if let Some(set) = self.env.floor_set {
            renderer.set_object_shared_material(floor_obj_idx, set);
        }
        if let Some(set) = self.env.wall_set {
            renderer.set_object_shared_material(wall_obj_idx, set);
        }

        // Still need individual ECS entities for collision
        for pos in &wall_positions {
            world.spawn((
                Transform::from_position(*pos + Vec3::new(0.0, 2.5, 0.0)),
                Collider::new(0.5, 2.5, 0.5),
                Static,
            ));
        }

        // Decorate rooms with static fantasy props (barrels, benches, candles, …).
        // Done before enemies spawn so the same seed picks consistent positions.
        self.props.decorate(world, renderer, &floor, seed);

        // Player — spawned via shared helper so the hub generator can
        // reuse the same skinned-character + animation-set bring-up.
        let spawn = floor.spawn_pos;
        self.spawn_player(world, renderer, spawn, player_state)?;

        // Enemies — pack-based spawning with mixed archetypes per pack
        let brute_mesh = Mesh::enemy();
        let stalker_mesh = Mesh::enemy_stalker();
        let caster_mesh = Mesh::enemy_caster();
        let elite_mesh = Mesh::elite_enemy();
        let floor_config = &floor.config;
        let arena_rooms = floor.arena_rooms();

        let total_enemies = floor_config.enemy_count();
        let progress_per_enemy =
            rift.progress_required / (total_enemies as f32).max(1.0);

        // Don't spawn enemies inside the player's aggro bubble at floor start.
        // Enemy detection range is ~12 units (see ai/trees.rs); add a small margin.
        const SAFE_SPAWN_DIST: f32 = 13.5;
        let safe_dist_sq = SAFE_SPAWN_DIST * SAFE_SPAWN_DIST;
        let safe_from_player = |p: Vec3| -> bool {
            let dx = p.x - spawn.x;
            let dz = p.z - spawn.z;
            (dx * dx + dz * dz) >= safe_dist_sq
        };

        let mut enemy_seed = 1000_u64 + rift.floor as u64;
        let mut spawned = 0u32;
        let mut count_brute = 0u32;
        let mut count_stalker = 0u32;
        let mut count_caster = 0u32;
        let mut count_elite = 0u32;

        for room in &arena_rooms {
            let packs = room.spawn_packs(
                floor_config.packs_per_room,
                floor_config.mobs_per_pack,
                enemy_seed,
            );
            enemy_seed = enemy_seed.wrapping_mul(6364136223846793005).wrapping_add(1);

            for (pack_center, positions) in &packs {
                // Skip entire pack if its center is inside the player's safe bubble.
                if !safe_from_player(*pack_center) {
                    continue;
                }
                // Determine if this pack has an elite leader
                let elite_roll = ((enemy_seed >> 16) as f32) / (u32::MAX as f32);
                enemy_seed = enemy_seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let has_elite = elite_roll < floor_config.elite_chance;

                for (i, pos) in positions.iter().enumerate() {
                    // Defensive: skip individual mob if it landed too close.
                    if !safe_from_player(*pos) {
                        continue;
                    }
                    let is_elite = has_elite && i == 0;

                    // Force pack diversity: rotate kinds within a pack so the
                    // player meets brute + stalker + caster every encounter.
                    let kind = if is_elite {
                        EnemyKind::Brute
                    } else {
                        match i % 3 {
                            0 => EnemyKind::Caster,
                            1 => EnemyKind::Stalker,
                            _ => EnemyKind::Brute,
                        }
                    };

                    let hp = if is_elite {
                        floor_config.enemy_health * floor_config.elite_hp_mult
                    } else {
                        match kind {
                            EnemyKind::Brute => floor_config.enemy_health * 1.15,
                            EnemyKind::Stalker => floor_config.enemy_health * 0.75,
                            EnemyKind::Caster => floor_config.enemy_health * 0.65,
                        }
                    };
                    let speed = if is_elite {
                        floor_config.enemy_speed * 0.8
                    } else {
                        match kind {
                            EnemyKind::Brute => floor_config.enemy_speed * 0.85,
                            EnemyKind::Stalker => floor_config.enemy_speed * 1.35,
                            EnemyKind::Caster => floor_config.enemy_speed * 0.95,
                        }
                    };

                    let tree = if is_elite {
                        elite_behavior()
                    } else {
                        match kind {
                            EnemyKind::Brute => brute_behavior(),
                            EnemyKind::Stalker => stalker_behavior(),
                            EnemyKind::Caster => caster_behavior(),
                        }
                    };

                    // Pick the matching skinned monster, falling back to
                    // the procedural mesh when the asset isn't available
                    // (e.g. on first floor before monsters preload).
                    let role = if is_elite { MonsterRole::Elite } else { MonsterRole::from_kind(kind) };
                    // Make sure the role's shared texture+descriptor is
                    // uploaded before we use it for this spawn.  This
                    // happens at most once per role per process — every
                    // subsequent spawn just reuses the same descriptor
                    // set instead of allocating a fresh one.
                    let shared_set = self.monsters
                        .slot_mut(role)
                        .as_mut()
                        .and_then(|a| a.ensure_shared_material(renderer));
                    let skinned_asset = self.monsters.get(role);
                    let (obj_index, skinned_component, anim_set, animator) =
                        if let Some(asset) = skinned_asset {
                            let mut bind_mesh = Mesh::empty();
                            bind_mesh.vertices = asset.mesh.bind_vertices.clone();
                            bind_mesh.indices = asset.mesh.indices.clone();
                            let scaled = Mat4::from_scale_rotation_translation(
                                Vec3::splat(role.scale()),
                                glam::Quat::IDENTITY,
                                *pos,
                            );
                            let idx = renderer.add_dynamic_mesh(&bind_mesh, scaled)?;
                            // Bind the shared per-role texture if it
                            // uploaded successfully; otherwise the
                            // monster falls back to the default white
                            // material (still better than a crash).
                            if let Some(set) = shared_set {
                                renderer.set_object_shared_material(idx, set);
                            }
                            let comp = Skinned { mesh: asset.mesh.clone(), scratch: Vec::new() };
                            let initial = asset.anims.find_any(&["Idle", "Idle_Loop"])
                                .or_else(|| asset.anims.find_any(&["Walk", "Walk_Loop"]))
                                .or_else(|| asset.anims.clips.values().next().cloned());
                            let animator = initial.map(rift_engine::animation::Animator::new);
                            (idx, Some(comp), Some(asset.anims.clone()), animator)
                        } else {
                            let mesh = if is_elite {
                                &elite_mesh
                            } else {
                                match kind {
                                    EnemyKind::Brute => &brute_mesh,
                                    EnemyKind::Stalker => &stalker_mesh,
                                    EnemyKind::Caster => &caster_mesh,
                                }
                            };
                            renderer.add_mesh(mesh, Mat4::from_translation(*pos))?;
                            (renderer.objects.len() - 1, None, None, None)
                        };

                    let mut builder = hecs::EntityBuilder::new();
                    builder.add(Transform::from_position(*pos));
                    builder.add(Velocity::default());
                    builder.add(Enemy {
                        speed,
                        progress_value: if is_elite { progress_per_enemy * 3.0 } else { progress_per_enemy },
                        kind,
                    });
                    builder.add(Collider::new(
                        if is_elite { 0.55 } else { 0.4 },
                        if is_elite { 0.6 } else { 0.45 },
                        if is_elite { 0.55 } else { 0.4 },
                    ));
                    builder.add(Health::new(hp));
                    builder.add(Renderable { object_index: obj_index });
                    builder.add(AiAgent::new(tree, *pack_center));
                    if is_elite {
                        builder.add(Elite::default());
                    }
                    if let Some(s) = skinned_component { builder.add(s); }
                    if let Some(a) = anim_set { builder.add(a); }
                    if let Some(a) = animator { builder.add(a); }
                    if skinned_asset.is_some() {
                        builder.add(EnemyAnim { last_hp: hp, attacking: false, lock_remaining: 0.0 });
                    }

                    world.spawn(builder.build());
                    spawned += 1;
                    if is_elite {
                        count_elite += 1;
                    } else {
                        match kind {
                            EnemyKind::Brute => count_brute += 1,
                            EnemyKind::Stalker => count_stalker += 1,
                            EnemyKind::Caster => count_caster += 1,
                        }
                    }
                }
            }
        }

        log::info!(
            "=== RIFT LEVEL {} === | {} rooms | {} enemies (Brute={}, Stalker={}, Caster={}, Elite={}) | Kill progress needed: {:.0}",
            rift.floor,
            floor.rooms.len(),
            spawned,
            count_brute,
            count_stalker,
            count_caster,
            count_elite,
            rift.progress_required
        );

        Ok(())
    }

    /// Generate the safe hub / starting zone: a single small stone room
    /// with no enemies, no fog wall, no boss progress.  Returns the
    /// world-space position of the centre point where the caller should
    /// spawn the "enter the rift" portal.
    pub fn generate_hub(
        &mut self,
        world: &mut hecs::World,
        renderer: &mut Renderer,
        player_state: &PlayerState,
    ) -> anyhow::Result<Vec3> {
        *world = hecs::World::new();
        renderer.clear_objects();

        let floor = Floor::hub();
        self.boss_room_center = Vec3::ZERO;
        self.nav_grid = NavGrid::from_floor(&floor);

        // Calmer ambience for the hub: warm torchlit stone, less murk
        // than the rift floors but still moody.
        renderer.clear_color = [0.018, 0.014, 0.010, 1.0];
        renderer.fog_color = [
            renderer.clear_color[0] * 1.4 + 0.004,
            renderer.clear_color[1] * 1.2 + 0.002,
            renderer.clear_color[2] * 1.1 + 0.001,
        ];

        let floor_positions = floor.floor_positions();
        let floor_mesh = Mesh::dungeon_floor(&floor_positions, 0);
        renderer.add_mesh(&floor_mesh, Mat4::IDENTITY)?;
        let floor_obj_idx = renderer.objects.len() - 1;

        let wall_color = Vec3::new(0.30, 0.26, 0.22);
        let wall_mesh = Mesh::wall_colored(wall_color);
        let wall_positions = floor.wall_positions();
        let batched_walls = Mesh::batch_at_positions(&wall_mesh, &wall_positions);
        renderer.add_mesh(&batched_walls, Mat4::IDENTITY)?;
        let wall_obj_idx = renderer.objects.len() - 1;

        self.env.ensure(renderer);
        if let Some(set) = self.env.floor_set {
            renderer.set_object_shared_material(floor_obj_idx, set);
        }
        if let Some(set) = self.env.wall_set {
            renderer.set_object_shared_material(wall_obj_idx, set);
        }

        for pos in &wall_positions {
            world.spawn((
                Transform::from_position(*pos + Vec3::new(0.0, 2.5, 0.0)),
                Collider::new(0.5, 2.5, 0.5),
                Static,
            ));
        }

        // Light prop decoration is fine in the hub (banners, candles).
        // Reuse the existing seeded pass with a fixed seed so the layout
        // is stable across deaths.
        self.props.decorate(world, renderer, &floor, 0xC0FFEE);

        let spawn = floor.spawn_pos;
        self.spawn_player(world, renderer, spawn, player_state)?;

        let portal_pos = floor.first_room_center() + Vec3::new(0.0, 0.5, 0.0);
        log::info!("Hub generated. Portal at {:?}", portal_pos);
        Ok(portal_pos)
    }

    /// Shared player-entity construction used by both rift and hub
    /// generation.  Loads the rigged base mesh, binds the animation
    /// library, builds the upper-body spell-cast layer and inserts the
    /// player into the world at `spawn`.
    fn spawn_player(
        &mut self,
        world: &mut hecs::World,
        renderer: &mut Renderer,
        spawn: Vec3,
        player_state: &PlayerState,
    ) -> anyhow::Result<()> {
        let (player_path, tex_path) = crate::classes::base_model_paths(player_state.gender);
        let (player_obj_index, skinned_component) = match SkinnedMesh::from_gltf(player_path) {
            Ok(skinned) => {
                let mut bind_mesh = Mesh::empty();
                bind_mesh.vertices = skinned.bind_vertices.clone();
                bind_mesh.indices = skinned.indices.clone();
                let idx = renderer.add_dynamic_mesh(&bind_mesh, Mat4::from_translation(spawn))?;
                if let Err(e) = renderer.set_object_texture(idx, tex_path) {
                    log::warn!("Player texture load failed: {}", e);
                }
                let comp = Skinned {
                    mesh: Arc::new(skinned),
                    scratch: Vec::new(),
                };
                (idx, Some(comp))
            }
            Err(e) => {
                log::warn!("Falling back to procedural player mesh: {}", e);
                let cube = Mesh::cube();
                renderer.add_mesh(&cube, Mat4::from_translation(spawn))?;
                (renderer.objects.len() - 1, None)
            }
        };

        let player_entity = world.spawn((
            Transform::from_position(spawn),
            Velocity::default(),
            Player {
                speed: player_state.config.base_move_speed,
                aim_dir: glam::Vec3::Z,
                spine_joint: u32::MAX,
                action: rift_engine::ecs::components::PlayerAction::default(),
                action_timer: 0.0,
                vy: 0.0,
                airborne: false,
            },
            Collider::new(0.3, 0.5, 0.3),
            Health::new(player_state.max_hp()),
            Renderable { object_index: player_obj_index },
        ));
        if let Some(comp) = skinned_component {
            // Bind / re-bind the animation library to this skeleton.
            // The clip glTF is loaded from disk only the first time;
            // subsequent floors clone the cached `AnimationSet` so a
            // transient I/O failure can't strand the player without a
            // Death / Hit clip mid-run.
            let anim_set: Option<AnimationSet> = if let Some(cached) = self.player_anim_cache.as_ref() {
                Some(cached.clone())
            } else {
                let anim_path = "assets/models/animation-library/Unreal-Godot/UAL1_Standard.glb";
                match rift_engine::animation::Clip::load_all(anim_path) {
                    Ok(clips) => {
                        let mut set = AnimationSet::default();
                        for clip in &clips {
                            let bound = clip.bind_to_skeleton(
                                &comp.mesh.joint_index_by_name,
                                comp.mesh.joints.len(),
                            );
                            set.clips.insert(clip.name.to_ascii_lowercase(), Arc::new(bound));
                        }
                        log::info!(
                            "Bound {} animation clip(s) to player skeleton (cached)",
                            set.clips.len(),
                        );
                        self.player_anim_cache = Some(set.clone());
                        Some(set)
                    }
                    Err(e) => {
                        log::warn!("Animation library load failed: {}", e);
                        None
                    }
                }
            };
            let animator = anim_set
                .as_ref()
                .and_then(|set| {
                    set.find_any(&["Idle_Loop", "Idle", "TPose"])
                        .or_else(|| set.clips.values().next().cloned())
                })
                .map(rift_engine::animation::Animator::new);
            world.insert_one(player_entity, comp).ok();
            if let Some(set) = anim_set {
                world.insert_one(player_entity, set).ok();
            }
            if let Some(anim) = animator {
                world.insert_one(player_entity, anim).ok();
            }
            let (mask_opt, spine_idx): (Option<Vec<f32>>, Option<usize>) =
                match world.get::<&rift_engine::ecs::components::Skinned>(player_entity) {
                    Ok(s) => (Some(s.mesh.upper_body_mask()), s.mesh.spine_root_joint()),
                    Err(_) => (None, None),
                };
            if let Some(mask) = mask_opt {
                world.insert_one(
                    player_entity,
                    rift_engine::ecs::components::SpellCast::new(mask),
                ).ok();
            }
            if let Some(idx) = spine_idx {
                if let Ok(mut p) = world.get::<&mut Player>(player_entity) {
                    p.spine_joint = idx as u32;
                }
            }
        }
        Ok(())
    }

    /// Spawn the rift boss.
    pub fn spawn_boss(
        &mut self,
        world: &mut hecs::World,
        renderer: &mut Renderer,
        rift: &RiftState,
    ) {
        let pos = self.boss_room_center;
        let boss_health = 100.0 + rift.floor as f32 * 50.0;

        // Skinned boss if available, procedural cube otherwise.
        let role = MonsterRole::Boss;
        let shared_set = self.monsters
            .slot_mut(role)
            .as_mut()
            .and_then(|a| a.ensure_shared_material(renderer));
        let asset = self.monsters.get(role);
        let (obj_index, skinned_component, anim_set, animator) = if let Some(asset) = asset {
            let mut bind_mesh = Mesh::empty();
            bind_mesh.vertices = asset.mesh.bind_vertices.clone();
            bind_mesh.indices = asset.mesh.indices.clone();
            let scaled = Mat4::from_scale_rotation_translation(
                Vec3::splat(role.scale()),
                glam::Quat::IDENTITY,
                pos,
            );
            match renderer.add_dynamic_mesh(&bind_mesh, scaled) {
                Ok(idx) => {
                    if let Some(set) = shared_set {
                        renderer.set_object_shared_material(idx, set);
                    }
                    let comp = Skinned { mesh: asset.mesh.clone(), scratch: Vec::new() };
                    let initial = asset.anims.find_any(&["Idle", "Idle_Loop", "Walk"])
                        .or_else(|| asset.anims.clips.values().next().cloned());
                    let animator = initial.map(rift_engine::animation::Animator::new);
                    (idx, Some(comp), Some(asset.anims.clone()), animator)
                }
                Err(e) => {
                    log::warn!("boss skinned mesh upload failed: {}", e);
                    let cube = Mesh::boss();
                    if let Err(e) = renderer.add_mesh(&cube, Mat4::from_translation(pos)) {
                        log::warn!("boss fallback mesh failed: {}", e);
                        return;
                    }
                    (renderer.objects.len() - 1, None, None, None)
                }
            }
        } else {
            let cube = Mesh::boss();
            if let Err(e) = renderer.add_mesh(&cube, Mat4::from_translation(pos)) {
                log::warn!("boss fallback mesh failed: {}", e);
                return;
            }
            (renderer.objects.len() - 1, None, None, None)
        };

        let mut builder = hecs::EntityBuilder::new();
        builder.add(Transform::from_position(pos));
        builder.add(Velocity::default());
        builder.add(Enemy {
            speed: rift.boss_speed(),
            progress_value: 0.0,
            kind: EnemyKind::Brute,
        });
        builder.add(Boss);
        builder.add(Collider::new(0.8, 0.9, 0.8));
        builder.add(Health::new(boss_health));
        builder.add(Renderable { object_index: obj_index });
        builder.add(AiAgent::new(boss_behavior(), pos));
        if let Some(s) = skinned_component { builder.add(s); }
        if let Some(a) = anim_set { builder.add(a); }
        if let Some(a) = animator { builder.add(a); }
        if asset.is_some() {
            builder.add(EnemyAnim { last_hp: boss_health, attacking: false, lock_remaining: 0.0 });
        }
        world.spawn(builder.build());

        log::info!(
            ">>> BOSS SPAWNED! HP: {:.0} | Location: boss room <<<",
            boss_health
        );
    }
}
