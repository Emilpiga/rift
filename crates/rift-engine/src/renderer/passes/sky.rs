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
//!   running the cel-shaded forward opaque fragment shader on every pixel.
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
    /// Procedural storm-cloud coverage. `0.0` = clear sky;
    /// `1.0` = dense storm wall. Driven by an animated fbm noise
    /// in the sky shader; the only CPU cost is filling the push
    /// constant. Coverage stretches denser toward the horizon
    /// line so the dome reads as an oncoming storm rather than a
    /// uniform overcast.
    pub cloud_strength: f32,
    /// Lightning-flash boost on the cloud body. `0.0` = calm;
    /// values 1.0–3.0 brighten the cloud color toward
    /// `cloud_flash_color` for the duration of a strike. Game
    /// code (the `HubStorm` driver) modulates this each frame.
    pub cloud_flash: f32,
    /// RGB colour the cloud body lifts toward during a flash.
    /// Cool-white-blue for normal sky lightning, warm amber for
    /// the rare "hellfire" strikes.
    pub cloud_flash_color: Vec3,
    /// Procedural abyss layer drawn below the horizon. Rift floors
    /// use this to make the space under floating dungeon pieces read
    /// as a deep void instead of a flat ground-colour gradient.
    pub void_depth_strength: f32,
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
            cloud_strength: 0.0,
            cloud_flash: 0.0,
            cloud_flash_color: Vec3::new(0.85, 0.95, 1.25),
            void_depth_strength: 0.0,
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
            cloud_strength: 0.0,
            cloud_flash: 0.0,
            cloud_flash_color: Vec3::new(0.85, 0.95, 1.25),
            void_depth_strength: 0.0,
        }
    }

    /// Crimson-and-black gloom preset for the rift floors. The
    /// dungeon walls are ceilingless, so this is what the player
    /// sees above the parapets — paired with a black fog wall it
    /// reads as a bleeding sky strangled by smoke.
    ///
    /// No sun disc: the rift isn't sunlit, the only direct light
    /// in the scene comes from torches. The horizon sits darker
    /// than the zenith so the sky-to-fog seam blends rather than
    /// banding (fog is near-black, horizon is dim oxblood).
    pub fn rift() -> Self {
        Self {
            enabled: true,
            // Deep oxblood overhead, almost-black at the horizon.
            // Slight blue lift in the zenith keeps the dome from
            // looking flat-painted.
            zenith: [0.42, 0.04, 0.06],
            horizon: [0.06, 0.012, 0.012],
            // Ground band is just black — anything below the
            // horizon line is hidden by the fog wall + walls
            // anyway, but keeping it dark prevents a bright
            // smear on the floor-line edge cases (debug cams,
            // boss-room reveal).
            ground: [0.010, 0.005, 0.005],
            // Dummy sun direction — disc disabled, but a
            // non-degenerate vector keeps the shader happy.
            sun_dir: Vec3::new(0.0, 1.0, 0.0),
            sun_size: 1.0,
            sun_strength: 0.0,
            // Tight horizon band so the crimson reads as a
            // bruised dome overhead rather than washing the
            // whole sky red — keeps the gloom bias.
            horizon_falloff: 3.5,
            cloud_strength: 0.0,
            cloud_flash: 0.0,
            cloud_flash_color: Vec3::new(0.85, 0.95, 1.25),
            void_depth_strength: 1.0,
        }
    }

    /// Hub preset: floating obsidian platform over an abyss with
    /// a thunderstorm crimson sky. Brighter and more-saturated
    /// than `rift()` (the rift floors want claustrophobic gloom;
    /// the hub wants brooding-grandeur), and the horizon is
    /// pushed *up* into a roiling crimson-orange band so distant
    /// hellish mountains can silhouette against it.
    pub fn abyss_hub() -> Self {
        Self {
            enabled: true,
            // Charcoal-black overhead with a faint plum lift, so
            // the dome doesn't look flat.
            zenith: [0.025, 0.012, 0.030],
            // Dim oxblood horizon — visible enough that the
            // mountain ridge silhouettes against it, but dark
            // enough that the sky doesn't wash the whole scene
            // out. The fog color is tuned to match this band so
            // the sky-to-mountain-base seam blends instead of
            // banding.
            horizon: [0.18, 0.05, 0.06],
            // Ground band matches the fog so the player can
            // look down past the platform edge without seeing
            // a hard horizon-to-ground transition.
            ground: [0.06, 0.025, 0.035],
            sun_dir: Vec3::new(0.0, 1.0, 0.0),
            sun_size: 1.0,
            sun_strength: 0.0,
            // Tight horizon band — the bright slice stays
            // narrow so the dome overall reads as oppressively
            // dark with just a smouldering rim.
            horizon_falloff: 3.0,
            // Heavy cloud cover — the abyss hub is meant to
            // sit under a roiling thunderstorm.
            cloud_strength: 0.95,
            cloud_flash: 0.0,
            cloud_flash_color: Vec3::new(0.85, 0.95, 1.25),
            void_depth_strength: 0.55,
        }
    }

    /// Sandstorm hub preset: airborne dust everywhere. The
    /// dome is a warm tan gradient (no sun disc — the sun is
    /// fully veiled by dust) and the cloud layer is repurposed
    /// as drifting dust streaks that thicken toward the
    /// horizon. Pairs with a tight warm-tan fog on the
    /// renderer side to limit visibility to ~25 m.
    pub fn sandstorm_hub() -> Self {
        Self {
            enabled: true,
            // Warm overhead — pale dust-orange where the sun
            // is supposed to be, never quite blue.
            zenith: [0.42, 0.30, 0.18],
            // Saturated tan band on the horizon — the dust
            // wall.
            horizon: [0.78, 0.55, 0.30],
            // Ground band matches the fog so looking down off
            // the platform edge fades smoothly into haze.
            ground: [0.58, 0.40, 0.24],
            // Low-angle sun: heavy on +X/+Z, light on Y, so
            // god rays rake across the platform near
            // grazing and the shadow direction matches the
            // long, dramatic shadows the hub point light
            // throws at the same axis.
            sun_dir: Vec3::new(0.70, 0.32, 0.65).normalize(),
            // Small disc — the sun is mostly veiled by dust
            // so we only want a hot point that pokes through
            // the gaps the cloud-gap mask carves.
            sun_size: 0.9990,
            // Disc strength is dialled low because the
            // god-ray bloom in the sky shader does most of
            // the visual work; this is just the hot core.
            sun_strength: 1.4,
            // Wide, soft horizon — the dust band fills most of
            // the lower hemisphere.
            horizon_falloff: 1.5,
            // No cloud / dust-streak layer in the sandstorm
            // hub. The procedural FBM band reads as random
            // multi-coloured ribbons rather than dust at the
            // viewing distances the hub camera uses; a flat
            // tan dome with the sun's god-ray bloom is more
            // legible. The visible drama still comes from
            // the warm horizon band and the sun's bloom — no
            // streaks needed.
            cloud_strength: 0.0,
            cloud_flash: 0.0,
            cloud_flash_color: Vec3::new(0.85, 0.95, 1.25),
            void_depth_strength: 0.0,
        }
    }

    /// Hub / character-select void aesthetic: deep violet dome, no sun,
    /// optional abyss band below the horizon. Drive `cloud_flash` from the
    /// client hub storm while `cloud_strength` stays at zero (clear-sky
    /// lightning path in `sky.frag`).
    pub fn void_hub() -> Self {
        Self {
            enabled: true,
            zenith: [0.018, 0.008, 0.038],
            horizon: [0.032, 0.012, 0.055],
            ground: [0.014, 0.006, 0.028],
            sun_dir: Vec3::new(0.0, 1.0, 0.0),
            sun_size: 1.0,
            sun_strength: 0.0,
            horizon_falloff: 3.25,
            cloud_strength: 0.0,
            cloud_flash: 0.0,
            cloud_flash_color: Vec3::new(0.72, 0.62, 1.05),
            void_depth_strength: 0.42,
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
    cloud_params: [f32; 4],
    cloud_flash_color: [f32; 4],
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
        let vert_spv =
            hot_reload::compile_glsl(&vert_src, "sky.vert", shaderc::ShaderKind::Vertex)?;
        let frag_spv =
            hot_reload::compile_glsl(&frag_src, "sky.frag", shaderc::ShaderKind::Fragment)?;
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
        let dynamic_state =
            vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

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

    pub fn reload_pipeline(
        &mut self,
        device: &ash::Device,
        render_pass: vk::RenderPass,
        shader_dir: &Path,
    ) -> Result<()> {
        let next = Self::new(device, render_pass, shader_dir)?;
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
        }
        self.pipeline = next.pipeline;
        self.pipeline_layout = next.pipeline_layout;
        Ok(())
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
        time_secs: f32,
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
                config.zenith[0],
                config.zenith[1],
                config.zenith[2],
                config.horizon_falloff,
            ],
            horizon_sun_size: [
                config.horizon[0],
                config.horizon[1],
                config.horizon[2],
                config.sun_size,
            ],
            ground_sun_str: [
                config.ground[0],
                config.ground[1],
                config.ground[2],
                config.sun_strength,
            ],
            sun_dir: [config.sun_dir.x, config.sun_dir.y, config.sun_dir.z, 0.0],
            cloud_params: [
                time_secs,
                config.cloud_strength,
                config.cloud_flash,
                config.void_depth_strength,
            ],
            cloud_flash_color: [
                config.cloud_flash_color.x,
                config.cloud_flash_color.y,
                config.cloud_flash_color.z,
                0.0,
            ],
        };
        let push_bytes: &[u8] = bytemuck::bytes_of(&push);

        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
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
