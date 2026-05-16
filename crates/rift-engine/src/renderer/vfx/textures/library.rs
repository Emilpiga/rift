//! Loads and binds VFX textures for hybrid (authored + procedural) particles.

use anyhow::{Context, Result};
use ash::vk;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::renderer::asset_decode::resolve_asset_path;
use crate::renderer::passes::post::PostProcessing;
use crate::renderer::texture::Texture;
use crate::renderer::vfx::spec::{HybridMaterial, HybridProfile, HybridProfileKind};
use gpu_allocator::vulkan::Allocator;

/// Max textures in the particle shader `sampler2D vfxTextures[N]` array.
pub const MAX_VFX_TEXTURES: u32 = 8;

pub const SMOKE_BILLOW_PATH: &str = "assets/vfx/smoke_billow.png";

/// Stable GPU texture slot. Add variants here as art is authored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum VfxTextureId {
    SmokeBillow = 0,
}

impl VfxTextureId {
    pub const fn gpu_index(self) -> u32 {
        self as u32
    }
}

/// GPU-side pack of [`HybridProfile`] for instance + shader.
#[derive(Clone, Copy, Debug, Default)]
pub struct HybridProfileGpu {
    pub kind: u32,
    pub p0: f32,
    pub p1: f32,
    pub p2: f32,
    pub p3: f32,
}

impl HybridProfileGpu {
    pub fn from_profile(profile: &HybridProfile) -> Self {
        match profile {
            HybridProfile::TilingBillow {
                tile,
                flow_strength,
                ..
            } => Self {
                kind: HybridProfileKind::TilingBillow as u32,
                p0: *tile,
                p1: *flow_strength,
                p2: 0.0,
                p3: 0.0,
            },
            HybridProfile::Flipbook {
                cols,
                rows,
                ..
            } => Self {
                kind: HybridProfileKind::Flipbook as u32,
                p0: *cols as f32,
                p1: *rows as f32,
                p2: 0.0,
                p3: 0.0,
            },
        }
    }
}

/// Pack per-instance hybrid fields for [`crate::renderer::vfx::runtime::VfxParticleInstance`].
pub fn pack_hybrid_instance(material: &HybridMaterial) -> ([f32; 4], [f32; 4]) {
    let gpu = HybridProfileGpu::from_profile(&material.profile);
    let meta = [
        material.texture as f32,
        gpu.kind as f32,
        gpu.p0,
        gpu.p1,
    ];
    let params = match &material.profile {
        HybridProfile::TilingBillow { puff_footprint, .. } => [*puff_footprint, 0.0, 0.0, 0.0],
        HybridProfile::Flipbook {
            frame_start,
            frame_count,
            fps,
            looped,
            ..
        } => [
            *fps,
            *frame_start as f32,
            *frame_count as f32,
            if *looped { 1.0 } else { 0.0 },
        ],
    };
    (meta, params)
}

pub struct VfxTextureLibrary {
    loaded: HashMap<u32, Texture>,
    fallback: Texture,
}

impl VfxTextureLibrary {
    pub fn load(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
    ) -> Result<Self> {
        let fallback = Texture::from_rgba_with_format(
            device,
            allocator,
            queue,
            command_pool,
            1,
            1,
            &[255, 255, 255, 255],
            vk::Format::R8G8B8A8_UNORM,
        )?;

        let mut loaded = HashMap::new();
        loaded.insert(
            VfxTextureId::SmokeBillow.gpu_index(),
            load_smoke_billow(device, allocator, queue, command_pool)?,
        );

        Ok(Self { loaded, fallback })
    }

    fn texture_at(&self, slot: u32) -> &Texture {
        self.loaded
            .get(&slot)
            .unwrap_or(&self.fallback)
    }

    pub fn bind_translucent_descriptors(&self, device: &ash::Device, post: &PostProcessing) {
        let views: Vec<_> = (0..MAX_VFX_TEXTURES)
            .map(|i| self.texture_at(i).view)
            .collect();
        let sampler = self.texture_at(0).sampler;
        post.bind_vfx_texture_array(device, &views, sampler);
    }

    pub fn cleanup(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        for tex in self.loaded.values_mut() {
            tex.cleanup(device, allocator);
        }
        self.fallback.cleanup(device, allocator);
    }
}

fn load_smoke_billow(
    device: &ash::Device,
    allocator: &Arc<Mutex<Allocator>>,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
) -> Result<Texture> {
    let path = resolve_asset_path(Path::new(SMOKE_BILLOW_PATH)).with_context(|| {
        format!(
            "hybrid smoke billow texture not found at {SMOKE_BILLOW_PATH} — add a linear grayscale PNG"
        )
    })?;
    Texture::from_file_linear(device, allocator, queue, command_pool, path)
}
