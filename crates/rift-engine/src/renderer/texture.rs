use anyhow::Result;
use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};
use gpu_allocator::MemoryLocation;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Describes where the bytes for a single texture come from. Pass to
/// [`Texture::load`] (or the corresponding `Renderer` method) to upload
/// without picking a specific decoder yourself.
pub enum TextureSource<'a> {
    /// Decode a PNG/JPG file from disk as sRGB color data.
    File(&'a Path),
    /// Decode a PNG/JPG file from disk as linear (UNORM) data.
    /// Use for normal maps, MR atlases, AO, height — anything
    /// the shader reads as numbers, not color.
    FileLinear(&'a Path),
    /// Decode a PNG/JPG image from raw bytes as sRGB color
    /// (e.g. an image embedded in a glTF bufferView).
    Bytes(&'a [u8]),
    /// Upload raw RGBA8 sRGB pixels (e.g. procedurally generated).
    Rgba {
        width: u32,
        height: u32,
        pixels: &'a [u8],
    },
    /// Upload an image already decoded off-thread by
    /// [`crate::renderer::asset_decode`].
    Decoded(crate::renderer::asset_decode::DecodedTexture),
}

/// Describes a full PBR material source. Pass to
/// `Renderer::upload_shared_pbr_material` to bind every channel into a
/// single descriptor set; missing channels fall back to the material
/// pool's neutral defaults.
pub enum PbrSource<'a> {
    /// Disk-backed pack with a pre-merged metallic+roughness atlas.
    /// Color textures (basecolor) are decoded sRGB; data textures
    /// (normal/MR/AO/height) stay linear.
    Files {
        basecolor: &'a Path,
        normal: Option<&'a Path>,
        metallic_roughness: Option<&'a Path>,
        ao: Option<&'a Path>,
        height: Option<&'a Path>,
    },
    /// Disk-backed pack where metallic and roughness are separate
    /// single-channel PNGs (the convention most asset packs ship in).
    /// They're packed CPU-side into a single MR atlas before upload.
    FilesSplitMr {
        basecolor: &'a Path,
        normal: Option<&'a Path>,
        metallic: Option<&'a Path>,
        roughness: Option<&'a Path>,
        ao: Option<&'a Path>,
        height: Option<&'a Path>,
    },
    /// Pre-decoded pack from the off-thread asset pipeline; the
    /// metallic + roughness channels must already be merged into
    /// the `mr` atlas. The upload path does only GPU work.
    Decoded(crate::renderer::asset_decode::DecodedPbrPack),
}

pub struct Texture {
    pub image: vk::Image,
    pub view: vk::ImageView,
    pub sampler: vk::Sampler,
    pub allocation: Option<Allocation>,
    pub width: u32,
    pub height: u32,
}

impl Texture {
    /// Create a texture from raw RGBA8 pixel data, sampled as
    /// SRGB (the default for color textures). Equivalent to
    /// `from_rgba_with_format(.., R8G8B8A8_SRGB)`.
    pub fn from_rgba(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<Self> {
        Self::from_rgba_with_format(
            device,
            allocator,
            queue,
            command_pool,
            width,
            height,
            pixels,
            vk::Format::R8G8B8A8_SRGB,
        )
    }

    /// Create a texture from raw RGBA8 pixel data with an
    /// explicit Vulkan format. Use `R8G8B8A8_SRGB` for color
    /// inputs (basecolor / albedo) and `R8G8B8A8_UNORM` for
    /// data textures (normal maps, metallic / roughness / AO
    /// channels, height maps) so the GPU doesn't apply an
    /// sRGB → linear curve to non-color data.
    pub fn from_rgba_with_format(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        width: u32,
        height: u32,
        pixels: &[u8],
        format: vk::Format,
    ) -> Result<Self> {
        Self::from_rgba_with_format_address(
            device,
            allocator,
            queue,
            command_pool,
            width,
            height,
            pixels,
            format,
            vk::SamplerAddressMode::REPEAT,
        )
    }

    pub fn from_rgba_with_format_address(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        width: u32,
        height: u32,
        pixels: &[u8],
        format: vk::Format,
        address_mode: vk::SamplerAddressMode,
    ) -> Result<Self> {
        let image_size = (width * height * 4) as vk::DeviceSize;

        // Mip chain depth: floor(log2(max(w,h))) + 1. Without
        // a mip chain the bilinear sampler reads a single
        // texel per fragment regardless of how minified the
        // surface is on screen — which produces the shimmering
        // / "pixelated" aliasing the character mesh shows when
        // it covers a small fraction of the viewport. With
        // mips, the sampler picks the appropriate level for
        // the screen-space derivatives and the 16× anisotropy
        // filter actually has something to filter. Generated
        // online via `vkCmdBlitImage` from level N-1 → N. The
        // base-level data (level 0) is the buffer we just
        // staged.
        let mip_levels: u32 = (width.max(height) as f32).log2().floor() as u32 + 1;

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
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(mip_levels)
            .array_layers(1)
            .format(format)
            .tiling(vk::ImageTiling::OPTIMAL)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            // TRANSFER_SRC is required so we can blit from level
            // i-1 down to level i during mip generation.
            .usage(
                vk::ImageUsageFlags::TRANSFER_DST
                    | vk::ImageUsageFlags::TRANSFER_SRC
                    | vk::ImageUsageFlags::SAMPLED,
            )
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

        // Transition all mip levels UNDEFINED -> TRANSFER_DST. We
        // then write the base level from the staging buffer and
        // generate the rest by blitting down. The base needs the
        // DST layout for the buffer copy; the higher mips need
        // it for the blit destination side.
        let barrier = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: mip_levels,
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

        // Copy buffer to image (mip 0 only — the rest are filled
        // by the blit chain below).
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
            image_extent: vk::Extent3D {
                width,
                height,
                depth: 1,
            },
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

        // ── Generate mip chain via blits ─────────────────────
        // For each successive mip level: transition the source
        // (i-1) from TRANSFER_DST to TRANSFER_SRC, then blit it
        // halved into level i. The previous level then becomes
        // shader-read-ready as a side effect of the loop's last
        // transition (we batch the final move into the layout
        // pass below to keep barriers simple).
        let mut mip_w = width as i32;
        let mut mip_h = height as i32;
        for i in 1..mip_levels {
            let prev = i - 1;
            // src: TRANSFER_DST -> TRANSFER_SRC for mip `prev`.
            let to_src = vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: prev,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ);
            unsafe {
                device.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[to_src],
                );
            }

            let next_w = (mip_w / 2).max(1);
            let next_h = (mip_h / 2).max(1);
            let blit = vk::ImageBlit::default()
                .src_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: prev,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .src_offsets([
                    vk::Offset3D { x: 0, y: 0, z: 0 },
                    vk::Offset3D {
                        x: mip_w,
                        y: mip_h,
                        z: 1,
                    },
                ])
                .dst_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: i,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .dst_offsets([
                    vk::Offset3D { x: 0, y: 0, z: 0 },
                    vk::Offset3D {
                        x: next_w,
                        y: next_h,
                        z: 1,
                    },
                ]);
            unsafe {
                device.cmd_blit_image(
                    cmd,
                    image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[blit],
                    vk::Filter::LINEAR,
                );
            }

            mip_w = next_w;
            mip_h = next_h;
        }

        // Final layout: every mip ends up in SHADER_READ_ONLY.
        // Mips 0..mip_levels-1 are currently in TRANSFER_SRC
        // (they were promoted as blit sources); the last mip is
        // still in TRANSFER_DST (it was only ever a blit dest).
        // For a single-mip image (1×1 textures) the loop ran
        // zero times and the only mip is in TRANSFER_DST.
        let mut final_barriers: Vec<vk::ImageMemoryBarrier> = Vec::with_capacity(2);
        if mip_levels > 1 {
            final_barriers.push(
                vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(image)
                    .subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: mip_levels - 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    })
                    .src_access_mask(vk::AccessFlags::TRANSFER_READ)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ),
            );
        }
        final_barriers.push(
            vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: mip_levels - 1,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ),
        );

        unsafe {
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &final_barriers,
            );
        }

        end_single_command(device, command_pool, queue, cmd)?;

        // Clean up staging
        unsafe {
            device.destroy_buffer(staging_buffer, None);
        }
        allocator.lock().unwrap().free(staging_alloc)?;

        // Create image view
        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(format)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: mip_levels,
                base_array_layer: 0,
                layer_count: 1,
            });

        let view = unsafe { device.create_image_view(&view_info, None)? };

        // Create sampler
        let sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .address_mode_u(address_mode)
            .address_mode_v(address_mode)
            .address_mode_w(address_mode)
            .anisotropy_enable(true)
            .max_anisotropy(16.0)
            .border_color(vk::BorderColor::INT_OPAQUE_BLACK)
            .unnormalized_coordinates(false)
            .compare_enable(false)
            .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
            .mip_lod_bias(0.0)
            .min_lod(0.0)
            // Allow the sampler to walk all the way down the
            // generated mip chain. With this set the 16×
            // anisotropy filter has data to work with — without
            // it the previous `max_lod(0.0)` clamped the
            // sampler to mip 0 and bilinear minification
            // shimmered on small / oblique surfaces.
            .max_lod(mip_levels as f32);

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

    /// Create a single-channel R8_UNORM texture (e.g. for procgen
    /// alpha masks). Pixels are uploaded 1 byte per texel; the GPU
    /// samples them through the shader's `.r` swizzle.
    pub fn from_r8(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<Self> {
        let format = vk::Format::R8_UNORM;
        let image_size = (width * height) as vk::DeviceSize;

        let buffer_info = vk::BufferCreateInfo::default()
            .size(image_size)
            .usage(vk::BufferUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let staging_buffer = unsafe { device.create_buffer(&buffer_info, None)? };
        let staging_reqs = unsafe { device.get_buffer_memory_requirements(staging_buffer) };
        let staging_alloc = allocator.lock().unwrap().allocate(&AllocationCreateDesc {
            name: "r8_texture_staging",
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
        let dst = staging_alloc.mapped_slice().unwrap();
        unsafe {
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), dst.as_ptr() as *mut u8, pixels.len());
        }

        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .format(format)
            .tiling(vk::ImageTiling::OPTIMAL)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .samples(vk::SampleCountFlags::TYPE_1);
        let image = unsafe { device.create_image(&image_info, None)? };
        let mem_reqs = unsafe { device.get_image_memory_requirements(image) };
        let allocation = allocator.lock().unwrap().allocate(&AllocationCreateDesc {
            name: "r8_texture_image",
            requirements: mem_reqs,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })?;
        unsafe {
            device.bind_image_memory(image, allocation.memory(), allocation.offset())?;
        }

        let cmd = begin_single_command(device, command_pool)?;
        let to_dst = vk::ImageMemoryBarrier::default()
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
                &[to_dst],
            );
        }
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
            image_extent: vk::Extent3D {
                width,
                height,
                depth: 1,
            },
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
        let to_shader = vk::ImageMemoryBarrier::default()
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
                &[to_shader],
            );
        }
        end_single_command(device, command_pool, queue, cmd)?;
        unsafe {
            device.destroy_buffer(staging_buffer, None);
        }
        allocator.lock().unwrap().free(staging_alloc)?;

        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(format)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        let view = unsafe { device.create_image_view(&view_info, None)? };

        let sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .anisotropy_enable(false)
            .border_color(vk::BorderColor::INT_OPAQUE_BLACK)
            .unnormalized_coordinates(false)
            .compare_enable(false)
            .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
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
                    let n = a
                        .wrapping_mul(374761393)
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

    // ---- File/memory decoders ----------------------------------------
    //
    // These are the canonical entry points for "decode this image and
    // upload it to the GPU". Lower-level callers (procedural pixel
    // generation) hit `from_rgba` / `from_rgba_with_format` /
    // `from_r8` directly; off-thread asset pipelines hit `from_decoded`
    // with a `DecodedTexture` produced by `renderer::asset_decode`.
    //
    // Most call sites should prefer the unified [`Self::load`] entry
    // point with a [`TextureSource`] value — it dispatches to the
    // right `from_*` based on the variant.

    /// Unified texture-upload entry point. Dispatches to the
    /// appropriate `from_*` decoder based on the variant of `src`.
    /// All Renderer-facing texture methods funnel through this.
    pub fn load(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        src: TextureSource<'_>,
    ) -> Result<Self> {
        match src {
            TextureSource::File(p) => Self::from_file(device, allocator, queue, command_pool, p),
            TextureSource::FileLinear(p) => {
                Self::from_file_linear(device, allocator, queue, command_pool, p)
            }
            TextureSource::Bytes(b) => Self::from_memory(device, allocator, queue, command_pool, b),
            TextureSource::Rgba {
                width,
                height,
                pixels,
            } => Self::from_rgba(
                device,
                allocator,
                queue,
                command_pool,
                width,
                height,
                pixels,
            ),
            TextureSource::Decoded(d) => {
                Self::from_decoded(device, allocator, queue, command_pool, &d)
            }
        }
    }

    /// Decode a PNG/JPG file from disk and upload it as an SRGB
    /// RGBA8 texture (the right format for colour inputs). For
    /// linear data textures (normal, metallic/roughness, AO,
    /// height) use [`Self::from_file_linear`].
    pub fn from_file<P: AsRef<std::path::Path>>(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        path: P,
    ) -> Result<Self> {
        Self::from_file_with_format(
            device,
            allocator,
            queue,
            command_pool,
            path,
            vk::Format::R8G8B8A8_SRGB,
        )
    }

    /// Decode a PNG/JPG file from disk and upload it as a UNORM
    /// RGBA8 texture. Use this for non-colour data inputs (normal
    /// maps, packed metallic/roughness, AO, height) so the GPU
    /// doesn't apply an sRGB → linear curve to numeric data.
    pub fn from_file_linear<P: AsRef<std::path::Path>>(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        path: P,
    ) -> Result<Self> {
        Self::from_file_with_format_address(
            device,
            allocator,
            queue,
            command_pool,
            path,
            vk::Format::R8G8B8A8_UNORM,
            vk::SamplerAddressMode::REPEAT,
        )
    }

    /// Linear data file with clamped edges — for hybrid VFX cards
    /// where repeat would show seams at the billboard boundary.
    pub fn from_file_linear_clamp<P: AsRef<std::path::Path>>(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        path: P,
    ) -> Result<Self> {
        Self::from_file_with_format_address(
            device,
            allocator,
            queue,
            command_pool,
            path,
            vk::Format::R8G8B8A8_UNORM,
            vk::SamplerAddressMode::CLAMP_TO_EDGE,
        )
    }

    fn from_file_with_format<P: AsRef<std::path::Path>>(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        path: P,
        format: vk::Format,
    ) -> Result<Self> {
        Self::from_file_with_format_address(
            device,
            allocator,
            queue,
            command_pool,
            path,
            format,
            vk::SamplerAddressMode::REPEAT,
        )
    }

    fn from_file_with_format_address<P: AsRef<std::path::Path>>(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        path: P,
        format: vk::Format,
        address_mode: vk::SamplerAddressMode,
    ) -> Result<Self> {
        let original = path.as_ref();
        let resolved = crate::renderer::asset_decode::resolve_asset_path(original)
            .map_err(|e| anyhow::anyhow!("texture file not found: {:?}: {}", original, e))?;
        let img = image::open(&resolved)
            .map_err(|e| anyhow::anyhow!("texture decode failed for {:?}: {}", resolved, e))?
            .to_rgba8();
        let (w, h) = (img.width(), img.height());
        let pixels = img.into_raw();
        log::info!(
            "Loaded texture {:?}: {}x{} ({:?})",
            resolved.file_name().unwrap_or_default(),
            w,
            h,
            format,
        );
        Self::from_rgba_with_format_address(
            device,
            allocator,
            queue,
            command_pool,
            w,
            h,
            &pixels,
            format,
            address_mode,
        )
    }

    /// Decode a PNG/JPG image from an in-memory byte buffer (e.g.
    /// one extracted from an embedded glTF bufferView) and upload
    /// it as an SRGB RGBA8 texture.
    pub fn from_memory(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        bytes: &[u8],
    ) -> Result<Self> {
        let img = image::load_from_memory(bytes)
            .map_err(|e| anyhow::anyhow!("texture decode from memory failed: {}", e))?
            .to_rgba8();
        let (w, h) = (img.width(), img.height());
        let pixels = img.into_raw();
        Self::from_rgba(device, allocator, queue, command_pool, w, h, &pixels)
    }

    /// Upload an already-decoded RGBA8 buffer (produced by
    /// [`crate::renderer::asset_decode::decode_srgb`] or
    /// [`crate::renderer::asset_decode::decode_linear`] on a worker
    /// thread). Pairs with the off-thread decode helpers so callers
    /// can do the slow PNG work in the background and only touch
    /// Vulkan from the main thread.
    pub fn from_decoded(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        decoded: &crate::renderer::asset_decode::DecodedTexture,
    ) -> Result<Self> {
        Self::from_rgba_with_format(
            device,
            allocator,
            queue,
            command_pool,
            decoded.width,
            decoded.height,
            &decoded.pixels,
            decoded.format,
        )
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

fn begin_single_command(
    device: &ash::Device,
    command_pool: vk::CommandPool,
) -> Result<vk::CommandBuffer> {
    let alloc_info = vk::CommandBufferAllocateInfo::default()
        .command_pool(command_pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    let cmd = unsafe { device.allocate_command_buffers(&alloc_info)?[0] };
    let begin_info =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
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
