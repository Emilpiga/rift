//! Shared "spawn a skinned player character" path used by both
//! single-player floor generation (the local player) and the
//! multiplayer net client (each remote player). Builds the
//! rigged base mesh, binds the animation library, sets up the
//! upper-body spell-cast layer, and inserts the entity into the
//! world.
//!
//! The animation library is loaded from disk only the first time
//! and re-used via the caller-owned cache so that a transient I/O
//! failure can't strand later spawns without their Idle/Run/Hit
//! clips.

use std::sync::Arc;

use glam::{Mat4, Vec3};
use rift_engine::ecs::components::{
    AnimationSet, Collider, Health, Player, Renderable, Skinned, Transform, Velocity,
};
use rift_engine::renderer::mesh::SkinnedMesh;
use rift_engine::{Mesh, Renderer};

use rift_game::character::Gender;

use super::avatar_cosmetics::{self, AvatarCosmeticsCache, is_body_mesh_name};

/// Paths to the animation packs we ship. Loaded in order; later
/// entries override clips with the same (case-insensitive) name
/// from earlier ones, so library-two acts as an extension/patch
/// pack on top of library-one.
const ANIM_LIBRARY_PATHS: &[&str] = &[
    "assets/models/animation-library/Unreal-Godot/UAL1_Standard.glb",
    "assets/models/animation-library-two/Unreal-Godot/UAL2_Standard.glb",
];

/// Caller-owned cache for the rigged animation library. Kept by
/// gender since the bind-to-skeleton step is gender-specific (the
/// male and female base meshes have different joint counts /
/// orderings, so the bound clips can't be shared between them).
#[derive(Default)]
pub struct AnimLibraryCache {
    pub female: Option<AnimationSet>,
    pub male: Option<AnimationSet>,
}

impl AnimLibraryCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn slot_for(&mut self, gender: Gender) -> &mut Option<AnimationSet> {
        match gender {
            Gender::Female => &mut self.female,
            Gender::Male => &mut self.male,
        }
    }
}

/// Inputs that drive a character spawn. Kept as a small struct so
/// the call sites read like configuration rather than a wall of
/// positional parameters.
pub struct CharacterSpawn {
    pub position: Vec3,
    pub gender: Gender,
    /// Movement speed handed to the `Player` component (used by SP
    /// `player_input_system` for the local player; remote players
    /// don't read it but it's convenient to keep one shape).
    pub move_speed: f32,
    /// Maximum HP. Remote players' authoritative HP comes from the
    /// server snapshot, but we still want a sensible component
    /// value so health-bar code has something to read.
    pub max_hp: f32,
}

/// Spawn a fully-rigged character entity at `cfg.position` and
/// return its `Entity`. Callers attach role markers (`LocalPlayer`,
/// `RemotePlayer { net_id }`, etc.) to the returned entity as
/// appropriate.
///
/// On asset-load failure the function falls back to a procedural
/// cube so we don't crash mid-game; only the visual is degraded.
pub fn spawn_character_entity(
    world: &mut hecs::World,
    renderer: &mut Renderer,
    cache: &mut AnimLibraryCache,
    cosmetics: &mut AvatarCosmeticsCache,
    cfg: CharacterSpawn,
) -> anyhow::Result<hecs::Entity> {
    let (mesh_path, tex_path) = rift_game::hero::base_model_paths(cfg.gender);

    let (object_index, skinned_component) = match SkinnedMesh::from_gltf_filtered(
        mesh_path,
        |n| is_body_mesh_name(n),
    ) {
        Ok(skinned) => {
            let mut bind_mesh = Mesh::empty();
            bind_mesh.vertices = skinned.bind_vertices.clone();
            bind_mesh.indices = skinned.indices.clone();
            let idx = renderer
                .add_dynamic_mesh(&bind_mesh, Mat4::from_translation(cfg.position))?;
            if let Err(e) = renderer.set_object_texture(idx, tex_path) {
                log::warn!("Character texture load failed: {}", e);
            }
            let comp = Skinned {
                mesh: Arc::new(skinned),
                scratch: Vec::new(),
                joint_worlds: Vec::new(),
            };
            (idx, Some(comp))
        }
        Err(e) => {
            log::warn!("Falling back to procedural character mesh: {}", e);
            let cube = Mesh::cube();
            renderer.add_mesh(&cube, Mat4::from_translation(cfg.position))?;
            (renderer.objects.len() - 1, None)
        }
    };

    let entity = world.spawn((
        Transform::from_position(cfg.position),
        Velocity::default(),
        Player {
            speed: cfg.move_speed,
            aim_dir: Vec3::Z,
            spine_joint: u32::MAX,
            hand_joint: u32::MAX,
            action: rift_engine::ecs::components::PlayerAction::default(),
            action_timer: 0.0,
            vy: 0.0,
            airborne: false,
            vy_accum: 0.0,
        },
        Collider::new(0.3, 0.5, 0.3),
        Health::new(cfg.max_hp),
        Renderable { object_index },
    ));

    let Some(skinned) = skinned_component else {
        // Procedural-fallback path: no rig, no animation library.
        return Ok(entity);
    };

    // Resolve the animation library for this gender, loading from
    // disk on first use and caching the bound result for later
    // spawns of the same gender.
    let anim_set: Option<AnimationSet> = {
        if let Some(cached) = cache.slot_for(cfg.gender) {
            Some(cached.clone())
        } else {
            let mut set = AnimationSet::default();
            let mut loaded_any = false;
            for path in ANIM_LIBRARY_PATHS {
                match rift_engine::animation::Clip::load_all(path) {
                    Ok(clips) => {
                        for clip in &clips {
                            let bound = clip.bind_to_skeleton(
                                &skinned.mesh.joint_index_by_name,
                                skinned.mesh.joints.len(),
                            );
                            set.clips
                                .insert(clip.name.to_ascii_lowercase(), Arc::new(bound));
                        }
                        loaded_any = true;
                    }
                    Err(e) => {
                        log::warn!("Animation library {:?} failed to load: {}", path, e);
                    }
                }
            }
            if loaded_any {
                log::info!(
                    "Bound {} animation clip(s) to {:?} skeleton (cached)",
                    set.clips.len(),
                    cfg.gender,
                );
                let mut names: Vec<&String> = set.clips.keys().collect();
                names.sort();
                log::debug!("Animation clips: {:?}", names);
                *cache.slot_for(cfg.gender) = Some(set.clone());
                Some(set)
            } else {
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

    // Insert the heavy components after we've decided what to
    // attach. `insert_one` lets us add them piecewise without
    // moving the original `Player` tuple around.
    world.insert_one(entity, skinned).ok();
    if let Some(set) = anim_set {
        world.insert_one(entity, set).ok();
    }
    if let Some(anim) = animator {
        world.insert_one(entity, anim).ok();
    }

    // Read mask + spine joint back from the now-inserted Skinned
    // component so we don't have to thread it through.
    let (mask_opt, spine_idx, hand_idx): (Option<Vec<f32>>, Option<usize>, Option<usize>) =
        match world.get::<&Skinned>(entity) {
            Ok(s) => (
                Some(s.mesh.upper_body_mask()),
                s.mesh.spine_root_joint(),
                s.mesh.left_hand_joint(),
            ),
            Err(_) => (None, None, None),
        };
    if let Some(mask) = mask_opt {
        world
            .insert_one(entity, rift_engine::ecs::components::SpellCast::new(mask))
            .ok();
    }
    if let Some(idx) = spine_idx {
        if let Ok(mut p) = world.get::<&mut Player>(entity) {
            p.spine_joint = idx as u32;
        }
    }
    if let Some(idx) = hand_idx {
        if let Ok(mut p) = world.get::<&mut Player>(entity) {
            p.hand_joint = idx as u32;
        }
        log::debug!("hand joint resolved at index {}", idx);
    } else {
        log::warn!("no right-hand joint found in player skeleton; beam VFX will fall back to chest");
    }

    // Dress the avatar with white eyes, eyebrows, and per-gender
    // hair before returning. Idempotent + cached, so cheap on
    // every subsequent spawn of the same gender.
    avatar_cosmetics::apply_avatar_cosmetics(world, renderer, cosmetics, entity, cfg.gender);

    Ok(entity)
}
