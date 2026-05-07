//! Procedural sky dome.
//!
//! A single fullscreen-triangle pipeline that paints a three-band
//! gradient (zenith / horizon / ground) plus an optional sun disc
//! before any scene geometry draws. Because the triangle covers the
//! whole framebuffer the main render pass's colour clear is moot —
//! the sky overwrites every pixel — but we keep the clear in place
//! anyway so no path is ever undefined when the sky is disabled.
//!
//! ## Why a separate pipeline
//!
//! - **Independent shader.** The sky doesn't sample fog, lights or
//!   the shadow map, so a tiny fragment shader is far cheaper than
//!   running the cel-shaded `triangle.frag` on every pixel.
//! - **No descriptor sets.** All parameters fit in 128 bytes of
//!   push constants, so the pipeline layout has zero set bindings
//!   and never has to wait on UBO updates.
//! - **No vertex buffer.** `gl_VertexIndex` builds the fullscreen
//!   triangle in the vertex shader, so we issue a single
//!   `cmd_draw(3, 1, 0, 0)` per frame.
//! - **Dynamic viewport / scissor.** Pipeline survives swapchain
//!   resize without recreation.
//!
//! ## Push-constant layout (128 B)
//!
//! ```text
//!   0 .. 64    mat4 inv_view_proj_dir   // camera rotation only
//!  64 .. 80    vec4 zenith.rgb + falloff.a
//!  80 .. 96    vec4 horizon.rgb + sun_size.a
//!  96 .. 112   vec4 ground.rgb  + sun_strength.a
//! 112 .. 128   vec4 sun_dir.xyz + 0
//! ```
//!
//! Matches `assets/shaders/sky.frag` exactly — keep them in sync.

use anyhow::Result;
use ash::vk;
use glam::{Mat4, Vec3};
use std::path::Path;

use crate::hot_reload;
use crate::vulkan::pipeline as pipe;

/// CPU-side description of the sky. Cheap to clone; game code mutates
/// the renderer's `sky` field whenever the active biome changes.
#[derive(Clone, Copy, Debug)]
pub struct SkyConfig {
    /// `false` = skip the sky draw entirely (the framebuffer's
    /// clear colour is what the scene draws over). Use this for
    /// indoor dungeons where the cel-shaded fog wall already does
    /// the job.
    pub enabled: bool,
    /// Colour at the top of the dome.
    pub zenith: [f32; 3],
    /// Colour at the horizon line.
    pub horizon: [f32; 3],
    /// Colour below the horizon (visible when the floor opens out
    /// — e.g. the hub's grass apron).
    pub ground: [f32; 3],
    /// Direction *toward* the sun (normalised). Doesn't have to
    /// match the directional light used by the main pass — we
    /// often want the visual sun displaced from the shadow-casting
    /// light so shadows don't sit dead-on under the player.
    pub sun_dir: Vec3,
    /// Cosine of the sun disc's angular radius. 0.9995 ≈ small
    /// realistic disc; 0.99 ≈ a fat stylised sun. Pass `1.0` to
    /// disable the disc without touching `sun_strength`.
    pub sun_size: f32,
    /// Multiplier on the sun's emitted colour. `0.0` = no sun.
    pub sun_strength: f32,
    /// Horizon-to-zenith falloff exponent. `1.0` = linear; higher
    /// = a tighter horizon band, deep colour overhead.
    pub horizon_falloff: f32,
}

impl Default for SkyConfig {
    /// Very dark, near-black gradient. Used for dungeon biomes — the
    /// player won't see the sky directly through the fog wall, but
    /// it sits as a coherent backdrop for any hand-cranked moments
    /// (camera zoom, debug fly-cam, etc.).
    fn default() -> Self {
        Self {
            enabled: false,
            zenith: [0.005, 0.005, 0.010],
            horizon: [0.020, 0.014, 0.012],
            ground: [0.005, 0.004, 0.004],
            sun_dir: Vec3::new(0.0, 1.0, 0.0),
            sun_size: 1.0,
            sun_strength: 0.0,
            horizon_falloff: 2.0,
        }
    }
}

impl SkyConfig {
    /// Sunny outdoor preset for the hub.
    pub fn meadow() -> Self {
        Self {
            enabled: true,
            // Soft cyan zenith fading to warm pale horizon.
            zenith: [0.42, 0.66, 0.92],
            horizon: [0.85, 0.88, 0.92],
            ground: [0.55, 0.50, 0.42],
            sun_dir: Vec3::new(-0.35, 0.85, 0.40).normalize(),
            sun_size: 0.9990,
            sun_strength: 1.4,
            horizon_falloff: 2.5,
        }
    }
}

/// Push-constant struct sent to `sky.frag`. Matches the layout in
/// the shader byte-for-byte.
#[repr(C)]
#[derive(Clone, Copy)]
struct SkyPush {
    inv_view_proj_dir: [[f32; 4]; 4],
    zenith_falloff: [f32; 4],
    horizon_sun_size: [f32; 4],
    ground_sun_str: [f32; 4],
    sun_dir: [f32; 4],
}

pub struct SkyRenderer {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
}

impl SkyRenderer {
    pub fn new(
        device: &ash::Device,
        render_pass: vk::RenderPass,
        shader_dir: &Path,
    ) -> Result<Self> {
        // ---- Compile shaders ----
        let vert_src = std::fs::read_to_string(shader_dir.join("sky.vert"))
            .map_err(|e| anyhow::anyhow!("read sky.vert: {e}"))?;
        let frag_src = std::fs::read_to_string(shader_dir.join("sky.frag"))
            .map_err(|e| anyhow::anyhow!("read sky.frag: {e}"))?;
        let vert_spv = hot_reload::compile_glsl(
            &vert_src, "sky.vert", shaderc::ShaderKind::Vertex,
        )?;
        let frag_spv = hot_reload::compile_glsl(
            &frag_src, "sky.frag", shaderc::ShaderKind::Fragment,
        )?;
        let vert_module = pipe::create_shader_module(device, &vert_spv)?;
        let frag_module = pipe::create_shader_module(device, &frag_spv)?;

        let entry = c"main";
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vert_module)
                .name(entry),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(frag_module)
                .name(entry),
        ];

        // No vertex buffer — the vert shader generates positions
        // from `gl_VertexIndex`, so the input state stays empty.
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);

        // Dynamic viewport + scissor: the pipeline survives a
        // swapchain resize without recreation. Set per-frame in
        // `record`.
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic_state = vk::PipelineDynamicStateCreateInfo::default()
            .dynamic_states(&dynamic_states);

        let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .line_width(1.0)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE);

        let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);

        // Sky doesn't read or write depth: scene geometry that
        // follows always passes its own depth test against the
        // cleared 1.0 depth buffer.
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(false)
            .depth_write_enable(false)
            .stencil_test_enable(false);

        let color_blend_attachment = vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(false)
            .color_write_mask(vk::ColorComponentFlags::RGBA);
        let color_blending = vk::PipelineColorBlendStateCreateInfo::default()
            .attachments(std::slice::from_ref(&color_blend_attachment));

        // Single 128-byte push-constant range visible to fragment
        // only — the vertex shader is parameterless.
        let push_range = vk::PushConstantRange {
            stage_flags: vk::ShaderStageFlags::FRAGMENT,
            offset: 0,
            size: std::mem::size_of::<SkyPush>() as u32,
        };
        let layout_info = vk::PipelineLayoutCreateInfo::default()
            .push_constant_ranges(std::slice::from_ref(&push_range));
        let pipeline_layout = unsafe { device.create_pipeline_layout(&layout_info, None)? };

        let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterizer)
            .multisample_state(&multisampling)
            .depth_stencil_state(&depth_stencil)
            .color_blend_state(&color_blending)
            .dynamic_state(&dynamic_state)
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

        Ok(Self {
            pipeline,
            pipeline_layout,
        })
    }

    /// Draw the sky. Caller has already begun the main render pass.
    /// `view` and `proj` are the current camera matrices; we strip
    /// the translation from `view` so the dome appears anchored at
    /// infinity rather than scaling with player position.
    pub fn record(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        extent: vk::Extent2D,
        view: Mat4,
        proj: Mat4,
        config: &SkyConfig,
    ) {
        if !config.enabled {
            return;
        }

        // Strip translation so we get a pure rotation-only view
        // matrix. Using the full inverse would scale the dome with
        // distance from the world origin — not what we want for
        // an "at infinity" backdrop.
        let mut view_rot = view;
        view_rot.w_axis.x = 0.0;
        view_rot.w_axis.y = 0.0;
        view_rot.w_axis.z = 0.0;
        let inv_vp = (proj * view_rot).inverse();

        let push = SkyPush {
            inv_view_proj_dir: inv_vp.to_cols_array_2d(),
            zenith_falloff: [
                config.zenith[0], config.zenith[1], config.zenith[2],
                config.horizon_falloff,
            ],
            horizon_sun_size: [
                config.horizon[0], config.horizon[1], config.horizon[2],
                config.sun_size,
            ],
            ground_sun_str: [
                config.ground[0], config.ground[1], config.ground[2],
                config.sun_strength,
            ],
            sun_dir: [
                config.sun_dir.x, config.sun_dir.y, config.sun_dir.z,
                0.0,
            ],
        };
        let push_bytes: &[u8] = bytemuck::bytes_of(&push);

        unsafe {
            device.cmd_bind_pipeline(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline,
            );
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
            device.cmd_set_viewport(cmd, 0, std::slice::from_ref(&viewport));
            device.cmd_set_scissor(cmd, 0, std::slice::from_ref(&scissor));
            device.cmd_push_constants(
                cmd,
                self.pipeline_layout,
                vk::ShaderStageFlags::FRAGMENT,
                0,
                push_bytes,
            );
            device.cmd_draw(cmd, 3, 1, 0, 0);
        }
    }

    pub fn cleanup(&mut self, device: &ash::Device) {
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
        }
    }
}

// SAFETY: SkyPush is `Copy`, contains only POD `f32` arrays.
unsafe impl bytemuck::Pod for SkyPush {}
unsafe impl bytemuck::Zeroable for SkyPush {}
