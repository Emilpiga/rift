//! Per-object texture descriptor sets.
//!
//! The forward 3D pipeline uses two descriptor sets:
//! - **Set 0** — shared per-frame data (UBO + global default sampler), owned
//!   by `UniformBuffers`. Same set is bound for every draw.
//! - **Set 1** — per-object PBR material with five combined-image samplers:
//!   `binding 0 = baseColor`, `binding 1 = normal`, `binding 2 = metallic-
//!   roughness packed (R = metallic, G = roughness)`, `binding 3 = AO`,
//!   `binding 4 = height`. Owned by [`MaterialPool`]. Each `RenderObject`
//!   that wants a custom material allocates one of these.
//!
//! Objects without a custom material share the pool's `default_set`,
//! which binds a 1×1 white basecolor and 1×1 neutral fallbacks for
//! the data channels (flat normal, metallic 0 / roughness 1, AO 1,
//! mid-grey height). The shader's PBR path collapses to plain
//! Lambert when those neutral values are sampled, so legacy
//! single-texture objects keep their existing look.

use anyhow::Result;
use ash::vk;
use gpu_allocator::vulkan::Allocator;
use std::sync::{Arc, Mutex};

use super::texture::Texture;

/// Maximum number of distinct PBR material sets we expect to need
/// at once. Each set consumes 5 combined-image-sampler descriptors,
/// so the pool reserves `MAX_MATERIAL_SETS * 5` of them.
const MAX_MATERIAL_SETS: u32 = 256;

/// Per-binding slot ordering inside a PBR material set. Mirrored
/// in `assets/shaders/forward/common.glsl` — change in lockstep.
pub const BINDING_BASE_COLOR: u32 = 0;
pub const BINDING_NORMAL: u32 = 1;
pub const BINDING_METALLIC_ROUGHNESS: u32 = 2;
pub const BINDING_AO: u32 = 3;
pub const BINDING_HEIGHT: u32 = 4;

/// Number of texture bindings in a single PBR material set.
pub const PBR_BINDING_COUNT: u32 = 5;

pub struct MaterialPool {
    pub layout: vk::DescriptorSetLayout,
    pub pool: vk::DescriptorPool,
    /// Default 1×1 white texture used for the base-color slot of
    /// untextured objects.
    pub default_basecolor: Texture,
    /// Flat tangent-space normal `(0, 0, 1)` packed as
    /// `(0.5, 0.5, 1.0, 1.0)`. Stored UNORM so the GPU doesn't
    /// gamma-correct it.
    pub default_normal: Texture,
    /// Metallic-roughness packed `(R = metallic = 0, G =
    /// roughness = 1, B = unused, A = 1)`. Roughness 1 makes the
    /// PBR specular lobe collapse to a near-uniform glossy
    /// almost-Lambert response, matching the old diffuse-only
    /// look for legacy objects.
    pub default_mr: Texture,
    /// Ambient occlusion = 1.0 (no occlusion).
    pub default_ao: Texture,
    /// Height = 0.5 (mid-grey, the neutral parallax sample so
    /// the parallax UV offset is zero).
    pub default_height: Texture,
    pub default_set: vk::DescriptorSet,
}

impl MaterialPool {
    pub fn new(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
    ) -> Result<Self> {
        // Five combined image samplers per set. Layout binding
        // indices match the `BINDING_*` constants above.
        let bindings = [
            sampler_binding(BINDING_BASE_COLOR),
            sampler_binding(BINDING_NORMAL),
            sampler_binding(BINDING_METALLIC_ROUGHNESS),
            sampler_binding(BINDING_AO),
            sampler_binding(BINDING_HEIGHT),
        ];
        let layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        let layout = unsafe { device.create_descriptor_set_layout(&layout_info, None)? };

        // Pool large enough for `MAX_MATERIAL_SETS` PBR-bound
        // material sets, each consuming `PBR_BINDING_COUNT`
        // image-sampler descriptors.
        let pool_sizes = [vk::DescriptorPoolSize {
            ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
            descriptor_count: MAX_MATERIAL_SETS * PBR_BINDING_COUNT,
        }];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(MAX_MATERIAL_SETS)
            .pool_sizes(&pool_sizes);
        let pool = unsafe { device.create_descriptor_pool(&pool_info, None)? };

        // Allocate the neutral fallback textures. Color textures
        // (basecolor) go through SRGB; data textures (normal, MR,
        // AO, height) stay UNORM so the sampler doesn't apply a
        // gamma curve to non-color values.
        let default_basecolor = Texture::from_rgba(
            device,
            allocator,
            queue,
            command_pool,
            1,
            1,
            &[255, 255, 255, 255],
        )?;
        let default_normal = Texture::from_rgba_with_format(
            device,
            allocator,
            queue,
            command_pool,
            1,
            1,
            &[127, 127, 255, 255],
            vk::Format::R8G8B8A8_UNORM,
        )?;
        let default_mr = Texture::from_rgba_with_format(
            device,
            allocator,
            queue,
            command_pool,
            1,
            1,
            &[0, 255, 0, 255],
            vk::Format::R8G8B8A8_UNORM,
        )?;
        let default_ao = Texture::from_rgba_with_format(
            device,
            allocator,
            queue,
            command_pool,
            1,
            1,
            &[255, 255, 255, 255],
            vk::Format::R8G8B8A8_UNORM,
        )?;
        let default_height = Texture::from_rgba_with_format(
            device,
            allocator,
            queue,
            command_pool,
            1,
            1,
            &[127, 127, 127, 255],
            vk::Format::R8G8B8A8_UNORM,
        )?;

        let layouts = [layout];
        let alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool)
            .set_layouts(&layouts);
        let default_set = unsafe { device.allocate_descriptor_sets(&alloc_info)?[0] };
        write_pbr_set(
            device,
            default_set,
            &default_basecolor,
            &default_normal,
            &default_mr,
            &default_ao,
            &default_height,
        );

        Ok(Self {
            layout,
            pool,
            default_basecolor,
            default_normal,
            default_mr,
            default_ao,
            default_height,
            default_set,
        })
    }

    /// Allocate a material set whose only customised binding is
    /// the basecolor slot. The other four slots fall back to the
    /// pool's neutral defaults (flat normal, metallic 0 /
    /// roughness 1, AO 1, height 0.5) so the shader's PBR path
    /// collapses to plain Lambert. Caller must keep `texture`
    /// alive for as long as anything binds the returned set.
    pub fn alloc_set(&self, device: &ash::Device, texture: &Texture) -> Result<vk::DescriptorSet> {
        let layouts = [self.layout];
        let alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(self.pool)
            .set_layouts(&layouts);
        let set = unsafe { device.allocate_descriptor_sets(&alloc_info)?[0] };
        write_pbr_set(
            device,
            set,
            texture,
            &self.default_normal,
            &self.default_mr,
            &self.default_ao,
            &self.default_height,
        );
        Ok(set)
    }

    /// Allocate a fully customised PBR material set. Pass `None`
    /// for any channel the asset doesn't ship; the corresponding
    /// neutral fallback is bound in its place.
    pub fn alloc_pbr_set(
        &self,
        device: &ash::Device,
        basecolor: &Texture,
        normal: Option<&Texture>,
        metallic_roughness: Option<&Texture>,
        ao: Option<&Texture>,
        height: Option<&Texture>,
    ) -> Result<vk::DescriptorSet> {
        let layouts = [self.layout];
        let alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(self.pool)
            .set_layouts(&layouts);
        let set = unsafe { device.allocate_descriptor_sets(&alloc_info)?[0] };
        write_pbr_set(
            device,
            set,
            basecolor,
            normal.unwrap_or(&self.default_normal),
            metallic_roughness.unwrap_or(&self.default_mr),
            ao.unwrap_or(&self.default_ao),
            height.unwrap_or(&self.default_height),
        );
        Ok(set)
    }

    pub fn cleanup(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        self.default_basecolor.cleanup(device, allocator);
        self.default_normal.cleanup(device, allocator);
        self.default_mr.cleanup(device, allocator);
        self.default_ao.cleanup(device, allocator);
        self.default_height.cleanup(device, allocator);
        unsafe {
            // Destroying the pool implicitly frees all sets allocated from it.
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_descriptor_set_layout(self.layout, None);
        }
    }
}

fn sampler_binding(binding: u32) -> vk::DescriptorSetLayoutBinding<'static> {
    vk::DescriptorSetLayoutBinding::default()
        .binding(binding)
        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::FRAGMENT)
}

fn write_pbr_set(
    device: &ash::Device,
    set: vk::DescriptorSet,
    basecolor: &Texture,
    normal: &Texture,
    mr: &Texture,
    ao: &Texture,
    height: &Texture,
) {
    let infos = [
        image_info(basecolor),
        image_info(normal),
        image_info(mr),
        image_info(ao),
        image_info(height),
    ];
    let writes: [vk::WriteDescriptorSet; 5] = std::array::from_fn(|i| {
        vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(i as u32)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(std::slice::from_ref(&infos[i]))
    });
    unsafe { device.update_descriptor_sets(&writes, &[]) };
}

fn image_info(tex: &Texture) -> vk::DescriptorImageInfo {
    vk::DescriptorImageInfo {
        sampler: tex.sampler,
        image_view: tex.view,
        image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
    }
}
