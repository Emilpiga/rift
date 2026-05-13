//! Client-side prop rendering.
//!
//! Prop *placement* (which prop, where, what yaw, what scale)
//! is owned by `rift_dungeon::props_placement` and lives on
//! [`rift_dungeon::Floor::props`] — both the server and the
//! client read from there, so the player's authoritative
//! collider in `kinematic::integrate` always matches the
//! geometry the player can see.
//!
//! This module is the *visual* half: a [`Props`] resource
//! manager that owns the per-asset GPU caches (mesh + shared
//! material descriptor sets) and exposes
//! [`Props::render_floor`] to draw every prop on a floor in
//! one pass. Per-id render metadata (gltf path, material,
//! authored asset scale) lives in the static [`render_meta`]
//! table.
//!
//! `Props` does not write any ECS components. Prop colliders
//! are not ECS-visible at all post-refactor — the engine's
//! `collision_system` (which skipped the `NetControlled`
//! player anyway) no longer sees props, and that's the
//! correct authority split: server owns motion, dungeon owns
//! geometry.

pub mod render_meta;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use glam::{Mat4, Quat, Vec3};
use rift_dungeon::{props::PropId, Floor, PlacedProp};
use rift_engine::ash::{vk, Device};
use rift_engine::gpu_allocator::vulkan::Allocator;
use rift_engine::renderer::texture::Texture;
use rift_engine::{AssetServer, Renderer};

use render_meta::{render_meta, RenderMaterial};

/// Centralised prop GPU resource manager: mesh cache (via
/// [`AssetServer`]) and shared-material descriptor sets,
/// plus the rendering entry points the floor manager calls
/// after dungeon generation.
pub struct Props {
    assets: AssetServer,
    /// Shared descriptor sets keyed by texture path.
    /// Allocated lazily the first time a prop with
    /// [`RenderMaterial::SharedTexture`] renders.
    material_sets: HashMap<&'static str, vk::DescriptorSet>,
    /// Owned textures backing those sets. Freed in
    /// [`Self::cleanup_gpu`].
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

    /// Borrow the shared asset cache. Used by the torch
    /// system, which probes mesh bounds to size flame VFX
    /// and replicate the wall-snap math.
    pub fn assets(&self) -> &AssetServer {
        &self.assets
    }

    /// Incrementally load up to `budget` un-cached gltfs from
    /// `paths`. Returns how many were attempted this call.
    /// Idempotent.
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

    /// Render every [`PlacedProp`] on `floor` into the
    /// renderer. Idempotent against the placed list — the
    /// caller is responsible for having cleared renderer
    /// objects on regen (the existing `clear_objects` call
    /// in `floor::generate` / `floor::generate_hub` covers
    /// this).
    pub fn render_floor(&mut self, renderer: &mut Renderer, floor: &Floor) {
        for placed in &floor.props {
            self.render_one(renderer, placed);
        }
    }

    /// Render a single placed prop. Returns the renderer
    /// object index on success; used by the torch system,
    /// which spawns a candlestick visual independently of
    /// the floor's placed-prop list.
    pub fn render_one(&mut self, renderer: &mut Renderer, placed: &PlacedProp) -> Option<usize> {
        let rm = render_meta(placed.id);
        self.render_raw(
            renderer,
            rm.gltf,
            rm.material,
            placed.pos,
            placed.yaw,
            rm.asset_scale * placed.scale,
            placed.wall_dir,
        )
    }

    /// Low-level render entry used by `render_one` and the
    /// torch system. Computes wall-snap (when `wall_dir` is
    /// `Some`) and the bbox-centre offset using the gltf's
    /// runtime mesh bounds, then uploads the model matrix.
    pub fn render_raw(
        &mut self,
        renderer: &mut Renderer,
        gltf: &'static str,
        material: RenderMaterial,
        anchor: Vec3,
        yaw: f32,
        scale: f32,
        wall_dir: Option<(i8, i8)>,
    ) -> Option<usize> {
        let mesh = self.assets.load_mesh(gltf)?;
        let (mn, mx) = self.assets.mesh_bounds(gltf)?;

        // Local AABB after scale.
        let half_x = ((mx.x - mn.x) * 0.5 * scale).max(0.05);
        let half_z = ((mx.z - mn.z) * 0.5 * scale).max(0.05);
        let local_center = ((mn + mx) * 0.5) * scale;

        // World-space half-extents after yaw rotation.
        let (sin_y, cos_y) = yaw.sin_cos();
        let world_half_x = (cos_y.abs() * half_x) + (sin_y.abs() * half_z);
        let world_half_z = (sin_y.abs() * half_x) + (cos_y.abs() * half_z);

        let mut pos = anchor;
        // Wall snap: push the prop's back face flush with the
        // wall surface (4 cm air gap). Server-side placement
        // emits the tile-centre anchor + a wall_dir hint;
        // the actual snap distance depends on the gltf's
        // bounds, which only the client has.
        if let Some((ox, oz)) = wall_dir {
            let inner_wall_dist = 0.5;
            let half_along = if ox != 0 { world_half_x } else { world_half_z };
            let push = (inner_wall_dist - half_along - 0.04).max(0.0);
            pos.x += ox as f32 * push;
            pos.z += oz as f32 * push;
        }
        // Sit on the *anchor's* ground plane (set by the
        // dungeon to the tile floor Y for the tile under
        // the prop).
        pos.y = anchor.y - mn.y * scale;

        // Compensate for the model's authored origin not
        // matching its bbox centre (otherwise the placement
        // skews).
        let centre_offset = Vec3::new(
            cos_y * local_center.x + sin_y * local_center.z,
            0.0,
            -sin_y * local_center.x + cos_y * local_center.z,
        );
        let placement = pos - Vec3::new(centre_offset.x, 0.0, centre_offset.z);

        let model = Mat4::from_scale_rotation_translation(
            Vec3::splat(scale),
            Quat::from_rotation_y(yaw),
            placement,
        );

        // Whitening pass for shared-texture props (see the
        // long comment that used to live in `Props::spawn`):
        // the static gltf loader bakes baseColorFactor into
        // vertex colours when it can, the forward shader
        // multiplies texture * vertex_color, and authored
        // factors below 1.0 then double-tint the prop. Bound
        // textures already carry the full albedo so we
        // overwrite the vertex colours to white.
        if matches!(material, RenderMaterial::SharedTexture(_)) {
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

        if let RenderMaterial::SharedTexture(path) = material {
            if let Some(ds) = self.ensure_material(renderer, path) {
                renderer.set_object_shared_material(idx, ds);
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

    /// Free GPU textures owned by this system. Call before
    /// the renderer's allocator drops.
    pub fn cleanup_gpu(&mut self, device: &Device, allocator: &Arc<Mutex<Allocator>>) {
        for mut tex in self.material_textures.drain(..) {
            tex.cleanup(device, allocator);
        }
        self.material_sets.clear();
    }
}

/// Every gltf path the hub references — used by the preload
/// phase to stream assets in before generation. Mirrors the
/// catalog: hub uses grass + pebbles + the stash chest plus
/// the candlestick stand for torches.
pub fn hub_asset_paths() -> Vec<&'static str> {
    use PropId::*;
    [
        GrassCommonShort,
        GrassWispyShort,
        PebbleRound2,
        PebbleRound4,
        StashChest,
        CandleStickStand,
    ]
    .iter()
    .map(|id| render_meta(*id).gltf)
    .collect()
}

pub fn hub_total_assets() -> usize {
    hub_asset_paths().len()
}
