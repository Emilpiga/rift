use anyhow::Result;
use ash::vk;
use std::sync::{Arc, Mutex};

use crate::hot_reload;
use crate::renderer::particles::ParticleInstance;
use crate::vulkan::buffer::GpuBuffer;
use crate::vulkan::sync::MAX_FRAMES_IN_FLIGHT;
use gpu_allocator::vulkan::Allocator;

/// Quad vertex for the particle billboard (4 corners).
const QUAD_VERTICES: [[f32; 2]; 4] = [
    [-0.5, -0.5],
    [ 0.5, -0.5],
    [ 0.5,  0.5],
    [-0.5,  0.5],
];

const QUAD_INDICES: [u32; 6] = [0, 1, 2, 0, 2, 3];

/// Manages the particle rendering pipeline and GPU resources.
pub struct ParticleRenderer {
    pub pipeline: vk::Pipeline,
    pub pipeline_layout: vk::PipelineLayout,
    quad_vb: GpuBuffer,
    quad_ib: GpuBuffer,
    /// Per-frame instance buffers; freeing slot[frame] is safe because draw_frame
    /// waits on its fence before calling upload.
    instance_buffers: Vec<Option<GpuBuffer>>,
    instance_counts: Vec<u32>,
}

impl ParticleRenderer {
    pub fn new(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        render_pass: vk::RenderPass,
        extent: vk::Extent2D,
        descriptor_set_layout: vk::DescriptorSetLayout,
        shader_dir: &std::path::Path,
    ) -> Result<Self> {
        let quad_vb = crate::vulkan::buffer::create_device_local_buffer(
            device, allocator, queue, command_pool,
            &QUAD_VERTICES,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            "particle_quad_vb",
        )?;

        let quad_ib = crate::vulkan::buffer::create_device_local_buffer(
            device, allocator, queue, command_pool,
            &QUAD_INDICES,
            vk::BufferUsageFlags::INDEX_BUFFER,
            "particle_quad_ib",
        )?;

        let (pipeline, pipeline_layout) = Self::create_pipeline(
            device, render_pass, extent, descriptor_set_layout, shader_dir,
        )?;

        Ok(Self {
            pipeline,
            pipeline_layout,
            quad_vb,
            quad_ib,
            instance_buffers: (0..MAX_FRAMES_IN_FLIGHT).map(|_| None).collect(),
            instance_counts: vec![0; MAX_FRAMES_IN_FLIGHT],
        })
    }

    /// Upload particle instance data to GPU. Call once per frame before recording.
    pub fn upload(
        &mut self,
        frame: usize,
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        instances: &[ParticleInstance],
    ) -> Result<()> {
        // Free the old buffer in this frame slot. Safe because the caller waited
        // on this frame's fence before invoking upload.
        if let Some(mut buf) = self.instance_buffers[frame].take() {
            buf.cleanup(device, allocator);
        }

        if instances.is_empty() {
            self.instance_counts[frame] = 0;
            return Ok(());
        }

        // Create host-visible instance buffer (particles change every frame)
        self.instance_buffers[frame] = Some(crate::vulkan::buffer::create_host_buffer(
            device, allocator, instances,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            "particle_instances",
        )?);
        self.instance_counts[frame] = instances.len() as u32;

        Ok(())
    }

    /// Record particle draw commands into the current render pass.
    pub fn record(
        &self,
        frame: usize,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        descriptor_set: vk::DescriptorSet,
    ) {
        let count = self.instance_counts[frame];
        if count == 0 {
            return;
        }
        let instance_buf = match &self.instance_buffers[frame] {
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
                &[descriptor_set],
                &[],
            );

            // Bind quad vertices at binding 0, instances at binding 1
            device.cmd_bind_vertex_buffers(cmd, 0, &[self.quad_vb.buffer, instance_buf], &[0, 0]);
            device.cmd_bind_index_buffer(cmd, self.quad_ib.buffer, 0, vk::IndexType::UINT32);

            // Draw 6 indices (quad), instanced by particle count
            device.cmd_draw_indexed(cmd, 6, count, 0, 0, 0);
        }
    }

    pub fn recreate_pipeline(
        &mut self,
        device: &ash::Device,
        render_pass: vk::RenderPass,
        extent: vk::Extent2D,
        descriptor_set_layout: vk::DescriptorSetLayout,
        shader_dir: &std::path::Path,
    ) -> Result<()> {
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
        }
        let (pipeline, layout) = Self::create_pipeline(
            device, render_pass, extent, descriptor_set_layout, shader_dir,
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
        let vert_src = std::fs::read_to_string(shader_dir.join("particle.vert"))?;
        let frag_src = std::fs::read_to_string(shader_dir.join("particle.frag"))?;

        let vert_spv = hot_reload::compile_glsl(&vert_src, "particle.vert", shaderc::ShaderKind::Vertex)?;
        let frag_spv = hot_reload::compile_glsl(&frag_src, "particle.frag", shaderc::ShaderKind::Fragment)?;

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

        // Binding 0: per-vertex quad corner (vec2)
        // Binding 1: per-instance particle data
        let binding_descs = [
            vk::VertexInputBindingDescription {
                binding: 0,
                stride: std::mem::size_of::<[f32; 2]>() as u32,
                input_rate: vk::VertexInputRate::VERTEX,
            },
            vk::VertexInputBindingDescription {
                binding: 1,
                stride: std::mem::size_of::<ParticleInstance>() as u32,
                input_rate: vk::VertexInputRate::INSTANCE,
            },
        ];

        // location 0: inCorner (vec2) from binding 0
        // location 1: inPosition (vec3) from binding 1, offset 0
        // location 2: inColor (vec4) from binding 1, offset 12
        // location 3: inSizeLife (vec2) from binding 1, offset 28
        let attr_descs = [
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 0,
                format: vk::Format::R32G32_SFLOAT,
                offset: 0,
            },
            vk::VertexInputAttributeDescription {
                binding: 1,
                location: 1,
                format: vk::Format::R32G32B32_SFLOAT,
                offset: 0,
            },
            vk::VertexInputAttributeDescription {
                binding: 1,
                location: 2,
                format: vk::Format::R32G32B32A32_SFLOAT,
                offset: 12,
            },
            vk::VertexInputAttributeDescription {
                binding: 1,
                location: 3,
                format: vk::Format::R32G32_SFLOAT,
                offset: 28,
            },
        ];

        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&binding_descs)
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
            .cull_mode(vk::CullModeFlags::NONE) // billboards face camera
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE);

        let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);

        // Depth: test but don't write (particles are transparent)
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(false)
            .depth_compare_op(vk::CompareOp::LESS);

        // Additive blending
        let color_blend_attachment = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE) // Additive
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE)
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

    pub fn cleanup(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        for slot in self.instance_buffers.iter_mut() {
            if let Some(mut buf) = slot.take() {
                buf.cleanup(device, allocator);
            }
        }
        self.quad_vb.cleanup(device, allocator);
        self.quad_ib.cleanup(device, allocator);
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
        }
    }
}
