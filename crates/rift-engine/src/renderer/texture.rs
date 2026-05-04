use anyhow::Result;
use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};
use gpu_allocator::MemoryLocation;
use std::sync::{Arc, Mutex};

pub struct Texture {
    pub image: vk::Image,
    pub view: vk::ImageView,
    pub sampler: vk::Sampler,
    pub allocation: Option<Allocation>,
    pub width: u32,
    pub height: u32,
}

impl Texture {
    /// Create a texture from raw RGBA8 pixel data.
    pub fn from_rgba(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<Self> {
        let image_size = (width * height * 4) as vk::DeviceSize;

        // Create staging buffer
        let buffer_info = vk::BufferCreateInfo::default()
            .size(image_size)
            .usage(vk::BufferUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let staging_buffer = unsafe { device.create_buffer(&buffer_info, None)? };
        let staging_reqs = unsafe { device.get_buffer_memory_requirements(staging_buffer) };

        let staging_alloc = allocator.lock().unwrap().allocate(&AllocationCreateDesc {
            name: "texture_staging",
            requirements: staging_reqs,
            location: MemoryLocation::CpuToGpu,
            linear: true,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })?;

        unsafe {
            device.bind_buffer_memory(
                staging_buffer,
                staging_alloc.memory(),
                staging_alloc.offset(),
            )?;
        }

        // Copy pixel data to staging
        let dst = staging_alloc.mapped_slice().unwrap();
        unsafe {
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), dst.as_ptr() as *mut u8, pixels.len());
        }

        // Create image
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .extent(vk::Extent3D { width, height, depth: 1 })
            .mip_levels(1)
            .array_layers(1)
            .format(vk::Format::R8G8B8A8_SRGB)
            .tiling(vk::ImageTiling::OPTIMAL)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .samples(vk::SampleCountFlags::TYPE_1);

        let image = unsafe { device.create_image(&image_info, None)? };
        let mem_reqs = unsafe { device.get_image_memory_requirements(image) };

        let allocation = allocator.lock().unwrap().allocate(&AllocationCreateDesc {
            name: "texture_image",
            requirements: mem_reqs,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })?;

        unsafe {
            device.bind_image_memory(image, allocation.memory(), allocation.offset())?;
        }

        // Transition + copy via command buffer
        let cmd = begin_single_command(device, command_pool)?;

        // Transition UNDEFINED -> TRANSFER_DST
        let barrier = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            })
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);

        unsafe {
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
        }

        // Copy buffer to image
        let region = vk::BufferImageCopy {
            buffer_offset: 0,
            buffer_row_length: 0,
            buffer_image_height: 0,
            image_subresource: vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            },
            image_offset: vk::Offset3D { x: 0, y: 0, z: 0 },
            image_extent: vk::Extent3D { width, height, depth: 1 },
        };

        unsafe {
            device.cmd_copy_buffer_to_image(
                cmd,
                staging_buffer,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[region],
            );
        }

        // Transition TRANSFER_DST -> SHADER_READ_ONLY
        let barrier = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            })
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ);

        unsafe {
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
        }

        end_single_command(device, command_pool, queue, cmd)?;

        // Clean up staging
        unsafe { device.destroy_buffer(staging_buffer, None); }
        allocator.lock().unwrap().free(staging_alloc)?;

        // Create image view
        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(vk::Format::R8G8B8A8_SRGB)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });

        let view = unsafe { device.create_image_view(&view_info, None)? };

        // Create sampler
        let sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .address_mode_u(vk::SamplerAddressMode::REPEAT)
            .address_mode_v(vk::SamplerAddressMode::REPEAT)
            .address_mode_w(vk::SamplerAddressMode::REPEAT)
            .anisotropy_enable(true)
            .max_anisotropy(16.0)
            .border_color(vk::BorderColor::INT_OPAQUE_BLACK)
            .unnormalized_coordinates(false)
            .compare_enable(false)
            .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
            .mip_lod_bias(0.0)
            .min_lod(0.0)
            .max_lod(0.0);

        let sampler = unsafe { device.create_sampler(&sampler_info, None)? };

        Ok(Self {
            image,
            view,
            sampler,
            allocation: Some(allocation),
            width,
            height,
        })
    }

    /// Generate a 64x64 checkerboard texture (white/gray).
    pub fn checkerboard(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
    ) -> Result<Self> {
        let size = 128u32;
        let mut pixels = vec![0u8; (size * size * 4) as usize];

        // Procedural stone brick texture
        for y in 0..size {
            for x in 0..size {
                let idx = ((y * size + x) * 4) as usize;

                // Brick pattern: offset every other row
                let brick_w = 32u32;
                let brick_h = 16u32;
                let mortar = 2u32;

                let row = y / brick_h;
                let offset = if row % 2 == 0 { 0 } else { brick_w / 2 };
                let lx = (x + offset) % brick_w;
                let ly = y % brick_h;

                // Mortar lines (dark grout between bricks)
                let is_mortar = lx < mortar || ly < mortar;

                // Pseudo-random noise for stone variation
                let hash = |a: u32, b: u32| -> u8 {
                    let n = a.wrapping_mul(374761393)
                        .wrapping_add(b.wrapping_mul(668265263))
                        .wrapping_add(row.wrapping_mul(1013904223));
                    let n = n ^ (n >> 13);
                    let n = n.wrapping_mul(1274126177);
                    let n = n ^ (n >> 16);
                    (n & 0xFF) as u8
                };

                // Per-brick color variation (seeded by brick cell)
                let brick_col = (x + offset) / brick_w;
                let brick_seed = hash(brick_col, row);
                let brick_variation = (brick_seed as f32 / 255.0) * 0.15;

                if is_mortar {
                    // Dark mortar
                    let m = 40u8;
                    pixels[idx] = m;
                    pixels[idx + 1] = m;
                    pixels[idx + 2] = m;
                } else {
                    // Stone base: warm gray with per-pixel noise
                    let noise = hash(x, y);
                    let noise_f = (noise as f32 / 255.0) * 0.12;
                    let base = 0.55 + brick_variation + noise_f;
                    let c = (base.clamp(0.0, 1.0) * 255.0) as u8;
                    // Slight warm tint
                    pixels[idx] = c;
                    pixels[idx + 1] = (c as f32 * 0.95) as u8;
                    pixels[idx + 2] = (c as f32 * 0.9) as u8;
                }
                pixels[idx + 3] = 255;
            }
        }
        Self::from_rgba(device, allocator, queue, command_pool, size, size, &pixels)
    }

    pub fn cleanup(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        unsafe {
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
        }
        if let Some(alloc) = self.allocation.take() {
            allocator.lock().unwrap().free(alloc).ok();
        }
    }
}

fn begin_single_command(device: &ash::Device, command_pool: vk::CommandPool) -> Result<vk::CommandBuffer> {
    let alloc_info = vk::CommandBufferAllocateInfo::default()
        .command_pool(command_pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    let cmd = unsafe { device.allocate_command_buffers(&alloc_info)?[0] };
    let begin_info = vk::CommandBufferBeginInfo::default()
        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    unsafe { device.begin_command_buffer(cmd, &begin_info)? };
    Ok(cmd)
}

fn end_single_command(
    device: &ash::Device,
    command_pool: vk::CommandPool,
    queue: vk::Queue,
    cmd: vk::CommandBuffer,
) -> Result<()> {
    unsafe {
        device.end_command_buffer(cmd)?;
        let submit_info = vk::SubmitInfo::default().command_buffers(std::slice::from_ref(&cmd));
        device.queue_submit(queue, &[submit_info], vk::Fence::null())?;
        device.queue_wait_idle(queue)?;
        device.free_command_buffers(command_pool, &[cmd]);
    }
    Ok(())
}
