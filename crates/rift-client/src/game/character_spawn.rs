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
use rift_engine::animation_profile::{
    AnimBindings, AnimClipKey, JointKey, SkeletonBindings, PLAYER_PROFILE,
};
use rift_engine::ecs::components::{
    AnimationSet, Collider, Health, Player, Renderable, Resource, Skinned, Transform, Velocity,
};
use rift_engine::renderer::mesh::SkinnedMesh;
use rift_engine::{Mesh, Renderer};

use rift_game::character::Gender;

use super::avatar_cosmetics::{self, is_body_mesh_name, AvatarCosmeticsCache};

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
        |node, mesh| is_body_mesh_name(node, mesh),
    ) {
        Ok(skinned) => {
            let idx = renderer.add_skinned_mesh(
                &skinned.bind_vertices,
                &skinned.vertex_skin,
                &skinned.indices,
                Mat4::from_translation(cfg.position),
                0.0,
            )?;
            if let Err(e) = renderer.set_object_texture(
                idx,
                rift_engine::TextureSource::File(std::path::Path::new(tex_path)),
            ) {
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
            foot_l_joint: u32::MAX,
            foot_r_joint: u32::MAX,
            action: rift_engine::ecs::components::PlayerAction::default(),
            action_timer: 0.0,
            vy: 0.0,
            airborne: false,
            vy_accum: 0.0,
            grounded_y: cfg.position.y,
        },
        Collider::new(0.3, 0.5, 0.3),
        Health::new(cfg.max_hp),
        // Placeholder essence pool. The bar code reads only
        // the ratio, and `world_sync` mirrors the snapshot's
        // `resource_pct` onto `current` each tick, so a
        // unit max is fine.
        Resource::new(1.0),
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
            // Root joint of the bound skeleton, used by the
            // strip pass. Joint with no parent in the rig.
            let root_joint_idx: Option<u16> = skinned
                .mesh
                .joints
                .iter()
                .position(|j| j.parent.is_none())
                .map(|i| i as u16);
            for path in ANIM_LIBRARY_PATHS {
                match rift_engine::animation::Clip::load_all(path) {
                    Ok(clips) => {
                        for clip in &clips {
                            let mut bound = clip.bind_to_skeleton(
                                &skinned.mesh.joint_index_by_name,
                                skinned.mesh.joints.len(),
                            );
                            // ARPG motion-ownership rule:
                            // attack clips are in-place, the
                            // kinematic owns forward lunge via
                            // [`rift_game::kinematic::ActionProfile::forward_step`].
                            // Strip baked translation so a
                            // clip authored with root motion
                            // (Mixamo / Synty default) doesn't
                            // double up with the code-side
                            // step or fight the locked
                            // `attack_dir`.
                            let lowered = clip.name.to_ascii_lowercase();
                            if let Some(root) = root_joint_idx {
                                if PLAYER_PROFILE.is_in_place_clip_name(&lowered) {
                                    bound.strip_root_translation(root);
                                    log::debug!(
                                        "stripped root motion from in-place clip '{}'",
                                        clip.name,
                                    );
                                }
                            }
                            set.clips.insert(lowered, Arc::new(bound));
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

    let anim_bindings = anim_set
        .as_ref()
        .map(|set| AnimBindings::resolve(PLAYER_PROFILE, set));
    let animator = anim_set
        .as_ref()
        .and_then(|set| {
            anim_bindings
                .as_ref()
                .and_then(|bindings| bindings.get(AnimClipKey::Idle))
                .or_else(|| {
                    set.find_any(PLAYER_PROFILE.names_for(AnimClipKey::Idle))
                        .or_else(|| set.clips.values().next().cloned())
                })
        })
        .map(rift_engine::animation::Animator::new);
    let skeleton_bindings = SkeletonBindings::resolve_player(&skinned.mesh);

    // Insert the heavy components after we've decided what to
    // attach. `insert_one` lets us add them piecewise without
    // moving the original `Player` tuple around.
    world.insert_one(entity, skinned).ok();
    if let Some(set) = anim_set {
        world.insert_one(entity, set).ok();
    }
    if let Some(bindings) = anim_bindings {
        world.insert_one(entity, bindings).ok();
    }
    if let Some(anim) = animator {
        world.insert_one(entity, anim).ok();
    }

    if !skeleton_bindings.upper_body_mask.is_empty() {
        world
            .insert_one(
                entity,
                rift_engine::ecs::components::SpellCast::new_with_axis(
                    skeleton_bindings.upper_body_mask.clone(),
                    skeleton_bindings.yaw_only_mask.clone(),
                ),
            )
            .ok();
    }
    if let Some(idx) = skeleton_bindings.get(JointKey::SpineRoot) {
        if let Ok(mut p) = world.get::<&mut Player>(entity) {
            p.spine_joint = idx;
        }
    }
    if let Some(idx) = skeleton_bindings.get(JointKey::CastHand) {
        if let Ok(mut p) = world.get::<&mut Player>(entity) {
            p.hand_joint = idx;
        }
        log::debug!("hand joint resolved at index {}", idx);
    } else {
        log::warn!(
            "no right-hand joint found in player skeleton; beam VFX will fall back to chest"
        );
    }

    // Foot joints — used by the terrain-pitch + foot-IK pass.
    let foot_pair = (
        skeleton_bindings.get(JointKey::LeftFoot),
        skeleton_bindings.get(JointKey::RightFoot),
    );
    if let Ok(mut p) = world.get::<&mut Player>(entity) {
        if let Some(li) = foot_pair.0 {
            p.foot_l_joint = li;
        }
        if let Some(ri) = foot_pair.1 {
            p.foot_r_joint = ri;
        }
    }
    if foot_pair.0.is_none() || foot_pair.1.is_none() {
        log::warn!(
            "foot joints partially unresolved (l={:?}, r={:?}); foot IK on ramps will be skipped",
            foot_pair.0,
            foot_pair.1,
        );
    }
    world.insert_one(entity, skeleton_bindings).ok();
    // Persistent foot-IK smoothing state. Attached unconditionally
    // — the IK pass is no-op when the dungeon floor handle isn't
    // available (hub, menu) or when foot joints didn't resolve.
    world
        .insert_one(entity, rift_engine::ecs::components::FootIkState::default())
        .ok();

    // Dress the avatar with white eyes, eyebrows, and per-gender
    // hair before returning. Idempotent + cached, so cheap on
    // every subsequent spawn of the same gender.
    avatar_cosmetics::apply_avatar_cosmetics(world, renderer, cosmetics, entity, cfg.gender);

    Ok(entity)
}
