use anyhow::Result;
use ash::vk;
use bytemuck::{Pod, Zeroable};

use crate::hot_reload;
use crate::renderer::font::BitmapFont;
use crate::vulkan::buffer::{self, GpuBuffer};
use crate::vulkan::sync::MAX_FRAMES_IN_FLIGHT;

/// A 2D vertex for the overlay (screen-space NDC).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct OverlayVertex {
    pub position: [f32; 2], // NDC coords: -1..1
    pub color: [f32; 4],    // RGBA
    pub uv: [f32; 2],       // Texture coords (font atlas)
}

impl OverlayVertex {
    pub fn binding_description() -> vk::VertexInputBindingDescription {
        vk::VertexInputBindingDescription {
            binding: 0,
            stride: std::mem::size_of::<Self>() as u32,
            input_rate: vk::VertexInputRate::VERTEX,
        }
    }

    pub fn attribute_descriptions() -> [vk::VertexInputAttributeDescription; 3] {
        [
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 0,
                format: vk::Format::R32G32_SFLOAT,
                offset: 0,
            },
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 1,
                format: vk::Format::R32G32B32A32_SFLOAT,
                offset: 8,
            },
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 2,
                format: vk::Format::R32G32_SFLOAT,
                offset: 24,
            },
        ]
    }
}

/// A batch of overlay quads to draw this frame.
pub struct OverlayBatch {
    pub vertices: Vec<OverlayVertex>,
    pub indices: Vec<u32>,
    font: BitmapFont,
}

impl OverlayBatch {
    pub fn new() -> Self {
        Self {
            vertices: Vec::new(),
            indices: Vec::new(),
            font: BitmapFont::new(),
        }
    }

    pub fn clear(&mut self) {
        self.vertices.clear();
        self.indices.clear();
    }

    /// UV for the solid-white region of the atlas (top-left 1x1 pixel area).
    fn white_uv() -> [f32; 2] {
        // The font atlas has a solid white pixel at (0,0)
        [0.0, 0.0]
    }

    /// Add a filled rectangle. Coords in NDC (-1..1).
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
        let uv = Self::white_uv();
        let base = self.vertices.len() as u32;
        self.vertices.push(OverlayVertex { position: [x, y], color, uv });
        self.vertices.push(OverlayVertex { position: [x + w, y], color, uv });
        self.vertices.push(OverlayVertex { position: [x + w, y + h], color, uv });
        self.vertices.push(OverlayVertex { position: [x, y + h], color, uv });
        self.indices.extend_from_slice(&[
            base, base + 1, base + 2,
            base, base + 2, base + 3,
        ]);
    }

    /// Add a filled rect with pixel coordinates (top-left origin).
    pub fn rect_px(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 4], screen_w: f32, screen_h: f32) {
        let ndc_x = (x / screen_w) * 2.0 - 1.0;
        let ndc_y = (y / screen_h) * 2.0 - 1.0;
        let ndc_w = (w / screen_w) * 2.0;
        let ndc_h = (h / screen_h) * 2.0;
        self.rect(ndc_x, ndc_y, ndc_w, ndc_h, color);
    }

    /// Draw a text string at pixel position (top-left origin).
    /// Returns the width in pixels of the rendered text.
    pub fn text(&mut self, text: &str, x: f32, y: f32, size: f32, color: [f32; 4], screen_w: f32, screen_h: f32) -> f32 {
        let scale = size / self.font.glyph_height as f32;
        let mut cursor_x = x;

        for ch in text.chars() {
            if let Some(glyph) = self.font.glyph(ch) {
                let gw = self.font.glyph_width as f32 * scale;
                let gh = self.font.glyph_height as f32 * scale;

                // Convert pixel position to NDC
                let ndc_x = (cursor_x / screen_w) * 2.0 - 1.0;
                let ndc_y = (y / screen_h) * 2.0 - 1.0;
                let ndc_w = (gw / screen_w) * 2.0;
                let ndc_h = (gh / screen_h) * 2.0;

                let base = self.vertices.len() as u32;
                self.vertices.push(OverlayVertex { position: [ndc_x, ndc_y], color, uv: [glyph.u0, glyph.v0] });
                self.vertices.push(OverlayVertex { position: [ndc_x + ndc_w, ndc_y], color, uv: [glyph.u1, glyph.v0] });
                self.vertices.push(OverlayVertex { position: [ndc_x + ndc_w, ndc_y + ndc_h], color, uv: [glyph.u1, glyph.v1] });
                self.vertices.push(OverlayVertex { position: [ndc_x, ndc_y + ndc_h], color, uv: [glyph.u0, glyph.v1] });
                self.indices.extend_from_slice(&[
                    base, base + 1, base + 2,
                    base, base + 2, base + 3,
                ]);

                cursor_x += gw;
            } else {
                // Space or unknown — advance cursor
                cursor_x += self.font.glyph_width as f32 * scale;
            }
        }

        cursor_x - x
    }

    /// Measure text width in pixels without drawing.
    pub fn measure_text(&self, text: &str, size: f32) -> f32 {
        let scale = size / self.font.glyph_height as f32;
        text.len() as f32 * self.font.glyph_width as f32 * scale
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// Manages the overlay pipeline, font texture, and per-frame GPU buffers.
pub struct OverlayRenderer {
    pub pipeline: vk::Pipeline,
    pub pipeline_layout: vk::PipelineLayout,
    descriptor_set_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,
    font_image: vk::Image,
    font_image_view: vk::ImageView,
    font_sampler: vk::Sampler,
    font_allocation: Option<gpu_allocator::vulkan::Allocation>,
    vertex_buffers: Vec<Option<GpuBuffer>>,
    index_buffers: Vec<Option<GpuBuffer>>,
    index_counts: Vec<u32>,
}

impl OverlayRenderer {
    pub fn new(
        device: &ash::Device,
        allocator: &std::sync::Arc<std::sync::Mutex<gpu_allocator::vulkan::Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        render_pass: vk::RenderPass,
        extent: vk::Extent2D,
        shader_dir: &std::path::Path,
    ) -> Result<Self> {
        // Create font atlas texture
        let font = BitmapFont::new();
        let atlas_data = font.atlas_data();
        let atlas_w = font.atlas_width;
        let atlas_h = font.atlas_height;

        let (font_image, font_allocation) = Self::create_font_image(
            device, allocator, queue, command_pool, &atlas_data, atlas_w, atlas_h,
        )?;

        let font_image_view = Self::create_image_view(device, font_image)?;
        let font_sampler = Self::create_sampler(device)?;

        // Descriptor set for the font texture
        let descriptor_set_layout = Self::create_descriptor_set_layout(device)?;
        let descriptor_pool = Self::create_descriptor_pool(device)?;
        let descriptor_set = Self::allocate_descriptor_set(device, descriptor_pool, descriptor_set_layout)?;
        Self::update_descriptor_set(device, descriptor_set, font_image_view, font_sampler);

        let (pipeline, pipeline_layout) = Self::create_pipeline(
            device, render_pass, extent, descriptor_set_layout, shader_dir,
        )?;

        Ok(Self {
            pipeline,
            pipeline_layout,
            descriptor_set_layout,
            descriptor_pool,
            descriptor_set,
            font_image,
            font_image_view,
            font_sampler,
            font_allocation: Some(font_allocation),
            vertex_buffers: (0..MAX_FRAMES_IN_FLIGHT).map(|_| None).collect(),
            index_buffers: (0..MAX_FRAMES_IN_FLIGHT).map(|_| None).collect(),
            index_counts: vec![0; MAX_FRAMES_IN_FLIGHT],
        })
    }

    /// Upload overlay batch to GPU. Call once per frame before recording.
    pub fn upload(
        &mut self,
        frame: usize,
        device: &ash::Device,
        allocator: &std::sync::Arc<std::sync::Mutex<gpu_allocator::vulkan::Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        batch: &OverlayBatch,
    ) -> Result<()> {
        // Free old buffers in this frame slot. Safe because draw_frame waited
        // on this frame's fence before invoking upload.
        if let Some(mut vb) = self.vertex_buffers[frame].take() {
            vb.cleanup(device, allocator);
        }
        if let Some(mut ib) = self.index_buffers[frame].take() {
            ib.cleanup(device, allocator);
        }

        if batch.is_empty() {
            self.index_counts[frame] = 0;
            return Ok(());
        }

        self.vertex_buffers[frame] = Some(buffer::create_device_local_buffer(
            device, allocator, queue, command_pool,
            &batch.vertices,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            "overlay_vb",
        )?);

        self.index_buffers[frame] = Some(buffer::create_device_local_buffer(
            device, allocator, queue, command_pool,
            &batch.indices,
            vk::BufferUsageFlags::INDEX_BUFFER,
            "overlay_ib",
        )?);

        self.index_counts[frame] = batch.indices.len() as u32;
        Ok(())
    }

    /// Record overlay draw commands into the current render pass.
    pub fn record(&self, frame: usize, device: &ash::Device, cmd: vk::CommandBuffer) {
        let count = self.index_counts[frame];
        if count == 0 {
            return;
        }
        let vb = match &self.vertex_buffers[frame] {
            Some(b) => b.buffer,
            None => return,
        };
        let ib = match &self.index_buffers[frame] {
            Some(b) => b.buffer,
            None => return,
        };

        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[self.descriptor_set],
                &[],
            );
            device.cmd_bind_vertex_buffers(cmd, 0, &[vb], &[0]);
            device.cmd_bind_index_buffer(cmd, ib, 0, vk::IndexType::UINT32);
            device.cmd_draw_indexed(cmd, count, 1, 0, 0, 0);
        }
    }

    pub fn recreate_pipeline(
        &mut self,
        device: &ash::Device,
        render_pass: vk::RenderPass,
        extent: vk::Extent2D,
        shader_dir: &std::path::Path,
    ) -> Result<()> {
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
        }
        let (pipeline, layout) = Self::create_pipeline(
            device, render_pass, extent, self.descriptor_set_layout, shader_dir,
        )?;
        self.pipeline = pipeline;
        self.pipeline_layout = layout;
        Ok(())
    }

    fn create_pipeline(
        device: &ash::Device,
        render_pass: vk::RenderPass,
        extent: vk::Extent2D,
        descriptor_set_layout: vk::DescriptorSetLayout,
        shader_dir: &std::path::Path,
    ) -> Result<(vk::Pipeline, vk::PipelineLayout)> {
        let vert_src = std::fs::read_to_string(shader_dir.join("overlay.vert"))?;
        let frag_src = std::fs::read_to_string(shader_dir.join("overlay.frag"))?;

        let vert_spv = hot_reload::compile_glsl(&vert_src, "overlay.vert", shaderc::ShaderKind::Vertex)?;
        let frag_spv = hot_reload::compile_glsl(&frag_src, "overlay.frag", shaderc::ShaderKind::Fragment)?;

        let vert_module = crate::vulkan::pipeline::create_shader_module(device, &vert_spv)?;
        let frag_module = crate::vulkan::pipeline::create_shader_module(device, &frag_spv)?;

        let entry_name = c"main";
        let shader_stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vert_module)
                .name(entry_name),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(frag_module)
                .name(entry_name),
        ];

        let binding_desc = [OverlayVertex::binding_description()];
        let attr_descs = OverlayVertex::attribute_descriptions();

        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&binding_desc)
            .vertex_attribute_descriptions(&attr_descs);

        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);

        let viewport = vk::Viewport {
            x: 0.0, y: 0.0,
            width: extent.width as f32, height: extent.height as f32,
            min_depth: 0.0, max_depth: 1.0,
        };
        let scissor = vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent };
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewports(std::slice::from_ref(&viewport))
            .scissors(std::slice::from_ref(&scissor));

        let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .line_width(1.0)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE);

        let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);

        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(false)
            .depth_write_enable(false);

        let color_blend_attachment = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
            .alpha_blend_op(vk::BlendOp::ADD);

        let color_blending = vk::PipelineColorBlendStateCreateInfo::default()
            .attachments(std::slice::from_ref(&color_blend_attachment));

        let set_layouts = [descriptor_set_layout];
        let layout_info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&set_layouts);
        let pipeline_layout = unsafe { device.create_pipeline_layout(&layout_info, None)? };

        let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&shader_stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterizer)
            .multisample_state(&multisampling)
            .depth_stencil_state(&depth_stencil)
            .color_blend_state(&color_blending)
            .layout(pipeline_layout)
            .render_pass(render_pass)
            .subpass(0);

        let pipeline = unsafe {
            device
                .create_graphics_pipelines(vk::PipelineCache::null(), &[pipeline_info], None)
                .map_err(|(_, e)| e)?[0]
        };

        unsafe {
            device.destroy_shader_module(vert_module, None);
            device.destroy_shader_module(frag_module, None);
        }

        Ok((pipeline, pipeline_layout))
    }

    fn create_descriptor_set_layout(device: &ash::Device) -> Result<vk::DescriptorSetLayout> {
        let binding = vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT);

        let layout_info = vk::DescriptorSetLayoutCreateInfo::default()
            .bindings(std::slice::from_ref(&binding));

        let layout = unsafe { device.create_descriptor_set_layout(&layout_info, None)? };
        Ok(layout)
    }

    fn create_descriptor_pool(device: &ash::Device) -> Result<vk::DescriptorPool> {
        let pool_size = vk::DescriptorPoolSize {
            ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
            descriptor_count: 1,
        };
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .pool_sizes(std::slice::from_ref(&pool_size))
            .max_sets(1);

        let pool = unsafe { device.create_descriptor_pool(&pool_info, None)? };
        Ok(pool)
    }

    fn allocate_descriptor_set(
        device: &ash::Device,
        pool: vk::DescriptorPool,
        layout: vk::DescriptorSetLayout,
    ) -> Result<vk::DescriptorSet> {
        let layouts = [layout];
        let alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool)
            .set_layouts(&layouts);

        let sets = unsafe { device.allocate_descriptor_sets(&alloc_info)? };
        Ok(sets[0])
    }

    fn update_descriptor_set(
        device: &ash::Device,
        set: vk::DescriptorSet,
        image_view: vk::ImageView,
        sampler: vk::Sampler,
    ) {
        let image_info = vk::DescriptorImageInfo::default()
            .sampler(sampler)
            .image_view(image_view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);

        let write = vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(std::slice::from_ref(&image_info));

        unsafe { device.update_descriptor_sets(&[write], &[]); }
    }

    fn create_font_image(
        device: &ash::Device,
        allocator: &std::sync::Arc<std::sync::Mutex<gpu_allocator::vulkan::Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        data: &[u8],
        width: u32,
        height: u32,
    ) -> Result<(vk::Image, gpu_allocator::vulkan::Allocation)> {
        use gpu_allocator::vulkan::{AllocationCreateDesc, AllocationScheme};
        use gpu_allocator::MemoryLocation;

        // Create staging buffer
        let staging = buffer::create_host_buffer(
            device, allocator, data, vk::BufferUsageFlags::TRANSFER_SRC, "font_staging",
        )?;

        // Create image
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::R8_UNORM)
            .extent(vk::Extent3D { width, height, depth: 1 })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);

        let image = unsafe { device.create_image(&image_info, None)? };
        let reqs = unsafe { device.get_image_memory_requirements(image) };

        let allocation = allocator.lock().unwrap().allocate(&AllocationCreateDesc {
            name: "font_atlas",
            requirements: reqs,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })?;

        unsafe { device.bind_image_memory(image, allocation.memory(), allocation.offset())? };

        // Copy staging → image
        let cmd_buf = Self::begin_single_time_commands(device, command_pool)?;
        unsafe {
            // Transition to TRANSFER_DST
            let barrier = vk::ImageMemoryBarrier::default()
                .image(image)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0, level_count: 1,
                    base_array_layer: 0, layer_count: 1,
                });
            device.cmd_pipeline_barrier(
                cmd_buf,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[], &[], &[barrier],
            );

            // Copy
            let region = vk::BufferImageCopy::default()
                .image_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0, base_array_layer: 0, layer_count: 1,
                })
                .image_extent(vk::Extent3D { width, height, depth: 1 });
            device.cmd_copy_buffer_to_image(
                cmd_buf, staging.buffer, image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[region],
            );

            // Transition to SHADER_READ_ONLY
            let barrier = vk::ImageMemoryBarrier::default()
                .image(image)
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0, level_count: 1,
                    base_array_layer: 0, layer_count: 1,
                });
            device.cmd_pipeline_barrier(
                cmd_buf,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[], &[], &[barrier],
            );
        }
        Self::end_single_time_commands(device, command_pool, queue, cmd_buf)?;

        // Free staging buffer
        let mut staging = staging;
        staging.cleanup(device, allocator);

        Ok((image, allocation))
    }

    fn create_image_view(device: &ash::Device, image: vk::Image) -> Result<vk::ImageView> {
        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(vk::Format::R8_UNORM)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0, level_count: 1,
                base_array_layer: 0, layer_count: 1,
            });
        let view = unsafe { device.create_image_view(&view_info, None)? };
        Ok(view)
    }

    fn create_sampler(device: &ash::Device) -> Result<vk::Sampler> {
        let sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::NEAREST)
            .min_filter(vk::Filter::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST);
        let sampler = unsafe { device.create_sampler(&sampler_info, None)? };
        Ok(sampler)
    }

    fn begin_single_time_commands(device: &ash::Device, pool: vk::CommandPool) -> Result<vk::CommandBuffer> {
        let alloc_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let cmd = unsafe { device.allocate_command_buffers(&alloc_info)? }[0];
        let begin_info = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        unsafe { device.begin_command_buffer(cmd, &begin_info)? };
        Ok(cmd)
    }

    fn end_single_time_commands(
        device: &ash::Device, pool: vk::CommandPool, queue: vk::Queue, cmd: vk::CommandBuffer,
    ) -> Result<()> {
        unsafe {
            device.end_command_buffer(cmd)?;
            let submit_info = vk::SubmitInfo::default()
                .command_buffers(std::slice::from_ref(&cmd));
            device.queue_submit(queue, &[submit_info], vk::Fence::null())?;
            device.queue_wait_idle(queue)?;
            device.free_command_buffers(pool, &[cmd]);
        }
        Ok(())
    }

    fn free_buffers(
        &mut self,
        device: &ash::Device,
        allocator: &std::sync::Arc<std::sync::Mutex<gpu_allocator::vulkan::Allocator>>,
    ) {
        for slot in self.vertex_buffers.iter_mut() {
            if let Some(mut vb) = slot.take() {
                vb.cleanup(device, allocator);
            }
        }
        for slot in self.index_buffers.iter_mut() {
            if let Some(mut ib) = slot.take() {
                ib.cleanup(device, allocator);
            }
        }
    }

    pub fn cleanup(
        &mut self,
        device: &ash::Device,
        allocator: &std::sync::Arc<std::sync::Mutex<gpu_allocator::vulkan::Allocator>>,
    ) {
        self.free_buffers(device, allocator);
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.descriptor_set_layout, None);
            device.destroy_sampler(self.font_sampler, None);
            device.destroy_image_view(self.font_image_view, None);
            device.destroy_image(self.font_image, None);
        }
        if let Some(alloc) = self.font_allocation.take() {
            allocator.lock().unwrap().free(alloc).ok();
        }
    }
}
