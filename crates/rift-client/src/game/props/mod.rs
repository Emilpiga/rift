//! Static environment props — unified system.
//!
//! One [`PropAsset`] type describes any prop (fantasy dungeon, nature
//! hub, or future packs). One [`Props`] system owns the GPU material
//! caches and an [`AssetServer`] handle, and exposes a single
//! [`Props::spawn`] that handles AABB-driven floor lifting, optional
//! wall-snap, material binding, and collider creation.
//!
//! Per-pack content lives in submodules: each one provides a static
//! data table of `PropAsset`s plus a `decorate_*` placement function
//! that calls back into `Props::spawn`.
//!
//! Today's packs:
//! - [`fantasy`] — dungeon furniture / metal / cloth / small props,
//!   trim-sheet textured, scattered along arena/boss room walls.
//! - [`nature`] — outdoor trees, bushes, flowers, mushrooms, rocks,
//!   pebbles. Vertex-coloured (default sampler). Forest border +
//!   ground scatter for the hub.
//!
//! Adding a new pack: drop a `pub const ASSETS: &[PropAsset]` table
//! + a `decorate_*` function in a new submodule, register it in
//! `mod.rs`, and call from `floor.rs`.

pub mod fantasy;
pub mod nature;
pub mod placement;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use glam::{Mat4, Quat, Vec3};
use rift_engine::ash::{vk, Device};
use rift_engine::ecs::components::{Collider, Static, Transform};
use rift_engine::gpu_allocator::vulkan::Allocator;
use rift_engine::renderer::texture::Texture;
use rift_engine::{AssetServer, Renderer};

// ---------------------------------------------------------------------
// Prop description
// ---------------------------------------------------------------------

/// One renderable, optionally collidable, optionally textured prop.
///
/// All fields are `'static` so tables can live in `pub const` arrays.
#[derive(Clone, Copy, Debug)]
pub struct PropAsset {
    pub gltf: &'static str,
    pub scale: f32,
    pub material: Material,
    pub collider: ColliderShape,
    pub placement: PlacementHint,
    /// Selection weight for weighted-random picks (1 = base rate).
    pub weight: u32,
}

/// How the prop is shaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Material {
    /// Engine default 1×1 white sampler. Vertex colours from the gltf
    /// (baked at load time from baseColorTexture + factor) supply all
    /// the look. Used by the nature pack.
    Default,
    /// Bind a shared descriptor set sampled from `path`. The first
    /// prop that requests this path uploads the texture and caches
    /// the descriptor set; subsequent props share it.
    SharedTexture(&'static str),
}

/// What kind of static collider the prop spawns.
#[derive(Clone, Copy, Debug)]
pub enum ColliderShape {
    None,
    /// Static AABB matching the rotated mesh footprint, shrunk by
    /// `shrink` (0.85 = 15 % smaller) so the player can squeeze past.
    Aabb {
        shrink: f32,
    },
    /// Square XZ footprint of fixed half-extent (used for tree trunks
    /// where the visual canopy is wide but the trunk is thin).
    Trunk {
        half_extent: f32,
    },
}

/// Placement-time hint consumed by [`Props::spawn`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacementHint {
    /// Free-standing — `pos` is the world centre.
    Free,
    /// Push backward toward the adjacent wall so the prop's back face
    /// touches the inner wall surface. The wall direction is supplied
    /// at spawn time (caller knows from tile-edge analysis).
    WallAligned,
}

// ---------------------------------------------------------------------
// Prop system
// ---------------------------------------------------------------------

/// Centralised prop manager: mesh cache (via [`AssetServer`]),
/// shared-material descriptor sets, and the generic spawner.
///
/// One instance lives on `FloorManager`. Both fantasy and nature
/// placement loops drive it.
pub struct Props {
    assets: AssetServer,
    /// Shared descriptor sets keyed by texture path. Allocated lazily
    /// the first time a prop with `Material::SharedTexture(path)` spawns.
    material_sets: HashMap<&'static str, vk::DescriptorSet>,
    /// Owned textures backing those sets. Freed in [`Self::cleanup_gpu`].
    material_textures: Vec<Texture>,
}

impl Default for Props {
    fn default() -> Self {
        Self::new()
    }
}

impl Props {
    pub fn new() -> Self {
        Self {
            assets: AssetServer::global().clone(),
            material_sets: HashMap::new(),
            material_textures: Vec::new(),
        }
    }

    /// Borrow the shared asset cache. Useful for callers that want
    /// to issue a `load_mesh` directly (e.g. preload progress).
    pub fn assets(&self) -> &AssetServer {
        &self.assets
    }

    /// Incrementally load up to `budget` un-cached gltfs from `paths`.
    /// Returns how many were attempted this call. Idempotent.
    pub fn preload_step(&self, paths: &[&'static str], budget: usize) -> usize {
        let mut done = 0;
        for path in paths {
            if done >= budget {
                break;
            }
            if self.assets.mesh_attempted(path) {
                continue;
            }
            self.assets.load_mesh(path);
            done += 1;
        }
        done
    }

    /// How many of `paths` have already been attempted (success or fail).
    pub fn loaded_count(&self, paths: &[&'static str]) -> usize {
        paths
            .iter()
            .filter(|p| self.assets.mesh_attempted(p))
            .count()
    }

    /// Spawn one prop into the world.
    ///
    /// `tile_pos` is the world-space anchor (typically a tile centre).
    /// `yaw` is the Y rotation in radians.
    /// `wall_dir` is the (ox, oz) offset from the anchor tile to the
    /// adjacent wall tile; pass `(0, 0)` (or any value) when the
    /// asset's placement is `Free` — it's ignored.
    /// `scale_override` (if `Some`) replaces `asset.scale`; useful for
    /// per-instance random scale jitter.
    ///
    /// Returns the renderer object index on success, `None` if the
    /// mesh failed to load or the renderer rejected the upload.
    pub fn spawn(
        &mut self,
        world: &mut hecs::World,
        renderer: &mut Renderer,
        asset: &PropAsset,
        tile_pos: Vec3,
        yaw: f32,
        wall_dir: (i32, i32),
        scale_override: Option<f32>,
    ) -> Option<usize> {
        let mesh = self.assets.load_mesh(asset.gltf)?;
        let (mn, mx) = self.assets.mesh_bounds(asset.gltf)?;
        let s = scale_override.unwrap_or(asset.scale);

        // Local AABB after scale.
        let half_x = ((mx.x - mn.x) * 0.5 * s).max(0.05);
        let half_y = ((mx.y - mn.y) * 0.5 * s).max(0.05);
        let half_z = ((mx.z - mn.z) * 0.5 * s).max(0.05);
        let local_center = ((mn + mx) * 0.5) * s;

        // World-space half-extents after yaw rotation.
        let (sin_y, cos_y) = yaw.sin_cos();
        let world_half_x = (cos_y.abs() * half_x) + (sin_y.abs() * half_z);
        let world_half_z = (sin_y.abs() * half_x) + (cos_y.abs() * half_z);

        // Apply wall-snap if the asset asked for it.
        let mut pos = tile_pos;
        if matches!(asset.placement, PlacementHint::WallAligned) && wall_dir != (0, 0) {
            let (ox, oz) = wall_dir;
            let inner_wall_dist = 0.5; // wall face is 0.5 from tile centre
            let half_along = if ox != 0 { world_half_x } else { world_half_z };
            let push = (inner_wall_dist - half_along - 0.04).max(0.0);
            pos.x += ox as f32 * push;
            pos.z += oz as f32 * push;
        }
        // Lift so the lowest vertex sits on the *tile's*
        // ground plane, not world y=0. Callers
        // (`place_on_walls`, `place_in_room`, the centerpiece
        // spawners) sample [`Floor::tile_floor_y_at`] before
        // calling us so `tile_pos.y` already carries the
        // raised-dais / sunken-pit elevation. Adding the
        // model's bbox-min offset on top means the lowest
        // vertex sits *on the slab* at any elevation;
        // overwriting `pos.y` (the previous behaviour) lifted
        // every prop back to world y=0 and made dais props
        // float and pit props clip into the ground.
        pos.y = tile_pos.y - mn.y * s;

        // Compensate for the model's authored origin not matching its
        // bbox centre (otherwise the placement skews).
        let centre_offset = Vec3::new(
            cos_y * local_center.x + sin_y * local_center.z,
            0.0,
            -sin_y * local_center.x + cos_y * local_center.z,
        );
        let placement = pos - Vec3::new(centre_offset.x, 0.0, centre_offset.z);

        let model = Mat4::from_scale_rotation_translation(
            Vec3::splat(s),
            Quat::from_rotation_y(yaw),
            placement,
        );
        // For shared-texture props, the bound `baseColorMap`
        // already carries the full albedo. The static gltf
        // loader bakes the gltf's `baseColorFactor` into the
        // mesh's vertex colours (and *also* multiplies by the
        // base-colour texture *only* when the gltf references
        // it via URI — `.glb` files with the texture in a
        // buffer view fall back to factor-only). The forward
        // shader then computes `albedo = texture * fragColor`,
        // which double-tints the prop and reads as
        // implausibly dark whenever the gltf's factor is
        // anything below 1.0 (the chest model authors theirs
        // around 0.4–0.6, which is what made it look out of
        // place against the brighter SANDSTORM key + ambient).
        // Override the vertex colours to white so the bound
        // texture is the single source of truth for albedo.
        // Costs one extra GPU mesh upload per spawn — fine,
        // since the prop spawn path already re-uploads
        // geometry per-instance (no instancing yet).
        if matches!(asset.material, Material::SharedTexture(_)) {
            let mut whitened = rift_engine::Mesh {
                vertices: mesh.vertices.clone(),
                indices: mesh.indices.clone(),
            };
            for v in &mut whitened.vertices {
                v.color = glam::Vec3::ONE;
            }
            renderer.add_mesh(&whitened, model).ok()?;
        } else {
            renderer.add_mesh(mesh.as_ref(), model).ok()?;
        }
        let idx = renderer.objects.len() - 1;

        // Material binding.
        if let Material::SharedTexture(path) = asset.material {
            if let Some(ds) = self.ensure_material(renderer, path) {
                renderer.set_object_shared_material(idx, ds);
                // Stay on the legacy cel path (default flags
                // = 0). The PBR path divides diffuse by π,
                // which combined with the chest's already-
                // dark wood albedo produces a noticeably
                // darker prop than the surrounding sand
                // ground. The cel path multiplies through
                // unattenuated, so the same ambient + key
                // produces visibly more contribution per
                // sample on dark albedos. The skin / cloth /
                // leather classifier inside `evalCharLight`
                // weights out near-zero on warm wood (the
                // leather mask only fires for *dark* low-
                // chroma surfaces), so the chest falls
                // through to the default wrap-Lambert curve
                // and reads cleanly.
            }
        }

        // Collider.
        match asset.collider {
            ColliderShape::None => {}
            ColliderShape::Aabb { shrink } => {
                let collider_half = Vec3::new(
                    (world_half_x * shrink).max(0.10),
                    half_y.max(0.20),
                    (world_half_z * shrink).max(0.10),
                );
                let collider_pos = pos + Vec3::new(0.0, half_y, 0.0);
                world.spawn((
                    Transform::from_position(collider_pos),
                    Collider::new(collider_half.x, collider_half.y, collider_half.z),
                    Static,
                ));
            }
            ColliderShape::Trunk { half_extent } => {
                // Anchor the trunk collider on the tile's
                // ground plane (matches the visual placement
                // logic above) instead of world y=0, so trees
                // on raised hub borders collide where they
                // visibly stand.
                let collider_pos = Vec3::new(tile_pos.x, tile_pos.y + half_y, tile_pos.z);
                world.spawn((
                    Transform::from_position(collider_pos),
                    Collider::new(half_extent, half_y.max(0.20), half_extent),
                    Static,
                ));
            }
        }

        Some(idx)
    }

    /// Allocate (or fetch) a shared descriptor set for `path`.
    fn ensure_material(
        &mut self,
        renderer: &mut Renderer,
        path: &'static str,
    ) -> Option<vk::DescriptorSet> {
        if let Some(ds) = self.material_sets.get(path).copied() {
            return Some(ds);
        }
        let candidates = [
            std::path::PathBuf::from(path),
            std::path::PathBuf::from("..").join(path),
            std::path::PathBuf::from("../..").join(path),
            std::path::PathBuf::from("../../..").join(path),
        ];
        let resolved = candidates.iter().find(|p| p.exists()).cloned()?;
        let bytes = std::fs::read(&resolved)
            .map_err(|e| log::warn!("prop material read {:?}: {}", resolved, e))
            .ok()?;
        let (tex, ds) = renderer
            .upload_shared_texture(rift_engine::TextureSource::Bytes(&bytes))
            .map_err(|e| log::warn!("prop material upload {:?}: {}", resolved, e))
            .ok()?;
        self.material_textures.push(tex);
        self.material_sets.insert(path, ds);
        Some(ds)
    }

    /// Free GPU textures owned by this system. Call before the
    /// renderer's allocator drops.
    pub fn cleanup_gpu(&mut self, device: &Device, allocator: &Arc<Mutex<Allocator>>) {
        for mut tex in self.material_textures.drain(..) {
            tex.cleanup(device, allocator);
        }
        self.material_sets.clear();
    }
}
