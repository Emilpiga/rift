use anyhow::Result;
use ash::vk;
use glam::{Mat4, Vec3, Vec4};
use gpu_allocator::vulkan::{Allocator, AllocatorCreateDesc};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::hot_reload::{self, HotReloader};
use crate::renderer::camera::Camera;
use crate::renderer::depth::DepthBuffer;
use crate::renderer::material::MaterialPool;
use crate::renderer::mesh::{Mesh, Vertex};
use crate::renderer::shadow::{self, ShadowMap};
use crate::renderer::sky::{SkyConfig, SkyRenderer};
use crate::renderer::post::{BloomConfig, PostProcessing};
use crate::renderer::overlay::{OverlayBatch, OverlayRenderer};
use crate::renderer::vfx::{ParticleVfxRenderer, RibbonRenderer, VfxSystem};
use crate::renderer::texture::Texture;
use crate::renderer::uniform::{UniformBuffers, UniformData};
use crate::vulkan::{
    buffer::{self, GpuBuffer},
    commands::{self, DrawCommand},
    pipeline,
    sync::{FrameSync, MAX_FRAMES_IN_FLIGHT},
    Swapchain, VulkanDevice, VulkanInstance,
};

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
    material_pool: MaterialPool,
    shadow_map: ShadowMap,
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
    // Deferred deletion queue for GPU buffers
    deletion_queue: Vec<(u64, GpuBuffer)>,
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
    // Dynamic point lights (populated each frame by game code)
    pub point_lights: Vec<PointLight>,
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
}

impl Default for KeyLight {
    fn default() -> Self {
        Self::DUNGEON
    }
}

/// A dynamic point light source.
#[derive(Clone, Copy)]
pub struct PointLight {
    pub position: Vec3,
    pub color: Vec3,
    pub radius: f32,
    pub intensity: f32,
}

impl Renderer {
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
            &device.device, &allocator, &swapchain, depth_buffer.view, &shader_dir,
        )?;
        let render_pass = post.scene_pass;
        let composite_pass = post.composite_pass;

        let uniforms = UniformBuffers::new(&device.device, &allocator)?;

        // Create default checkerboard texture and bind to descriptor sets
        let command_pool_init =
            commands::create_command_pool(&device.device, device.graphics_queue_family)?;
        let default_texture = Texture::checkerboard(
            &device.device,
            &allocator,
            device.graphics_queue,
            command_pool_init,
        )?;
        uniforms.bind_texture(&device.device, default_texture.view, default_texture.sampler);

        // Per-object material pool (set=1 for the forward pipeline). The
        // pool's default white texture is uploaded via the same init command
        // pool so it's ready before we destroy the pool below.
        let material_pool = MaterialPool::new(
            &device.device,
            &allocator,
            device.graphics_queue,
            command_pool_init,
        )?;
        unsafe { device.device.destroy_command_pool(command_pool_init, None); }

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
            &device.device, &allocator, device.graphics_queue, command_pool,
            composite_pass, swapchain.extent, &shader_dir,
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
            &device.device, &allocator, device.graphics_queue, command_pool,
            render_pass, swapchain.extent, uniforms.descriptor_set_layout, &shader_dir,
        )?;
        let vfx_particle_renderer = ParticleVfxRenderer::new(
            &device.device, &allocator, device.graphics_queue, command_pool,
            render_pass, swapchain.extent, uniforms.descriptor_set_layout, &shader_dir,
        )?;
        let vfx_system = VfxSystem::new(8192);
        let sky_renderer = SkyRenderer::new(&device.device, render_pass, &shader_dir)?;

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
            material_pool,
            shadow_map,
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
            deletion_queue: Vec::new(),
            clear_color: [0.008, 0.006, 0.010, 1.0],
            fog_color: [0.018, 0.012, 0.010],
            fog_start: 5.0,
            fog_end: 16.0,
            fog_origin: Vec3::ZERO,
            point_lights: Vec::new(),
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

        unsafe { self.device.device.device_wait_idle()?; }

        // Tear down post-process swapchain-dependent resources
        // (offscreen images, framebuffers, descriptor sets)
        // before the depth buffer that some of them reference.
        self.post.cleanup_swapchain_dependent(&self.device.device, &self.allocator);

        // Destroy old depth buffer
        self.depth_buffer.cleanup(&self.device.device, &self.allocator);

        // Destroy old pipeline
        unsafe {
            self.device.device.destroy_pipeline(self.pipeline, None);
            self.device.device.destroy_pipeline_layout(self.pipeline_layout, None);
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
        self.depth_buffer = DepthBuffer::new(&self.device.device, &self.allocator, self.swapchain.extent)?;

        // Recreate post-process resources (offscreen images,
        // framebuffers, descriptor sets). Render passes &
        // pipelines stay alive across resize because every post
        // pipeline uses dynamic viewport+scissor.
        self.post.recreate(
            &self.device.device, &self.allocator, &self.swapchain, self.depth_buffer.view,
        )?;

        // Recreate pipeline with new extent
        let (new_pipeline, new_layout) = Self::compile_pipeline_from_disk(
            &self.device.device,
            self.post.scene_pass,
            self.swapchain.extent,
            &[self.uniforms.descriptor_set_layout, self.material_pool.layout],
            &self.shader_dir,
        )?;
        self.pipeline = new_pipeline;
        self.pipeline_layout = new_layout;

        // Recreate overlay pipeline
        self.overlay.recreate_pipeline(&self.device.device, self.post.composite_pass, self.swapchain.extent, &self.shader_dir)?;

        // Recreate VFX ribbon pipeline alongside.
        self.vfx_ribbon_renderer.recreate_pipeline(
            &self.device.device, self.post.scene_pass, self.swapchain.extent,
            self.uniforms.descriptor_set_layout, &self.shader_dir,
        )?;
        self.vfx_particle_renderer.recreate_pipeline(
            &self.device.device, self.post.scene_pass, self.swapchain.extent,
            self.uniforms.descriptor_set_layout, &self.shader_dir,
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
        let bounds_radius = mesh.vertices.iter()
            .map(|v| v.position.length())
            .fold(0.0_f32, f32::max);

        self.objects.push(RenderObject {
            vertex_buffer,
            index_buffer,
            index_count: mesh.indices.len() as u32,
            model_matrix,
            bounds_radius,
            dynamic_vertex_buffers: None,
            material_set: self.material_pool.default_set,
            texture: None,
            tint: [1.0, 1.0, 1.0, 1.0],
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

        let bounds_radius = mesh.vertices.iter()
            .map(|v| v.position.length())
            .fold(0.0_f32, f32::max);

        self.objects.push(RenderObject {
            vertex_buffer: placeholder,
            index_buffer,
            index_count: mesh.indices.len() as u32,
            model_matrix,
            bounds_radius,
            dynamic_vertex_buffers: Some(dynamic_vertex_buffers),
            material_set: self.material_pool.default_set,
            texture: None,
            tint: [1.0, 1.0, 1.0, 1.0],
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

    /// True if this object was created via `add_dynamic_mesh`.
    pub fn is_dynamic_mesh(&self, obj_idx: usize) -> bool {
        self.objects.get(obj_idx)
            .map(|o| o.dynamic_vertex_buffers.is_some())
            .unwrap_or(false)
    }

    /// Bind a base-color texture from a PNG/JPG file to the object at
    /// `obj_idx`. The texture is owned by the renderer and freed when the
    /// renderer is dropped or the object is removed via `clear_objects`.
    /// Pass any path that exists at runtime; common parent prefixes are
    /// tried to handle different cwds.
    pub fn set_object_texture<P: AsRef<std::path::Path>>(
        &mut self,
        obj_idx: usize,
        path: P,
    ) -> Result<()> {
        if obj_idx >= self.objects.len() {
            return Ok(());
        }
        let texture = crate::renderer::material::load_texture_from_file(
            &self.device.device,
            &self.allocator,
            self.device.graphics_queue,
            self.command_pool,
            path,
        )?;
        let set = self.material_pool.alloc_set(&self.device.device, &texture)?;
        // Replace; old per-object texture (if any) is dropped after the
        // wait below to make sure no in-flight frame still references it.
        unsafe { self.device.device.device_wait_idle().ok(); }
        let obj = &mut self.objects[obj_idx];
        if let Some(mut old) = obj.texture.take() {
            old.cleanup(&self.device.device, &self.allocator);
        }
        obj.texture = Some(texture);
        obj.material_set = set;
        Ok(())
    }

    /// Bind a base-color texture decoded from raw PNG/JPG bytes — useful
    /// for textures embedded in glTF bufferViews where there's no file
    /// path to pass to `set_object_texture`.
    pub fn set_object_texture_from_bytes(
        &mut self,
        obj_idx: usize,
        bytes: &[u8],
    ) -> Result<()> {
        if obj_idx >= self.objects.len() {
            return Ok(());
        }
        let texture = crate::renderer::material::load_texture_from_memory(
            &self.device.device,
            &self.allocator,
            self.device.graphics_queue,
            self.command_pool,
            bytes,
        )?;
        let set = self.material_pool.alloc_set(&self.device.device, &texture)?;
        let obj = &mut self.objects[obj_idx];
        // Only wait for the GPU to drain if there's an existing per-object
        // texture to free; first-time texture binding is safe without a
        // wait (the default material set was never written to a texture
        // resource that we're about to free here).
        if obj.texture.is_some() {
            unsafe { self.device.device.device_wait_idle().ok(); }
            if let Some(mut old) = obj.texture.take() {
                old.cleanup(&self.device.device, &self.allocator);
            }
        }
        obj.texture = Some(texture);
        obj.material_set = set;
        Ok(())
    }

    /// Upload a texture from raw PNG/JPG bytes and return both the
    /// texture handle and a freshly-allocated descriptor set bound to
    /// it.  The caller is responsible for keeping the texture alive
    /// for as long as any object references the descriptor set.
    /// Use together with `set_object_shared_material` to share a
    /// single texture across many objects (e.g. one descriptor set
    /// per monster archetype rather than per spawn) — this avoids
    /// blowing through the per-pool descriptor-set budget when the
    /// floor spawns dozens of enemies.
    pub fn upload_shared_texture_from_bytes(
        &mut self,
        bytes: &[u8],
    ) -> Result<(crate::renderer::texture::Texture, vk::DescriptorSet)> {
        let texture = crate::renderer::material::load_texture_from_memory(
            &self.device.device,
            &self.allocator,
            self.device.graphics_queue,
            self.command_pool,
            bytes,
        )?;
        let set = self.material_pool.alloc_set(&self.device.device, &texture)?;
        Ok((texture, set))
    }

    /// Upload a texture from raw RGBA8 pixels (e.g. procedurally
    /// generated) and return both the texture handle and a freshly
    /// allocated shared descriptor set.  See
    /// [`upload_shared_texture_from_bytes`] for ownership semantics.
    pub fn upload_shared_texture_from_rgba(
        &mut self,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<(crate::renderer::texture::Texture, vk::DescriptorSet)> {
        let texture = crate::renderer::texture::Texture::from_rgba(
            &self.device.device,
            &self.allocator,
            self.device.graphics_queue,
            self.command_pool,
            width,
            height,
            pixels,
        )?;
        let set = self.material_pool.alloc_set(&self.device.device, &texture)?;
        Ok((texture, set))
    }

    /// Bind a previously-allocated shared descriptor set to an object.
    /// Unlike `set_object_texture*`, the renderer does NOT take
    /// ownership of any texture — the caller must keep the underlying
    /// `Texture` alive (typically inside a long-lived asset cache).
    pub fn set_object_shared_material(
        &mut self,
        obj_idx: usize,
        set: vk::DescriptorSet,
    ) {
        if obj_idx >= self.objects.len() {
            return;
        }
        let obj = &mut self.objects[obj_idx];
        if obj.texture.is_some() {
            unsafe { self.device.device.device_wait_idle().ok(); }
            if let Some(mut old) = obj.texture.take() {
                old.cleanup(&self.device.device, &self.allocator);
            }
        }
        obj.material_set = set;
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

        let bounds_radius = mesh.vertices.iter()
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
                self.device.device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty()).ok();
            }
        }
        // Now safe to destroy immediately + flush any pending deferred deletions
        for mut obj in self.objects.drain(..) {
            obj.vertex_buffer.cleanup(&self.device.device, &self.allocator);
            obj.index_buffer.cleanup(&self.device.device, &self.allocator);
            if let Some(bufs) = obj.dynamic_vertex_buffers.take() {
                for mut b in bufs {
                    b.cleanup(&self.device.device, &self.allocator);
                }
            }
            if let Some(mut tex) = obj.texture.take() {
                tex.cleanup(&self.device.device, &self.allocator);
            }
        }
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
                unsafe { device.destroy_buffer(buf.buffer, None); }
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
            self.device.device.wait_for_fences(&[self.frame_sync.in_flight[frame]], true, u64::MAX)?;
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
    pub fn total_icons(&self) -> usize { self.overlay.total_icons() }
    /// Icons whose decode + upload has completed.
    pub fn loaded_icons(&self) -> usize { self.overlay.loaded_icons() }

    pub fn draw_frame(&mut self) -> Result<()> {
        // Skip rendering when minimized
        if self.window_extent[0] == 0 || self.window_extent[1] == 0 {
            return Ok(());
        }

        self.frame_count += 1;

        let frame = self.current_frame;

        unsafe {
            self.device.device.wait_for_fences(&[self.frame_sync.in_flight[frame]], true, u64::MAX)?;
        }
        // Validation requires the cmd buffer to be reset before its referenced buffers are destroyed.
        let cmd = self.command_buffers[frame];
        unsafe { self.device.device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())? };
        self.flush_deletions();

        let device = &self.device.device;

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

        unsafe { device.reset_fences(&[self.frame_sync.in_flight[frame]])? };

        // Update view/proj UBO once per frame
        let mut point_light_pos = [Vec4::ZERO; 8];
        let mut point_light_color = [Vec4::ZERO; 8];
        let light_count = self.point_lights.len().min(8);
        for (i, pl) in self.point_lights.iter().take(8).enumerate() {
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
        // Snap shadow focus to camera position projected to ground (y=0). The
        // shadow module further snaps to texel size.
        let shadow_focus = Vec3::new(self.camera.position.x, 0.0, self.camera.position.z);
        let light_vp = shadow::light_view_proj(
            shadow_focus,
            Vec3::new(light_dir_normalized.x, light_dir_normalized.y, light_dir_normalized.z),
        );

        let ubo = UniformData {
            view: self.camera.view_matrix(),
            proj: self.camera.projection_matrix(),
            camera_pos: Vec4::new(
                self.camera.position.x,
                self.camera.position.y,
                self.camera.position.z,
                0.0,
            ),
            light_dir: light_dir_normalized,
            // Per-scene directional key + ambient. See
            // `KeyLight::DUNGEON` / `KeyLight::SUNLIT`.
            light_color: Vec4::new(
                self.key_light.color.x,
                self.key_light.color.y,
                self.key_light.color.z,
                self.key_light.ambient,
            ),
            fog_color: Vec4::new(self.fog_color[0], self.fog_color[1], self.fog_color[2], 0.0),
            fog_params: Vec4::new(self.fog_start, self.fog_end, 0.0, 0.0),
            fog_origin: Vec4::new(self.fog_origin.x, self.fog_origin.y, self.fog_origin.z, 0.0),
            point_light_pos,
            point_light_color,
            point_light_count: Vec4::new(light_count as f32, 0.0, 0.0, 0.0),
            light_vp,
        };
        self.uniforms.update(frame, &ubo);

        // Build draw commands with frustum culling + fog distance culling
        let frustum = self.camera.frustum_planes();
        let fog_cull_dist = self.fog_end + 2.0; // small margin beyond fog end
        let mut draws = Vec::with_capacity(self.objects.len());
        for obj in &self.objects {
            // Skip hidden objects (dead entities set matrix to zero)
            if obj.model_matrix == Mat4::ZERO {
                continue;
            }
            // Frustum cull: extract world-space center from model matrix column 3
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
            if !self.camera.sphere_in_frustum(&frustum, center, obj.bounds_radius + 1.0) {
                continue;
            }
            // Pick the per-frame dynamic VB if present, else the static one.
            let vb = match obj.dynamic_vertex_buffers.as_ref() {
                Some(bufs) => bufs[frame].buffer,
                None => obj.vertex_buffer.buffer,
            };
            draws.push(DrawCommand {
                vertex_buffer: vb,
                index_buffer: obj.index_buffer.buffer,
                index_count: obj.index_count,
                descriptor_set: self.uniforms.descriptor_sets[frame],
                material_set: obj.material_set,
                model_matrix: obj.model_matrix,
                tint: obj.tint,
            });
        }

        // Upload overlay batch
        self.overlay.upload(
            frame,
            &self.device.device,
            &self.allocator,
            self.device.graphics_queue,
            self.command_pool,
            &self.overlay_batch,
        )?;

        // Upload VFX ribbon instance data. Ribbons are rebuilt
        // every frame from the live VfxSystem effect set.
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

        // Record: begin command buffer + render pass, draw 3D, draw overlay, end
        unsafe {
            let begin_info = vk::CommandBufferBeginInfo::default();
            device.begin_command_buffer(cmd, &begin_info)?;

            // ---- Shadow pass: render scene depth from light's POV ----
            let shadow_clear = [vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue { depth: 1.0, stencil: 0 },
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
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.shadow_map.pipeline);
            for draw in &draws {
                device.cmd_bind_vertex_buffers(cmd, 0, &[draw.vertex_buffer], &[0]);
                device.cmd_bind_index_buffer(cmd, draw.index_buffer, 0, vk::IndexType::UINT32);
                device.cmd_bind_descriptor_sets(
                    cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.shadow_map.pipeline_layout,
                    0,
                    &[draw.descriptor_set],
                    &[],
                );
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

            // ---- Main pass (HDR scene) ----
            // Renders into the post-process HDR colour target,
            // not the swapchain. Sky, world meshes, ribbons and
            // particles all draw here. Overlay/UI moves to the
            // composite pass below.
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
            );

            // 3D scene
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            for draw in &draws {
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
                device.cmd_draw_indexed(cmd, draw.index_count, 1, 0, 0, 0);
            }

            // VFX ribbons (world-space, premultiplied additive).
            // Drawn before particles so the spark/glow trails
            // composite on top of the beam core.
            self.vfx_ribbon_renderer.record(frame, device, cmd, self.uniforms.descriptor_sets[frame]);
            // VFX particles (world-space, two pipelines).
            self.vfx_particle_renderer.record(frame, device, cmd, self.uniforms.descriptor_sets[frame]);

            device.cmd_end_render_pass(cmd);

            // ---- Bloom (bright extract → blur H → blur V) ----
            self.post.record_bloom(device, cmd, image_index, &self.bloom);

            // ---- Composite + overlay ----
            // Tonemap HDR + bloom into the swapchain, then draw
            // the UI overlay on top so it stays at native sRGB
            // crispness (no second tonemap pass).
            let composite_begin = vk::RenderPassBeginInfo::default()
                .render_pass(self.post.composite_pass)
                .framebuffer(self.post.composite_framebuffers[image_index as usize])
                .render_area(vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: self.swapchain.extent,
                });
            device.cmd_begin_render_pass(cmd, &composite_begin, vk::SubpassContents::INLINE);
            self.post.record_composite(device, cmd, image_index, &self.bloom, self.ghost_mix);

            // Overlay (HUD)
            self.overlay.record(frame, device, cmd);

            device.cmd_end_render_pass(cmd);
            device.end_command_buffer(cmd)?;
        }

        let wait_semaphores = [self.frame_sync.image_available[frame]];
        let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        let signal_semaphores = [self.frame_sync.render_finished[frame]];

        let submit_info = vk::SubmitInfo::default()
            .wait_semaphores(&wait_semaphores)
            .wait_dst_stage_mask(&wait_stages)
            .command_buffers(std::slice::from_ref(&cmd))
            .signal_semaphores(&signal_semaphores);

        unsafe {
            device.queue_submit(
                self.device.graphics_queue,
                &[submit_info],
                self.frame_sync.in_flight[frame],
            )?;
        }

        let swapchains = [self.swapchain.swapchain];
        let image_indices = [image_index];
        let present_info = vk::PresentInfoKHR::default()
            .wait_semaphores(&signal_semaphores)
            .swapchains(&swapchains)
            .image_indices(&image_indices);

        unsafe {
            match self
                .swapchain
                .swapchain_fn
                .queue_present(self.device.present_queue, &present_info)
            {
                Ok(_) => {}
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR | vk::Result::SUBOPTIMAL_KHR) => {
                    self.framebuffer_resized = false;
                    self.recreate_swapchain(self.window_extent[0], self.window_extent[1])?;
                    return Ok(());
                }
                Err(e) => return Err(e.into()),
            }
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
        (self.swapchain.extent.width as f32, self.swapchain.extent.height as f32)
    }

    /// Check for shader changes and hot-reload the pipeline if needed.
    pub fn check_hot_reload(&mut self) {
        let should_reload = self
            .hot_reloader
            .as_ref()
            .map(|hr| hr.check_and_reset())
            .unwrap_or(false);

        if should_reload {
            unsafe { self.device.device.device_wait_idle().ok(); }

            match Self::compile_pipeline_from_disk(
                &self.device.device,
                self.post.scene_pass,
                self.swapchain.extent,
                &[self.uniforms.descriptor_set_layout, self.material_pool.layout],
                &self.shader_dir,
            ) {
                Ok((new_pipeline, new_layout)) => {
                    unsafe {
                        self.device.device.destroy_pipeline(self.pipeline, None);
                        self.device.device.destroy_pipeline_layout(self.pipeline_layout, None);
                    }
                    self.pipeline = new_pipeline;
                    self.pipeline_layout = new_layout;
                    log::info!("Pipeline hot-reloaded successfully!");
                }
                Err(e) => {
                    log::error!("Hot-reload failed (keeping old pipeline): {}", e);
                }
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

        let vert_spv = hot_reload::compile_glsl(&vert_source, "triangle.vert", shaderc::ShaderKind::Vertex)?;
        let frag_spv = hot_reload::compile_glsl(&frag_source, "triangle.frag", shaderc::ShaderKind::Fragment)?;

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
                self.device.device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty()).ok();
            }
        }

        // Destroy all GPU buffers before freeing command pool/fences
        for obj in &mut self.objects {
            obj.vertex_buffer.cleanup(&self.device.device, &self.allocator);
            obj.index_buffer.cleanup(&self.device.device, &self.allocator);
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
            unsafe { device.destroy_buffer(buf.buffer, None); }
            if let Some(alloc) = buf.allocation {
                allocator.lock().unwrap().free(alloc).ok();
            }
        }

        self.frame_sync.cleanup(&self.device.device);
        unsafe {
            self.device.device.destroy_command_pool(self.command_pool, None);
        }

        self.uniforms.cleanup(&self.device.device, &self.allocator);
        self.default_texture.cleanup(&self.device.device, &self.allocator);
        self.material_pool.cleanup(&self.device.device, &self.allocator);
        self.shadow_map.cleanup(&self.device.device, &self.allocator);
        self.depth_buffer.cleanup(&self.device.device, &self.allocator);
        self.overlay.cleanup(&self.device.device, &self.allocator);
        self.vfx_ribbon_renderer.cleanup(&self.device.device, &self.allocator);
        self.vfx_particle_renderer.cleanup(&self.device.device, &self.allocator);
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
            return candidate.canonicalize().unwrap_or_else(|_| candidate.clone());
        }
    }

    // Fallback
    PathBuf::from("assets/shaders")
}
