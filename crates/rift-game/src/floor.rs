use glam::{Mat4, Vec3};
use rift_engine::ai::systems::AiAgent;
use rift_engine::ai::{boss_behavior, brute_behavior, caster_behavior, elite_behavior, stalker_behavior, NavGrid};
use rift_engine::ecs::components::{
    AnimationSet, Boss, Collider, Elite, Enemy, EnemyKind, Health, Player, Renderable, Skinned, Static, Transform, Velocity,
};
use rift_engine::renderer::mesh::SkinnedMesh;
use rift_engine::{Floor, FloorConfig, Mesh, Renderer};
use std::sync::Arc;

use crate::player::PlayerState;
use crate::rift_state::RiftState;

/// Manages floor generation: creating the dungeon, spawning entities.
pub struct FloorManager {
    pub boss_room_center: Vec3,
    pub nav_grid: NavGrid,
}

impl FloorManager {
    pub fn new() -> Self {
        let floor = Floor::generate(FloorConfig::for_floor(1), 42);
        Self {
            boss_room_center: Vec3::ZERO,
            nav_grid: NavGrid::from_floor(&floor),
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

        // Still need individual ECS entities for collision
        for pos in &wall_positions {
            world.spawn((
                Transform::from_position(*pos + Vec3::new(0.0, 2.5, 0.0)),
                Collider::new(0.5, 2.5, 0.5),
                Static,
            ));
        }

        // Player — try to load the rigged base character as a SkinnedMesh.
        // For Phase 2b we render its bind pose (no animation yet) so this
        // should look identical to the previous static load. Phase 3 will
        // drive the bone palette from animation clips.
        let player_path = "assets/models/base-characters/Base Characters/Godot - UE/Superhero_Female_FullBody.gltf";
        let spawn = floor.spawn_pos;
        let (player_obj_index, skinned_component) = match SkinnedMesh::from_gltf(player_path) {
            Ok(skinned) => {
                // Build a Mesh from the bind-pose vertices for initial upload.
                // Using add_dynamic_mesh so we can re-skin per frame.
                let mut bind_mesh = Mesh::empty();
                bind_mesh.vertices = skinned.bind_vertices.clone();
                bind_mesh.indices = skinned.indices.clone();
                let idx = renderer.add_dynamic_mesh(&bind_mesh, Mat4::from_translation(spawn))?;
                // Apply the female base color texture from the modular outfits pack.
                let tex_path = "assets/models/modular-character-outfits/Textures/Base/T_Regular_Female_Dark_BaseColor.png";
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
            },
            Collider::new(0.3, 0.5, 0.3),
            Health::new(player_state.max_hp()),
            Renderable { object_index: player_obj_index },
        ));
        if let Some(comp) = skinned_component {
            // Load the animation library once, bind every clip to this
            // skeleton, store them in an AnimationSet so the locomotion
            // system can switch clips at runtime, and start with idle.
            let anim_path = "assets/models/animation-library/Unreal-Godot/UAL1_Standard.glb";
            let (anim_set, animator) = match rift_engine::animation::Clip::load_all(anim_path) {
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
                        "Bound {} animation clip(s) to player skeleton",
                        set.clips.len(),
                    );
                    let mut names: Vec<&String> = set.clips.keys().collect();
                    names.sort();
                    log::info!("Available clips: {:?}", names);
                    let initial = set.find_any(&["Idle_Loop", "Idle", "TPose"])
                        .or_else(|| set.clips.values().next().cloned());
                    let animator = initial.map(rift_engine::animation::Animator::new);
                    (Some(set), animator)
                }
                Err(e) => {
                    log::warn!("Animation library load failed: {}", e);
                    (None, None)
                }
            };
            world.insert_one(player_entity, comp).ok();
            if let Some(set) = anim_set {
                world.insert_one(player_entity, set).ok();
            }
            if let Some(anim) = animator {
                world.insert_one(player_entity, anim).ok();
            }
            // Layered upper-body cast component. The mask comes from the
            // skeleton itself (joints under spine/head/clavicle/arm/hand
            // chain weight 1.0; legs and pelvis remain 0.0) so we can blend
            // a Spell_Simple_* clip on top of locomotion without disturbing
            // the run cycle on the lower body. Also captures the spine-root
            // joint index for the cursor-aim torso twist.
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

                    let mesh = if is_elite {
                        &elite_mesh
                    } else {
                        match kind {
                            EnemyKind::Brute => &brute_mesh,
                            EnemyKind::Stalker => &stalker_mesh,
                            EnemyKind::Caster => &caster_mesh,
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

                    renderer.add_mesh(mesh, Mat4::from_translation(*pos))?;
                    let obj_index = renderer.objects.len() - 1;

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

    /// Spawn the rift boss.
    pub fn spawn_boss(
        &self,
        world: &mut hecs::World,
        renderer: &mut Renderer,
        rift: &RiftState,
    ) {
        let boss_mesh = Mesh::boss();
        let pos = self.boss_room_center;

        if let Ok(()) = renderer.add_mesh(&boss_mesh, Mat4::from_translation(pos)) {
            let obj_index = renderer.objects.len() - 1;
            let boss_health = 100.0 + rift.floor as f32 * 50.0;

            world.spawn((
                Transform::from_position(pos),
                Velocity::default(),
                Enemy {
                    speed: rift.boss_speed(),
                    progress_value: 0.0,
                    kind: EnemyKind::Brute,
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
}
