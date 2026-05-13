//! GPU pipeline for [`crate::renderer::vfx`] particle layers.
//!
//! Maintains **two** pipelines — one for `Alpha`/`Premultiplied`
//! blend and one for `Additive` blend — both reading the same
//! shader and the same per-frame instance buffer. The CPU side
//! of [`super::runtime::VfxSystem`] emits a flat
//! `Vec<VfxParticleInstance>`; this renderer partitions that
//! buffer into two contiguous ranges (one per blend group) at
//! upload time and issues two `cmd_draw_indexed` calls per frame.
//!
//! Sprite shape is selected per-pixel via the `sprite` field on
//! the instance and a `switch` in the fragment shader. Colour
//! is pre-baked CPU-side from the layer's [`super::spec::Gradient`]
//! sampled at the particle's normalised life. No texture atlas.

use anyhow::Result;
use ash::vk;
use std::sync::{Arc, Mutex};

use crate::hot_reload;
use crate::renderer::vfx::runtime::VfxParticleInstance;
use crate::vulkan::buffer::GpuBuffer;
use crate::vulkan::sync::MAX_FRAMES_IN_FLIGHT;
use gpu_allocator::vulkan::Allocator;

const QUAD_VERTICES: [[f32; 2]; 4] = [[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]];

const QUAD_INDICES: [u32; 6] = [0, 1, 2, 0, 2, 3];

/// Blend grouping computed per-frame: `[alpha_first..additive_first)`
/// is the alpha range, `[additive_first..end)` is the additive
/// range. Premultiplied is folded into the alpha pipeline since
/// the shader already outputs pre-multiplied output.
#[derive(Default, Clone, Copy)]
struct FrameRanges {
    alpha_count: u32,
    additive_count: u32,
}

pub struct ParticleVfxRenderer {
    pub pipeline_alpha: vk::Pipeline,
    pub pipeline_additive: vk::Pipeline,
    pub pipeline_layout: vk::PipelineLayout,
    quad_vb: GpuBuffer,
    quad_ib: GpuBuffer,
    instance_buffers: Vec<Option<GpuBuffer>>,
    ranges: Vec<FrameRanges>,
}

impl ParticleVfxRenderer {
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
            "vfx_particle_quad_vb",
        )?;

        let quad_ib = crate::vulkan::buffer::create_device_local_buffer(
            device,
            allocator,
            queue,
            command_pool,
            &QUAD_INDICES,
            vk::BufferUsageFlags::INDEX_BUFFER,
            "vfx_particle_quad_ib",
        )?;

        let (pipeline_alpha, pipeline_additive, pipeline_layout) = Self::create_pipelines(
            device,
            render_pass,
            extent,
            descriptor_set_layout,
            translucent_set_layout,
            shader_dir,
        )?;

        Ok(Self {
            pipeline_alpha,
            pipeline_additive,
            pipeline_layout,
            quad_vb,
            quad_ib,
            instance_buffers: (0..MAX_FRAMES_IN_FLIGHT).map(|_| None).collect(),
            ranges: vec![FrameRanges::default(); MAX_FRAMES_IN_FLIGHT],
        })
    }

    /// Partition the input by blend mode and upload to the
    /// per-frame instance buffer. Alpha + premultiplied first,
    /// additive second; the per-pipeline `cmd_draw_indexed` calls
    /// in [`Self::record`] use a non-zero `first_instance` to
    /// pick out their range.
    pub fn upload(
        &mut self,
        frame: usize,
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        instances: &[VfxParticleInstance],
    ) -> Result<()> {
        if let Some(mut buf) = self.instance_buffers[frame].take() {
            buf.cleanup(device, allocator);
        }
        if instances.is_empty() {
            self.ranges[frame] = FrameRanges::default();
            return Ok(());
        }

        // Partition: alpha + premultiplied first, additive last.
        // `blend == 1` is Additive (see runtime.rs SpriteShape /
        // BlendMode discriminants).
        let mut sorted: Vec<VfxParticleInstance> = Vec::with_capacity(instances.len());
        for inst in instances {
            if inst.blend != 1 {
                sorted.push(*inst);
            }
        }
        let alpha_count = sorted.len() as u32;
        for inst in instances {
            if inst.blend == 1 {
                sorted.push(*inst);
            }
        }
        let additive_count = sorted.len() as u32 - alpha_count;

        self.instance_buffers[frame] = Some(crate::vulkan::buffer::create_host_buffer(
            device,
            allocator,
            &sorted,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            "vfx_particle_instances",
        )?);
        self.ranges[frame] = FrameRanges {
            alpha_count,
            additive_count,
        };
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
        let r = self.ranges[frame];
        if r.alpha_count + r.additive_count == 0 {
            return;
        }
        let instance_buf = match &self.instance_buffers[frame] {
            Some(b) => b.buffer,
            None => return,
        };

        unsafe {
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

            // Alpha (+ premultiplied) first — they read the
            // depth buffer correctly and don't double-light the
            // additive layers below.
            if r.alpha_count > 0 {
                device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline_alpha);
                device.cmd_draw_indexed(cmd, 6, r.alpha_count, 0, 0, 0);
            }
            if r.additive_count > 0 {
                device.cmd_bind_pipeline(
                    cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.pipeline_additive,
                );
                device.cmd_draw_indexed(cmd, 6, r.additive_count, 0, 0, r.alpha_count);
            }
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
            device.destroy_pipeline(self.pipeline_alpha, None);
            device.destroy_pipeline(self.pipeline_additive, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
        }
        let (pa, pb, layout) = Self::create_pipelines(
            device,
            render_pass,
            extent,
            descriptor_set_layout,
            translucent_set_layout,
            shader_dir,
        )?;
        self.pipeline_alpha = pa;
        self.pipeline_additive = pb;
        self.pipeline_layout = layout;
        Ok(())
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
        let (pipeline_alpha, pipeline_additive, pipeline_layout) = Self::create_pipelines(
            device,
            render_pass,
            extent,
            descriptor_set_layout,
            translucent_set_layout,
            shader_dir,
        )?;
        unsafe {
            device.destroy_pipeline(self.pipeline_alpha, None);
            device.destroy_pipeline(self.pipeline_additive, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
        }
        self.pipeline_alpha = pipeline_alpha;
        self.pipeline_additive = pipeline_additive;
        self.pipeline_layout = pipeline_layout;
        Ok(())
    }

    fn create_pipelines(
        device: &ash::Device,
        render_pass: vk::RenderPass,
        extent: vk::Extent2D,
        descriptor_set_layout: vk::DescriptorSetLayout,
        translucent_set_layout: vk::DescriptorSetLayout,
        shader_dir: &std::path::Path,
    ) -> Result<(vk::Pipeline, vk::Pipeline, vk::PipelineLayout)> {
        let vert_src = std::fs::read_to_string(shader_dir.join("vfx_particle.vert"))?;
        let frag_src = std::fs::read_to_string(shader_dir.join("vfx_particle.frag"))?;

        let vert_spv =
            hot_reload::compile_glsl(&vert_src, "vfx_particle.vert", shaderc::ShaderKind::Vertex)?;
        let frag_spv = hot_reload::compile_glsl(
            &frag_src,
            "vfx_particle.frag",
            shaderc::ShaderKind::Fragment,
        )?;

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
                stride: std::mem::size_of::<VfxParticleInstance>() as u32,
                input_rate: vk::VertexInputRate::INSTANCE,
            },
        ];

        // VfxParticleInstance layout (must match runtime.rs):
        //   position: vec3 @  0
        //   size:     f32  @ 12
        //   color:    vec4 @ 16
        //   seed:     f32  @ 32
        //   sprite:   u32  @ 36
        //   blend:    u32  @ 40
        //   _pad:     u32  @ 44
        //   velocity: vec3 @ 48
        //   spin:     f32  @ 60
        //
        // Vertex attributes pack `(position, size)` into a single
        // vec4 at offset 0 (location 1), `color` as vec4 at 16
        // (location 2), `(seed, sprite, blend, _pad)` as vec4
        // at offset 32 (location 3), and `(velocity, spin)` as
        // vec4 at offset 48 (location 4). The fragment shader
        // reads sprite via floatBitsToUint — the bytes are u32
        // verbatim regardless of the SFLOAT format.
        let attrs = [
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 0,
                format: vk::Format::R32G32_SFLOAT,
                offset: 0,
            },
            vk::VertexInputAttributeDescription {
                binding: 1,
                location: 1,
                format: vk::Format::R32G32B32A32_SFLOAT,
                offset: 0, // position.xyz + size.w
            },
            vk::VertexInputAttributeDescription {
                binding: 1,
                location: 2,
                format: vk::Format::R32G32B32A32_SFLOAT,
                offset: 16, // color
            },
            vk::VertexInputAttributeDescription {
                binding: 1,
                location: 3,
                format: vk::Format::R32G32B32A32_SFLOAT,
                offset: 32, // seed.x + sprite.y(u32) + blend.z(u32) + pad.w
            },
            vk::VertexInputAttributeDescription {
                binding: 1,
                location: 4,
                format: vk::Format::R32G32B32A32_SFLOAT,
                offset: 48, // velocity.xyz + spin.w
            },
        ];

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

        // Both pipelines consume **pre-multiplied** colour from
        // the fragment shader (vec4(rgb*a, a)). Alpha pipeline
        // uses pre-multiplied alpha blend; additive uses
        // straight-add.
        let attach_alpha = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::ONE)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .alpha_blend_op(vk::BlendOp::ADD);
        let attach_additive = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::ONE)
            .dst_color_blend_factor(vk::BlendFactor::ONE)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE)
            .alpha_blend_op(vk::BlendOp::ADD);

        let blend_alpha = vk::PipelineColorBlendStateCreateInfo::default()
            .attachments(std::slice::from_ref(&attach_alpha));
        let blend_additive = vk::PipelineColorBlendStateCreateInfo::default()
            .attachments(std::slice::from_ref(&attach_additive));

        let set_layouts = [descriptor_set_layout, translucent_set_layout];
        let layout_info = vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts);
        let pipeline_layout = unsafe { device.create_pipeline_layout(&layout_info, None)? };

        let info_alpha = vk::GraphicsPipelineCreateInfo::default()
            .stages(&shader_stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterizer)
            .multisample_state(&multisampling)
            .depth_stencil_state(&depth_stencil)
            .color_blend_state(&blend_alpha)
            .layout(pipeline_layout)
            .render_pass(render_pass)
            .subpass(0);
        let info_additive = vk::GraphicsPipelineCreateInfo::default()
            .stages(&shader_stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterizer)
            .multisample_state(&multisampling)
            .depth_stencil_state(&depth_stencil)
            .color_blend_state(&blend_additive)
            .layout(pipeline_layout)
            .render_pass(render_pass)
            .subpass(0);

        let pipelines = unsafe {
            device
                .create_graphics_pipelines(
                    vk::PipelineCache::null(),
                    &[info_alpha, info_additive],
                    None,
                )
                .map_err(|(_, e)| e)?
        };

        unsafe {
            device.destroy_shader_module(vert_module, None);
            device.destroy_shader_module(frag_module, None);
        }

        Ok((pipelines[0], pipelines[1], pipeline_layout))
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
            device.destroy_pipeline(self.pipeline_alpha, None);
            device.destroy_pipeline(self.pipeline_additive, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
        }
    }
}
