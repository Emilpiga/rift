use anyhow::Result;
use ash::vk;
use glam::{Mat4, Vec4};
use gpu_allocator::vulkan::Allocator;
use gpu_allocator::MemoryLocation;
use std::sync::{Arc, Mutex};

use crate::vulkan::buffer::GpuBuffer;
use crate::vulkan::sync::MAX_FRAMES_IN_FLIGHT;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct UniformData {
    pub view: Mat4,
    pub proj: Mat4,
    pub camera_pos: Vec4,  // xyz = position, w unused
    pub light_dir: Vec4,   // xyz = direction (toward light), w unused
    pub light_color: Vec4, // xyz = color, w = ambient intensity
    pub fog_color: Vec4,   // xyz = fog color, w unused
    pub fog_params: Vec4,  // x = start dist, y = end dist, z = density, w unused
    pub fog_origin: Vec4,  // xyz = world-space anchor for fog distance, w unused
    /// Point lights: [pos.xyz, radius] packed into vec4s, then [color.rgb, intensity].
    /// Capacity is `MAX_POINT_LIGHTS = 16`, sized to comfortably fit
    /// every torch within the fog radius (typical dungeon torch
    /// spacing × ~24 m visibility ≈ 8–12 torches at once).
    pub point_light_pos: [Vec4; 16], // xyz = position, w = radius
    pub point_light_color: [Vec4; 16], // xyz = color, w = intensity
    pub point_light_count: Vec4, // x = count (as float), yzw unused
    /// Directional-light view-projection matrix. Used by both the shadow
    /// pass (to project geometry) and the main pass (to sample the shadow
    /// map at each fragment's projected position).
    pub light_vp: Mat4,
    /// Per-face view-projection matrices for the point-light cube shadow
    /// atlas. Layout: `[light0 +X, -X, +Y, -Y, +Z, -Z, light1 +X, ...]`
    /// for `MAX_POINT_SHADOWS = 8` lights = 48 matrices. Filled by
    /// [`crate::renderer::shadow_point::cube_face_view_projs`] each
    /// frame for every active shadow-casting point light.
    pub point_shadow_face_vp: [Mat4; 48],
    /// x = number of point lights that currently have a shadow slot
    /// (0..=MAX_POINT_SHADOWS). The main fragment shader iterates over
    /// just this many entries when computing point-light occlusion.
    pub point_shadow_meta: Vec4,
    /// x = seconds since renderer start (used for blood-field aging
    /// and any future time-driven shader effects). yzw reserved.
    pub time: Vec4,
    /// Blood-field world-to-UV transform. xy = world-space origin
    /// (min XZ corner of the floor AABB), zw = inverse extent so that
    /// `uv = (worldXZ - origin) * invExtent` lands in [0, 1] across
    /// the field. All-zero when no blood field is active (the default
    /// 1×1 placeholder texture is bound and produces no contribution).
    pub blood_field_xform: Vec4,
    /// XZ AABB of the room the player is currently in:
    /// `(min_x, min_z, max_x, max_z)`. Read by the cel shader
    /// to gate the see-through-wall porthole so it only opens
    /// against walls of the player's current room. All-zero
    /// disables the porthole entirely.
    pub player_room_aabb: Vec4,
}

pub struct UniformBuffers {
    pub buffers: Vec<GpuBuffer>,
    pub descriptor_pool: vk::DescriptorPool,
    pub descriptor_sets: Vec<vk::DescriptorSet>,
    pub descriptor_set_layout: vk::DescriptorSetLayout,
}

impl UniformBuffers {
    pub fn new(device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) -> Result<Self> {
        let buffer_size = std::mem::size_of::<UniformData>() as vk::DeviceSize;

        // Create one uniform buffer per frame in flight
        let mut buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            let buffer = GpuBuffer::new(
                device,
                allocator,
                buffer_size,
                vk::BufferUsageFlags::UNIFORM_BUFFER,
                MemoryLocation::CpuToGpu,
                &format!("uniform_buffer_{}", i),
            )?;
            buffers.push(buffer);
        }

        // Descriptor set layout: binding 0 = UBO, binding 1 = legacy texture sampler,
        // binding 2 = directional shadow map (sampler2DShadow in the fragment shader),
        // binding 3 = point-light cube shadow atlas (samplerCubeArray),
        // binding 4 = per-floor blood field (R16G16_SFLOAT: R = wet intensity,
        // G = spawn time in seconds; sampled by the forward shader to composite
        // wet/dry blood onto floor fragments).
        let bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(2)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(3)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(4)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];

        let layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);

        let descriptor_set_layout =
            unsafe { device.create_descriptor_set_layout(&layout_info, None)? };

        // Descriptor pool
        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UNIFORM_BUFFER,
                descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                // 4 image samplers per set: legacy + directional shadow +
                // point-shadow cube atlas + blood field.
                descriptor_count: 4 * MAX_FRAMES_IN_FLIGHT as u32,
            },
        ];

        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(MAX_FRAMES_IN_FLIGHT as u32)
            .pool_sizes(&pool_sizes);

        let descriptor_pool = unsafe { device.create_descriptor_pool(&pool_info, None)? };

        // Allocate descriptor sets
        let layouts = vec![descriptor_set_layout; MAX_FRAMES_IN_FLIGHT];
        let alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(descriptor_pool)
            .set_layouts(&layouts);

        let descriptor_sets = unsafe { device.allocate_descriptor_sets(&alloc_info)? };

        // Update descriptor sets — UBO only (texture binding updated later)
        for (i, &set) in descriptor_sets.iter().enumerate() {
            let buffer_info = vk::DescriptorBufferInfo {
                buffer: buffers[i].buffer,
                offset: 0,
                range: buffer_size,
            };

            let write = vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .buffer_info(std::slice::from_ref(&buffer_info));

            unsafe { device.update_descriptor_sets(&[write], &[]) };
        }

        Ok(Self {
            buffers,
            descriptor_pool,
            descriptor_sets,
            descriptor_set_layout,
        })
    }

    /// Bind a texture to all descriptor sets at binding 1.
    pub fn bind_texture(&self, device: &ash::Device, view: vk::ImageView, sampler: vk::Sampler) {
        for &set in &self.descriptor_sets {
            let image_info = vk::DescriptorImageInfo {
                sampler,
                image_view: view,
                image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            };

            let write = vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(1)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(std::slice::from_ref(&image_info));

            unsafe { device.update_descriptor_sets(&[write], &[]) };
        }
    }

    /// Bind the shadow map (depth image + comparison sampler) to all
    /// descriptor sets at binding 2. Caller is responsible for keeping the
    /// view+sampler alive for the renderer's lifetime.
    pub fn bind_shadow_map(&self, device: &ash::Device, view: vk::ImageView, sampler: vk::Sampler) {
        for &set in &self.descriptor_sets {
            let image_info = vk::DescriptorImageInfo {
                sampler,
                image_view: view,
                image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            };
            let write = vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(2)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(std::slice::from_ref(&image_info));
            unsafe { device.update_descriptor_sets(&[write], &[]) };
        }
    }

    /// Bind the point-light cube shadow atlas (color image + linear
    /// sampler) to all descriptor sets at binding 3. Caller keeps the
    /// view+sampler alive for the renderer's lifetime.
    pub fn bind_point_shadow_atlas(
        &self,
        device: &ash::Device,
        view: vk::ImageView,
        sampler: vk::Sampler,
    ) {
        for &set in &self.descriptor_sets {
            let image_info = vk::DescriptorImageInfo {
                sampler,
                image_view: view,
                image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            };
            let write = vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(3)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(std::slice::from_ref(&image_info));
            unsafe { device.update_descriptor_sets(&[write], &[]) };
        }
    }

    /// Bind the blood-field texture (R16G16_SFLOAT) to all descriptor
    /// sets at binding 4. The renderer first installs a 1×1 zero
    /// placeholder at startup (so the descriptor is always valid), and
    /// re-binds a floor-sized field whenever a new floor is built.
    pub fn bind_blood_field(
        &self,
        device: &ash::Device,
        view: vk::ImageView,
        sampler: vk::Sampler,
    ) {
        for &set in &self.descriptor_sets {
            let image_info = vk::DescriptorImageInfo {
                sampler,
                image_view: view,
                image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            };
            let write = vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(4)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(std::slice::from_ref(&image_info));
            unsafe { device.update_descriptor_sets(&[write], &[]) };
        }
    }

    pub fn update(&mut self, frame: usize, data: &UniformData) {
        self.buffers[frame].write(std::slice::from_ref(data));
    }

    pub fn cleanup(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        for buf in &mut self.buffers {
            buf.cleanup(device, allocator);
        }
        unsafe {
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.descriptor_set_layout, None);
        }
    }
}
