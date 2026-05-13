//! GPU pipeline for [`crate::renderer::vfx`] ribbon layers.
//!
//! Mirrors [`crate::renderer::particle_renderer::ParticleRenderer`]
//! in shape: per-frame instance buffers, recreate-on-swapchain,
//! shader hot-reload friendly. Each instance is a single beam
//! with everything (endpoints, width, time, brightness, baked
//! gradients, noise params) packed into [`VfxRibbonInstance`].
//!
//! The vertex shader expands a billboard quad oriented along the
//! beam, and the fragment shader samples the baked cross/length
//! gradients with optional scrolling fbm noise — no texture
//! atlas, no asset pipeline.
//!
//! Ribbons are always drawn with **premultiplied additive blend**
//! (`SRC = ONE`, `DST = ONE`) since they're emissive volumes.
//! The shader pre-multiplies by alpha so transparency still works.
//! Depth test on, depth write off — beams glow through fog but
//! don't occlude what's behind them.

use anyhow::Result;
use ash::vk;
use std::sync::{Arc, Mutex};

use crate::hot_reload;
use crate::renderer::vfx::runtime::VfxRibbonInstance;
use crate::vulkan::buffer::GpuBuffer;
use crate::vulkan::sync::MAX_FRAMES_IN_FLIGHT;
use gpu_allocator::vulkan::Allocator;

/// Quad corners in `(cross, length)` space:
///   x ∈ [-0.5, 0.5] = perpendicular to the beam,
///   y ∈ [ 0.0, 1.0] = origin → tip.
const QUAD_VERTICES: [[f32; 2]; 4] = [[-0.5, 0.0], [0.5, 0.0], [0.5, 1.0], [-0.5, 1.0]];

const QUAD_INDICES: [u32; 6] = [0, 1, 2, 0, 2, 3];

pub struct RibbonRenderer {
    pub pipeline: vk::Pipeline,
    pub pipeline_layout: vk::PipelineLayout,
    quad_vb: GpuBuffer,
    quad_ib: GpuBuffer,
    instance_buffers: Vec<Option<GpuBuffer>>,
    instance_counts: Vec<u32>,
}

impl RibbonRenderer {
    pub fn new(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        render_pass: vk::RenderPass,
        extent: vk::Extent2D,
        descriptor_set_layout: vk::DescriptorSetLayout,
        translucent_set_layout: vk::DescriptorSetLayout,
        shader_dir: &std::path::Path,
    ) -> Result<Self> {
        let quad_vb = crate::vulkan::buffer::create_device_local_buffer(
            device,
            allocator,
            queue,
            command_pool,
            &QUAD_VERTICES,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            "ribbon_quad_vb",
        )?;

        let quad_ib = crate::vulkan::buffer::create_device_local_buffer(
            device,
            allocator,
            queue,
            command_pool,
            &QUAD_INDICES,
            vk::BufferUsageFlags::INDEX_BUFFER,
            "ribbon_quad_ib",
        )?;

        let (pipeline, pipeline_layout) = Self::create_pipeline(
            device,
            render_pass,
            extent,
            descriptor_set_layout,
            translucent_set_layout,
            shader_dir,
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

    pub fn upload(
        &mut self,
        frame: usize,
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        instances: &[VfxRibbonInstance],
    ) -> Result<()> {
        if let Some(mut buf) = self.instance_buffers[frame].take() {
            buf.cleanup(device, allocator);
        }
        if instances.is_empty() {
            self.instance_counts[frame] = 0;
            return Ok(());
        }
        self.instance_buffers[frame] = Some(crate::vulkan::buffer::create_host_buffer(
            device,
            allocator,
            instances,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            "vfx_ribbon_instances",
        )?);
        self.instance_counts[frame] = instances.len() as u32;
        Ok(())
    }

    pub fn record(
        &self,
        frame: usize,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        descriptor_set: vk::DescriptorSet,
        translucent_set: vk::DescriptorSet,
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
                &[descriptor_set, translucent_set],
                &[],
            );
            device.cmd_bind_vertex_buffers(cmd, 0, &[self.quad_vb.buffer, instance_buf], &[0, 0]);
            device.cmd_bind_index_buffer(cmd, self.quad_ib.buffer, 0, vk::IndexType::UINT32);
            device.cmd_draw_indexed(cmd, 6, count, 0, 0, 0);
        }
    }

    pub fn recreate_pipeline(
        &mut self,
        device: &ash::Device,
        render_pass: vk::RenderPass,
        extent: vk::Extent2D,
        descriptor_set_layout: vk::DescriptorSetLayout,
        translucent_set_layout: vk::DescriptorSetLayout,
        shader_dir: &std::path::Path,
    ) -> Result<()> {
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
        }
        let (pipeline, layout) = Self::create_pipeline(
            device,
            render_pass,
            extent,
            descriptor_set_layout,
            translucent_set_layout,
            shader_dir,
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
        translucent_set_layout: vk::DescriptorSetLayout,
        shader_dir: &std::path::Path,
    ) -> Result<(vk::Pipeline, vk::PipelineLayout)> {
        let vert_src = std::fs::read_to_string(shader_dir.join("ribbon.vert"))?;
        let frag_src = std::fs::read_to_string(shader_dir.join("ribbon.frag"))?;

        let vert_spv =
            hot_reload::compile_glsl(&vert_src, "ribbon.vert", shaderc::ShaderKind::Vertex)?;
        let frag_spv =
            hot_reload::compile_glsl(&frag_src, "ribbon.frag", shaderc::ShaderKind::Fragment)?;

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

        let binding_descs = [
            vk::VertexInputBindingDescription {
                binding: 0,
                stride: std::mem::size_of::<[f32; 2]>() as u32,
                input_rate: vk::VertexInputRate::VERTEX,
            },
            vk::VertexInputBindingDescription {
                binding: 1,
                stride: std::mem::size_of::<VfxRibbonInstance>() as u32,
                input_rate: vk::VertexInputRate::INSTANCE,
            },
        ];

        // Each gradient stop is a vec4 = 16 bytes. Layout below
        // must match the `#[repr(C)]` of VfxRibbonInstance exactly.
        //
        // origin: vec4    @  0
        // tip:    vec4    @ 16
        // params: vec4    @ 32
        // flags:  vec4    @ 48
        // cross:  vec4×8  @ 64..192
        // length: vec4×4  @192..256
        let mut attrs: Vec<vk::VertexInputAttributeDescription> = Vec::with_capacity(17);

        // location 0: per-vertex quad corner
        attrs.push(vk::VertexInputAttributeDescription {
            binding: 0,
            location: 0,
            format: vk::Format::R32G32_SFLOAT,
            offset: 0,
        });

        let push = |attrs: &mut Vec<vk::VertexInputAttributeDescription>, loc: u32, off: u32| {
            attrs.push(vk::VertexInputAttributeDescription {
                binding: 1,
                location: loc,
                format: vk::Format::R32G32B32A32_SFLOAT,
                offset: off,
            });
        };

        push(&mut attrs, 1, 0); // origin
        push(&mut attrs, 2, 16); // tip
        push(&mut attrs, 3, 32); // params
        push(&mut attrs, 4, 48); // flags
        for i in 0..8u32 {
            push(&mut attrs, 5 + i, 64 + i * 16); // cross[i]
        }
        for i in 0..4u32 {
            push(&mut attrs, 13 + i, 192 + i * 16); // length[i]
        }

        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&binding_descs)
            .vertex_attribute_descriptions(&attrs);

        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);

        let viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: extent.width as f32,
            height: extent.height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        let scissor = vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent,
        };
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
            .depth_test_enable(true)
            .depth_write_enable(false)
            .depth_compare_op(vk::CompareOp::LESS);

        // Premultiplied additive: shader outputs `rgb * a` so the
        // factor here is ONE/ONE for colour and ONE/ONE for alpha.
        let color_blend_attachment = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::ONE)
            .dst_color_blend_factor(vk::BlendFactor::ONE)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE)
            .alpha_blend_op(vk::BlendOp::ADD);

        let color_blending = vk::PipelineColorBlendStateCreateInfo::default()
            .attachments(std::slice::from_ref(&color_blend_attachment));

        let set_layouts = [descriptor_set_layout, translucent_set_layout];
        let layout_info = vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts);
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

    pub fn reload_pipeline(
        &mut self,
        device: &ash::Device,
        render_pass: vk::RenderPass,
        extent: vk::Extent2D,
        descriptor_set_layout: vk::DescriptorSetLayout,
        translucent_set_layout: vk::DescriptorSetLayout,
        shader_dir: &std::path::Path,
    ) -> Result<()> {
        let (pipeline, pipeline_layout) = Self::create_pipeline(
            device,
            render_pass,
            extent,
            descriptor_set_layout,
            translucent_set_layout,
            shader_dir,
        )?;
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
        }
        self.pipeline = pipeline;
        self.pipeline_layout = pipeline_layout;
        Ok(())
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
