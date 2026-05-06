//! Centralised asset cache.
//!
//! Today this owns:
//! - Decoded RGBA8 image blobs keyed by canonical filesystem path.
//!   Used by [`Mesh::from_gltf`] to bake base-colour textures into
//!   vertex colours without re-decoding the same PNG for every glTF
//!   that references it (e.g. all `CommonTree_*.gltf` share
//!   `Bark_NormalTree.png`).
//! - Static (non-skinned) [`Mesh`] objects keyed by glTF path,
//!   ref-counted via [`Arc`]. Lookup is `O(1)`; hits are free.
//!
//! Future steps will fold in the skinned-mesh, animation, and
//! GPU-texture caches that currently live as ad-hoc HashMaps inside
//! `PropLibrary`, `MonsterCache`, `EquipmentVisuals`, etc.
//!
//! Threading: every cache is wrapped in a [`Mutex`] so `AssetServer`
//! is `Sync` and can be shared between the main thread and (later)
//! a background loader worker. Lookups are short critical sections;
//! the actual decode happens with the lock released.
//!
//! Lifetime: the server is created by the engine at startup and
//! lives for the whole process. Cached image blobs are pure CPU
//! memory and are never freed (they're tiny relative to the GPU
//! upload they back). When the GPU-texture cache is added it will
//! gain an explicit `cleanup_gpu` like the existing ad-hoc caches.
//!
//! See `notes/assets.md` (TODO) for the full migration plan.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::renderer::mesh::Mesh;

/// One decoded RGBA8 image, ready for either CPU sampling (e.g. baking
/// vertex colours) or GPU upload.
#[derive(Debug)]
pub struct ImageData {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl ImageData {
    /// Sample at uv (wrap addressing). Returns linear-ish [0, 1]^3.
    pub fn sample(&self, uv: [f32; 2]) -> glam::Vec3 {
        if self.width == 0 || self.height == 0 {
            return glam::Vec3::ONE;
        }
        let u = uv[0] - uv[0].floor();
        let v = uv[1] - uv[1].floor();
        let x = ((u * self.width as f32) as u32).min(self.width - 1);
        let y = ((v * self.height as f32) as u32).min(self.height - 1);
        let i = ((y * self.width + x) * 4) as usize;
        glam::Vec3::new(
            self.pixels[i] as f32 / 255.0,
            self.pixels[i + 1] as f32 / 255.0,
            self.pixels[i + 2] as f32 / 255.0,
        )
    }
}

#[derive(Default)]
struct Caches {
    /// Decoded image blobs, keyed by canonical absolute path. `None`
    /// means a previous decode failed and we shouldn't retry.
    images: HashMap<PathBuf, Option<Arc<ImageData>>>,
    /// Static glTF meshes, keyed by the path string the caller used.
    /// `None` means a previous load failed.
    meshes: HashMap<String, Option<Arc<Mesh>>>,
    /// Local-space AABBs (min, max) for cached meshes. Populated
    /// alongside `meshes` so callers don't recompute on every spawn.
    bounds: HashMap<String, (glam::Vec3, glam::Vec3)>,
}

/// Global asset cache. Cheap to clone (`Arc` internally) and `Sync`.
#[derive(Clone, Default)]
pub struct AssetServer {
    caches: Arc<Mutex<Caches>>,
}

impl AssetServer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Process-wide singleton. Existing call sites that don't yet
    /// thread an `AssetServer` through can grab this for transparent
    /// dedup; new code should accept `&AssetServer` explicitly.
    pub fn global() -> &'static AssetServer {
        static GLOBAL: OnceLock<AssetServer> = OnceLock::new();
        GLOBAL.get_or_init(AssetServer::new)
    }

    /// Decode (or fetch from cache) the RGBA8 representation of `path`.
    /// `base_dir` is prepended if `path` is relative; the canonical
    /// absolute form is what we cache by.
    pub fn load_image(&self, base_dir: &Path, path: &str) -> Option<Arc<ImageData>> {
        let joined = base_dir.join(path);
        let key = std::fs::canonicalize(&joined).unwrap_or_else(|_| joined.clone());
        {
            let guard = self.caches.lock().unwrap();
            if let Some(entry) = guard.images.get(&key) {
                return entry.clone();
            }
        }
        let loaded = image::open(&joined)
            .map_err(|e| log::warn!("asset image decode {:?}: {}", joined, e))
            .ok()
            .map(|img| {
                let rgba = img.to_rgba8();
                Arc::new(ImageData {
                    width: rgba.width(),
                    height: rgba.height(),
                    pixels: rgba.into_raw(),
                })
            });
        self.caches
            .lock()
            .unwrap()
            .images
            .insert(key, loaded.clone());
        loaded
    }

    /// Load a static (non-skinned) glTF and return a ref-counted
    /// [`Mesh`]. Subsequent calls with the same `path` return the
    /// cached `Arc` for free.
    pub fn load_mesh(&self, path: &str) -> Option<Arc<Mesh>> {
        {
            let guard = self.caches.lock().unwrap();
            if let Some(entry) = guard.meshes.get(path) {
                return entry.clone();
            }
        }
        let loaded = match Mesh::from_gltf_with_assets(path, self) {
            Ok(m) => {
                let bounds = compute_aabb(&m);
                let arc = Arc::new(m);
                let mut guard = self.caches.lock().unwrap();
                guard.bounds.insert(path.to_string(), bounds);
                Some(arc)
            }
            Err(e) => {
                log::warn!("asset mesh load {}: {}", path, e);
                None
            }
        };
        self.caches
            .lock()
            .unwrap()
            .meshes
            .insert(path.to_string(), loaded.clone());
        loaded
    }

    /// True if a load for `path` has already been attempted (success
    /// or failure).
    pub fn mesh_attempted(&self, path: &str) -> bool {
        self.caches.lock().unwrap().meshes.contains_key(path)
    }

    /// Cached local-space AABB `(min, max)` for a previously loaded
    /// mesh. Returns `None` if the mesh hasn't been loaded yet or
    /// the load failed.
    pub fn mesh_bounds(&self, path: &str) -> Option<(glam::Vec3, glam::Vec3)> {
        self.caches.lock().unwrap().bounds.get(path).copied()
    }
}

fn compute_aabb(mesh: &Mesh) -> (glam::Vec3, glam::Vec3) {
    let mut mn = glam::Vec3::splat(f32::INFINITY);
    let mut mx = glam::Vec3::splat(f32::NEG_INFINITY);
    for v in &mesh.vertices {
        mn = mn.min(v.position);
        mx = mx.max(v.position);
    }
    if !mn.is_finite() {
        (glam::Vec3::ZERO, glam::Vec3::ZERO)
    } else {
        (mn, mx)
    }
}
