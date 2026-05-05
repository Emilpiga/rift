//! Per-object texture descriptor sets.
//!
//! The forward 3D pipeline uses two descriptor sets:
//! - **Set 0** — shared per-frame data (UBO + global default sampler), owned
//!   by `UniformBuffers`. Same set is bound for every draw.
//! - **Set 1** — per-object material (currently just one combined image
//!   sampler at binding 0), owned by `MaterialPool`. Each `RenderObject`
//!   that wants a custom texture allocates one of these.
//!
//! Objects without a custom texture share a single "default white" set
//! that's allocated once on pool init and produces an unmodified
//! per-vertex color (white * tint = tint).

use anyhow::Result;
use ash::vk;
use gpu_allocator::vulkan::Allocator;
use std::sync::{Arc, Mutex};

use super::texture::Texture;

/// Maximum number of distinct textured objects we expect to need at once.
/// Rough budget: 1 player + ~64 enemies + ~32 props.
const MAX_MATERIAL_SETS: u32 = 256;

pub struct MaterialPool {
    pub layout: vk::DescriptorSetLayout,
    pub pool: vk::DescriptorPool,
    /// Default 1×1 white texture used when an object has no custom material.
    pub default_texture: Texture,
    pub default_set: vk::DescriptorSet,
}

impl MaterialPool {
    pub fn new(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
    ) -> Result<Self> {
        // Descriptor set layout: just one combined image sampler at binding 0.
        let bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];
        let layout_info = vk::DescriptorSetLayoutCreateInfo::default()
            .bindings(&bindings);
        let layout = unsafe { device.create_descriptor_set_layout(&layout_info, None)? };

        // Pool large enough for many textured objects.
        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                descriptor_count: MAX_MATERIAL_SETS,
            },
        ];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(MAX_MATERIAL_SETS)
            .pool_sizes(&pool_sizes);
        let pool = unsafe { device.create_descriptor_pool(&pool_info, None)? };

        // Default white 1x1 RGBA texture for untextured objects.
        let default_texture = Texture::from_rgba(
            device, allocator, queue, command_pool, 1, 1, &[255, 255, 255, 255],
        )?;

        // Allocate the default set and bind the default texture.
        let layouts = [layout];
        let alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool)
            .set_layouts(&layouts);
        let default_set = unsafe { device.allocate_descriptor_sets(&alloc_info)?[0] };
        Self::write_set(device, default_set, &default_texture);

        Ok(Self { layout, pool, default_texture, default_set })
    }

    /// Allocate a new descriptor set bound to `texture`. Caller is
    /// responsible for keeping `texture` alive for the lifetime of the set.
    pub fn alloc_set(&self, device: &ash::Device, texture: &Texture) -> Result<vk::DescriptorSet> {
        let layouts = [self.layout];
        let alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(self.pool)
            .set_layouts(&layouts);
        let set = unsafe { device.allocate_descriptor_sets(&alloc_info)?[0] };
        Self::write_set(device, set, texture);
        Ok(set)
    }

    fn write_set(device: &ash::Device, set: vk::DescriptorSet, tex: &Texture) {
        let image_info = vk::DescriptorImageInfo {
            sampler: tex.sampler,
            image_view: tex.view,
            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        };
        let write = vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(std::slice::from_ref(&image_info));
        unsafe { device.update_descriptor_sets(&[write], &[]) };
    }

    pub fn cleanup(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        self.default_texture.cleanup(device, allocator);
        unsafe {
            // Destroying the pool implicitly frees all sets allocated from it.
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_descriptor_set_layout(self.layout, None);
        }
    }
}

/// Decode a PNG/JPG file from disk and upload it as an SRGB RGBA8 Texture.
pub fn load_texture_from_file<P: AsRef<std::path::Path>>(
    device: &ash::Device,
    allocator: &Arc<Mutex<Allocator>>,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    path: P,
) -> Result<Texture> {
    let original = path.as_ref().to_path_buf();
    let candidates = [
        original.clone(),
        std::path::PathBuf::from("..").join(&original),
        std::path::PathBuf::from("../..").join(&original),
        std::path::PathBuf::from("../../..").join(&original),
    ];
    let resolved = candidates.iter().find(|p| p.exists()).cloned()
        .ok_or_else(|| anyhow::anyhow!(
            "texture file not found in any candidate path (cwd={:?}): {:?}",
            std::env::current_dir().ok(), original
        ))?;
    let img = image::open(&resolved)
        .map_err(|e| anyhow::anyhow!("texture decode failed for {:?}: {}", resolved, e))?
        .to_rgba8();
    let (w, h) = (img.width(), img.height());
    let pixels = img.into_raw();
    log::info!("Loaded texture {:?}: {}x{}", resolved.file_name().unwrap_or_default(), w, h);
    Texture::from_rgba(device, allocator, queue, command_pool, w, h, &pixels)
}

/// Decode a PNG/JPG image from an in-memory byte buffer (e.g. one
/// extracted from an embedded glTF bufferView) and upload it as an
/// SRGB RGBA8 Texture.
pub fn load_texture_from_memory(
    device: &ash::Device,
    allocator: &Arc<Mutex<Allocator>>,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    bytes: &[u8],
) -> Result<Texture> {
    let img = image::load_from_memory(bytes)
        .map_err(|e| anyhow::anyhow!("texture decode from memory failed: {}", e))?
        .to_rgba8();
    let (w, h) = (img.width(), img.height());
    let pixels = img.into_raw();
    Texture::from_rgba(device, allocator, queue, command_pool, w, h, &pixels)
}
