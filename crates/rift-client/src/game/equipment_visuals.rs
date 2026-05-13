//! Modular equipment visuals: load the per-item glTF mesh declared
//! by `BaseItem::model_path`, remap its joints onto the host
//! avatar's skeleton, and ride the existing `SkinnedAttachments`
//! pipeline so it skins with the rest of the body every frame.
//!
//! This module is the bridge between gameplay state (Equipment /
//! peer-equipment mirrors maintained in the binary) and the
//! engine-side rendering of those pieces. It is intentionally
//! agnostic about *whose* avatar is being dressed — both the local
//! `LocalPlayer` entity and every `RemotePlayer` entity go through
//! the same `apply_equipment_visuals` entry point.
//!
//! Loaded `SkinnedMesh`es are cached by path so a re-equip (or a
//! second player wearing the same chest) doesn't re-decode the glb.

use std::collections::HashMap;
use std::sync::Arc;

use glam::Mat4;
use hecs::Entity;

use rift_engine::ecs::components::{
    AttachmentPiece, LocalPlayer, Skinned, SkinnedAttachments, Transform,
};
use rift_engine::renderer::mesh::SkinnedMesh;
use rift_engine::Renderer;

use rift_game::character::Gender;
use rift_game::loot::{Equipment, BASE_ITEMS};

use super::state::GameState;

/// Process-wide cache of remapped attachment meshes, keyed by the
/// pair `(model_path, host_skeleton_signature)`. The signature is
/// just the host skeleton's joint count for now — both player
/// genders share the modular outfit pipeline so a single bind
/// matrix per path works in practice; this can be tightened to a
/// stronger fingerprint later if needed.
#[derive(Default)]
pub struct EquipmentVisualCache {
    /// (path, host joint count) -> remapped mesh.
    meshes: HashMap<(String, usize), Arc<SkinnedMesh>>,
}

impl EquipmentVisualCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load + remap the attachment mesh at `path` against the
    /// host skeleton described by `host_joint_names`. Returns
    /// `None` on I/O failure or when remap fails (joint name
    /// mismatch); caller should log and skip.
    fn fetch(
        &mut self,
        path: &str,
        host_joint_names: &HashMap<String, u16>,
        host_joint_count: usize,
    ) -> Option<Arc<SkinnedMesh>> {
        let key = (path.to_string(), host_joint_count);
        if let Some(m) = self.meshes.get(&key) {
            return Some(m.clone());
        }
        let mut mesh = match SkinnedMesh::from_gltf(path) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("equipment visual {:?} failed to load: {}", path, e);
                return None;
            }
        };
        if !mesh.remap_joint_indices_to(host_joint_names) {
            log::warn!(
                "equipment visual {:?}: joint remap failed (skeleton mismatch)",
                path
            );
            return None;
        }
        let arc = Arc::new(mesh);
        self.meshes.insert(key, arc.clone());
        Some(arc)
    }
}

/// Build the desired `(slot_byte, model_path)` set for an
/// `Equipment` mirror, picking the gender-specific mesh from
/// each item's `BaseItem::models`. Items without a model entry
/// (or without art for `gender`) are skipped.
///
/// The `Weapon` slot is intentionally excluded — weapons are
/// rigid props attached to the casting-hand joint by
/// [`super::weapon_visuals`], not skinned outfit pieces.
pub fn desired_visuals_for_equipment(equip: &Equipment, gender: Gender) -> Vec<(u8, &'static str)> {
    equip
        .iter()
        .filter(|(slot, _)| *slot != rift_game::loot::items::EquipSlot::Weapon)
        .filter_map(|(slot, item)| {
            item.base
                .models
                .as_ref()
                .and_then(|m| m.for_gender(gender))
                .map(|p| (slot.to_u8(), p))
        })
        .collect()
}

/// Build the desired set from a list of `BaseItem` indices
/// (the `PeerEquipmentVisuals` wire shape) for an avatar of
/// the given `gender`. Excludes the `Weapon` slot — see
/// [`desired_visuals_for_equipment`] for the rationale.
pub fn desired_visuals_for_base_ids(base_ids: &[u16], gender: Gender) -> Vec<(u8, &'static str)> {
    base_ids
        .iter()
        .filter_map(|&bid| {
            let base = BASE_ITEMS.get(bid as usize)?;
            // Weapons are wielded props, not skinned outfit
            // pieces \u2014 their visuals live in
            // `weapon_visuals`. Bag-only items
            // (`equip_slot == None`) never produce equipment
            // visuals either; the `?` on `equip_slot` filters
            // them out before the discriminator check.
            let slot = base.equip_slot?;
            if slot == rift_game::loot::items::EquipSlot::Weapon {
                return None;
            }
            base.models
                .as_ref()
                .and_then(|m| m.for_gender(gender))
                .map(|p| (slot.to_u8(), p))
        })
        .collect()
}

/// Reconcile the avatar `entity`'s `SkinnedAttachments` against
/// `desired` (one `(slot_byte, model_path)` per visible piece):
///   * pieces tagged with a slot no longer desired are hidden
///     (`visible=false`) so the skinning pass collapses their
///     renderer slot;
///   * pieces tagged with a slot whose model_path changed are
///     replaced (old hidden, new piece pushed);
///   * brand-new desired pieces load + remap + push.
///
/// We never delete pieces from the vector — keeping them around as
/// hidden entries means renderer slot indices stay stable for the
/// rest of the engine. Repeated equip churn on the same slot is
/// bounded by the number of distinct meshes for that slot.
pub fn apply_equipment_visuals(
    world: &mut hecs::World,
    renderer: &mut Renderer,
    cache: &mut EquipmentVisualCache,
    entity: Entity,
    desired: &[(u8, &'static str)],
) {
    // Pull what we need off the host's Skinned component up front
    // so we can release that borrow before mutating attachments.
    let (host_joint_names, host_joint_count) = match world.get::<&Skinned>(entity) {
        Ok(s) => (s.mesh.joint_index_by_name.clone(), s.mesh.joints.len()),
        Err(_) => {
            // No skinned base mesh — nothing to attach to. Quiet
            // skip: this happens for procedural-fallback avatars.
            return;
        }
    };
    let host_xform = world
        .get::<&Transform>(entity)
        .map(|t| t.matrix())
        .unwrap_or(Mat4::IDENTITY);

    // Resolve every desired piece *before* we take a mutable
    // borrow on the SkinnedAttachments component, so the cache
    // (which itself is borrowed mutably) doesn't fight the world
    // borrow.
    let mut resolved: Vec<(u8, Arc<SkinnedMesh>, &'static str)> = Vec::with_capacity(desired.len());
    for &(slot, path) in desired {
        if let Some(mesh) = cache.fetch(path, &host_joint_names, host_joint_count) {
            resolved.push((slot, mesh, path));
        }
    }

    // Ensure the entity has a SkinnedAttachments component to mutate.
    if world.get::<&SkinnedAttachments>(entity).is_err() {
        let _ = world.insert_one(entity, SkinnedAttachments::default());
    }

    // Phase 1: hide every existing piece whose tag isn't in
    // `desired`, or whose mesh pointer differs from the resolved
    // one. We collect tags-to-add after this pass so we can drop
    // the borrow before adding new dynamic meshes (which need
    // &mut Renderer + &mut World).
    let mut to_add: Vec<(u8, Arc<SkinnedMesh>)> = Vec::new();
    {
        let Ok(mut atts) = world.get::<&mut SkinnedAttachments>(entity) else {
            return;
        };

        // Fast-path: already up-to-date.
        for (slot, mesh, _path) in &resolved {
            let want_ptr = Arc::as_ptr(mesh);
            let mut found = false;
            for piece in atts.pieces.iter_mut() {
                if piece.tag == *slot as u32 {
                    let same_mesh = Arc::as_ptr(&piece.mesh) == want_ptr;
                    if same_mesh {
                        piece.visible = true;
                        found = true;
                    } else {
                        // Mesh changed for this slot; hide the old
                        // and queue the new.
                        piece.visible = false;
                    }
                }
            }
            if !found {
                to_add.push((*slot, mesh.clone()));
            }
        }

        // Hide pieces whose slot is no longer in `desired`.
        // Cosmetic pieces (eyes / hair, tagged with values >=
        // `COSMETIC_TAG_BASE`) live in the same attachment list
        // but are managed by `avatar_cosmetics`, so we must
        // skip them here — otherwise equipping any item would
        // hide the avatar's hair / eyes, and unequipping it
        // would bring them back, which was exactly the
        // observed bug.
        let desired_slots: Vec<u32> = resolved.iter().map(|(s, _, _)| *s as u32).collect();
        for piece in atts.pieces.iter_mut() {
            if piece.tag >= super::avatar_cosmetics::COSMETIC_TAG_BASE {
                continue;
            }
            if !desired_slots.contains(&piece.tag) {
                piece.visible = false;
            }
        }
    }

    // Phase 2: register new pieces with the renderer + push them.
    for (slot, mesh) in to_add {
        let object_index = match renderer.add_skinned_mesh(
            &mesh.bind_vertices,
            &mesh.vertex_skin,
            &mesh.indices,
            host_xform,
            0.022,
        ) {
            Ok(idx) => idx,
            Err(e) => {
                log::warn!("equipment visual: failed to register skinned mesh: {}", e);
                continue;
            }
        };
        if let Ok(mut atts) = world.get::<&mut SkinnedAttachments>(entity) {
            atts.pieces.push(AttachmentPiece {
                mesh,
                object_index,
                scratch: Vec::new(),
                visible: true,
                tag: slot as u32,
                // Equipment is clothing — needs the outward
                // inflate to clear the base skin without
                // z-fighting.
                inflate: true,
            });
        }
    }
}

/// Convenience: locate the `LocalPlayer` avatar entity inside
/// `state.world` and reconcile its attachments against the
/// authoritative `state.loot.equipment` mirror. Called from the
/// binary right after every `EquipmentSync` drain.
pub fn apply_local_equipment_visuals(state: &mut GameState, renderer: &mut Renderer) {
    let entity = {
        let mut q = state.world.query::<&LocalPlayer>();
        match q.iter().next() {
            Some((e, _)) => e,
            None => return,
        }
    };
    let gender = state.player_state.gender;
    let desired = desired_visuals_for_equipment(&state.loot.equipment, gender);
    apply_equipment_visuals(
        &mut state.world,
        renderer,
        &mut state.equipment_visual_cache,
        entity,
        &desired,
    );
}

/// True when the world contains exactly one `LocalPlayer`
/// avatar entity. Used by the binary's frame loop to gate the
/// "retry the deferred local-equipment apply" path: if the
/// avatar still doesn't exist, the dirty flag stays set for
/// next frame.
pub fn has_local_player(world: &hecs::World) -> bool {
    world.query::<&LocalPlayer>().iter().next().is_some()
}
