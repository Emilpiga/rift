//! Modular outfit visuals: load skinned outfit pieces (peasant + ranger sets
//! from the `modular-character-outfits` pack) as attachments that ride on
//! the player's bone palette, then toggle which ones are visible based on
//! current equipment. Pieces share the host skeleton, so they animate
//! perfectly with the body — no offsets or hand-tuned positioning needed.

use std::sync::Arc;

use rift_engine::ecs::components::{AttachmentPiece, SkinnedAttachments};
use rift_engine::loot::item::{ItemRarity, ItemSlot};
use rift_engine::loot::Equipment;
use rift_engine::renderer::mesh::SkinnedMesh;
use rift_engine::renderer::mesh::Mesh as PlainMesh;
use rift_engine::Renderer;

/// One outfit "set" in the modular pack. Each set ships a body, legs,
/// arms, and feet piece, plus optional accessories (hood, pauldrons).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OutfitSet {
    Peasant,
    Ranger,
}

impl OutfitSet {
    /// Pick an outfit set for an equipped item, based on rarity.
    fn for_rarity(r: ItemRarity) -> Self {
        match r {
            ItemRarity::Common | ItemRarity::Magic => OutfitSet::Peasant,
            _ => OutfitSet::Ranger,
        }
    }

    fn texture(self) -> &'static str {
        match self {
            OutfitSet::Peasant => {
                "assets/models/modular-character-outfits/Exports/glTF (Godot-Unreal)/Outfits/T_Peasant_BaseColor.png"
            }
            OutfitSet::Ranger => {
                "assets/models/modular-character-outfits/Exports/glTF (Godot-Unreal)/Outfits/T_Ranger_BaseColor.png"
            }
        }
    }
}

/// Logical attachment role — which body region each piece covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PieceKind {
    Body,
    Arms,
    Legs,
    Feet,
    Hood,
}

const ASSETS_DIR: &str =
    "assets/models/modular-character-outfits/Exports/glTF (Godot-Unreal)/Modular Parts";

fn piece_path(set: OutfitSet, kind: PieceKind) -> Option<String> {
    let name = match (set, kind) {
        (OutfitSet::Peasant, PieceKind::Body) => "Female_Peasant_Body.gltf",
        (OutfitSet::Peasant, PieceKind::Arms) => "Female_Peasant_Arms.gltf",
        (OutfitSet::Peasant, PieceKind::Legs) => "Female_Peasant_Legs.gltf",
        (OutfitSet::Peasant, PieceKind::Feet) => "Female_Peasant_Feet.gltf",
        (OutfitSet::Peasant, PieceKind::Hood) => return None, // no peasant hood
        (OutfitSet::Ranger, PieceKind::Body) => "Female_Ranger_Body.gltf",
        (OutfitSet::Ranger, PieceKind::Arms) => "Female_Ranger_Arms.gltf",
        (OutfitSet::Ranger, PieceKind::Legs) => "Female_Ranger_Legs.gltf",
        (OutfitSet::Ranger, PieceKind::Feet) => "Female_Ranger_Feet.gltf",
        (OutfitSet::Ranger, PieceKind::Hood) => "Female_Ranger_Head_Hood.gltf",
    };
    Some(format!("{ASSETS_DIR}/{name}"))
}

/// Map equipment slot → which piece kinds it controls.
fn slot_pieces(slot: ItemSlot) -> &'static [PieceKind] {
    match slot {
        // Equipping a chest swaps in the whole shirt+legs+arms look.
        ItemSlot::Chest => &[PieceKind::Body, PieceKind::Arms, PieceKind::Legs],
        ItemSlot::Boots => &[PieceKind::Feet],
        ItemSlot::Helmet => &[PieceKind::Hood],
        _ => &[],
    }
}

/// Lifecycle of a single outfit piece.
enum LoadState {
    /// Registered but not yet loaded.
    Pending,
    /// Loaded and registered with the renderer. `att_index` is the slot
    /// in `SkinnedAttachments::pieces`.
    Ready { att_index: usize },
    /// Load failed. We won't retry.
    Failed,
}

/// Index into `EquipmentVisuals::pieces`.
struct PieceIndex {
    set: OutfitSet,
    kind: PieceKind,
    state: LoadState,
}

pub struct EquipmentVisuals {
    pieces: Vec<PieceIndex>,
    /// Host skeleton's joint name → palette index. Captured at build
    /// time so lazy-loaded pieces can remap their skin without us
    /// having to thread the table through every call.
    host_joints: std::collections::HashMap<String, u16>,
    /// Shared per-set textures.  Each outfit set's BaseColor PNG is
    /// 4096×4096 — we used to load it once per piece (4 or 5 times
    /// per set!) which dominated the loading screen.  Now we upload
    /// the texture once and bind the resulting descriptor set on
    /// every piece of that outfit set.
    shared_sets: std::collections::HashMap<OutfitSet, rift_engine::ash::vk::DescriptorSet>,
    /// Owned shared textures (kept alive while the renderer holds
    /// references).  Dropped on `clear()` together with the renderer
    /// objects via `device_wait_idle`.
    shared_textures: Vec<rift_engine::renderer::texture::Texture>,
}

impl EquipmentVisuals {
    pub fn new() -> Self {
        Self {
            pieces: Vec::new(),
            host_joints: std::collections::HashMap::new(),
            shared_sets: std::collections::HashMap::new(),
            shared_textures: Vec::new(),
        }
    }

    /// Register every (set, kind) outfit slot we *might* show. Each
    /// piece is left in the `Pending` state until `step_load` actually
    /// loads it (one piece per call). This lets the engine's loading
    /// screen tick a progress bar while glTFs decode + upload, instead
    /// of doing all 9 in one stalling burst.
    pub fn build_attachments(
        &mut self,
        _renderer: &mut Renderer,
        host_joint_index_by_name: &std::collections::HashMap<String, u16>,
    ) -> SkinnedAttachments {
        self.pieces.clear();
        self.host_joints = host_joint_index_by_name.clone();
        let kinds = [
            PieceKind::Body,
            PieceKind::Arms,
            PieceKind::Legs,
            PieceKind::Feet,
            PieceKind::Hood,
        ];
        for set in [OutfitSet::Peasant, OutfitSet::Ranger] {
            for kind in kinds {
                if piece_path(set, kind).is_none() { continue }
                self.pieces.push(PieceIndex { set, kind, state: LoadState::Pending });
            }
        }
        SkinnedAttachments::default()
    }

    /// Total number of pieces registered (for progress reporting).
    pub fn total_pieces(&self) -> usize { self.pieces.len() }

    /// Number of pieces that have been resolved (loaded successfully or
    /// failed permanently). When this equals `total_pieces`, loading is
    /// complete.
    pub fn loaded_pieces(&self) -> usize {
        self.pieces.iter().filter(|p| !matches!(p.state, LoadState::Pending)).count()
    }

    /// Load the next pending piece into `atts`. Returns `Some((set, kind))`
    /// describing what was just loaded (for logging / labels), or `None`
    /// if no pending pieces remain.
    pub fn step_load(
        &mut self,
        renderer: &mut Renderer,
        atts: &mut SkinnedAttachments,
    ) -> Option<(OutfitSet, PieceKind)> {
        let idx = self.pieces.iter().position(|p| matches!(p.state, LoadState::Pending))?;
        let set = self.pieces[idx].set;
        let kind = self.pieces[idx].kind;
        let path = match piece_path(set, kind) {
            Some(p) => p,
            None => {
                self.pieces[idx].state = LoadState::Failed;
                return Some((set, kind));
            }
        };
        let mut mesh = match SkinnedMesh::from_gltf(&path) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("modular outfit load failed ({:?}): {}", path, e);
                self.pieces[idx].state = LoadState::Failed;
                return Some((set, kind));
            }
        };
        if !mesh.remap_joint_indices_to(&self.host_joints) {
            log::warn!("modular outfit {:?} joints don't match player skeleton", path);
            self.pieces[idx].state = LoadState::Failed;
            return Some((set, kind));
        }
        let mut bind_mesh = PlainMesh::empty();
        bind_mesh.vertices = mesh.bind_vertices.clone();
        bind_mesh.indices = mesh.indices.clone();
        let obj_idx = match renderer.add_dynamic_mesh(&bind_mesh, glam::Mat4::ZERO) {
            Ok(i) => i,
            Err(e) => {
                log::warn!("renderer.add_dynamic_mesh failed: {}", e);
                self.pieces[idx].state = LoadState::Failed;
                return Some((set, kind));
            }
        };
        // Bind the shared per-set texture, uploading it on first use
        // so we only decode and GPU-upload each 4K BaseColor PNG once
        // (instead of once per piece — that was the bulk of the
        // outfit loading time).
        if let Some(shared) = self.shared_set_for(set, renderer) {
            renderer.set_object_shared_material(obj_idx, shared);
        }
        let att_index = atts.pieces.len();
        atts.pieces.push(AttachmentPiece {
            mesh: Arc::new(mesh),
            object_index: obj_idx,
            scratch: Vec::new(),
            visible: false,
        });
        self.pieces[idx].state = LoadState::Ready { att_index };
        Some((set, kind))
    }

    /// Toggle visibility of every piece based on what's currently equipped.
    /// Always returns `false` for the (legacy) `hide_base` flag — we keep
    /// the base body rendered and rely on a small outward inflate of the
    /// outfit pieces during skinning to avoid z-fighting. This way any
    /// body parts the outfit doesn't cover (head, hands) stay visible.
    pub fn sync(
        &mut self,
        equipment: &Equipment,
        atts: &mut SkinnedAttachments,
        _renderer: &mut Renderer,
    ) -> bool {
        // Compute desired set per piece kind.
        let mut want: [(bool, OutfitSet); 5] = [(false, OutfitSet::Peasant); 5];
        let kind_index = |k: PieceKind| match k {
            PieceKind::Body => 0,
            PieceKind::Arms => 1,
            PieceKind::Legs => 2,
            PieceKind::Feet => 3,
            PieceKind::Hood => 4,
        };
        for slot in [ItemSlot::Chest, ItemSlot::Boots, ItemSlot::Helmet] {
            if let Some(item) = equipment.get(slot) {
                let mut set = OutfitSet::for_rarity(item.rarity);
                // The Peasant set has no hood mesh, so any helmet — even
                // a Common one — is visualized with the Ranger hood. This
                // keeps "I equipped a helmet" always producing a visible
                // change on the character.
                if slot == ItemSlot::Helmet { set = OutfitSet::Ranger; }
                for &k in slot_pieces(slot) {
                    want[kind_index(k)] = (true, set);
                }
            }
        }

        for i in 0..self.pieces.len() {
            let (wanted, wanted_set) = want[kind_index(self.pieces[i].kind)];
            let should_show = wanted && wanted_set == self.pieces[i].set;
            if let LoadState::Ready { att_index } = self.pieces[i].state {
                if let Some(piece) = atts.pieces.get_mut(att_index) {
                    piece.visible = should_show;
                }
            }
        }
        false
    }

    pub fn clear(&mut self) {
        self.pieces.clear();
    }

    /// Free GPU resources owned by the visuals (shared textures).  Must
    /// run before the renderer's allocator is dropped.  The renderer
    /// itself frees the per-object dynamic vertex buffers; we only
    /// own the shared texture handles.
    pub fn cleanup_gpu(
        &mut self,
        device: &rift_engine::ash::Device,
        allocator: &std::sync::Arc<std::sync::Mutex<rift_engine::gpu_allocator::vulkan::Allocator>>,
    ) {
        for mut tex in self.shared_textures.drain(..) {
            tex.cleanup(device, allocator);
        }
        self.shared_sets.clear();
    }

    /// Get-or-upload the shared descriptor set for an outfit set.
    fn shared_set_for(
        &mut self,
        set: OutfitSet,
        renderer: &mut Renderer,
    ) -> Option<rift_engine::ash::vk::DescriptorSet> {
        if let Some(s) = self.shared_sets.get(&set) {
            return Some(*s);
        }
        // Read the texture file once; resolve common parent prefixes
        // so it works regardless of cwd, then decode + upload.
        let path = set.texture();
        let candidates = [
            std::path::PathBuf::from(path),
            std::path::PathBuf::from("..").join(path),
            std::path::PathBuf::from("../..").join(path),
        ];
        let resolved = candidates.iter().find(|p| p.exists())?;
        let bytes = match std::fs::read(resolved) {
            Ok(b) => b,
            Err(e) => {
                log::warn!("outfit texture read failed for {:?}: {}", resolved, e);
                return None;
            }
        };
        match renderer.upload_shared_texture_from_bytes(&bytes) {
            Ok((tex, ds)) => {
                self.shared_textures.push(tex);
                self.shared_sets.insert(set, ds);
                Some(ds)
            }
            Err(e) => {
                log::warn!("outfit shared texture upload failed for {:?}: {}", path, e);
                None
            }
        }
    }
}
