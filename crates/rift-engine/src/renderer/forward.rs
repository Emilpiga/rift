//! Core `Renderer` definition: struct layout, `new` constructor,
//! swapchain (re)creation, trivial accessors, and `Drop`.
//!
//! The bulk of the per-frame work — object CRUD, light & uniform
//! building, draw recording, pipeline lifecycle — lives in sibling
//! modules (`objects`, `uniforms`, `draw_loop`, `pipeline`) that each
//! `impl Renderer` block onto this same struct.

use anyhow::Result;
use ash::vk;
use glam::{Vec3, Vec4};
use gpu_allocator::vulkan::{Allocator, AllocatorCreateDesc};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::hot_reload::HotReloader;
use crate::renderer::blood;
use crate::renderer::camera::Camera;
use crate::renderer::depth::DepthBuffer;
use crate::renderer::gpu_skin::SkinningSystem;
use crate::renderer::material::MaterialPool;
use crate::renderer::objects::RenderObject;
use crate::renderer::passes::overlay::{OverlayBatch, OverlayRenderer};
use crate::renderer::passes::post::{BloomConfig, PostProcessing};
use crate::renderer::passes::shadow::ShadowMap;
use crate::renderer::passes::shadow_point::{self, PointShadowAtlas};
use crate::renderer::passes::sky::{SkyConfig, SkyRenderer};
use crate::renderer::texture::Texture;
use crate::renderer::uniform::UniformBuffers;
use crate::renderer::uniforms::PointShadowSlotState;
use crate::renderer::vfx::textures::VfxTextureLibrary;
use crate::renderer::vfx::{ParticleVfxRenderer, RibbonRenderer, VfxSystem};
use crate::vulkan::{
    buffer::GpuBuffer,
    commands::{self, DrawCommand},
    sync::{FrameSync, MAX_FRAMES_IN_FLIGHT},
    Swapchain, VulkanDevice, VulkanInstance,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayResolution {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PortraitDraw {
    pub object_index: usize,
    pub rect_px_bl: [f32; 4],
}

// Backwards-compat re-exports: external code imports these as
// `rift_engine::renderer::forward::{KeyLight, PointLight}`.
// The types themselves now live in the `uniforms` sibling module.
pub use crate::renderer::uniforms::{KeyLight, PointLight};

pub struct Renderer {
    // Fields drop in declaration order — keep instance/device/surface LAST
    pub objects: Vec<RenderObject>,
    pub camera: Camera,
    pub(super) start_time: std::time::Instant,
    pub(super) current_frame: usize,
    pub(super) frame_count: u64,
    pub(super) frame_sync: FrameSync,
    pub(super) command_buffers: Vec<vk::CommandBuffer>,
    pub(super) command_pool: vk::CommandPool,
    pub(super) pipeline: vk::Pipeline,
    pub(super) pipeline_layout: vk::PipelineLayout,
    pub(super) outline_pipeline: vk::Pipeline,
    pub(super) outline_pipeline_layout: vk::PipelineLayout,
    /// HDR offscreen + bloom + composite. Owns three render
    /// passes (scene/bloom/composite), the HDR & bloom images,
    /// all per-image framebuffers and the post-process
    /// pipelines. The forward scene pipeline is built against
    /// `post.scene_pass`; overlay is built against
    /// `post.composite_pass`.
    pub(super) post: PostProcessing,
    pub(super) depth_buffer: DepthBuffer,
    pub(super) default_texture: Texture,
    /// 1×1 R16G16_SFLOAT zero-valued texture bound at set 0,
    /// binding 4 as the placeholder blood field. Replaced by a
    /// floor-sized field when a floor is built; kept around for
    /// scenes (hub, menus) that don't have one.
    pub(super) default_blood_field: Texture,
    /// Per-floor blood field. Owns the splat render pass, pipeline,
    /// mask atlas, and the actual `R16G16_SFLOAT` accumulation image.
    /// Inactive at startup; activated when a floor calls
    /// [`Renderer::recreate_blood_field`].
    pub blood_field: blood::BloodField,
    pub(super) material_pool: MaterialPool,
    pub(super) shadow_map: ShadowMap,
    pub(super) point_shadow_atlas: PointShadowAtlas,
    pub(super) uniforms: UniformBuffers,
    pub(super) swapchain: Swapchain,
    pub(super) allocator: Arc<Mutex<Allocator>>,
    pub(super) surface: vk::SurfaceKHR,
    pub(super) surface_fn: ash::khr::surface::Instance,
    pub(super) device: VulkanDevice,
    pub(super) instance: VulkanInstance,
    // Hot reload
    pub(super) hot_reloader: Option<HotReloader>,
    pub(super) shader_dir: PathBuf,
    // Resize tracking
    pub(super) framebuffer_resized: bool,
    pub(super) window_extent: [u32; 2],
    display_resolutions: Vec<DisplayResolution>,
    selected_display_resolution: DisplayResolution,
    requested_display_resolution: Option<DisplayResolution>,
    // Overlay (HUD)
    pub overlay: OverlayRenderer,
    pub overlay_batch: OverlayBatch,
    /// Declarative VFX system. Replaces the legacy imperative
    /// emitter system entirely; every ability visual is now a
    /// preset in [`crate::renderer::vfx::presets`].
    pub vfx_system: VfxSystem,
    /// Pipeline + per-frame instance buffers for VFX ribbon
    /// layers (beams).
    pub vfx_ribbon_renderer: RibbonRenderer,
    /// Pipeline + per-frame instance buffers for VFX particle
    /// layers. Maintains two pipelines (alpha/premultiplied and
    /// additive); instances are partitioned by `blend` field at
    /// upload time and drawn in two `cmd_draw_indexed` calls.
    pub vfx_particle_renderer: ParticleVfxRenderer,
    /// Authored VFX textures for hybrid particles (billow, flipbooks, …).
    vfx_textures: VfxTextureLibrary,
    /// Compute-shader mesh skinner. Owns the `skin.comp` pipeline
    /// and per-skinned-mesh GPU resources (rest VB, skin SSBO,
    /// palette UBO ring, output VB, descriptor sets). Replaces
    /// the legacy CPU `skin_to` + per-frame VB upload path.
    pub skin_system: SkinningSystem,
    // Deferred deletion queue for GPU buffers
    pub(super) deletion_queue: Vec<(u64, GpuBuffer)>,
    /// Per-frame visible-draw scratch buffer. Reused across
    /// frames (cleared in place) so the main render loop
    /// doesn't allocate a fresh `Vec` of length `objects.len()`
    /// every tick.
    pub(super) draw_scratch: Vec<DrawCommand>,
    /// Per-light visible-draw scratch buffer for the point
    /// shadow pass. The point-shadow pass renders the same
    /// culled list into 6 cube faces per light, so we cull
    /// once per light into this buffer and reuse it across
    /// the six render-pass invocations. Reused across frames.
    pub(super) point_shadow_draw_scratch: Vec<DrawCommand>,
    /// Shadow-caster scratch buffer. Same layout as
    /// `draw_scratch` but populated *without* the camera
    /// frustum cull — shadows must include casters that are
    /// outside the camera frustum (e.g. behind the player)
    /// because their projected shadows can still fall onto
    /// visible geometry. Used for both the directional
    /// shadow pass and as the input list for per-light point
    /// shadow culling.
    pub(super) shadow_draw_scratch: Vec<DrawCommand>,
    /// Per-frame mini scene draws rendered into UI portrait
    /// slots. The fragment shader clips these by screen rect
    /// and local head height so the overlay can show the
    /// target's actual skinned head mesh without a render
    /// texture path.
    pub(super) portrait_draws: Vec<PortraitDraw>,
    /// Per-slot dirty-tracking state for the point-light shadow
    /// atlas. `point_shadow_state[i]` is `Some(state)` if slot
    /// `i` was rendered on a previous frame; `None` means the
    /// slot has never been rendered (or was reset). Each frame
    /// we recompute the would-be state for each active slot
    /// and skip the 6-face render pass entirely when it
    /// matches the cached value — a static torch-lit room
    /// only re-renders point shadows when something actually
    /// moves through it. State is a hash of the light pose
    /// plus all caster transforms within the light's radius;
    /// see [`Self::compute_point_shadow_slot_hash`].
    pub(super) point_shadow_state: [Option<PointShadowSlotState>; shadow_point::MAX_POINT_SHADOWS],
    /// Session graphics setting: when false, skip all shadow-map
    /// passes and make the forward shader use unshadowed lighting.
    pub shadows_enabled: bool,
    /// Session graphics setting: when true, PBR materials perturb
    /// shadow receiver lookups with their height maps.
    pub height_shadows_enabled: bool,
    /// Session display setting: when true, use FIFO present mode.
    pub vsync_enabled: bool,
    /// Session graphics setting: when false, skip the bloom
    /// bright/blur passes and composite without bloom energy.
    pub bloom_enabled: bool,
    /// Session graphics setting: when false, skip the SSAO graph
    /// node and composite with neutral ambient occlusion.
    pub ssao_enabled: bool,
    /// Session graphics setting: when false, skip the volumetric
    /// ray graph node and composite without ray energy.
    pub volumetrics_enabled: bool,
    // Ambient clear color (themed per floor)
    pub clear_color: [f32; 4],
    // Fog parameters
    pub fog_color: [f32; 3],
    pub fog_start: f32,
    pub fog_end: f32,
    /// World-space anchor used as the origin for fog distance.
    /// Set per-frame by game code to the local player's position
    /// so zooming the camera out doesn't drag the fog wall in
    /// over the character. Falls back to the camera position
    /// (camera-anchored fog) until the game writes one.
    pub fog_origin: Vec3,
    /// Smoothed [0,1] strength of the see-through-wall x-ray
    /// porthole. Driven by `camera_follow_system`: target = 1.0
    /// while a wall raycast-occludes the player, 0.0 otherwise.
    /// Eased toward the target each frame so the porthole
    /// fades in/out instead of popping the moment the camera
    /// crosses a wall edge. Pumped to the shader via
    /// `fogOrigin.w`.
    pub wall_xray_strength: f32,
    /// World-space xz AABB of the room the player is currently
    /// in: `(min_x, min_z, max_x, max_z)`. The cel shader uses
    /// this to gate the see-through-wall porthole: only wall
    /// fragments inside this AABB (plus a small margin for
    /// the wall thickness on the boundary) can carve. All
    /// zero means "no active room" — the porthole is disabled
    /// entirely. Pumped to the shader via the existing UBO.
    pub player_room_aabb: Vec4,
    // Dynamic point lights (populated each frame by game code)
    pub point_lights: Vec<PointLight>,
    /// Transient VFX-driven point lights (projectile trails,
    /// impact bursts, breath weapons). Kept separate from
    /// `point_lights` because they MUST always make it into
    /// the per-frame UBO regardless of how many ambient
    /// torches are visible. The renderer packs `vfx_lights`
    /// first and fills the remainder from `point_lights`, so
    /// a fireball racing down a corridor packed with sconces
    /// still illuminates and casts shadows correctly.
    /// Game code clears + republishes this every frame
    /// (typically from `RiftRuntime::collect_lights`).
    pub vfx_lights: Vec<PointLight>,
    /// Procedural sky-dome configuration. Drawn before the
    /// scene each frame when `sky.enabled` is true (typically
    /// only outdoors). Game code mutates this field per biome.
    pub sky: SkyConfig,
    /// Pipeline + shaders for the procedural sky dome.
    pub(super) sky_renderer: SkyRenderer,
    /// Bloom / tonemap parameters, mutable from game code.
    pub bloom: BloomConfig,
    /// Directional key light + ambient floor for the scene.
    /// Defaults to the dim cave-moonlight tuning used by the
    /// rift dungeons; game code overrides this per scene
    /// (sunlit hub, etc.) before each frame's render.
    pub key_light: KeyLight,
    /// Ghost-view post-effect strength in `[0.0, 1.0]`. Driven
    /// by the client when the local player is in ghost mode.
    /// Sampled by `record_composite` and fed to
    /// `post_composite.frag` as a push constant. `0.0` is the
    /// default no-op (normal scene).
    pub ghost_mix: f32,
    /// Screen-space ambient occlusion strength. Kept mutable so
    /// clean preview scenes can opt out of the noisy low-sample
    /// AO pass without changing gameplay lighting.
    pub ssao_strength: f32,
    /// Visual progress used by the shared loading overlay. This
    /// eases toward the real loading target so staged work reads
    /// smoothly while never claiming more progress than the app
    /// has actually reported.
    pub(super) loading_progress_visual: f32,
    pub(super) loading_progress_target: f32,
    pub(super) loading_progress_last_update: std::time::Instant,
}

impl Renderer {
    pub fn clear_portrait_draws(&mut self) {
        self.portrait_draws.clear();
    }

    pub fn queue_object_portrait(&mut self, object_index: usize, rect_px: [f32; 4]) {
        if object_index >= self.objects.len() {
            return;
        }
        let [x, y, w, h] = rect_px;
        if w <= 1.0 || h <= 1.0 {
            return;
        }
        let x0 = x.max(0.0);
        let y0 = y.max(0.0);
        let x1 = (x + w).min(self.window_extent[0] as f32);
        let y1 = (y + h).min(self.window_extent[1] as f32);
        self.portrait_draws.push(PortraitDraw {
            object_index,
            rect_px_bl: [x0, y0, x1, y1],
        });
    }

    pub fn smooth_loading_progress(&mut self, target: f32) -> f32 {
        let target = target.clamp(0.0, 1.0);
        let now = std::time::Instant::now();
        let dt = (now - self.loading_progress_last_update)
            .as_secs_f32()
            .clamp(0.0, 0.1);
        self.loading_progress_last_update = now;

        if target + 0.02 < self.loading_progress_target || target <= 0.001 {
            self.loading_progress_visual = target;
        }
        self.loading_progress_target = target;

        if self.loading_progress_visual < target {
            let alpha = 1.0 - (-14.0 * dt).exp();
            self.loading_progress_visual += (target - self.loading_progress_visual) * alpha;
            if target - self.loading_progress_visual < 0.002 {
                self.loading_progress_visual = target;
            }
        } else {
            self.loading_progress_visual = target;
        }

        self.loading_progress_visual.min(target).clamp(0.0, 1.0)
    }

    pub fn new(window: &winit::window::Window) -> Result<Self> {
        let instance = VulkanInstance::new(window)?;

        let surface = unsafe {
            ash_window::create_surface(
                &instance.entry,
                &instance.instance,
                window.display_handle()?.as_raw(),
                window.window_handle()?.as_raw(),
                None,
            )?
        };
        let surface_fn = ash::khr::surface::Instance::new(&instance.entry, &instance.instance);

        let device = VulkanDevice::new(&instance.instance, surface, &surface_fn)?;

        let allocator = Allocator::new(&AllocatorCreateDesc {
            instance: instance.instance.clone(),
            device: device.device.clone(),
            physical_device: device.physical_device,
            debug_settings: Default::default(),
            buffer_device_address: false,
            allocation_sizes: Default::default(),
        })?;
        let allocator = Arc::new(Mutex::new(allocator));

        let size = window.inner_size();
        let swapchain = Swapchain::new(
            &instance.instance,
            &device.device,
            device.physical_device,
            surface,
            &surface_fn,
            device.graphics_queue_family,
            device.present_queue_family,
            [size.width, size.height],
            true,
        )?;

        // Determine shader directory (relative to executable or workspace)
        let shader_dir = crate::renderer::pipeline::find_shader_dir();

        let depth_buffer = DepthBuffer::new(&device.device, &allocator, swapchain.extent)?;

        // HDR offscreen + bloom + composite. Owns the three
        // render passes the rest of the renderer targets.
        let post = PostProcessing::new(
            &device.device,
            &allocator,
            &swapchain,
            depth_buffer.view,
            &shader_dir,
        )?;
        let render_pass = post.scene_pass;
        let composite_pass = post.composite_pass;

        let uniforms = UniformBuffers::new(&device.device, &allocator)?;

        // Materialise the four set-0/set-1 placeholder resources
        // that the forward pipeline needs bound *before* any
        // gameplay code calls `add_mesh`. All four share a single
        // throwaway command pool because their initial uploads
        // happen at startup and never need to outlive this scope.
        let (default_texture, default_blood_field, material_pool, blood_field) =
            Self::init_default_resources(&device, &allocator, &shader_dir, &uniforms)?;

        let (pipeline_handle, pipeline_layout) = Self::compile_pipeline_from_disk(
            &device.device,
            render_pass,
            swapchain.extent,
            &[uniforms.descriptor_set_layout, material_pool.layout],
            &shader_dir,
        )?;
        let (outline_pipeline, outline_pipeline_layout) = Self::compile_outline_pipeline_from_disk(
            &device.device,
            render_pass,
            swapchain.extent,
            &[uniforms.descriptor_set_layout, material_pool.layout],
            &shader_dir,
        )?;

        // Shadow map (depth-only render target sampled by the main pass).
        let shadow_map = ShadowMap::new(
            &device.device,
            &allocator,
            uniforms.descriptor_set_layout,
            &shader_dir,
        )?;
        uniforms.bind_shadow_map(&device.device, shadow_map.view, shadow_map.sampler);

        // Omnidirectional point-light shadow atlas. Reuses the same
        // descriptor-set-0 layout as the main pipeline so the
        // shadow_point pass can read the per-face VPs from the same
        // UBO that gets updated for the main draw.
        let point_shadow_atlas = PointShadowAtlas::new(
            &device.device,
            &allocator,
            uniforms.descriptor_set_layout,
            material_pool.layout,
            &shader_dir,
        )?;
        uniforms.bind_point_shadow_atlas(
            &device.device,
            point_shadow_atlas.cube_array_view,
            point_shadow_atlas.sampler,
        );

        // Set up hot-reloader
        let hot_reloader = match HotReloader::new(&shader_dir) {
            Ok(hr) => Some(hr),
            Err(e) => {
                log::warn!("Hot-reload unavailable: {}", e);
                None
            }
        };

        let command_pool =
            commands::create_command_pool(&device.device, device.graphics_queue_family)?;
        let command_buffers = commands::allocate_command_buffers(
            &device.device,
            command_pool,
            MAX_FRAMES_IN_FLIGHT as u32,
        )?;

        let frame_sync = FrameSync::new(&device.device)?;

        let aspect = size.width as f32 / size.height as f32;
        let camera = Camera::new(aspect);

        let overlay = OverlayRenderer::new(
            &device.device,
            &allocator,
            device.graphics_queue,
            command_pool,
            composite_pass,
            swapchain.extent,
            &shader_dir,
        )?;
        let mut overlay_batch = OverlayBatch::new();
        // Bind the batch to the renderer's shared icon UV
        // registry. Icons stream in across many frames via
        // `Renderer::step_load_icons`; the registry is mutated
        // in place, so the batch sees them as they arrive
        // without further hand-offs.
        let (atlas_w, atlas_h) = overlay.atlas_size();
        overlay_batch.bind_overlay_atlas(overlay.icon_uv_registry(), atlas_w, atlas_h);

        let vfx_ribbon_renderer = RibbonRenderer::new(
            &device.device,
            &allocator,
            device.graphics_queue,
            command_pool,
            post.translucent_pass,
            swapchain.extent,
            uniforms.descriptor_set_layout,
            post.translucent_set_layout,
            &shader_dir,
        )?;
        let vfx_particle_renderer = ParticleVfxRenderer::new(
            &device.device,
            &allocator,
            device.graphics_queue,
            command_pool,
            post.translucent_pass,
            swapchain.extent,
            uniforms.descriptor_set_layout,
            post.translucent_set_layout,
            &shader_dir,
        )?;
        let vfx_textures = VfxTextureLibrary::load(
            &device.device,
            &allocator,
            device.graphics_queue,
            command_pool,
        )?;
        vfx_textures.bind_translucent_descriptors(&device.device, &post);
        let vfx_system = VfxSystem::new(8192);
        let sky_renderer = SkyRenderer::new(&device.device, render_pass, &shader_dir)?;
        let skin_system = SkinningSystem::new(&device.device, &shader_dir)?;

        log::info!("Renderer initialized successfully");

        Ok(Self {
            instance,
            surface,
            surface_fn,
            device,
            allocator,
            swapchain,
            post,
            pipeline: pipeline_handle,
            pipeline_layout,
            outline_pipeline,
            outline_pipeline_layout,
            command_pool,
            command_buffers,
            frame_sync,
            current_frame: 0,
            depth_buffer,
            default_texture,
            default_blood_field,
            blood_field,
            material_pool,
            shadow_map,
            point_shadow_atlas,
            uniforms,
            objects: Vec::new(),
            camera,
            start_time: std::time::Instant::now(),
            frame_count: 0,
            hot_reloader,
            shader_dir,
            framebuffer_resized: false,
            window_extent: [size.width, size.height],
            display_resolutions: Vec::new(),
            selected_display_resolution: DisplayResolution {
                width: size.width,
                height: size.height,
            },
            requested_display_resolution: None,
            overlay,
            overlay_batch,
            vfx_system,
            vfx_ribbon_renderer,
            vfx_particle_renderer,
            vfx_textures,
            skin_system,
            deletion_queue: Vec::new(),
            // Per-frame scratch lists. Pre-sized to avoid the
            // first-frame allocator dance; they grow naturally
            // as `objects` does so the steady-state cost is
            // zero allocations.
            draw_scratch: Vec::with_capacity(256),
            point_shadow_draw_scratch: Vec::with_capacity(64),
            shadow_draw_scratch: Vec::with_capacity(256),
            portrait_draws: Vec::with_capacity(16),
            point_shadow_state: [None; shadow_point::MAX_POINT_SHADOWS],
            shadows_enabled: true,
            height_shadows_enabled: false,
            vsync_enabled: true,
            bloom_enabled: true,
            ssao_enabled: true,
            volumetrics_enabled: true,
            clear_color: [0.008, 0.006, 0.010, 1.0],
            fog_color: [0.018, 0.012, 0.010],
            fog_start: 5.0,
            fog_end: 16.0,
            fog_origin: Vec3::ZERO,
            wall_xray_strength: 0.0,
            player_room_aabb: Vec4::ZERO,
            point_lights: Vec::new(),
            vfx_lights: Vec::new(),
            sky: SkyConfig::default(),
            sky_renderer,
            bloom: BloomConfig::default(),
            key_light: KeyLight::default(),
            ghost_mix: 0.0,
            ssao_strength: 0.7,
            loading_progress_visual: 0.0,
            loading_progress_target: 0.0,
            loading_progress_last_update: std::time::Instant::now(),
        })
    }

    /// Recreate the swapchain, depth buffer, framebuffers, and pipeline for new dimensions.
    pub fn recreate_swapchain(&mut self, width: u32, height: u32) -> Result<()> {
        if width == 0 || height == 0 {
            return Ok(()); // Minimized — skip
        }

        unsafe {
            self.device.device.device_wait_idle()?;
        }

        // Tear down post-process swapchain-dependent resources
        // (offscreen images, framebuffers, descriptor sets)
        // before the depth buffer that some of them reference.
        self.post
            .cleanup_swapchain_dependent(&self.device.device, &self.allocator);

        // Destroy old depth buffer
        self.depth_buffer
            .cleanup(&self.device.device, &self.allocator);

        // Destroy old pipeline
        unsafe {
            self.device.device.destroy_pipeline(self.pipeline, None);
            self.device
                .device
                .destroy_pipeline_layout(self.pipeline_layout, None);
            self.device
                .device
                .destroy_pipeline(self.outline_pipeline, None);
            self.device
                .device
                .destroy_pipeline_layout(self.outline_pipeline_layout, None);
        }

        // Destroy old swapchain
        self.swapchain.cleanup(&self.device.device);

        // Create new swapchain
        self.swapchain = Swapchain::new(
            &self.instance.instance,
            &self.device.device,
            self.device.physical_device,
            self.surface,
            &self.surface_fn,
            self.device.graphics_queue_family,
            self.device.present_queue_family,
            [width, height],
            self.vsync_enabled,
        )?;

        // Recreate depth buffer at new size
        self.depth_buffer =
            DepthBuffer::new(&self.device.device, &self.allocator, self.swapchain.extent)?;

        // Recreate post-process resources (offscreen images,
        // framebuffers, descriptor sets). Render passes &
        // pipelines stay alive across resize because every post
        // pipeline uses dynamic viewport+scissor.
        self.post.recreate(
            &self.device.device,
            &self.allocator,
            &self.swapchain,
            self.depth_buffer.view,
        )?;
        self.vfx_textures
            .bind_translucent_descriptors(&self.device.device, &self.post);

        // Recreate pipeline with new extent
        let (new_pipeline, new_layout) = Self::compile_pipeline_from_disk(
            &self.device.device,
            self.post.scene_pass,
            self.swapchain.extent,
            &[
                self.uniforms.descriptor_set_layout,
                self.material_pool.layout,
            ],
            &self.shader_dir,
        )?;
        let (new_outline_pipeline, new_outline_layout) = Self::compile_outline_pipeline_from_disk(
            &self.device.device,
            self.post.scene_pass,
            self.swapchain.extent,
            &[
                self.uniforms.descriptor_set_layout,
                self.material_pool.layout,
            ],
            &self.shader_dir,
        )?;
        self.pipeline = new_pipeline;
        self.pipeline_layout = new_layout;
        self.outline_pipeline = new_outline_pipeline;
        self.outline_pipeline_layout = new_outline_layout;

        // Recreate overlay pipeline
        self.overlay.recreate_pipeline(
            &self.device.device,
            self.post.composite_pass,
            self.swapchain.extent,
            &self.shader_dir,
        )?;

        // Recreate VFX ribbon pipeline alongside.
        self.vfx_ribbon_renderer.recreate_pipeline(
            &self.device.device,
            self.post.translucent_pass,
            self.swapchain.extent,
            self.uniforms.descriptor_set_layout,
            self.post.translucent_set_layout,
            &self.shader_dir,
        )?;
        self.vfx_particle_renderer.recreate_pipeline(
            &self.device.device,
            self.post.translucent_pass,
            self.swapchain.extent,
            self.uniforms.descriptor_set_layout,
            self.post.translucent_set_layout,
            &self.shader_dir,
        )?;

        // Update camera aspect ratio
        let aspect = self.swapchain.extent.width as f32 / self.swapchain.extent.height as f32;
        self.camera.aspect = aspect;

        log::info!(
            "Swapchain recreated: {}x{}",
            self.swapchain.extent.width,
            self.swapchain.extent.height
        );
        Ok(())
    }

    pub fn set_vsync_enabled(&mut self, enabled: bool) -> Result<()> {
        if self.vsync_enabled == enabled {
            return Ok(());
        }
        self.vsync_enabled = enabled;
        self.recreate_swapchain(self.window_extent[0], self.window_extent[1])
    }

    pub fn set_bloom_enabled(&mut self, enabled: bool) {
        self.bloom_enabled = enabled;
    }

    pub fn set_ssao_enabled(&mut self, enabled: bool) {
        self.ssao_enabled = enabled;
        let _ = self.post.set_post_node_enabled("ssao", enabled);
    }

    pub fn set_volumetrics_enabled(&mut self, enabled: bool) {
        self.volumetrics_enabled = enabled;
        let _ = self.post.set_post_node_enabled("volumetrics", enabled);
    }

    pub fn window_extent(&self) -> [u32; 2] {
        self.window_extent
    }

    pub fn display_resolutions(&self) -> &[DisplayResolution] {
        &self.display_resolutions
    }

    pub fn selected_display_resolution(&self) -> DisplayResolution {
        self.selected_display_resolution
    }

    pub fn set_display_resolutions(
        &mut self,
        resolutions: Vec<DisplayResolution>,
        selected: DisplayResolution,
    ) {
        self.display_resolutions = resolutions;
        self.selected_display_resolution = selected;
    }

    pub fn request_display_resolution(&mut self, resolution: DisplayResolution) {
        if resolution.width == 0 || resolution.height == 0 {
            return;
        }
        self.requested_display_resolution = Some(resolution);
        self.selected_display_resolution = resolution;
    }

    pub fn take_requested_display_resolution(&mut self) -> Option<DisplayResolution> {
        self.requested_display_resolution.take()
    }

    /// Direct access to the underlying Vulkan device handle (for code
    /// that needs to free GPU resources owned outside the renderer
    /// during shutdown).
    pub fn ash_device(&self) -> &ash::Device {
        &self.device.device
    }

    /// Shared handle to the GPU allocator used by all of the
    /// renderer's image/buffer creation paths.
    pub fn allocator_arc(&self) -> Arc<Mutex<Allocator>> {
        self.allocator.clone()
    }

    /// Notify the renderer that the window has been resized.
    pub fn notify_resized(&mut self, width: u32, height: u32) {
        self.framebuffer_resized = true;
        self.window_extent = [width, height];
        self.selected_display_resolution = DisplayResolution { width, height };
    }

    pub fn elapsed_secs(&self) -> f32 {
        self.start_time.elapsed().as_secs_f32()
    }

    /// Get the current screen dimensions in pixels.
    pub fn screen_size(&self) -> (f32, f32) {
        (
            self.swapchain.extent.width as f32,
            self.swapchain.extent.height as f32,
        )
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            self.device.device.device_wait_idle().ok();
        }

        // Reset all command buffers so validation doesn't flag buffers as "in use"
        for &cmd in &self.command_buffers {
            unsafe {
                self.device
                    .device
                    .reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())
                    .ok();
            }
        }

        // Destroy all GPU buffers before freeing command pool/fences
        for obj in &mut self.objects {
            obj.vertex_buffer
                .cleanup(&self.device.device, &self.allocator);
            obj.index_buffer
                .cleanup(&self.device.device, &self.allocator);
            if let Some(bufs) = obj.dynamic_vertex_buffers.take() {
                for mut b in bufs {
                    b.cleanup(&self.device.device, &self.allocator);
                }
            }
            if let Some(mut tex) = obj.texture.take() {
                tex.cleanup(&self.device.device, &self.allocator);
            }
        }
        self.objects.clear();

        // Flush deferred deletions
        let device = &self.device.device;
        let allocator = &self.allocator;
        for (_, buf) in self.deletion_queue.drain(..) {
            unsafe {
                device.destroy_buffer(buf.buffer, None);
            }
            if let Some(alloc) = buf.allocation {
                allocator.lock().unwrap().free(alloc).ok();
            }
        }

        self.frame_sync.cleanup(&self.device.device);
        unsafe {
            self.device
                .device
                .destroy_command_pool(self.command_pool, None);
        }

        self.uniforms.cleanup(&self.device.device, &self.allocator);
        self.default_texture
            .cleanup(&self.device.device, &self.allocator);
        self.default_blood_field
            .cleanup(&self.device.device, &self.allocator);
        self.blood_field
            .cleanup(&self.device.device, &self.allocator);
        self.material_pool
            .cleanup(&self.device.device, &self.allocator);
        self.shadow_map
            .cleanup(&self.device.device, &self.allocator);
        self.point_shadow_atlas
            .cleanup(&self.device.device, &self.allocator);
        self.depth_buffer
            .cleanup(&self.device.device, &self.allocator);
        self.overlay.cleanup(&self.device.device, &self.allocator);
        self.vfx_ribbon_renderer
            .cleanup(&self.device.device, &self.allocator);
        self.vfx_particle_renderer
            .cleanup(&self.device.device, &self.allocator);
        self.vfx_textures
            .cleanup(&self.device.device, &self.allocator);
        self.skin_system
            .cleanup(&self.device.device, &self.allocator);
        self.sky_renderer.cleanup(&self.device.device);
        // Tear down all post-process resources (offscreen
        // images, framebuffers, pipelines, render passes,
        // descriptor pool, sampler).
        self.post.cleanup(&self.device.device, &self.allocator);

        unsafe {
            self.device.device.destroy_pipeline(self.pipeline, None);
            self.device
                .device
                .destroy_pipeline_layout(self.pipeline_layout, None);
            self.device
                .device
                .destroy_pipeline(self.outline_pipeline, None);
            self.device
                .device
                .destroy_pipeline_layout(self.outline_pipeline_layout, None);

            self.swapchain.cleanup(&self.device.device);
            self.surface_fn.destroy_surface(self.surface, None);
        }

        // Drop allocator before device & instance (auto-drop handles the rest)
        drop(self.allocator.lock());
    }
}
