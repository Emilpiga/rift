//! Cache of bind-pose meshes for ground-loot 3D visuals.
//!
//! Loot models are authored as skinned glTF/GLB files (the same
//! art used to dress equipped pieces on the avatar — see
//! [`super::equipment_visuals`]). On the ground we don't animate
//! them; we just want the bind-pose silhouette so the player can
//! recognise the dropped item at a glance. This cache lazily
//! decodes each path on first encounter and hands out a shared
//! [`Mesh`] (vertices + indices, no skinning data) suitable for
//! [`Renderer::add_dynamic_mesh`].

use std::collections::HashMap;
use std::sync::Arc;

use rift_engine::renderer::mesh::SkinnedMesh;
use rift_engine::Mesh;

/// Lazily-populated map from glTF/GLB path → bind-pose mesh.
/// Decoded once per path (a re-drop of the same base item
/// re-uses the cached mesh). Failures are remembered as a `None`
/// entry so we don't re-attempt every frame.
#[derive(Default)]
pub struct LootModelCache {
    entries: HashMap<String, Option<Arc<LootModel>>>,
}

/// One decoded loot mesh: bind-pose geometry plus the bounding
/// info needed to size the on-ground "pop" animation. Texture
/// loading is deferred to a follow-up — for now the renderer's
/// rarity-tinted default material reads well enough.
pub struct LootModel {
    pub mesh: Arc<Mesh>,
    /// Half-extent of the bind-pose AABB along the longest
    /// axis. Used to pick a runtime scale that lands every
    /// dropped item at roughly the same on-ground footprint
    /// regardless of how the artist authored the source file.
    pub bounds_max_extent: f32,
    /// Min corner of the bind-pose AABB in mesh-local space.
    /// Used by the ground-loot transform to lift the model so
    /// its lowest point sits exactly at the configured rest
    /// height (no more half-buried rings) and to centre its
    /// XZ centroid under the loot beam (no more leaning chest
    /// pieces hanging off the spawn point).
    pub bounds_min: glam::Vec3,
    /// Max corner of the bind-pose AABB in mesh-local space.
    pub bounds_max: glam::Vec3,
}

impl LootModelCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get-or-load the bind-pose mesh for `path`. Returns
    /// `None` if the file failed to decode (logged once on
    /// the first attempt; subsequent calls are silent).
    pub fn fetch(&mut self, path: &str) -> Option<Arc<LootModel>> {
        if let Some(entry) = self.entries.get(path) {
            return entry.clone();
        }
        let resolved = match SkinnedMesh::from_gltf(path) {
            Ok(skinned) => Some(Arc::new(build_loot_model(&skinned))),
            Err(e) => {
                log::warn!("loot model {:?} failed to load: {}", path, e);
                None
            }
        };
        self.entries.insert(path.to_string(), resolved.clone());
        resolved
    }
}

fn build_loot_model(skinned: &SkinnedMesh) -> LootModel {
    let mesh = Mesh {
        vertices: skinned.bind_vertices.clone(),
        indices: skinned.indices.clone(),
    };
    // Longest half-extent of the bind-pose AABB. Used by the
    // spawner to normalise on-ground size: items authored at
    // character scale (~1.8m tall) and tiny accessories (a ring
    // ~0.05m) both land within the same visual footprint.
    let mut min = glam::Vec3::splat(f32::INFINITY);
    let mut max = glam::Vec3::splat(f32::NEG_INFINITY);
    for v in &mesh.vertices {
        min = min.min(v.position);
        max = max.max(v.position);
    }
    let extent = if mesh.vertices.is_empty() {
        1.0
    } else {
        (max - min).max_element() * 0.5
    };
    let (bounds_min, bounds_max) = if mesh.vertices.is_empty() {
        (glam::Vec3::ZERO, glam::Vec3::ZERO)
    } else {
        (min, max)
    };
    LootModel {
        mesh: Arc::new(mesh),
        bounds_max_extent: extent.max(0.01),
        bounds_min,
        bounds_max,
    }
}
