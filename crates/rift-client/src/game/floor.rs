use glam::{Mat4, Vec3};
use rift_engine::ai::NavGrid;
use rift_engine::ecs::components::{
    Collider, Enemy, EnemyAnim, EnemyKind, Health, LocalPlayer, NetControlled, Renderable, Skinned, Static, Transform, Velocity,
};
use rift_engine::{Floor, FloorConfig, Mesh, Renderer};

use super::environment::EnvTextures;
use super::monster_assets::MonsterCache;
use rift_game::monsters::MonsterRole;
use crate::game::PlayerState;
use super::props::{self, Props};
use super::rift_state::RiftState;
use super::torches::TorchSystem;

/// Manages floor generation: creating the dungeon, spawning entities.
pub struct FloorManager {
    pub boss_room_center: Vec3,
    pub nav_grid: NavGrid,
    pub monsters: MonsterCache,
    pub props: Props,
    pub env: EnvTextures,
    /// Wall-mounted torch flames + warm point lights, regenerated
    /// per floor. Despawned by [`Self::clear_torches`] on floor
    /// regen.
    pub torches: TorchSystem,
    /// World position of the hub stash chest, set at the end of
    /// [`Self::generate_hub`]. `None` when the active floor is a
    /// rift floor (the chest only exists in the hub). Read by
    /// `GameState::tick_stash_chest` for the proximity prompt.
    pub stash_chest_pos: Option<Vec3>,
}

impl FloorManager {
    pub fn new() -> Self {
        let floor = Floor::generate(FloorConfig::for_floor(1), 42);
        Self {
            boss_room_center: Vec3::ZERO,
            nav_grid: NavGrid::from_floor(&floor),
            monsters: MonsterCache::default(),
            props: Props::new(),
            env: EnvTextures::default(),
            torches: TorchSystem::new(),
            stash_chest_pos: None,
        }
    }

    /// Generate a new floor: clear world, create dungeon, spawn player + enemies.
    pub fn generate(
        &mut self,
        world: &mut hecs::World,
        renderer: &mut Renderer,
        rift: &RiftState,
        player_state: &PlayerState,
        anim_cache: &mut super::character_spawn::AnimLibraryCache,
        seed_override: Option<u64>,
    ) -> anyhow::Result<()> {
        *world = hecs::World::new();
        renderer.clear_objects();
        // Rift floors don't host the chest; clear any stale
        // hub-floor position so proximity tests can't false-fire.
        self.stash_chest_pos = None;
        // Despawn the previous floor's torch VFX before we
        // regenerate. Their `EffectId`s belong to the old
        // particle system slots; leaving them around would
        // leak emitter capacity.
        self.torches.clear(renderer);

        let config = FloorConfig::for_floor(rift.floor);
        let seed = seed_override
            .map(|s| s + rift.floor as u64 * 7)
            .unwrap_or(42 + rift.floor as u64 * 7);
        let floor = Floor::generate(config, seed);

        self.boss_room_center = floor.boss_room_center;
        self.nav_grid = NavGrid::from_floor(&floor);

        // Set floor theme clear color — cave-dark Diablo ambience.
        // Torches carry the warm punctuation, so the unlit base
        // is intentionally near-black.
        renderer.clear_color = match rift.floor % 4 {
            0 => [0.006, 0.004, 0.003, 1.0], // dark stone dungeon
            1 => [0.004, 0.007, 0.004, 1.0], // mossy crypts
            2 => [0.014, 0.004, 0.003, 1.0], // infernal red tint
            _ => [0.003, 0.005, 0.010, 1.0], // icy depths
        };
        // Fog color slightly warmer than clear (distant haze tint)
        renderer.fog_color = [
            renderer.clear_color[0] * 1.4 + 0.002,
            renderer.clear_color[1] * 1.2 + 0.001,
            renderer.clear_color[2] * 1.1 + 0.001,
        ];
        // Tighter fog for damp, claustrophobic rift floors. The
        // player still has line-of-sight to the room they're in,
        // but anything past the next doorway dissolves into the
        // black, keeping torches dramatic.
        renderer.fog_start = 6.0;
        renderer.fog_end = 22.0;

        // Indoor dungeon: leave the sky disabled — the fog wall
        // already swallows everything past ~32 m, and a procedural
        // sky sneaking through the ceilingless walls would break
        // the dungeon-y feel.
        renderer.sky = rift_engine::SkyConfig::default();
        // Cave-dark key + low ambient so torches drive the look.
        renderer.key_light = rift_engine::KeyLight::DUNGEON;

        // Floor mesh — only walkable tiles, batched into one draw
        let floor_positions = floor.floor_positions();
        let floor_mesh = Mesh::dungeon_floor(&floor_positions, rift.floor);
        renderer.add_mesh(&floor_mesh, Mat4::IDENTITY)?;
        let floor_obj_idx = renderer.objects.len() - 1;

        // Walls — batched into a single draw call, themed per floor
        // (dark + slightly desaturated; torches will warm them up).
        let wall_color = match rift.floor % 4 {
            0 => Vec3::new(0.18, 0.16, 0.14), // damp weathered stone
            1 => Vec3::new(0.13, 0.18, 0.11), // deep mossy green
            2 => Vec3::new(0.24, 0.10, 0.08), // dried-blood crimson
            _ => Vec3::new(0.11, 0.15, 0.21), // glacial blue-gray
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
        props::fantasy::decorate_dungeon(&mut self.props, world, renderer, &floor, seed);

        // Wall torches: looping flame VFX (HDR additive → blooms)
        // + a warm point light at each sconce. The renderer caps
        // active lights at 8, but `TorchSystem::update_lights` is
        // called every frame to keep the nearest 8 to the player
        // active.
        self.torches.place(&floor, renderer, seed);

        // Player — spawned via shared helper so the hub generator can
        // reuse the same skinned-character + animation-set bring-up.
        let spawn = floor.spawn_pos;
        self.spawn_player(world, renderer, spawn, player_state, anim_cache)?;

        // Enemies — server-authoritative. The floor visuals (walls,
        // props, player) are spawned here but enemy entities arrive
        // via `sync_enemies` once the server's snapshot lands.
        log::info!(
            "=== RIFT LEVEL {} === | {} rooms | enemies: server-authoritative | Kill progress needed: {:.0}",
            rift.floor,
            floor.rooms.len(),
            rift.progress_required,
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
        anim_cache: &mut super::character_spawn::AnimLibraryCache,
    ) -> anyhow::Result<Vec3> {
        *world = hecs::World::new();
        renderer.clear_objects();

        let floor = Floor::hub();
        self.boss_room_center = Vec3::ZERO;
        self.nav_grid = NavGrid::from_floor(&floor);

        // Bright sunny meadow ambience: pale-blue sky, soft warm haze.
        renderer.clear_color = [0.55, 0.78, 0.95, 1.0];
        renderer.fog_color = [0.78, 0.88, 0.96];
        // Pushed-out fog so the procedural sky is actually
        // visible — keeping the old 7..14 wall meant the dome
        // got tinted out before it could read. The grass apron
        // disc is 80 m wide so 14..60 still hides the rim.
        renderer.fog_start = 14.0;
        renderer.fog_end = 60.0;

        // Sunny outdoor sky for the hub.
        renderer.sky = rift_engine::SkyConfig::meadow();
        // Bright warm key + lifted ambient — the hub is meant
        // to feel safe and readable, not cave-dark like the
        // rift floors.
        renderer.key_light = rift_engine::KeyLight::SUNLIT;

        // Hub floor: use the oversized grass-apron disc only.
        // The dungeon-floor batch tints itself with a per-floor
        // base color (dark stone for floor_num 0), which left a
        // visibly darker square patch over the playable tiles
        // even when the grass texture was bound. Drawing just
        // the apron \u2014 which uses a neutral white tint and
        // world-space UVs \u2014 gives a single uniform grass
        // surface across the whole hub. Wall collision still
        // comes from `wall_positions` below; the inner floor
        // mesh was always purely visual.
        let hub_centre = Vec3::new(
            (floor.width / 2) as f32,
            -0.01,
            (floor.depth / 2) as f32,
        );
        let apron = Mesh::ground_disc(hub_centre, 80.0, 96, Vec3::splat(1.0));
        renderer.add_mesh(&apron, Mat4::IDENTITY)?;
        let apron_obj_idx = renderer.objects.len() - 1;
        self.env.ensure_grass(renderer);
        if let Some(set) = self.env.grass_floor_set {
            renderer.set_object_shared_material(apron_obj_idx, set);
        }

        // Wall colliders only — no wall mesh, no tree perimeter. The
        // fog horizon hides the floor edge so the hub reads as a
        // mysterious circular platform floating in mist.
        let wall_positions = floor.wall_positions();
        for pos in &wall_positions {
            world.spawn((
                Transform::from_position(*pos + Vec3::new(0.0, 2.5, 0.0)),
                Collider::new(0.5, 2.5, 0.5),
                Static,
            ));
        }

        // Outdoor decoration: forest border + ground scatter.
        // Fixed seed = stable layout across deaths.
        props::nature::decorate_hub(&mut self.props, world, renderer, &floor, 0xC0FFEE);

        // Player stash chest. Sits a couple of tiles to the south-east
        // of the central portal so it's visible from the spawn point
        // without blocking the walk-up to the portal. Yaw rotates it
        // ~30° so the lid faces the spawn approach.
        let portal_centre = floor.first_room_center();
        let stash_pos = portal_centre + Vec3::new(2.6, 0.0, 2.2);
        self.props.spawn(
            world,
            renderer,
            &props::nature::STASH_CHEST,
            stash_pos,
            std::f32::consts::FRAC_PI_6 * -1.0,
            (0, 0),
            None,
        );
        self.stash_chest_pos = Some(stash_pos);

        let spawn = floor.spawn_pos;
        self.spawn_player(world, renderer, spawn, player_state, anim_cache)?;

        let portal_pos = floor.first_room_center() + Vec3::new(0.0, 0.5, 0.0);
        log::info!("Hub generated. Portal at {:?}", portal_pos);
        Ok(portal_pos)
    }

    /// Shared local-player entity construction used by both rift and
    /// hub generation. Delegates to `character_spawn::spawn_character_entity`
    /// for the heavy lifting and then attaches the `LocalPlayer`
    /// marker so SP systems (camera, HUD, abilities) recognise it.
    fn spawn_player(
        &mut self,
        world: &mut hecs::World,
        renderer: &mut Renderer,
        spawn: Vec3,
        player_state: &PlayerState,
        anim_cache: &mut super::character_spawn::AnimLibraryCache,
    ) -> anyhow::Result<()> {
        let entity = super::character_spawn::spawn_character_entity(
            world,
            renderer,
            anim_cache,
            super::character_spawn::CharacterSpawn {
                position: spawn,
                gender: player_state.gender,
                move_speed: player_state.config.base_move_speed,
                max_hp: player_state.max_hp(),
            },
        )?;
        world.insert_one(entity, LocalPlayer).ok();
        Ok(())
    }
}

/// Spawn a remote (server-replicated) enemy's visual + ECS shell.
/// Used by `net_client::sync_enemies` when a fresh enemy `NetId`
/// shows up in a snapshot. We omit `Enemy`, `AiAgent`, and
/// `Collider` on purpose so SP combat / AI / damage systems leave
/// the entity alone — the server is sole authority for movement,
/// hits, and HP. The entity still gets `Health` so the HUD HP
/// bar can render off the snapshot's `health_pct`, and
/// `NetControlled` so any future SP gates that look for it can
/// short-circuit.
pub fn spawn_remote_enemy_entity(
    world: &mut hecs::World,
    renderer: &mut Renderer,
    monsters: &mut MonsterCache,
    role: MonsterRole,
    position: Vec3,
    hp_max: f32,
) -> anyhow::Result<hecs::Entity> {
    // Make sure the role's shared texture is uploaded before we
    // bind it. First call per role does the upload; subsequent
    // calls return the cached descriptor set.
    let shared_set = monsters
        .slot_mut(role)
        .as_mut()
        .and_then(|a| a.ensure_shared_material(renderer));
    let asset = monsters
        .get(role)
        .ok_or_else(|| anyhow::anyhow!("monster role {role:?} not loaded"))?;

    let mut bind_mesh = Mesh::empty();
    bind_mesh.vertices = asset.mesh.bind_vertices.clone();
    bind_mesh.indices = asset.mesh.indices.clone();
    let scaled = Mat4::from_scale_rotation_translation(
        Vec3::splat(role.scale()),
        glam::Quat::IDENTITY,
        position,
    );
    let obj_index = renderer.add_dynamic_mesh(&bind_mesh, scaled)?;
    if let Some(set) = shared_set {
        renderer.set_object_shared_material(obj_index, set);
    }
    let skinned = Skinned { mesh: asset.mesh.clone(), scratch: Vec::new(), joint_worlds: Vec::new() };
    let initial_clip = asset
        .anims
        .find_any(&["Idle", "Idle_Loop"])
        .or_else(|| asset.anims.find_any(&["Walk", "Walk_Loop"]))
        .or_else(|| asset.anims.clips.values().next().cloned());
    let animator = initial_clip.map(rift_engine::animation::Animator::new);

    let mut builder = hecs::EntityBuilder::new();
    builder.add(Transform::from_position(position));
    builder.add(Velocity::default());
    builder.add(Health::new(hp_max));
    builder.add(Renderable { object_index: obj_index });
    builder.add(NetControlled);
    // Tag as `Enemy` so the HUD pass picks it up for floating health
    // bars + boss arrow. Speed/progress_value are server-authoritative
    // so we leave them at safe defaults; only `kind` matters visually.
    builder.add(Enemy {
        speed: 0.0,
        progress_value: 0.0,
        kind: match role {
            MonsterRole::Brute | MonsterRole::Elite | MonsterRole::Boss => EnemyKind::Brute,
            MonsterRole::Stalker => EnemyKind::Stalker,
            MonsterRole::Caster => EnemyKind::Caster,
        },
    });
    builder.add(skinned);
    builder.add(asset.anims.clone());
    if let Some(a) = animator {
        builder.add(a);
    }
    builder.add(EnemyAnim {
        last_hp: hp_max,
        attacking: false,
        lock_remaining: 0.0,
    });
    if matches!(role, MonsterRole::Boss) {
        builder.add(rift_engine::ecs::components::Boss);
    }
    Ok(world.spawn(builder.build()))
}
