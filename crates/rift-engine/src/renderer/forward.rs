use anyhow::Result;
use ash::vk;
use glam::{Mat4, Vec3, Vec4};
use gpu_allocator::vulkan::{Allocator, AllocatorCreateDesc};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::hot_reload::{self, HotReloader};
use crate::renderer::blood;
use crate::renderer::camera::Camera;
use crate::renderer::depth::DepthBuffer;
use crate::renderer::gpu_skin::{SkinHandle, SkinningSystem};
use crate::renderer::material::MaterialPool;
use crate::renderer::mesh::{Mesh, Vertex, VertexSkin};
use crate::renderer::overlay::{OverlayBatch, OverlayRenderer};
use crate::renderer::post::{BloomConfig, PostProcessing};
use crate::renderer::shadow::{self, ShadowMap};
use crate::renderer::shadow_point::{self, PointShadowAtlas};
use crate::renderer::sky::{SkyConfig, SkyRenderer};
use crate::renderer::texture::{PbrSource, Texture, TextureSource};
use crate::renderer::uniform::{UniformBuffers, UniformData};
use crate::renderer::vfx::{ParticleVfxRenderer, RibbonRenderer, VfxSystem};
use crate::vulkan::{
    buffer::{self, GpuBuffer},
    commands::{self, DrawCommand},
    pipeline,
    sync::{FrameSync, MAX_FRAMES_IN_FLIGHT},
    Swapchain, VulkanDevice, VulkanInstance,
};
use std::path::Path;

pub struct RenderObject {
    pub vertex_buffer: GpuBuffer,
    pub index_buffer: GpuBuffer,
    pub index_count: u32,
    pub model_matrix: Mat4,
    /// Bounding sphere radius for frustum culling.
    pub bounds_radius: f32,
    /// If Some, this object's vertex data is streamed per-frame from CPU
    /// (host-visible). One buffer per in-flight frame to avoid hazards.
    /// When set, `vertex_buffer` above is unused for drawing; the per-frame
    /// buffer at index `current_frame` is bound instead.
    pub dynamic_vertex_buffers: Option<Vec<GpuBuffer>>,
    /// If Some, this object is GPU-skinned: `vertex_buffer` is unused
    /// at draw time and the renderer binds the compute shader's
    /// output VB (via `SkinningSystem::output_vertex_buffer`)
    /// instead. Takes precedence over `dynamic_vertex_buffers`.
    pub skin_handle: Option<SkinHandle>,
    /// Per-object material descriptor set (set 1). Defaults to the
    /// MaterialPool's white-texture set when no custom texture is bound.
    pub material_set: vk::DescriptorSet,
    /// Owned per-object texture, if any. None = default white.
    pub texture: Option<Texture>,
    /// RGBA tint pushed alongside the model matrix. RGB multiplies
    /// the lit fragment color, A is the output alpha. Default is
    /// `[1, 1, 1, 1]` (no-op opaque). Used to make the local
    /// player's avatar translucent / cyan-tinted while in ghost
    /// mode — the forward pipeline has alpha blending enabled
    /// with `SRC_ALPHA / ONE_MINUS_SRC_ALPHA`, so any object with
    /// `tint.a < 1.0` blends against the framebuffer.
    pub tint: [f32; 4],
    /// Per-object PBR / sampling tweaks. Layout:
    /// `(uv_scale, parallax_scale, flags, _reserved)`. Default
    /// `(1.0, 0.0, 0.0, 0.0)` keeps the legacy cel-shaded
    /// diffuse path. Setting `flags.x` bit 0 (numeric value
    /// `1.0`) flips the shader into PBR + normal-mapping mode
    /// and reads the material set's normal / MR / AO / height
    /// bindings. `parallax_scale` enables parallax-occlusion
    /// when non-zero (typical 0.02–0.05 in world units).
    pub material_params: [f32; 4],
    /// Whether this object should be rasterised into the
    /// shadow passes (directional + cube atlas). Defaults to
    /// `true`. Set to `false` for large flat terrain pieces
    /// (hub platform, sand-dune ring, dungeon floor) which
    /// would explode the shadow-pass triangle count without
    /// contributing visible cast shadows. The heightmap-
    /// displaced shadow position + heightmap self-shadow in
    /// the lit pass already give those surfaces a believable
    /// shadow boundary against their own ripples; the missing
    /// geometric pass would only matter at shallow grazing
    /// angles that our gameplay camera never reaches.
    pub casts_shadow: bool,
}

pub struct Renderer {
    // Fields drop in declaration order — keep instance/device/surface LAST
    pub objects: Vec<RenderObject>,
    pub camera: Camera,
    start_time: std::time::Instant,
    current_frame: usize,
    frame_count: u64,
    frame_sync: FrameSync,
    command_buffers: Vec<vk::CommandBuffer>,
    command_pool: vk::CommandPool,
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    /// HDR offscreen + bloom + composite. Owns three render
    /// passes (scene/bloom/composite), the HDR & bloom images,
    /// all per-image framebuffers and the post-process
    /// pipelines. The forward scene pipeline is built against
    /// `post.scene_pass`; overlay is built against
    /// `post.composite_pass`.
    post: PostProcessing,
    depth_buffer: DepthBuffer,
    default_texture: Texture,
    /// 1×1 R16G16_SFLOAT zero-valued texture bound at set 0,
    /// binding 4 as the placeholder blood field. Replaced by a
    /// floor-sized field when a floor is built; kept around for
    /// scenes (hub, menus) that don't have one.
    default_blood_field: Texture,
    /// Per-floor blood field. Owns the splat render pass, pipeline,
    /// mask atlas, and the actual `R16G16_SFLOAT` accumulation image.
    /// Inactive at startup; activated when a floor calls
    /// [`Renderer::recreate_blood_field`].
    pub blood_field: blood::BloodField,
    material_pool: MaterialPool,
    shadow_map: ShadowMap,
    point_shadow_atlas: PointShadowAtlas,
    uniforms: UniformBuffers,
    swapchain: Swapchain,
    allocator: Arc<Mutex<Allocator>>,
    surface: vk::SurfaceKHR,
    surface_fn: ash::khr::surface::Instance,
    device: VulkanDevice,
    instance: VulkanInstance,
    // Hot reload
    hot_reloader: Option<HotReloader>,
    shader_dir: PathBuf,
    // Resize tracking
    framebuffer_resized: bool,
    window_extent: [u32; 2],
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
    /// Compute-shader mesh skinner. Owns the `skin.comp` pipeline
    /// and per-skinned-mesh GPU resources (rest VB, skin SSBO,
    /// palette UBO ring, output VB, descriptor sets). Replaces
    /// the legacy CPU `skin_to` + per-frame VB upload path.
    pub skin_system: SkinningSystem,
    // Deferred deletion queue for GPU buffers
    deletion_queue: Vec<(u64, GpuBuffer)>,
    /// Per-frame visible-draw scratch buffer. Reused across
    /// frames (cleared in place) so the main render loop
    /// doesn't allocate a fresh `Vec` of length `objects.len()`
    /// every tick.
    draw_scratch: Vec<DrawCommand>,
    /// Per-light visible-draw scratch buffer for the point
    /// shadow pass. The point-shadow pass renders the same
    /// culled list into 6 cube faces per light, so we cull
    /// once per light into this buffer and reuse it across
    /// the six render-pass invocations. Reused across frames.
    point_shadow_draw_scratch: Vec<DrawCommand>,
    /// Shadow-caster scratch buffer. Same layout as
    /// `draw_scratch` but populated *without* the camera
    /// frustum cull — shadows must include casters that are
    /// outside the camera frustum (e.g. behind the player)
    /// because their projected shadows can still fall onto
    /// visible geometry. Used for both the directional
    /// shadow pass and as the input list for per-light point
    /// shadow culling.
    shadow_draw_scratch: Vec<DrawCommand>,
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
    point_shadow_state: [Option<PointShadowSlotState>; shadow_point::MAX_POINT_SHADOWS],
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
    /// Heat-distortion sources, written each frame by VFX
    /// effects that opt in via `EffectLight::heat_haze`. The
    /// composite pass picks the strongest one and applies a
    /// noise-driven UV warp around its screen position. This
    /// is intentionally separate from `point_lights` so that
    /// ambient warm lights (torches, braziers, the
    /// character-select scene) don't shimmer the air — only
    /// gameplay-driven explosions / breath weapons do.
    pub heat_sources: Vec<HeatSource>,
    /// Procedural sky-dome configuration. Drawn before the
    /// scene each frame when `sky.enabled` is true (typically
    /// only outdoors). Game code mutates this field per biome.
    pub sky: SkyConfig,
    /// Pipeline + shaders for the procedural sky dome.
    sky_renderer: SkyRenderer,
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
}

/// Directional key light + ambient floor. The forward shader
/// reads `direction` as the light vector, `color` as its tint
/// (multiplied into diffuse + specular + rim), and `ambient`
/// as the unconditional floor added to every fragment.
#[derive(Clone, Copy, Debug)]
pub struct KeyLight {
    /// World-space direction *toward* the light (will be
    /// normalised before upload).
    pub direction: Vec3,
    /// RGB tint of the directional contribution. Treat as
    /// linear, pre-tonemap. ~0.2 reads as moonlight, ~1.0 as
    /// midday sun.
    pub color: Vec3,
    /// Unconditional ambient floor. ~0.05 = cave-dark, ~0.30 =
    /// outdoor overcast.
    pub ambient: f32,
}

impl KeyLight {
    /// Default rift / dungeon mood: very dim cool moonlight
    /// with low ambient so torches carry the warmth.
    pub const DUNGEON: Self = Self {
        direction: Vec3::new(0.4, 0.8, 0.3),
        color: Vec3::new(0.18, 0.20, 0.28),
        ambient: 0.05,
    };

    /// Sunny outdoor hub: warm bright key + lifted ambient so
    /// the open meadow reads as midday rather than dusk.
    pub const SUNLIT: Self = Self {
        direction: Vec3::new(0.4, 0.8, 0.3),
        color: Vec3::new(1.10, 1.00, 0.85),
        ambient: 0.35,
    };

    /// Brooding crimson stormlight for the abyss hub. Cooler-than-
    /// sunlit, slightly biased red on the directional, with a
    /// dim warm ambient so the platform reads as lit by the
    /// distant fire-storm horizon rather than a sun.
    pub const STORMLIT: Self = Self {
        direction: Vec3::new(0.2, 0.7, 0.5),
        color: Vec3::new(0.65, 0.30, 0.28),
        ambient: 0.18,
    };

    /// Diffuse warm sandstorm light. A single strong sun-like
    /// directional aimed to match the sandstorm sky's hot
    /// spot, lifted ambient so the dust-scattered fill
    /// bathes the whole platform, and the warm tan tint
    /// pre-bakes the dust scattering into the directional
    /// contribution. Combined with a sky-anchored point light
    /// in the hub, this gives the platform a dramatic
    /// "sunbeam through the dust" key/fill split rather than
    /// the flat overcast a high-ambient sandstorm would
    /// otherwise produce.
    ///
    /// The directional is intentionally HDR-bright (>1.0 on
    /// red/green) so the dunes' lit faces punch through the
    /// dust horizon and the platform reads as midday-veiled
    /// rather than dusk; the matching ambient lift keeps the
    /// shaded side from going muddy.
    ///
    /// Ambient sits high (`0.55`) on purpose: a real
    /// sandstorm has so much airborne dust that *every*
    /// surface picks up a strong omni-directional warm fill,
    /// not just the sun-facing side. Without this, props
    /// directly opposite the sun (the chest, the player
    /// standing with their back to the sun) read as
    /// near-black silhouettes against the lifted sky/fog —
    /// fine for a dungeon but wrong for a midday hub. The
    /// directional is bumped in lock-step so the lit side
    /// still has clear contrast against the shaded side.
    pub const SANDSTORM: Self = Self {
        // Matches `SkyConfig::sandstorm_hub`'s `sun_dir`
        // (normalised) so the shadow map lays the platform's
        // shadow opposite the visible sun in the sky.
        direction: Vec3::new(0.70, 0.32, 0.65),
        color: Vec3::new(2.10, 1.55, 0.95),
        ambient: 0.55,
    };
}

impl Default for KeyLight {
    fn default() -> Self {
        Self::DUNGEON
    }
}

/// Maximum number of point lights uploaded to the camera UBO
/// per frame. Kept in sync with the `[16]` array sizes in every
/// shader that binds the camera UBO (triangle.frag, particle.vert,
/// ribbon.vert, shadow*.{vert,frag}). The first
/// `point_shadow_count` slots are shadow-casters, then VFX
/// additive lights, then any additional ambient/torch lights;
/// see `Renderer::merge_per_frame_lights`.
const MAX_POINT_LIGHTS: usize = 16;

/// A dynamic point light source.
#[derive(Clone, Copy)]
pub struct PointLight {
    pub position: Vec3,
    pub color: Vec3,
    pub radius: f32,
    pub intensity: f32,
}

/// Per-slot dirty-tracking state for the point-shadow atlas.
/// Captured immediately after a slot's 6 cube faces are
/// rendered; on the next frame the renderer recomputes the
/// would-be value and skips the render entirely if it matches.
///
/// The hash collapses (a) the light's pose & radius and (b)
/// the bit pattern of every shadow-caster's translation +
/// bounds_radius within the light's effective range. That's
/// cheap to compute (a single FNV-style fold per slot) and
/// stable bit-for-bit across frames as long as the inputs are
/// genuinely unchanged. False positives (skip when shouldn't)
/// require a 64-bit hash collision *and* a coincidence in the
/// recorded light pose — both vanishingly rare; the worst
/// observable artefact would be a one-frame stale shadow.
#[derive(Clone, Copy, PartialEq, Eq)]
struct PointShadowSlotState {
    light_bits: [u32; 4], // pos.x/y/z + radius, all to_bits()
    caster_hash: u64,
}

/// One screen-space heat-distortion source. The composite pass
/// picks the strongest of these each frame and applies a
/// noise-driven UV warp to the HDR sample. Pushed only by VFX
/// effects whose attached light has `heat_haze: true` —
/// passive scene lights (torches, ambient flames) are
/// excluded by design so the world doesn't shimmer
/// permanently around them.
#[derive(Clone, Copy, Debug)]
pub struct HeatSource {
    /// World-space origin (the same point as the source
    /// light). Projected to screen UV in the composite path.
    pub position: Vec3,
    /// Falloff radius in metres. Drives the on-screen extent
    /// of the warp via a perspective projection.
    pub radius: f32,
    /// Strength in `[0, 1]`. Drives both the warp amplitude
    /// and the noise scroll rate. Should fade to 0 alongside
    /// the source effect's animation.
    pub strength: f32,
}

impl Renderer {
    /// Materialise the four placeholder/default GPU resources the
    /// forward pipeline needs bound at startup:
    ///
    ///   * `default_texture` — a 32×32 magenta checkerboard bound to
    ///     set 0 / binding 1 so any object that hasn't received a
    ///     real albedo yet still rasterises (and the missing texture
    ///     is visually obvious during dev).
    ///   * `default_blood_field` — a 1×1 R16G16_SFLOAT zero texture
    ///     bound to set 0 / binding 4 so the forward shader's
    ///     `wet * intensity` term collapses to zero until a real
    ///     floor calls `recreate_blood_field`.
    ///   * `material_pool` — set-1 material descriptor pool plus its
    ///     own 1×1 white default albedo (uploaded via the same
    ///     command pool, then ready for `set_object_shared_material`
    ///     calls).
    ///   * `blood_field` — the per-floor blood splat subsystem,
    ///     constructed in its inactive (zero `world_xform`) state.
    ///     Owns its own render pass + splat pipeline + procgen
    ///     mask atlas; the pass is a no-op until a floor binds.
    ///
    /// All four uploads share a single throwaway command pool that
    /// is created and destroyed inside this helper — keeping the
    /// `new` orchestrator free of one-shot Vulkan plumbing.
    fn init_default_resources(
        device: &VulkanDevice,
        allocator: &Arc<Mutex<Allocator>>,
        shader_dir: &std::path::Path,
        uniforms: &UniformBuffers,
    ) -> Result<(Texture, Texture, MaterialPool, blood::BloodField)> {
        let command_pool_init =
            commands::create_command_pool(&device.device, device.graphics_queue_family)?;

        let default_texture = Texture::checkerboard(
            &device.device,
            allocator,
            device.graphics_queue,
            command_pool_init,
        )?;
        uniforms.bind_texture(
            &device.device,
            default_texture.view,
            default_texture.sampler,
        );

        // 1×1 zero-valued R16G16_SFLOAT texture (two 16-bit floats
        // encoded as zero = four zero bytes → wet=0, age=0).
        let default_blood_field = Texture::from_rgba_with_format(
            &device.device,
            allocator,
            device.graphics_queue,
            command_pool_init,
            1,
            1,
            &[0u8; 4],
            vk::Format::R16G16_SFLOAT,
        )?;
        uniforms.bind_blood_field(
            &device.device,
            default_blood_field.view,
            default_blood_field.sampler,
        );

        let material_pool = MaterialPool::new(
            &device.device,
            allocator,
            device.graphics_queue,
            command_pool_init,
        )?;

        let blood_field = blood::BloodField::new(
            &device.device,
            allocator,
            device.graphics_queue,
            command_pool_init,
            shader_dir,
        )?;

        unsafe {
            device.device.destroy_command_pool(command_pool_init, None);
        }

        Ok((
            default_texture,
            default_blood_field,
            material_pool,
            blood_field,
        ))
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
        )?;

        // Determine shader directory (relative to executable or workspace)
        let shader_dir = find_shader_dir();

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
            overlay,
            overlay_batch,
            vfx_system,
            vfx_ribbon_renderer,
            vfx_particle_renderer,
            skin_system,
            deletion_queue: Vec::new(),
            // Per-frame scratch lists. Pre-sized to avoid the
            // first-frame allocator dance; they grow naturally
            // as `objects` does so the steady-state cost is
            // zero allocations.
            draw_scratch: Vec::with_capacity(256),
            point_shadow_draw_scratch: Vec::with_capacity(64),
            shadow_draw_scratch: Vec::with_capacity(256),
            point_shadow_state: [None; shadow_point::MAX_POINT_SHADOWS],
            clear_color: [0.008, 0.006, 0.010, 1.0],
            fog_color: [0.018, 0.012, 0.010],
            fog_start: 5.0,
            fog_end: 16.0,
            fog_origin: Vec3::ZERO,
            wall_xray_strength: 0.0,
            player_room_aabb: Vec4::ZERO,
            point_lights: Vec::new(),
            vfx_lights: Vec::new(),
            heat_sources: Vec::new(),
            sky: SkyConfig::default(),
            sky_renderer,
            bloom: BloomConfig::default(),
            key_light: KeyLight::default(),
            ghost_mix: 0.0,
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
        self.pipeline = new_pipeline;
        self.pipeline_layout = new_layout;

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

    pub fn window_extent(&self) -> [u32; 2] {
        self.window_extent
    }

    /// Bind a new per-floor blood field that covers the world-space
    /// XZ box from `min` to `max`. Wipes any pending splats and
    /// rebinds the field texture to set 0 / binding 4 so the forward
    /// shader samples this floor's accumulation image rather than
    /// the placeholder. Call once at floor build time after walls
    /// are populated.
    pub fn recreate_blood_field(
        &mut self,
        min_xz: glam::Vec2,
        max_xz: glam::Vec2,
        floor_y_min: f32,
        floor_y_max: f32,
    ) {
        // The descriptor set at binding 4 is referenced by the
        // forward pipeline's command buffers. Without
        // VK_EXT_descriptor_indexing's UPDATE_AFTER_BIND /
        // UPDATE_UNUSED_WHILE_PENDING flags on this binding,
        // calling vkUpdateDescriptorSets while the previous
        // frame's command buffer is still in the pending state
        // is a validation error. Floor transitions are rare
        // and already pay GPU stalls elsewhere (texture
        // uploads, mesh creation), so wait for idle here.
        unsafe {
            self.device.device.device_wait_idle().ok();
        }
        self.blood_field
            .bind_floor(min_xz, max_xz, floor_y_min, floor_y_max);
        self.uniforms.bind_blood_field(
            &self.device.device,
            self.blood_field.field_view,
            self.blood_field.field_sampler,
        );
    }

    /// Unbind the per-floor blood field (e.g. on hub entry). The
    /// shader sampler points back at the 1\u00d71 placeholder so the
    /// composite contributes nothing.
    pub fn unbind_blood_field(&mut self) {
        // See `recreate_blood_field`: rebinding binding 4 while
        // a previous frame still holds the descriptor set in
        // its command buffer is a validation error without
        // descriptor-indexing's UPDATE_AFTER_BIND. Stall once
        // — hub entry is rare.
        unsafe {
            self.device.device.device_wait_idle().ok();
        }
        self.blood_field.unbind();
        self.uniforms.bind_blood_field(
            &self.device.device,
            self.default_blood_field.view,
            self.default_blood_field.sampler,
        );
    }

    pub fn add_mesh(&mut self, mesh: &Mesh, model_matrix: Mat4) -> Result<()> {
        let vertex_buffer = buffer::create_device_local_buffer(
            &self.device.device,
            &self.allocator,
            self.device.graphics_queue,
            self.command_pool,
            &mesh.vertices,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            "vertex_buffer",
        )?;

        let index_buffer = buffer::create_device_local_buffer(
            &self.device.device,
            &self.allocator,
            self.device.graphics_queue,
            self.command_pool,
            &mesh.indices,
            vk::BufferUsageFlags::INDEX_BUFFER,
            "index_buffer",
        )?;

        // Compute bounding sphere radius from vertices
        let bounds_radius = mesh
            .vertices
            .iter()
            .map(|v| v.position.length())
            .fold(0.0_f32, f32::max);

        self.objects.push(RenderObject {
            vertex_buffer,
            index_buffer,
            index_count: mesh.indices.len() as u32,
            model_matrix,
            bounds_radius,
            dynamic_vertex_buffers: None,
            skin_handle: None,
            material_set: self.material_pool.default_set,
            texture: None,
            tint: [1.0, 1.0, 1.0, 1.0],
            material_params: [1.0, 0.0, 0.0, 0.0],
            casts_shadow: true,
        });

        Ok(())
    }

    /// Add a mesh whose vertex data will be re-uploaded each frame from the
    /// CPU (e.g. CPU skinning). Allocates one host-visible vertex buffer per
    /// in-flight frame so the renderer can write next-frame data while the
    /// GPU is still reading the previous one. The index buffer stays
    /// device-local since topology is constant.
    ///
    /// Returns the object index (same convention as `add_mesh`). The initial
    /// per-frame buffers are populated with `mesh.vertices` so the object
    /// renders correctly before the first `update_dynamic_vertices` call.
    pub fn add_dynamic_mesh(&mut self, mesh: &Mesh, model_matrix: Mat4) -> Result<usize> {
        // Index buffer: device-local, one-shot upload.
        let index_buffer = buffer::create_device_local_buffer(
            &self.device.device,
            &self.allocator,
            self.device.graphics_queue,
            self.command_pool,
            &mesh.indices,
            vk::BufferUsageFlags::INDEX_BUFFER,
            "dynamic_index_buffer",
        )?;

        // Vertex buffers: one host-visible per in-flight frame.
        let mut dynamic_vertex_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            let buf = buffer::create_host_buffer(
                &self.device.device,
                &self.allocator,
                &mesh.vertices,
                vk::BufferUsageFlags::VERTEX_BUFFER,
                &format!("dynamic_vertex_buffer[{}]", i),
            )?;
            dynamic_vertex_buffers.push(buf);
        }

        // Use the first dynamic buffer as the "primary" handle so cleanup is
        // uniform; we'll move the rest into the Option vec below. Actually we
        // keep a separate dummy for `vertex_buffer` so the field is always
        // populated. To avoid wasting memory we make a tiny 16-byte placeholder.
        let placeholder = buffer::create_host_buffer(
            &self.device.device,
            &self.allocator,
            &[0u8; 16],
            vk::BufferUsageFlags::VERTEX_BUFFER,
            "dynamic_vertex_placeholder",
        )?;

        let bounds_radius = mesh
            .vertices
            .iter()
            .map(|v| v.position.length())
            .fold(0.0_f32, f32::max);

        self.objects.push(RenderObject {
            vertex_buffer: placeholder,
            index_buffer,
            index_count: mesh.indices.len() as u32,
            model_matrix,
            bounds_radius,
            dynamic_vertex_buffers: Some(dynamic_vertex_buffers),
            skin_handle: None,
            material_set: self.material_pool.default_set,
            texture: None,
            tint: [1.0, 1.0, 1.0, 1.0],
            material_params: [1.0, 0.0, 0.0, 0.0],
            casts_shadow: true,
        });

        Ok(self.objects.len() - 1)
    }

    /// Write `vertices` into the dynamic vertex buffer for the *next* frame
    /// the renderer will record (i.e. `current_frame`). Safe to call any
    /// time before `render` for that frame. No-op if the object isn't
    /// dynamic or `obj_idx` is out of range.
    ///
    /// `vertices.len()` must equal the original mesh's vertex count
    /// (vertex buffers are not resized).
    pub fn update_dynamic_vertices(&mut self, obj_idx: usize, vertices: &[Vertex]) {
        let frame = self.current_frame;
        if let Some(obj) = self.objects.get_mut(obj_idx) {
            if let Some(bufs) = obj.dynamic_vertex_buffers.as_mut() {
                if let Some(buf) = bufs.get_mut(frame) {
                    let needed = (std::mem::size_of::<Vertex>() * vertices.len()) as u64;
                    if needed <= buf.size {
                        buf.write(vertices);
                    } else {
                        log::warn!(
                            "update_dynamic_vertices: data {} bytes exceeds buffer {} bytes (obj {})",
                            needed, buf.size, obj_idx,
                        );
                    }
                }
            }
        }
    }

    /// Register a skinned mesh with the GPU skinning system and create a
    /// `RenderObject` that draws from the compute shader's output buffer.
    /// Replaces the legacy `add_dynamic_mesh` + per-frame CPU `skin_to`
    /// pipeline for any mesh whose vertices are produced by skinning.
    ///
    /// `rest_vertices` is the unskinned bind pose; `skin_data` carries the
    /// per-vertex `(joints, weights)` influences (length must match);
    /// `inflate` pushes every output vertex along its skinned normal by
    /// that many world units (use `0.0` for body, ~`0.022` for outfit
    /// shells so they sit just outside the body and don't z-fight).
    ///
    /// Returns the new object index so callers can later `update_palette`
    /// and bind material textures the same way as for static meshes.
    pub fn add_skinned_mesh(
        &mut self,
        rest_vertices: &[Vertex],
        skin_data: &[VertexSkin],
        indices: &[u32],
        model_matrix: Mat4,
        inflate: f32,
    ) -> Result<usize> {
        let handle = self.skin_system.register_mesh(
            &self.device.device,
            &self.allocator,
            self.device.graphics_queue,
            self.command_pool,
            rest_vertices,
            skin_data,
            inflate,
        )?;

        // Index buffer: device-local, immutable — same as a static mesh.
        let index_buffer = buffer::create_device_local_buffer(
            &self.device.device,
            &self.allocator,
            self.device.graphics_queue,
            self.command_pool,
            indices,
            vk::BufferUsageFlags::INDEX_BUFFER,
            "skinned_index_buffer",
        )?;

        // Tiny placeholder so `vertex_buffer` always has a real handle to
        // clean up. Draw loop ignores it whenever `skin_handle` is set.
        let placeholder = buffer::create_host_buffer(
            &self.device.device,
            &self.allocator,
            &[0u8; 16],
            vk::BufferUsageFlags::VERTEX_BUFFER,
            "skinned_vertex_placeholder",
        )?;

        let bounds_radius = rest_vertices
            .iter()
            .map(|v| v.position.length())
            .fold(0.0_f32, f32::max);

        self.objects.push(RenderObject {
            vertex_buffer: placeholder,
            index_buffer,
            index_count: indices.len() as u32,
            model_matrix,
            bounds_radius,
            dynamic_vertex_buffers: None,
            skin_handle: Some(handle),
            material_set: self.material_pool.default_set,
            texture: None,
            tint: [1.0, 1.0, 1.0, 1.0],
            material_params: [1.0, 0.0, 0.0, 0.0],
            casts_shadow: true,
        });

        Ok(self.objects.len() - 1)
    }

    /// Upload a fresh bone palette for the GPU skinner. Mirrors the old
    /// CPU path's per-frame `skin_to` step — call once per visible
    /// skinned object before `render`.
    pub fn update_palette(&mut self, obj_idx: usize, palette: &[Mat4]) {
        let frame = self.current_frame;
        let handle = match self.objects.get(obj_idx).and_then(|o| o.skin_handle) {
            Some(h) => h,
            None => return,
        };
        self.skin_system.update_palette(frame, handle, palette);
    }

    /// Release the GPU skinning resources backing this object's
    /// mesh. Buffers are deferred for `MAX_FRAMES_IN_FLIGHT` frames
    /// before destruction so any in-flight pass that already
    /// recorded a reference to them finishes safely. The
    /// `RenderObject` itself is *not* removed (`object_index`
    /// references elsewhere stay valid); we just clear its
    /// `skin_handle` and zero its model matrix so it disappears
    /// from the draw list. Cheap to call repeatedly: a no-op once
    /// the slot is already free.
    pub fn free_skinned_mesh(&mut self, obj_idx: usize) {
        let obj = match self.objects.get_mut(obj_idx) {
            Some(o) => o,
            None => return,
        };
        if let Some(handle) = obj.skin_handle.take() {
            self.skin_system.free_mesh(handle);
        }
        obj.model_matrix = Mat4::ZERO;
    }

    /// True if this object was created via `add_dynamic_mesh`.
    pub fn is_dynamic_mesh(&self, obj_idx: usize) -> bool {
        self.objects
            .get(obj_idx)
            .map(|o| o.dynamic_vertex_buffers.is_some())
            .unwrap_or(false)
    }

    /// Bind a base-color texture to the object at `obj_idx`. The
    /// texture is decoded according to `src` (file path, raw bytes,
    /// procedural pixels, or pre-decoded buffer) and owned by the
    /// renderer; it's freed when the renderer is dropped or the
    /// object is removed via `clear_objects`.
    ///
    /// Replaces the previous `set_object_texture` /
    /// `set_object_texture_from_bytes` pair.
    pub fn set_object_texture(&mut self, obj_idx: usize, src: TextureSource<'_>) -> Result<()> {
        if obj_idx >= self.objects.len() {
            return Ok(());
        }
        let texture = Texture::load(
            &self.device.device,
            &self.allocator,
            self.device.graphics_queue,
            self.command_pool,
            src,
        )?;
        let set = self
            .material_pool
            .alloc_set(&self.device.device, &texture)?;
        let obj = &mut self.objects[obj_idx];
        // Only stall the GPU when there's an existing per-object
        // texture to free; first-time binding can skip the wait
        // since the default material set was never written to
        // anything we're about to drop.
        if obj.texture.is_some() {
            unsafe {
                self.device.device.device_wait_idle().ok();
            }
            if let Some(mut old) = obj.texture.take() {
                old.cleanup(&self.device.device, &self.allocator);
            }
        }
        obj.texture = Some(texture);
        obj.material_set = set;
        Ok(())
    }

    /// Upload a texture from `src` and return both the texture
    /// handle and a freshly-allocated descriptor set bound to it.
    /// The caller owns the texture and must keep it alive for as
    /// long as the descriptor set is bound to any object — pair
    /// with [`Self::set_object_shared_material`] to share one
    /// texture across many objects (e.g. one set per monster
    /// archetype rather than per spawn) so the per-pool descriptor
    /// budget doesn't blow up when a floor spawns dozens of
    /// enemies.
    ///
    /// Replaces the previous `upload_shared_texture_from_bytes` /
    /// `_from_rgba` / `_from_file` / `_decoded` quartet.
    pub fn upload_shared_texture(
        &mut self,
        src: TextureSource<'_>,
    ) -> Result<(Texture, vk::DescriptorSet)> {
        let texture = Texture::load(
            &self.device.device,
            &self.allocator,
            self.device.graphics_queue,
            self.command_pool,
            src,
        )?;
        let set = self
            .material_pool
            .alloc_set(&self.device.device, &texture)?;
        Ok((texture, set))
    }

    /// Upload a full PBR material described by `src` and bind every
    /// loaded channel into a fresh per-object descriptor set.
    /// Missing channels (`None`, or absent from a `Decoded` pack)
    /// fall back to the pool's neutral defaults so the shader's
    /// PBR path degrades gracefully. Color textures (basecolor)
    /// are decoded SRGB; data textures (normal / MR / AO / height)
    /// stay linear (UNORM) so the GPU doesn't gamma-correct
    /// numeric data.
    ///
    /// Replaces the previous `upload_shared_pbr_material` /
    /// `_split_mr` / `_decoded` trio.
    pub fn upload_shared_pbr_material(
        &mut self,
        src: PbrSource<'_>,
    ) -> Result<(Vec<Texture>, vk::DescriptorSet)> {
        // Each match arm builds an `owned: Vec<Texture>` where
        // index 0 is the basecolor and the optional channel
        // indices (`normal_idx`, `mr_idx`, `ao_idx`, `height_idx`)
        // re-borrow into `owned` after every push has settled.
        let (owned, normal_idx, mr_idx, ao_idx, height_idx) = match src {
            PbrSource::Files {
                basecolor,
                normal,
                metallic_roughness,
                ao,
                height,
            } => {
                let basecolor = Texture::from_file(
                    &self.device.device,
                    &self.allocator,
                    self.device.graphics_queue,
                    self.command_pool,
                    basecolor,
                )?;
                let mut owned: Vec<Texture> = vec![basecolor];
                let mut load_linear = |path: Option<&Path>| -> Result<Option<usize>> {
                    let Some(p) = path else { return Ok(None) };
                    let t = Texture::from_file_linear(
                        &self.device.device,
                        &self.allocator,
                        self.device.graphics_queue,
                        self.command_pool,
                        p,
                    )?;
                    owned.push(t);
                    Ok(Some(owned.len() - 1))
                };
                let normal_idx = load_linear(normal)?;
                let mr_idx = load_linear(metallic_roughness)?;
                let ao_idx = load_linear(ao)?;
                let height_idx = load_linear(height)?;
                (owned, normal_idx, mr_idx, ao_idx, height_idx)
            }
            PbrSource::FilesSplitMr {
                basecolor,
                normal,
                metallic,
                roughness,
                ao,
                height,
            } => {
                let basecolor = Texture::from_file(
                    &self.device.device,
                    &self.allocator,
                    self.device.graphics_queue,
                    self.command_pool,
                    basecolor,
                )?;
                let mut owned: Vec<Texture> = vec![basecolor];
                let mut load_linear = |path: Option<&Path>| -> Result<Option<usize>> {
                    let Some(p) = path else { return Ok(None) };
                    let t = Texture::from_file_linear(
                        &self.device.device,
                        &self.allocator,
                        self.device.graphics_queue,
                        self.command_pool,
                        p,
                    )?;
                    owned.push(t);
                    Ok(Some(owned.len() - 1))
                };
                let normal_idx = load_linear(normal)?;
                let ao_idx = load_linear(ao)?;
                let height_idx = load_linear(height)?;
                // Pack metallic + roughness into a single UNORM
                // RGBA image. `metallic` lands in R, `roughness`
                // in G, B/A unused. Path resolution funnels
                // through `asset_decode::resolve_asset_path` so
                // callers can pass `assets/...` from any cwd.
                let mr_idx = if metallic.is_some() || roughness.is_some() {
                    use crate::renderer::asset_decode::resolve_asset_path;
                    let metallic_img = if let Some(p) = metallic {
                        Some(image::open(resolve_asset_path(p)?)?.to_luma8())
                    } else {
                        None
                    };
                    let roughness_img = if let Some(p) = roughness {
                        Some(image::open(resolve_asset_path(p)?)?.to_luma8())
                    } else {
                        None
                    };
                    let (w, h) = match (&metallic_img, &roughness_img) {
                        (Some(m), Some(r)) => {
                            if m.dimensions() != r.dimensions() {
                                return Err(anyhow::anyhow!(
                                    "metallic and roughness map dimensions differ: {:?} vs {:?}",
                                    m.dimensions(),
                                    r.dimensions()
                                ));
                            }
                            m.dimensions()
                        }
                        (Some(m), None) => m.dimensions(),
                        (None, Some(r)) => r.dimensions(),
                        (None, None) => unreachable!(),
                    };
                    let mut packed = vec![0u8; (w * h * 4) as usize];
                    for i in 0..(w * h) as usize {
                        packed[i * 4 + 0] =
                            metallic_img.as_ref().map(|m| m.as_raw()[i]).unwrap_or(0);
                        packed[i * 4 + 1] =
                            roughness_img.as_ref().map(|r| r.as_raw()[i]).unwrap_or(255);
                        packed[i * 4 + 2] = 0;
                        packed[i * 4 + 3] = 255;
                    }
                    let tex = Texture::from_rgba_with_format(
                        &self.device.device,
                        &self.allocator,
                        self.device.graphics_queue,
                        self.command_pool,
                        w,
                        h,
                        &packed,
                        vk::Format::R8G8B8A8_UNORM,
                    )?;
                    owned.push(tex);
                    Some(owned.len() - 1)
                } else {
                    None
                };
                (owned, normal_idx, mr_idx, ao_idx, height_idx)
            }
            PbrSource::Decoded(pack) => {
                let crate::renderer::asset_decode::DecodedPbrPack {
                    name: _,
                    basecolor,
                    normal,
                    mr,
                    ao,
                    height,
                } = pack;
                let upload = |this: &Renderer,
                              d: crate::renderer::asset_decode::DecodedTexture|
                 -> Result<Texture> {
                    Texture::from_decoded(
                        &this.device.device,
                        &this.allocator,
                        this.device.graphics_queue,
                        this.command_pool,
                        &d,
                    )
                };
                let mut owned: Vec<Texture> = Vec::with_capacity(5);
                owned.push(upload(self, basecolor)?);
                let push_opt =
                    |opt: Option<_>, owned: &mut Vec<Texture>| -> Result<Option<usize>> {
                        if let Some(d) = opt {
                            owned.push(upload(self, d)?);
                            Ok(Some(owned.len() - 1))
                        } else {
                            Ok(None)
                        }
                    };
                let normal_idx = push_opt(normal, &mut owned)?;
                let mr_idx = push_opt(mr, &mut owned)?;
                let ao_idx = push_opt(ao, &mut owned)?;
                let height_idx = push_opt(height, &mut owned)?;
                (owned, normal_idx, mr_idx, ao_idx, height_idx)
            }
        };

        let basecolor_ref = &owned[0];
        let set = self.material_pool.alloc_pbr_set(
            &self.device.device,
            basecolor_ref,
            normal_idx.map(|i| &owned[i]),
            mr_idx.map(|i| &owned[i]),
            ao_idx.map(|i| &owned[i]),
            height_idx.map(|i| &owned[i]),
        )?;
        Ok((owned, set))
    }

    /// Bind a previously-allocated shared descriptor set to an object.
    /// Unlike `set_object_texture*`, the renderer does NOT take
    /// ownership of any texture — the caller must keep the underlying
    /// `Texture` alive (typically inside a long-lived asset cache).
    pub fn set_object_shared_material(&mut self, obj_idx: usize, set: vk::DescriptorSet) {
        if obj_idx >= self.objects.len() {
            return;
        }
        let obj = &mut self.objects[obj_idx];
        if obj.texture.is_some() {
            unsafe {
                self.device.device.device_wait_idle().ok();
            }
            if let Some(mut old) = obj.texture.take() {
                old.cleanup(&self.device.device, &self.allocator);
            }
        }
        obj.material_set = set;
    }

    /// Set the per-object PBR / sampling tweaks pushed at offset
    /// 80 of the per-draw push-constant range. Layout matches
    /// the `material_params` field on [`RenderObject`]:
    /// `(uv_scale, parallax_scale, flags, _reserved)`. Bit 0
    /// of `flags` (numeric value `1.0`) flips the shader into
    /// PBR + normal-mapping mode and starts reading the
    /// material set's normal / MR / AO / height bindings.
    pub fn set_object_material_params(&mut self, obj_idx: usize, params: [f32; 4]) {
        if let Some(obj) = self.objects.get_mut(obj_idx) {
            obj.material_params = params;
        }
    }

    /// Toggle whether this object contributes to the shadow
    /// passes. Set to `false` for large flat receivers (hub
    /// platform, dune ring, dungeon floor) whose tiny height-
    /// map relief would never produce a perceptible cast
    /// shadow but whose triangle count blows up the per-light
    /// cube atlas pass. The lit-pass heightmap-displaced
    /// shadow position + heightmap self-shadow already give
    /// those surfaces a believable shadow boundary against
    /// their own ripples without rasterising them into the
    /// shadow maps.
    pub fn set_object_casts_shadow(&mut self, obj_idx: usize, casts: bool) {
        if let Some(obj) = self.objects.get_mut(obj_idx) {
            obj.casts_shadow = casts;
        }
    }

    /// Replace mesh data at an existing object index.
    /// Old buffers are deferred for safe deletion after in-flight frames complete.
    pub fn replace_mesh(&mut self, obj_idx: usize, mesh: &Mesh) -> Result<()> {
        if obj_idx >= self.objects.len() {
            return Ok(());
        }

        let vertex_buffer = buffer::create_device_local_buffer(
            &self.device.device,
            &self.allocator,
            self.device.graphics_queue,
            self.command_pool,
            &mesh.vertices,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            "vertex_buffer",
        )?;

        let index_buffer = buffer::create_device_local_buffer(
            &self.device.device,
            &self.allocator,
            self.device.graphics_queue,
            self.command_pool,
            &mesh.indices,
            vk::BufferUsageFlags::INDEX_BUFFER,
            "index_buffer",
        )?;

        let bounds_radius = mesh
            .vertices
            .iter()
            .map(|v| v.position.length())
            .fold(0.0_f32, f32::max);

        // Defer old buffer destruction — in-flight command buffers may still reference them
        let old = &mut self.objects[obj_idx];
        let retire_frame = self.frame_count + MAX_FRAMES_IN_FLIGHT as u64;
        let old_vb = std::mem::replace(&mut old.vertex_buffer, vertex_buffer);
        let old_ib = std::mem::replace(&mut old.index_buffer, index_buffer);
        self.deletion_queue.push((retire_frame, old_vb));
        self.deletion_queue.push((retire_frame, old_ib));
        old.index_count = mesh.indices.len() as u32;
        old.bounds_radius = bounds_radius;
        // Static path: ensure no stale dynamic buffers linger.
        if let Some(dyn_bufs) = old.dynamic_vertex_buffers.take() {
            for buf in dyn_bufs {
                self.deletion_queue.push((retire_frame, buf));
            }
        }

        Ok(())
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

    /// Clear all render objects, deferring GPU buffer destruction until safe.
    pub fn clear_objects(&mut self) {
        // Wait for all GPU work to complete before destroying buffers
        unsafe {
            self.device.device.device_wait_idle().ok();
        }
        // Reset all command buffers so validation doesn't consider buffers "in use"
        for &cmd in &self.command_buffers {
            unsafe {
                self.device
                    .device
                    .reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())
                    .ok();
            }
        }
        // Now safe to destroy immediately + flush any pending deferred deletions
        for mut obj in self.objects.drain(..) {
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
        // Wipe every GPU-skinning slot so the next floor's monsters
        // don't immediately blow past the registration cap (objects
        // and skin slots are 1:1; the indices we just dropped above
        // referenced these slots).
        self.skin_system.clear(&self.device.device, &self.allocator);
        // Also flush deferred deletion queue since everything is idle
        let device = &self.device.device;
        let allocator = &self.allocator;
        for (_, mut buf) in self.deletion_queue.drain(..) {
            buf.cleanup(device, allocator);
        }
    }

    /// Flush deletion queue: destroy buffers whose retire frame has passed.
    fn flush_deletions(&mut self) {
        let current = self.frame_count;
        let device = &self.device.device;
        let allocator = &self.allocator;
        self.deletion_queue.retain_mut(|(retire_frame, buf)| {
            if current >= *retire_frame {
                unsafe {
                    device.destroy_buffer(buf.buffer, None);
                }
                if let Some(alloc) = buf.allocation.take() {
                    allocator.lock().unwrap().free(alloc).ok();
                }
                false
            } else {
                true
            }
        });
    }

    /// Load a glTF/GLB file and add all meshes with the given transform.
    pub fn load_gltf(&mut self, path: &std::path::Path, model_matrix: Mat4) -> Result<()> {
        let scene = crate::resources::gltf_loader::load_gltf(path)?;
        for mesh in &scene.meshes {
            self.add_mesh(mesh, model_matrix)?;
        }
        Ok(())
    }

    /// Notify the renderer that the window has been resized.
    pub fn notify_resized(&mut self, width: u32, height: u32) {
        self.framebuffer_resized = true;
        self.window_extent = [width, height];
    }

    /// Wait until the GPU has finished any prior submission that used the
    /// resources for the upcoming frame. Call this BEFORE writing into any
    /// per-frame host-visible buffer (e.g. dynamic vertex buffers for CPU
    /// skinning) and then call `draw_frame` afterwards. Calling `draw_frame`
    /// without `prepare_frame` is still safe — it does the same wait
    /// internally — but then per-frame writes done before `draw_frame` may
    /// race with in-flight GPU reads.
    pub fn prepare_frame(&mut self) -> Result<()> {
        if self.window_extent[0] == 0 || self.window_extent[1] == 0 {
            return Ok(());
        }
        let frame = self.current_frame;
        unsafe {
            self.device.device.wait_for_fences(
                &[self.frame_sync.in_flight[frame]],
                true,
                u64::MAX,
            )?;
        }
        Ok(())
    }

    /// Drive incremental icon-atlas streaming. Decodes + uploads
    /// up to `budget` PNGs from `assets/icons/` per call so the
    /// loading screen can stay responsive while ~hundreds of
    /// icons are processed. Returns `(loaded, total)` for
    /// progress reporting; loading is complete when
    /// `loaded == total`.
    pub fn step_load_icons(&mut self, budget: usize) -> Result<(usize, usize)> {
        self.overlay.step_load_icons(
            &self.device.device,
            &self.allocator,
            self.device.graphics_queue,
            self.command_pool,
            budget,
        )
    }

    /// Total icons discovered at startup (for progress UI).
    pub fn total_icons(&self) -> usize {
        self.overlay.total_icons()
    }
    /// Icons whose decode + upload has completed.
    pub fn loaded_icons(&self) -> usize {
        self.overlay.loaded_icons()
    }

    // ---- draw_frame helpers --------------------------------------------
    //
    // `draw_frame` itself is the orchestrator: it acquires a swapchain
    // image, builds per-frame data, records every render pass via the
    // helpers below, and submits + presents. Each helper owns one
    // logical phase so the pass structure is readable top-to-bottom in
    // `draw_frame`.

    /// Merge `point_lights` and `vfx_lights` into a single
    /// `[PointLight; MAX_POINT_LIGHTS]` with a deliberate
    /// ordering:
    ///
    ///   slots [0..n_shadow)     shadow-casting torches
    ///                           (`point_lights`, capped at
    ///                           `MAX_POINT_SHADOWS = 8`)
    ///   slots [n_shadow..M)     VFX lights — additive only,
    ///                           never cast shadows. A
    ///                           projectile-trail light sits
    ///                           *inside* the projectile mesh, so
    ///                           any cube shadow rendered for it
    ///                           would be occluded in every
    ///                           outward direction (back-faces of
    ///                           the fireball) and the world
    ///                           would go pitch black around it.
    ///   slots [M..16)           remaining torches as additive
    ///                           lights.
    ///
    /// The shader uses `lightIdx < pointShadowMeta.x` as
    /// the shadow-test, so shadow-casters MUST occupy the leading
    /// prefix. VFX lives just past that prefix, which also
    /// reserves it a slot even in dense torch rooms (worst
    /// case: 8 shadowed + 2 VFX + 6 plain = 16).
    ///
    /// Returns `(merged, light_count, n_shadow)`.
    fn merge_per_frame_lights(&self) -> ([PointLight; MAX_POINT_LIGHTS], usize, usize) {
        let n_shadow = self.point_lights.len().min(shadow_point::MAX_POINT_SHADOWS);
        // Build the merged light list directly into a stack
        // array — saves the per-frame heap allocation that
        // a `.chain().take(N).collect()` version would do.
        // The default-init `PointLight` value never ships to
        // the GPU because `light_count` bounds every consumer.
        const DEFAULT_LIGHT: PointLight = PointLight {
            position: Vec3::ZERO,
            color: Vec3::ZERO,
            radius: 0.0,
            intensity: 0.0,
        };
        let mut merged = [DEFAULT_LIGHT; MAX_POINT_LIGHTS];
        let mut count = 0usize;
        let mut push = |src: PointLight, count: &mut usize| {
            if *count < MAX_POINT_LIGHTS {
                merged[*count] = src;
                *count += 1;
            }
        };
        for pl in self.point_lights.iter().take(n_shadow) {
            push(*pl, &mut count);
        }
        for pl in self.vfx_lights.iter() {
            push(*pl, &mut count);
        }
        for pl in self.point_lights.iter().skip(n_shadow) {
            push(*pl, &mut count);
        }
        (merged, count, n_shadow)
    }

    /// Build the per-face VPs for the point-light cube shadow atlas.
    /// Only the first `point_shadow_count` slots are populated; the
    /// rest stay identity (the shader only samples the active range
    /// via `pointShadowMeta.x`).
    fn build_point_shadow_face_vp(
        &self,
        point_shadow_count: usize,
        merged_lights: &[PointLight; MAX_POINT_LIGHTS],
    ) -> [Mat4; shadow_point::MAX_POINT_SHADOWS * 6] {
        let mut face_vp = [Mat4::IDENTITY; shadow_point::MAX_POINT_SHADOWS * 6];
        for (i, pl) in merged_lights.iter().take(point_shadow_count).enumerate() {
            let faces = shadow_point::cube_face_view_projs(pl.position, pl.radius.max(0.1));
            for (f, m) in faces.iter().enumerate() {
                face_vp[i * 6 + f] = *m;
            }
        }
        face_vp
    }

    /// Build the camera/lighting/fog UBO from current renderer
    /// state plus the merged per-frame light list.
    fn build_uniform_data(
        &self,
        merged_lights: &[PointLight; MAX_POINT_LIGHTS],
        light_count: usize,
        point_shadow_count: usize,
        point_shadow_face_vp: [Mat4; shadow_point::MAX_POINT_SHADOWS * 6],
    ) -> UniformData {
        let mut point_light_pos = [Vec4::ZERO; MAX_POINT_LIGHTS];
        let mut point_light_color = [Vec4::ZERO; MAX_POINT_LIGHTS];
        for (i, pl) in merged_lights.iter().take(light_count).enumerate() {
            point_light_pos[i] = Vec4::new(pl.position.x, pl.position.y, pl.position.z, pl.radius);
            point_light_color[i] = Vec4::new(pl.color.x, pl.color.y, pl.color.z, pl.intensity);
        }

        let light_dir_world = Vec4::new(
            self.key_light.direction.x,
            self.key_light.direction.y,
            self.key_light.direction.z,
            0.0,
        );
        let light_dir_normalized = light_dir_world.normalize();
        // Snap the shadow focus to the camera *target* (the
        // player / look-at point) projected onto y=0 — NOT the
        // camera position. The camera sits behind+above the
        // player, so anchoring the 28 m ortho box on the camera
        // makes the shadow frustum extend mostly behind the
        // player; the in-front cutoff lands only a few metres
        // past the player and reads as a square that tracks
        // the camera. Using `target` re-centres the box on the
        // player so the cutoff is symmetric and far enough out
        // in every direction that the edge feather in
        // `sampleShadow` hides it. The shadow module further
        // snaps to texel size to suppress shimmering.
        let shadow_focus = Vec3::new(self.camera.target.x, 0.0, self.camera.target.z);
        let light_vp = shadow::light_view_proj(
            shadow_focus,
            Vec3::new(
                light_dir_normalized.x,
                light_dir_normalized.y,
                light_dir_normalized.z,
            ),
        );

        UniformData {
            view: self.camera.view_matrix(),
            proj: self.camera.projection_matrix(),
            camera_pos: Vec4::new(
                self.camera.position.x,
                self.camera.position.y,
                self.camera.position.z,
                0.0,
            ),
            light_dir: light_dir_normalized,
            light_color: Vec4::new(
                self.key_light.color.x,
                self.key_light.color.y,
                self.key_light.color.z,
                self.key_light.ambient,
            ),
            fog_color: Vec4::new(self.fog_color[0], self.fog_color[1], self.fog_color[2], 0.0),
            fog_params: Vec4::new(self.fog_start, self.fog_end, 0.0, 0.0),
            fog_origin: Vec4::new(
                self.fog_origin.x,
                self.fog_origin.y,
                self.fog_origin.z,
                self.wall_xray_strength,
            ),
            point_light_pos,
            point_light_color,
            point_light_count: Vec4::new(light_count as f32, 0.0, 0.0, 0.0),
            light_vp,
            point_shadow_face_vp,
            point_shadow_meta: Vec4::new(point_shadow_count as f32, 0.0, 0.0, 0.0),
            // `time` packs scalar globals consumed by the
            // forward fragment shader:
            //   x = elapsed seconds (used by VFX time-driven
            //       hashes and the blood splat age curve)
            //   y = floor_y_min   (lowest walkable plane Y)
            //   z = floor_y_max   (highest walkable plane Y)
            //   w = unused
            // The blood-field shader gate accepts fragments
            // whose Y is within a small epsilon of
            // `[floor_y_min, floor_y_max]` so dungeons with
            // raised platforms / lowered pits still receive
            // splats on every walkable surface.
            time: Vec4::new(
                self.start_time.elapsed().as_secs_f32(),
                self.blood_field.floor_y,
                self.blood_field.floor_y_max,
                0.0,
            ),
            blood_field_xform: self.blood_field.world_xform,
            player_room_aabb: self.player_room_aabb,
        }
    }

    /// Build this frame's two draw lists by walking `self.objects` once,
    /// applying frustum + fog culling for the visible-draw list and only
    /// fog culling for the shadow-caster list (off-screen casters can
    /// still project shadows onto visible floor).
    ///
    /// Reuses the per-renderer scratch Vecs via `mem::take` so the
    /// hot loop allocates zero heap per frame; the caller must restore
    /// the Vecs into `self.draw_scratch` / `self.shadow_draw_scratch`
    /// before the next frame.
    fn build_draw_lists(&mut self, frame: usize) -> (Vec<DrawCommand>, Vec<DrawCommand>) {
        let frustum = self.camera.frustum_planes();
        let fog_cull_dist = self.fog_end + 2.0; // small margin beyond fog end
        let mut draws = std::mem::take(&mut self.draw_scratch);
        let mut shadow_draws = std::mem::take(&mut self.shadow_draw_scratch);
        draws.clear();
        shadow_draws.clear();
        for obj in &self.objects {
            // Skip hidden objects (dead entities set matrix to zero).
            if obj.model_matrix == Mat4::ZERO {
                continue;
            }
            // Frustum cull: extract world-space center from model matrix col 3.
            let center = obj.model_matrix.w_axis.truncate();
            // Distance cull: skip objects fully outside the
            // fog volume. Anchored on `fog_origin` (player) to
            // match the shader's fog math — otherwise zooming
            // the camera out would pop in geometry the player
            // can still see.
            let dist_to_fog_origin = (center - self.fog_origin).length();
            if dist_to_fog_origin - obj.bounds_radius > fog_cull_dist {
                continue;
            }
            // Pick the GPU-skinner output VB if present, then fall
            // back to the legacy host-visible dynamic ring, then to
            // the static device-local VB. The skin handle wins over
            // dynamic_vertex_buffers because a freshly converted
            // mesh may briefly carry both during transition.
            let vb = match obj
                .skin_handle
                .and_then(|h| self.skin_system.output_vertex_buffer(h))
            {
                Some(b) => b,
                None => match obj.dynamic_vertex_buffers.as_ref() {
                    Some(bufs) => bufs[frame].buffer,
                    None => obj.vertex_buffer.buffer,
                },
            };
            let is_dynamic = obj.skin_handle.is_some() || obj.dynamic_vertex_buffers.is_some();
            let cmd = DrawCommand {
                vertex_buffer: vb,
                index_buffer: obj.index_buffer.buffer,
                index_count: obj.index_count,
                descriptor_set: self.uniforms.descriptor_sets[frame],
                material_set: obj.material_set,
                model_matrix: obj.model_matrix,
                bounds_radius: obj.bounds_radius,
                tint: obj.tint,
                material_params: obj.material_params,
                dynamic_vertices: is_dynamic,
            };
            // Shadow casters must include geometry outside the
            // camera frustum: a wall / prop behind the camera
            // can still project a shadow that falls on visible
            // floor in front of the camera. Cull only by the
            // player-anchored fog distance for the shadow list.
            // `casts_shadow=false` opts giant flat receivers
            // (hub platform, dune ring, dungeon floor) out of
            // every shadow pass entirely \u2014 they're a major
            // chunk of triangles and contribute no visible
            // cast shadows worth the GPU time.
            if obj.casts_shadow {
                shadow_draws.push(cmd.clone());
            }
            // Visible-draw list: also gate on the camera
            // frustum so we don't rasterise off-screen geometry
            // into the forward pass.
            if !self
                .camera
                .sphere_in_frustum(&frustum, center, obj.bounds_radius + 1.0)
            {
                continue;
            }
            draws.push(cmd);
        }
        (draws, shadow_draws)
    }

    /// Project the sun direction into screen UV for the godrays
    /// post pass. Returns `(sun_screen, sun_color)` where
    /// `sun_screen = [u, v, strength, _]`. Both go zero when
    /// the sky is disabled or the sun is behind the camera.
    fn compute_sun_screen_uv(&self) -> ([f32; 4], [f32; 4]) {
        if !self.sky.enabled || self.sky.sun_strength <= 0.001 {
            return ([0.0; 4], [0.0; 4]);
        }
        let view = self.camera.view_matrix();
        let proj = self.camera.projection_matrix();
        let sd = self.sky.sun_dir.normalize();
        // Direction in view space (w=0 → infinitely far).
        let view_dir = view.transform_vector3(sd);
        if view_dir.z >= -0.05 {
            return ([0.0; 4], [0.0; 4]);
        }
        // Sun in front of camera. Project a point far along
        // that direction (distance has no effect on UV under
        // perspective when the point is treated as on a ray
        // from the eye, but using a finite distance gives
        // well-behaved w).
        let world_pt = self.camera.position + sd * 1000.0;
        let clip = proj * view * Vec4::new(world_pt.x, world_pt.y, world_pt.z, 1.0);
        if clip.w <= 0.0 {
            return ([0.0; 4], [0.0; 4]);
        }
        let ndc = Vec3::new(clip.x, clip.y, clip.z) / clip.w;
        // Vulkan/GLSL UV: (ndc.xy * 0.5 + 0.5).
        // The renderer flips Y in proj so this matches the
        // depth-sample UV convention already in the composite
        // shader.
        let uv = Vec3::new(ndc.x, ndc.y, 0.0) * 0.5 + Vec3::new(0.5, 0.5, 0.0);
        // Strength scales with how centred the sun is in view
        // (cosine of angle to camera forward) so off-screen
        // rays still appear but fade as the sun leaves the
        // frustum.
        let cam_fwd = -Vec3::new(view.row(2).x, view.row(2).y, view.row(2).z).normalize();
        let cosine = sd.dot(cam_fwd).max(0.0);
        // Bumped from `0.6` to `1.0` so the sandstorm hub's
        // sun (sun_strength = 1.4) drives the post pass at
        // ~1.4 instead of ~0.84 — the rays read clearly
        // through the dust without the sun disc itself going
        // blowout. Clamped to 2.0 so future skies with
        // sun_strength > 2 don't over-saturate the composite.
        let strength = (self.sky.sun_strength * 1.0 * cosine).clamp(0.0, 2.0);
        (
            [uv.x, uv.y, strength, 1.0],
            [
                self.sky.cloud_flash_color.x.max(1.0),
                self.sky.cloud_flash_color.y.max(0.95),
                self.sky.cloud_flash_color.z.max(0.85),
                1.0,
            ],
        )
    }

    /// Project the strongest active VFX-published heat source to
    /// screen UV for the composite pass's heat-haze warp. Only one
    /// source is forwarded per frame; additional bursts take over
    /// when the strongest fades.
    fn compute_heat_source_uv(&self) -> [f32; 4] {
        let view = self.camera.view_matrix();
        let proj = self.camera.projection_matrix();
        let mut best: Option<(f32, [f32; 4])> = None;
        for hs in self.heat_sources.iter() {
            if hs.strength < 1e-3 {
                continue;
            }

            let world = Vec4::new(hs.position.x, hs.position.y, hs.position.z, 1.0);
            let view_p = view * world;
            if view_p.z >= -0.05 {
                continue;
            }
            let clip = proj * view_p;
            if clip.w <= 0.0 {
                continue;
            }
            let ndc = Vec3::new(clip.x, clip.y, clip.z) / clip.w;
            let uv = Vec3::new(ndc.x, ndc.y, 0.0) * 0.5 + Vec3::new(0.5, 0.5, 0.0);
            if uv.x < -0.2 || uv.x > 1.2 || uv.y < -0.2 || uv.y > 1.2 {
                continue;
            }
            let dist = (-view_p.z).max(0.1);
            // proj[1][1] is the y-focal term; with the
            // renderer's flipped-Y projection it's negative,
            // but we only want magnitude.
            let focal_y = proj.col(1).y.abs();
            let radius_uv = (hs.radius / dist) * focal_y * 0.5;
            if radius_uv < 0.02 {
                continue;
            }
            let s = hs.strength.clamp(0.0, 1.0);
            if best.map(|(prev, _)| s > prev).unwrap_or(true) {
                best = Some((s, [uv.x, uv.y, radius_uv.min(0.6), s]));
            }
        }
        best.map(|(_, v)| v).unwrap_or([0.0; 4])
    }

    /// Record the directional (sun/key-light) shadow pass.
    /// Renders scene depth from the light's POV into the single
    /// orthographic shadow map.
    ///
    /// SAFETY: caller must have an active command buffer recording
    /// (i.e. between `begin_command_buffer` and `end_command_buffer`).
    unsafe fn record_dir_shadow_pass(&self, cmd: vk::CommandBuffer, shadow_draws: &[DrawCommand]) {
        let device = &self.device.device;
        let shadow_clear = [vk::ClearValue {
            depth_stencil: vk::ClearDepthStencilValue {
                depth: 1.0,
                stencil: 0,
            },
        }];
        let shadow_rp_begin = vk::RenderPassBeginInfo::default()
            .render_pass(self.shadow_map.render_pass)
            .framebuffer(self.shadow_map.framebuffer)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: vk::Extent2D {
                    width: shadow::SHADOW_MAP_SIZE,
                    height: shadow::SHADOW_MAP_SIZE,
                },
            })
            .clear_values(&shadow_clear);
        device.cmd_begin_render_pass(cmd, &shadow_rp_begin, vk::SubpassContents::INLINE);
        device.cmd_bind_pipeline(
            cmd,
            vk::PipelineBindPoint::GRAPHICS,
            self.shadow_map.pipeline,
        );
        // The shadow pipeline reads only the global UBO (set 0),
        // and every draw uses the same per-frame descriptor set.
        // Bind it once for the whole pass instead of once per
        // draw — saves ~draws.len() command buffer entries with
        // no behavioural change.
        if let Some(first) = shadow_draws.first() {
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.shadow_map.pipeline_layout,
                0,
                &[first.descriptor_set],
                &[],
            );
        }
        for draw in shadow_draws.iter() {
            device.cmd_bind_vertex_buffers(cmd, 0, &[draw.vertex_buffer], &[0]);
            device.cmd_bind_index_buffer(cmd, draw.index_buffer, 0, vk::IndexType::UINT32);
            let model_bytes: &[u8] = bytemuck::bytes_of(&draw.model_matrix);
            device.cmd_push_constants(
                cmd,
                self.shadow_map.pipeline_layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                model_bytes,
            );
            device.cmd_draw_indexed(cmd, draw.index_count, 1, 0, 0, 0);
        }
        device.cmd_end_render_pass(cmd);
    }

    /// Record the point-light cube shadow pass: 6 cube faces × N
    /// active shadow lights, with per-slot dirty-tracking that
    /// skips re-rendering when no caster has moved within range.
    /// Also transitions any unused atlas slots to
    /// SHADER_READ_ONLY_OPTIMAL so the main fragment shader's
    /// cube-array sampler is layout-legal.
    ///
    /// SAFETY: caller must have an active command buffer recording.
    unsafe fn record_point_shadow_pass(
        &mut self,
        cmd: vk::CommandBuffer,
        point_shadow_count: usize,
        merged_lights: &[PointLight; MAX_POINT_LIGHTS],
        shadow_draws: &[DrawCommand],
    ) {
        let device = &self.device.device;
        // Defined-layout pre-pass: any cube face slot we
        // *don't* render into this frame stays in
        // VK_IMAGE_LAYOUT_UNDEFINED, but the main
        // fragment shader still samples those layers via
        // the cube-array view (the conditional is in the
        // shader, not in descriptor binding). Vulkan
        // requires every subresource the descriptor
        // covers to be in SHADER_READ_ONLY_OPTIMAL at
        // submit time, so we transition the unused slots
        // here. UNDEFINED → SHADER_READ_ONLY_OPTIMAL is a
        // discard-and-set, so it's safe to issue every
        // frame: the render pass that follows will
        // implicitly transition any used slot back to
        // SHADER_READ_ONLY_OPTIMAL via its own attachment
        // descriptions.
        if point_shadow_count < shadow_point::MAX_POINT_SHADOWS {
            let unused_base = (point_shadow_count * 6) as u32;
            let unused_count = (shadow_point::MAX_POINT_SHADOWS * 6) as u32 - unused_base;
            let barrier = vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::SHADER_READ)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(self.point_shadow_atlas.color_image)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: unused_base,
                    layer_count: unused_count,
                });
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
        }
        if point_shadow_count == 0 {
            return;
        }
        let psh_clear = [
            vk::ClearValue {
                color: vk::ClearColorValue {
                    // Default to "fully unoccluded" (>= 1.0)
                    // so directions that were never rasterised
                    // sample as lit, not as black walls.
                    float32: [1.0, 1.0, 1.0, 1.0],
                },
            },
            vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            },
        ];
        device.cmd_bind_pipeline(
            cmd,
            vk::PipelineBindPoint::GRAPHICS,
            self.point_shadow_atlas.pipeline,
        );
        // Take the per-light scratch list out of `self`
        // (mirrors the trick used for `draws`) so we can
        // hold a `&self.point_shadow_atlas` borrow and
        // mutate the scratch Vec in the same scope.
        let mut light_draws = std::mem::take(&mut self.point_shadow_draw_scratch);
        for light_idx in 0..point_shadow_count {
            let pl = &merged_lights[light_idx];
            let lpos = pl.position;
            let lrad = pl.radius.max(0.1);
            // Cull once per light against the per-draw
            // bounding sphere. Reused for all 6 cube faces
            // so a light that touches `K` draws does
            // `K` sphere tests + `6*K` draws instead of
            // `6*K` sphere tests + `6*K` draws as before.
            light_draws.clear();
            for draw in shadow_draws.iter() {
                let center = draw.model_matrix.w_axis.truncate();
                if (center - lpos).length() > lrad + draw.bounds_radius {
                    continue;
                }
                light_draws.push(draw.clone());
            }
            // ---- Dirty check ----
            // Hash the current frame's slot inputs (light
            // pose + every relevant caster's translation &
            // bounds_radius). If it matches the cached
            // value from when we last rendered into this
            // slot, skip the 6-face render entirely — the
            // atlas contents are still valid because no
            // caster moved within range. FNV-1a 64 over
            // the bit patterns is cheap and stable.
            let new_state = {
                const FNV_OFFSET: u64 = 0xcbf29ce484222325;
                const FNV_PRIME: u64 = 0x100000001b3;
                let mut h: u64 = FNV_OFFSET;
                // If any caster has dynamic vertex contents
                // (CPU ring or GPU skin output), force the
                // slot to re-render every frame by mixing
                // the frame counter into the hash. The model
                // matrix is invariant across pure pose
                // changes (spine twist while standing still,
                // animation cycles in place), so without
                // this the cube atlas freezes for skinned
                // characters and the cast shadow visibly
                // lags the silhouette until the entity
                // translates.
                let mut force_dirty = false;
                for d in light_draws.iter() {
                    let m = d.model_matrix;
                    for col in [m.x_axis, m.y_axis, m.z_axis, m.w_axis] {
                        for word in [
                            col.x.to_bits(),
                            col.y.to_bits(),
                            col.z.to_bits(),
                            col.w.to_bits(),
                        ] {
                            h ^= word as u64;
                            h = h.wrapping_mul(FNV_PRIME);
                        }
                    }
                    h ^= d.bounds_radius.to_bits() as u64;
                    h = h.wrapping_mul(FNV_PRIME);
                    if d.dynamic_vertices {
                        force_dirty = true;
                    }
                }
                if force_dirty {
                    // Stagger shadow updates across frames.
                    // Each dynamic-caster light re-renders
                    // every *other* frame, and even-indexed
                    // lights refresh on opposite frames
                    // from odd-indexed lights so the work
                    // is spread evenly instead of spiking
                    // every two frames.
                    //
                    // With 8 shadow lights + 12 skinned
                    // monsters wandering between them, the
                    // previous behaviour refreshed every
                    // light every frame — even though a
                    // single skinned pose changes by
                    // sub-pixel amounts in 16 ms. Halving
                    // the refresh rate of dynamic shadows
                    // is invisible at 60+ FPS (the shadow
                    // follows the silhouette exactly one
                    // frame late, less than the monitor
                    // refresh) and roughly halves the
                    // per-frame shadow-pass GPU cost.
                    //
                    // `epoch = (frame_count + light_idx) >> 1`
                    // is constant across two adjacent
                    // frames per-light (so the cache hits),
                    // and adjacent lights have offset
                    // epochs (so half refresh on even
                    // frames, half on odd).
                    let epoch = (self.frame_count.wrapping_add(light_idx as u64)) >> 1;
                    h ^= epoch;
                    h = h.wrapping_mul(FNV_PRIME);
                }
                PointShadowSlotState {
                    light_bits: [
                        lpos.x.to_bits(),
                        lpos.y.to_bits(),
                        lpos.z.to_bits(),
                        lrad.to_bits(),
                    ],
                    caster_hash: h,
                }
            };
            if self.point_shadow_state[light_idx] == Some(new_state) {
                // Slot is clean: previous frame's atlas
                // contents are still valid and the image is
                // already in SHADER_READ_ONLY_OPTIMAL (left
                // there by the prior render pass's final
                // attachment layout). Nothing to do.
                continue;
            }
            self.point_shadow_state[light_idx] = Some(new_state);
            // Note: when `light_draws` is empty we still
            // run the 6 render passes — they hit the
            // LOAD_OP::CLEAR path with zero draws,
            // which paints the slot fully unoccluded.
            // Skipping the passes outright would leave any
            // previous frame's shadow content in the atlas
            // (visible as stale shadows for a frame after a
            // caster leaves the light's radius). The
            // descriptor-set bind below is guarded so an
            // empty `light_draws` is safe.
            for face_idx in 0..6 {
                let face_slot = light_idx * 6 + face_idx;
                // Per-face cone cull. Each cube face is a
                // 90° FOV view aligned with one of the six
                // cardinal axes. A caster (sphere `C`,
                // radius `r`) only appears in this face when
                // it intersects that view cone — testing
                // against the 5 frustum planes (skip far)
                // lets us skip ~5/6 of the per-face draws
                // when casters are clustered around the
                // light. With 12 monsters across 8 shadow
                // lights this turns ~576 per-frame shadow
                // draw calls into ~96, cutting both the
                // command-buffer submission cost and the
                // vertex-shader work for skinned meshes.
                //
                // Face axes: 0:+X 1:-X 2:+Y 3:-Y 4:+Z 5:-Z
                let face_axis: glam::Vec3 = match face_idx {
                    0 => glam::Vec3::X,
                    1 => glam::Vec3::NEG_X,
                    2 => glam::Vec3::Y,
                    3 => glam::Vec3::NEG_Y,
                    4 => glam::Vec3::Z,
                    _ => glam::Vec3::NEG_Z,
                };
                let rp_begin = vk::RenderPassBeginInfo::default()
                    .render_pass(self.point_shadow_atlas.render_pass)
                    .framebuffer(self.point_shadow_atlas.framebuffers[face_slot])
                    .render_area(vk::Rect2D {
                        offset: vk::Offset2D { x: 0, y: 0 },
                        extent: vk::Extent2D {
                            width: shadow_point::POINT_SHADOW_SIZE,
                            height: shadow_point::POINT_SHADOW_SIZE,
                        },
                    })
                    .clear_values(&psh_clear);
                device.cmd_begin_render_pass(cmd, &rp_begin, vk::SubpassContents::INLINE);
                if let Some(first) = light_draws.first() {
                    device.cmd_bind_descriptor_sets(
                        cmd,
                        vk::PipelineBindPoint::GRAPHICS,
                        self.point_shadow_atlas.pipeline_layout,
                        0,
                        &[first.descriptor_set],
                        &[],
                    );
                }
                for draw in light_draws.iter() {
                    // Sphere-vs-cube-face cone test. For a
                    // 90° FOV view down `face_axis`, a point
                    // is inside the view if its component
                    // along `face_axis` exceeds the magnitude
                    // of its perpendicular components. For a
                    // sphere, we extend the test by `r` to
                    // get a conservative include. Skip if the
                    // entire sphere is outside the cone.
                    let center = draw.model_matrix.w_axis.truncate();
                    let d = center - lpos;
                    let along = d.dot(face_axis);
                    let r = draw.bounds_radius;
                    if along + r < 0.0 {
                        continue; // entirely behind face
                    }
                    let perp_sq = d.length_squared() - along * along;
                    let perp = perp_sq.max(0.0).sqrt();
                    // Cone half-angle is 45° → tan = 1, so
                    // sphere fits inside cone when `perp <=
                    // along + r * sqrt(2)`. The sqrt(2)
                    // factor is the conservative inflation
                    // for a sphere-vs-plane test on the
                    // 45° side planes.
                    if perp > along + r * std::f32::consts::SQRT_2 {
                        continue;
                    }
                    device.cmd_bind_vertex_buffers(cmd, 0, &[draw.vertex_buffer], &[0]);
                    device.cmd_bind_index_buffer(cmd, draw.index_buffer, 0, vk::IndexType::UINT32);
                    // Push the model + indices payload as
                    // a single 80-byte block. The vert
                    // shader reads `mat4 model` at offset
                    // 0; the frag reads `uvec4 indices` at
                    // offset 64. One push call instead of
                    // two saves a command-buffer entry per
                    // draw.
                    let mut bytes = [0u8; 80];
                    bytes[..64].copy_from_slice(bytemuck::bytes_of(&draw.model_matrix));
                    let indices: [u32; 4] = [face_slot as u32, light_idx as u32, 0, 0];
                    bytes[64..].copy_from_slice(bytemuck::bytes_of(&indices));
                    device.cmd_push_constants(
                        cmd,
                        self.point_shadow_atlas.pipeline_layout,
                        vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                        0,
                        &bytes,
                    );
                    device.cmd_draw_indexed(cmd, draw.index_count, 1, 0, 0, 0);
                }
                device.cmd_end_render_pass(cmd);
            }
        }
        // Restore for next frame.
        self.point_shadow_draw_scratch = light_draws;
    }

    /// Record the main HDR scene pass: sky dome, then opaque
    /// 3D draws. Renders into the post-processing HDR colour
    /// target (not the swapchain). Overlay/UI moves to the
    /// composite pass.
    ///
    /// SAFETY: caller must have an active command buffer recording.
    unsafe fn record_scene_pass(
        &self,
        cmd: vk::CommandBuffer,
        image_index: u32,
        draws: &[DrawCommand],
    ) {
        let device = &self.device.device;
        let clear_values = [
            vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: self.clear_color,
                },
            },
            vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            },
        ];

        let render_pass_begin = vk::RenderPassBeginInfo::default()
            .render_pass(self.post.scene_pass)
            .framebuffer(self.post.scene_framebuffers[image_index as usize])
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.swapchain.extent,
            })
            .clear_values(&clear_values);

        device.cmd_begin_render_pass(cmd, &render_pass_begin, vk::SubpassContents::INLINE);

        // Sky dome — drawn first inside the main pass with
        // depth test/write disabled so subsequent scene
        // geometry occludes it naturally. No-op when
        // `sky.enabled` is false (indoor dungeons).
        self.sky_renderer.record(
            device,
            cmd,
            self.swapchain.extent,
            self.camera.view_matrix(),
            self.camera.projection_matrix(),
            &self.sky,
            self.start_time.elapsed().as_secs_f32(),
        );

        // 3D scene
        device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
        for draw in draws {
            device.cmd_bind_vertex_buffers(cmd, 0, &[draw.vertex_buffer], &[0]);
            device.cmd_bind_index_buffer(cmd, draw.index_buffer, 0, vk::IndexType::UINT32);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[draw.descriptor_set, draw.material_set],
                &[],
            );
            let model_bytes: &[u8] = bytemuck::bytes_of(&draw.model_matrix);
            device.cmd_push_constants(
                cmd,
                self.pipeline_layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                0,
                model_bytes,
            );
            let tint_bytes: &[u8] = bytemuck::bytes_of(&draw.tint);
            device.cmd_push_constants(
                cmd,
                self.pipeline_layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                64,
                tint_bytes,
            );
            let mp_bytes: &[u8] = bytemuck::bytes_of(&draw.material_params);
            device.cmd_push_constants(
                cmd,
                self.pipeline_layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                80,
                mp_bytes,
            );
            device.cmd_draw_indexed(cmd, draw.index_count, 1, 0, 0, 0);
        }

        // End the opaque scene pass. Depth is now in
        // DEPTH_STENCIL_READ_ONLY_OPTIMAL — translucent
        // pipelines can both depth-test against it and
        // sample it as a combined-image-sampler for soft-
        // particle fade.
        device.cmd_end_render_pass(cmd);
    }

    /// Record the translucent pass: ribbons + particles. Loads
    /// the HDR colour the opaque pass just wrote; depth is
    /// read-only so this pass can't write to it.
    ///
    /// SAFETY: caller must have an active command buffer recording.
    unsafe fn record_translucent_pass(
        &self,
        cmd: vk::CommandBuffer,
        image_index: u32,
        frame: usize,
    ) {
        let device = &self.device.device;
        let translucent_begin = vk::RenderPassBeginInfo::default()
            .render_pass(self.post.translucent_pass)
            .framebuffer(self.post.translucent_framebuffers[image_index as usize])
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.swapchain.extent,
            });
        device.cmd_begin_render_pass(cmd, &translucent_begin, vk::SubpassContents::INLINE);

        // VFX ribbons (world-space, premultiplied additive).
        // Drawn before particles so the spark/glow trails
        // composite on top of the beam core.
        self.vfx_ribbon_renderer.record(
            frame,
            device,
            cmd,
            self.uniforms.descriptor_sets[frame],
            self.post.translucent_in_sets[image_index as usize],
        );
        // VFX particles (world-space, two pipelines).
        self.vfx_particle_renderer.record(
            frame,
            device,
            cmd,
            self.uniforms.descriptor_sets[frame],
            self.post.translucent_in_sets[image_index as usize],
        );

        device.cmd_end_render_pass(cmd);
    }

    /// Record the composite + overlay pass: tonemap HDR + bloom
    /// into the swapchain, then draw the UI overlay on top so it
    /// stays at native sRGB crispness (no second tonemap pass).
    ///
    /// SAFETY: caller must have an active command buffer recording.
    unsafe fn record_composite_and_overlay(
        &self,
        cmd: vk::CommandBuffer,
        image_index: u32,
        frame: usize,
        sun_screen: [f32; 4],
        sun_color: [f32; 4],
        heat_source: [f32; 4],
    ) {
        let device = &self.device.device;
        let composite_begin = vk::RenderPassBeginInfo::default()
            .render_pass(self.post.composite_pass)
            .framebuffer(self.post.composite_framebuffers[image_index as usize])
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.swapchain.extent,
            });
        device.cmd_begin_render_pass(cmd, &composite_begin, vk::SubpassContents::INLINE);
        // Inverse projection matrix is needed by the inline
        // SSAO in the composite shader to reconstruct view-
        // space positions from sampled depth. Inverting on
        // CPU once per frame is essentially free vs. doing
        // it per pixel.
        let inv_proj = self.camera.projection_matrix().inverse().to_cols_array_2d();
        // SSAO strength baked at moderate level. The post
        // composite already applies AO multiplicatively to
        // the tonemapped HDR (rather than only to the
        // ambient term) so we keep this gentle to avoid
        // crushing direct-lit pixels in deep crevices.
        let ssao_strength = 0.7;

        self.post.record_composite(
            device,
            cmd,
            image_index,
            &self.bloom,
            self.ghost_mix,
            inv_proj,
            ssao_strength,
            sun_screen,
            sun_color,
            heat_source,
        );

        // Overlay (HUD)
        self.overlay.record(frame, device, cmd);

        device.cmd_end_render_pass(cmd);
    }

    pub fn draw_frame(&mut self) -> Result<()> {
        // Skip rendering when minimized.
        if self.window_extent[0] == 0 || self.window_extent[1] == 0 {
            return Ok(());
        }

        self.frame_count += 1;
        let frame = self.current_frame;

        unsafe {
            self.device.device.wait_for_fences(
                &[self.frame_sync.in_flight[frame]],
                true,
                u64::MAX,
            )?;
        }
        // Validation requires the cmd buffer to be reset before its
        // referenced buffers are destroyed.
        let cmd = self.command_buffers[frame];
        unsafe {
            self.device
                .device
                .reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())?;
        }
        self.flush_deletions();

        // ---- Acquire next swapchain image ----
        let image_index = unsafe {
            match self.swapchain.swapchain_fn.acquire_next_image(
                self.swapchain.swapchain,
                u64::MAX,
                self.frame_sync.image_available[frame],
                vk::Fence::null(),
            ) {
                Ok((index, _suboptimal)) => index,
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                    self.recreate_swapchain(self.window_extent[0], self.window_extent[1])?;
                    return Ok(());
                }
                Err(e) => return Err(e.into()),
            }
        };
        unsafe {
            self.device
                .device
                .reset_fences(&[self.frame_sync.in_flight[frame]])?;
        }

        // ---- Build per-frame light list, shadow VPs, UBO ----
        let (merged_lights, light_count, point_shadow_count) = self.merge_per_frame_lights();
        let point_shadow_face_vp =
            self.build_point_shadow_face_vp(point_shadow_count, &merged_lights);
        let ubo = self.build_uniform_data(
            &merged_lights,
            light_count,
            point_shadow_count,
            point_shadow_face_vp,
        );
        self.uniforms.update(frame, &ubo);

        // ---- Build per-frame draw lists ----
        let (draws, shadow_draws) = self.build_draw_lists(frame);

        // ---- Upload per-frame instance data ----
        self.overlay.upload(
            frame,
            &self.device.device,
            &self.allocator,
            self.device.graphics_queue,
            self.command_pool,
            &self.overlay_batch,
        )?;
        self.vfx_ribbon_renderer.upload(
            frame,
            &self.device.device,
            &self.allocator,
            self.vfx_system.ribbon_instances(),
        )?;
        self.vfx_particle_renderer.upload(
            frame,
            &self.device.device,
            &self.allocator,
            self.vfx_system.particle_instances(),
        )?;

        // ---- Compute screen-space post inputs (CPU math) ----
        let (sun_screen, sun_color) = self.compute_sun_screen_uv();
        let heat_source = self.compute_heat_source_uv();

        // ---- Record command buffer ----
        unsafe {
            let begin_info = vk::CommandBufferBeginInfo::default();
            self.device.device.begin_command_buffer(cmd, &begin_info)?;

            // GPU mesh skinning compute dispatches + a single
            // COMPUTE_SHADER_WRITE -> VERTEX_ATTRIBUTE_READ
            // barrier so the shadow + forward passes see the
            // new vertices. No-op if nothing's active.
            self.skin_system.record_dispatches(
                &self.device.device,
                cmd,
                self.current_frame,
                &self.allocator,
            );

            self.record_dir_shadow_pass(cmd, &shadow_draws);
            self.record_point_shadow_pass(cmd, point_shadow_count, &merged_lights, &shadow_draws);

            // Blood-field splat pass: drains kill splats queued
            // during the gameplay frame into this frame's instance
            // buffer and renders into the per-floor blood field.
            // Also handles the initial clear when a new floor is
            // bound. No-op when no floor is active or no splats
            // are pending.
            self.blood_field.record(&self.device.device, cmd, frame);

            self.record_scene_pass(cmd, image_index, &draws);
            self.record_translucent_pass(cmd, image_index, frame);
            self.post
                .record_bloom(&self.device.device, cmd, image_index, &self.bloom);
            self.record_composite_and_overlay(
                cmd,
                image_index,
                frame,
                sun_screen,
                sun_color,
                heat_source,
            );

            self.device.device.end_command_buffer(cmd)?;
        }

        // ---- Submit ----
        let wait_semaphores = [self.frame_sync.image_available[frame]];
        let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        let signal_semaphores = [self.frame_sync.render_finished[frame]];
        let submit_info = vk::SubmitInfo::default()
            .wait_semaphores(&wait_semaphores)
            .wait_dst_stage_mask(&wait_stages)
            .command_buffers(std::slice::from_ref(&cmd))
            .signal_semaphores(&signal_semaphores);
        unsafe {
            self.device.device.queue_submit(
                self.device.graphics_queue,
                &[submit_info],
                self.frame_sync.in_flight[frame],
            )?;
        }

        // ---- Present ----
        let swapchains = [self.swapchain.swapchain];
        let image_indices = [image_index];
        let present_info = vk::PresentInfoKHR::default()
            .wait_semaphores(&signal_semaphores)
            .swapchains(&swapchains)
            .image_indices(&image_indices);
        let present_result = unsafe {
            self.swapchain
                .swapchain_fn
                .queue_present(self.device.present_queue, &present_info)
        };

        // Restore scratch buffers so next frame reuses the same
        // allocation. Done before any potential early return so a
        // swapchain rebuild doesn't drop the scratch capacity.
        self.draw_scratch = draws;
        self.shadow_draw_scratch = shadow_draws;

        match present_result {
            Ok(_) => {}
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR | vk::Result::SUBOPTIMAL_KHR) => {
                self.framebuffer_resized = false;
                self.recreate_swapchain(self.window_extent[0], self.window_extent[1])?;
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        }

        if self.framebuffer_resized {
            self.framebuffer_resized = false;
            self.recreate_swapchain(self.window_extent[0], self.window_extent[1])?;
            return Ok(());
        }

        self.current_frame = (self.current_frame + 1) % MAX_FRAMES_IN_FLIGHT;
        Ok(())
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

    /// Check for shader changes and hot-reload the pipeline if needed.
    pub fn check_hot_reload(&mut self) {
        let should_reload = self
            .hot_reloader
            .as_ref()
            .map(|hr| hr.check_and_reset())
            .unwrap_or(false);

        if should_reload {
            unsafe {
                self.device.device.device_wait_idle().ok();
            }

            match Self::compile_pipeline_from_disk(
                &self.device.device,
                self.post.scene_pass,
                self.swapchain.extent,
                &[
                    self.uniforms.descriptor_set_layout,
                    self.material_pool.layout,
                ],
                &self.shader_dir,
            ) {
                Ok((new_pipeline, new_layout)) => {
                    unsafe {
                        self.device.device.destroy_pipeline(self.pipeline, None);
                        self.device
                            .device
                            .destroy_pipeline_layout(self.pipeline_layout, None);
                    }
                    self.pipeline = new_pipeline;
                    self.pipeline_layout = new_layout;
                    log::info!("Pipeline hot-reloaded successfully!");
                }
                Err(e) => {
                    log::error!("Hot-reload failed (keeping old pipeline): {}", e);
                }
            }

            // Also rebuild the post-process pipelines (bright /
            // blur / composite). The dirty flag fires for *any*
            // .frag/.vert change in the shader directory so we
            // can't tell whether triangle.* or post_*.* moved —
            // just rebuild everything. Cheap relative to the
            // device wait above. Compile failures are
            // non-fatal: the existing pipelines stay live.
            if let Err(e) = self
                .post
                .reload_pipelines(&self.device.device, &self.shader_dir)
            {
                log::error!(
                    "Post-pipeline hot-reload failed (keeping old pipelines): {}",
                    e
                );
            } else {
                log::info!("Post pipelines hot-reloaded successfully!");
            }
        }
    }

    fn compile_pipeline_from_disk(
        device: &ash::Device,
        render_pass: vk::RenderPass,
        extent: vk::Extent2D,
        descriptor_set_layouts: &[vk::DescriptorSetLayout],
        shader_dir: &std::path::Path,
    ) -> Result<(vk::Pipeline, vk::PipelineLayout)> {
        let vert_path = shader_dir.join("triangle.vert");
        let frag_path = shader_dir.join("triangle.frag");

        let vert_source = std::fs::read_to_string(&vert_path)
            .map_err(|e| anyhow::anyhow!("Failed to read {:?}: {}", vert_path, e))?;
        let frag_source = std::fs::read_to_string(&frag_path)
            .map_err(|e| anyhow::anyhow!("Failed to read {:?}: {}", frag_path, e))?;

        let vert_spv =
            hot_reload::compile_glsl(&vert_source, "triangle.vert", shaderc::ShaderKind::Vertex)?;
        let frag_spv =
            hot_reload::compile_glsl(&frag_source, "triangle.frag", shaderc::ShaderKind::Fragment)?;

        let vert_module = pipeline::create_shader_module(device, &vert_spv)?;
        let frag_module = pipeline::create_shader_module(device, &frag_spv)?;

        let result = pipeline::create_graphics_pipeline(
            device,
            render_pass,
            extent,
            descriptor_set_layouts,
            vert_module,
            frag_module,
        );

        unsafe {
            device.destroy_shader_module(vert_module, None);
            device.destroy_shader_module(frag_module, None);
        }

        result
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

            self.swapchain.cleanup(&self.device.device);
            self.surface_fn.destroy_surface(self.surface, None);
        }

        // Drop allocator before device & instance (auto-drop handles the rest)
        drop(self.allocator.lock());
    }
}

/// Find the shader directory by checking common locations.
fn find_shader_dir() -> PathBuf {
    // Try relative to current dir (workspace root)
    let candidates = [
        PathBuf::from("assets/shaders"),
        PathBuf::from("../assets/shaders"),
        PathBuf::from("../../assets/shaders"),
    ];

    for candidate in &candidates {
        if candidate.exists() && candidate.join("triangle.vert").exists() {
            return candidate
                .canonicalize()
                .unwrap_or_else(|_| candidate.clone());
        }
    }

    // Fallback
    PathBuf::from("assets/shaders")
}
