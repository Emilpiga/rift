//! Rigid weapon-prop visuals: load the per-item glTF/GLB declared
//! by `BaseItem::models`, register it once as a dynamic mesh on
//! the renderer, and re-drive its `model_matrix` each frame from
//! the wielder's casting-hand joint (the same joint that anchors
//! channel beams and projectile spawn).
//!
//! This is the rigid sibling of [`super::equipment_visuals`].
//! Where equipment pieces are *skinned* meshes that ride the
//! host skeleton via joint remap + per-frame palette upload,
//! weapons are *props* with their own pivot — they don't deform,
//! they just follow the hand joint with full transform (position
//! + rotation + scale). Future weapons (sword, dagger, staff)
//! drop in by setting `models = Some(...)` on the appropriate
//! `BaseItem` row; nothing in this module is wand-specific.
//!
//! The Blender-authored material colour + texture comes through
//! the standard static-mesh path
//! ([`rift_engine::Mesh::from_gltf`]), which samples each
//! primitive's `base_color_factor × base_color_texture` at the
//! vertex UVs and bakes the result into `Vertex.color`. No
//! per-object texture binding is required (and none exists for
//! non-ground props today).

use std::collections::HashMap;
use std::sync::Arc;

use glam::Mat4;
use hecs::Entity;

use rift_engine::animation_profile::{JointKey, SkeletonBindings};
use rift_engine::ecs::components::{LocalPlayer, Player, Skinned, Transform};
use rift_engine::Input;
use rift_engine::Mesh;
use rift_engine::Renderer;

use rift_game::character::Gender;
use rift_game::loot::items::{BaseItem, EquipSlot};
use rift_game::loot::BASE_ITEMS;

use super::state::GameState;

/// Lazily-populated map from glTF/GLB path → bind-pose static mesh.
/// Decoded once per path; failures are remembered as a `None`
/// entry so we don't re-decode every frame.
///
/// Uses [`Mesh::from_gltf`] (not the skinned loader) so the
/// authored material — `base_color_factor` plus an optional
/// texture sampled at vertex UVs — is baked into the vertex
/// colour stream. That keeps the wand the colour the artist
/// intended without wiring per-object texture bindings.
#[derive(Default)]
pub struct WeaponMeshCache {
    entries: HashMap<String, Option<Arc<Mesh>>>,
}

impl WeaponMeshCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get-or-load the bind-pose mesh at `path`. Returns `None`
    /// on I/O / decode failure (logged once on first attempt).
    pub fn fetch(&mut self, path: &str) -> Option<Arc<Mesh>> {
        if let Some(entry) = self.entries.get(path) {
            return entry.clone();
        }
        let resolved = match Mesh::from_gltf(path) {
            Ok(m) => Some(Arc::new(m)),
            Err(e) => {
                log::warn!("weapon visual {:?} failed to load: {}", path, e);
                None
            }
        };
        self.entries.insert(path.to_string(), resolved.clone());
        resolved
    }
}

/// Per-entity rigid weapon attachment. One per wielder; replacing
/// the equipped weapon hides the previous renderer object (set
/// `visible = false` + write `Mat4::ZERO` as its `model_matrix`)
/// and registers a new one. We never remove dynamic-mesh objects
/// from the renderer so previously-used slots stay stable.
pub struct WeaponAttachment {
    /// Dynamic-mesh object index returned by
    /// [`Renderer::add_dynamic_mesh`].
    pub object_index: usize,
    /// Resolved joint index inside the host's `Skinned.mesh.joints`
    /// table. Sourced from `Player.hand_joint` (set at character
    /// spawn from `SkinnedMesh::left_hand_joint()`, which is the
    /// same hand the casting / channel-beam systems already
    /// anchor on).
    pub joint_idx: u32,
    /// Path of the glTF/GLB currently bound — used to detect
    /// weapon swaps and skip work when the path is unchanged.
    pub mesh_path: String,
    /// When false, [`update_weapon_transforms`] writes
    /// `Mat4::ZERO` to the renderer object so the prop disappears
    /// without re-allocating GPU buffers.
    pub visible: bool,
}

/// Reconcile a single avatar `entity`'s rigid weapon attachment
/// against `weapon_item`. Idempotent: re-applying with the same
/// item is a no-op (path match + visible = true).
///
/// `gender` selects which gendered model path to pull when the
/// item has separate male/female variants — for current weapons
/// both entries point at the same file, but the API matches the
/// rest of the visuals pipeline.
///
/// Takes a `&BaseItem` rather than an `&Item` because the visual
/// path only consults `base.models`; the rolled affixes are
/// irrelevant. That also lets the peer flow feed in a base id
/// without rehydrating a full `Item`.
pub fn apply_weapon_visual(
    world: &mut hecs::World,
    renderer: &mut Renderer,
    cache: &mut WeaponMeshCache,
    entity: Entity,
    weapon_base: Option<&BaseItem>,
    gender: Gender,
) {
    // Desired mesh path for this entity (None = no weapon, hide).
    let desired_path: Option<&'static str> = weapon_base
        .and_then(|base| base.models.as_ref())
        .and_then(|m| m.for_gender(gender));

    // Fast-path: existing attachment already targets the same
    // path. Just make sure it's visible (covers the
    // unequip-then-re-equip-same-weapon case).
    if let Some(path) = desired_path {
        let already_correct = world
            .get::<&WeaponAttachment>(entity)
            .map(|att| att.mesh_path == path && att.visible)
            .unwrap_or(false);
        if already_correct {
            return;
        }
    }

    // Resolve the hand-joint index for this entity. We prefer
    // `Player.hand_joint` (resolved at character spawn), falling
    // back to `Skinned.mesh.left_hand_joint()` for cases where
    // the player component is missing or its joint hasn't been
    // resolved yet.
    let joint_idx: Option<u32> = {
        let from_player = world
            .get::<&Player>(entity)
            .ok()
            .map(|p| p.hand_joint)
            .filter(|&j| j != u32::MAX);
        let from_bindings = world
            .get::<&SkeletonBindings>(entity)
            .ok()
            .and_then(|bindings| bindings.get(JointKey::WeaponHand));
        match from_bindings {
            Some(idx) => Some(idx),
            None => from_player.or_else(|| {
                world
                    .get::<&Skinned>(entity)
                    .ok()
                    .and_then(|s| s.mesh.left_hand_joint().map(|i| i as u32))
            }),
        }
    };

    // No weapon requested — hide any existing attachment.
    let Some(path) = desired_path else {
        if let Ok(mut att) = world.get::<&mut WeaponAttachment>(entity) {
            att.visible = false;
            if let Some(obj) = renderer.objects.get_mut(att.object_index) {
                obj.model_matrix = Mat4::ZERO;
            }
        }
        return;
    };

    let Some(joint_idx) = joint_idx else {
        // Skeleton not ready yet — caller will retry on the next
        // EquipmentSync dirty pass (the same retry path that
        // covers late local-avatar spawn for armour visuals).
        return;
    };

    // Load the mesh (cache fetch — decoded at most once).
    let Some(mesh) = cache.fetch(path) else {
        return;
    };

    // Hide whatever was previously bound to this entity, if any.
    // We leave the old renderer object in place (slot indices
    // need to stay stable for the rest of the engine); writing
    // `Mat4::ZERO` collapses its on-screen footprint.
    if let Ok(att) = world.get::<&WeaponAttachment>(entity) {
        if let Some(obj) = renderer.objects.get_mut(att.object_index) {
            obj.model_matrix = Mat4::ZERO;
        }
    }

    // Register the new dynamic mesh. Initial matrix is ZERO so
    // it's invisible for the one frame between registration and
    // the first `update_weapon_transforms` pass.
    let object_index = match renderer.add_dynamic_mesh(&mesh, Mat4::ZERO) {
        Ok(idx) => idx,
        Err(e) => {
            log::warn!("weapon visual: failed to register dynamic mesh: {}", e);
            return;
        }
    };

    let _ = world.insert_one(
        entity,
        WeaponAttachment {
            object_index,
            joint_idx,
            mesh_path: path.to_string(),
            visible: true,
        },
    );
}

/// Per-frame system: for every entity with a visible
/// [`WeaponAttachment`] + [`Skinned`] + [`Transform`], write
/// `host_transform * joint_worlds[joint_idx] * HAND_GRIP_OFFSET`
/// to the renderer's dynamic-mesh object so the weapon follows
/// the casting hand through walk / run / cast animations.
///
/// Must run *after* `skinning_system`, which is what refreshes
/// `Skinned.joint_worlds` from the current animation state.
///
/// The grip offset is a fixed correction between the hand joint's
/// rest pose and the artist-facing "grip at origin" convention.
/// It's intentionally shared across every weapon — tune it once
/// for the skeleton and every new weapon authored with the same
/// convention plugs in automatically.
pub fn update_weapon_transforms(world: &mut hecs::World, renderer: &mut Renderer, _input: &Input) {
    for (_entity, (att, skinned, transform)) in
        world.query_mut::<(&WeaponAttachment, &Skinned, &Transform)>()
    {
        let Some(obj) = renderer.objects.get_mut(att.object_index) else {
            continue;
        };
        if !att.visible {
            obj.model_matrix = Mat4::ZERO;
            continue;
        }
        let idx = att.joint_idx as usize;
        let Some(joint_local) = skinned.joint_worlds.get(idx) else {
            obj.model_matrix = Mat4::ZERO;
            continue;
        };
        obj.model_matrix = transform.matrix() * *joint_local * HAND_GRIP_OFFSET;
    }
}

/// Fixed correction between the runtime hand-joint rest pose and
/// the Blender authoring convention "grip vertex at world origin,
/// body extending forward". The character `.blend` we use as a
/// visual reference for weapon authoring has the hand sitting at a
/// slightly different vertical position than the runtime skeleton
/// expects, so every weapon comes out translated down by the same
/// fixed amount. We compensate once, here, rather than dragging
/// every weapon mesh up by hand.
///
/// The three components are joint-local: `LIFT` runs along the
/// hand bone toward the fingertips, `INWARD` is perpendicular
/// toward the palm centre, `UP` is perpendicular to both. Values
/// were dialled in interactively against the runtime rest pose
/// (formerly via a numpad live-tuner, since removed).
const HAND_GRIP_OFFSET: Mat4 = {
    const LIFT: f32 = 0.080;
    const INWARD: f32 = 0.020;
    const UP: f32 = 0.070;
    Mat4::from_cols(
        glam::Vec4::new(1.0, 0.0, 0.0, 0.0),
        glam::Vec4::new(0.0, 1.0, 0.0, 0.0),
        glam::Vec4::new(0.0, 0.0, 1.0, 0.0),
        glam::Vec4::new(INWARD, LIFT, UP, 1.0),
    )
};

/// Convenience: locate the `LocalPlayer` avatar and reconcile its
/// weapon visual against `state.loot.equipment[Weapon]`. Called
/// alongside [`super::equipment_visuals::apply_local_equipment_visuals`]
/// on every `EquipmentSync` drain.
pub fn apply_local_weapon_visual(state: &mut GameState, renderer: &mut Renderer) {
    let entity = {
        let mut q = state.world.query::<&LocalPlayer>();
        match q.iter().next() {
            Some((e, _)) => e,
            None => return,
        }
    };
    let gender = state.player_state.gender;
    let weapon_base = state
        .loot
        .equipment
        .get(EquipSlot::Weapon)
        .map(|it| it.base);
    apply_weapon_visual(
        &mut state.world,
        renderer,
        &mut state.weapon_visual_cache,
        entity,
        weapon_base,
        gender,
    );
}

/// Peer / preview variant: extract the weapon `Item` from a list
/// of base-item indices (the `PeerEquipmentVisuals` wire shape)
/// and apply it to `entity`. Looking up a fresh `Item` from the
/// base id is enough — the renderer path only uses `base.models`,
/// not the rolled affixes.
pub fn apply_weapon_visual_for_base_ids(
    world: &mut hecs::World,
    renderer: &mut Renderer,
    cache: &mut WeaponMeshCache,
    entity: Entity,
    base_ids: &[u16],
    gender: Gender,
) {
    let weapon_base: Option<&'static BaseItem> = base_ids
        .iter()
        .filter_map(|&bid| BASE_ITEMS.get(bid as usize))
        .find(|base| base.equip_slot == Some(EquipSlot::Weapon));
    apply_weapon_visual(world, renderer, cache, entity, weapon_base, gender);
}
