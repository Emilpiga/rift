use anyhow::Result;
use ash::vk;
use glam::{Mat4, Vec3, Vec4};
use gpu_allocator::vulkan::{Allocator, AllocatorCreateDesc};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::hot_reload::{self, HotReloader};
use crate::renderer::camera::Camera;
use crate::renderer::depth::{DepthBuffer, DEPTH_FORMAT};
use crate::renderer::material::MaterialPool;
use crate::renderer::mesh::{Mesh, Vertex};
use crate::renderer::shadow::{self, ShadowMap};
use crate::renderer::overlay::{OverlayBatch, OverlayRenderer};
use crate::renderer::particle_renderer::ParticleRenderer;
use crate::renderer::particles::ParticleSystem;
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
    framebuffers: Vec<vk::Framebuffer>,
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    render_pass: vk::RenderPass,
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
    // Particles
    pub particle_renderer: ParticleRenderer,
    pub particle_system: ParticleSystem,
    // Deferred deletion queue for GPU buffers
    deletion_queue: Vec<(u64, GpuBuffer)>,
    // Ambient clear color (themed per floor)
    pub clear_color: [f32; 4],
    // Fog parameters
    pub fog_color: [f32; 3],
    pub fog_start: f32,
    pub fog_end: f32,
    // Dynamic point lights (populated each frame by game code)
    pub point_lights: Vec<PointLight>,
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

        let render_pass =
            pipeline::create_render_pass(&device.device, swapchain.format.format, DEPTH_FORMAT)?;

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

        // Determine shader directory (relative to executable or workspace)
        let shader_dir = find_shader_dir();

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

        let depth_buffer = DepthBuffer::new(&device.device, &allocator, swapchain.extent)?;

        let framebuffers =
            create_framebuffers(&device.device, &swapchain, &depth_buffer, render_pass)?;

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
            render_pass, swapchain.extent, &shader_dir,
        )?;
        let overlay_batch = OverlayBatch::new();

        let particle_renderer = ParticleRenderer::new(
            &device.device, &allocator, device.graphics_queue, command_pool,
            render_pass, swapchain.extent, uniforms.descriptor_set_layout, &shader_dir,
        )?;
        let particle_system = ParticleSystem::new(4096);

        log::info!("Renderer initialized successfully");

        Ok(Self {
            instance,
            surface,
            surface_fn,
            device,
            allocator,
            swapchain,
            render_pass,
            pipeline: pipeline_handle,
            pipeline_layout,
            framebuffers,
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
            particle_renderer,
            particle_system,
            deletion_queue: Vec::new(),
            clear_color: [0.008, 0.006, 0.010, 1.0],
            fog_color: [0.018, 0.012, 0.010],
            fog_start: 6.0,
            fog_end: 18.0,
            point_lights: Vec::new(),
        })
    }

    /// Recreate the swapchain, depth buffer, framebuffers, and pipeline for new dimensions.
    pub fn recreate_swapchain(&mut self, width: u32, height: u32) -> Result<()> {
        if width == 0 || height == 0 {
            return Ok(()); // Minimized — skip
        }

        unsafe { self.device.device.device_wait_idle()?; }

        // Destroy old framebuffers
        for &fb in &self.framebuffers {
            unsafe { self.device.device.destroy_framebuffer(fb, None); }
        }
        self.framebuffers.clear();

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

        // Recreate framebuffers
        self.framebuffers = create_framebuffers(
            &self.device.device,
            &self.swapchain,
            &self.depth_buffer,
            self.render_pass,
        )?;

        // Recreate pipeline with new extent
        let (new_pipeline, new_layout) = Self::compile_pipeline_from_disk(
            &self.device.device,
            self.render_pass,
            self.swapchain.extent,
            &[self.uniforms.descriptor_set_layout, self.material_pool.layout],
            &self.shader_dir,
        )?;
        self.pipeline = new_pipeline;
        self.pipeline_layout = new_layout;

        // Recreate overlay pipeline
        self.overlay.recreate_pipeline(&self.device.device, self.render_pass, self.swapchain.extent, &self.shader_dir)?;

        // Recreate particle pipeline
        self.particle_renderer.recreate_pipeline(
            &self.device.device, self.render_pass, self.swapchain.extent,
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

        let light_dir_world = Vec4::new(0.4, 0.8, 0.3, 0.0);
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
            light_color: Vec4::new(1.05, 0.82, 0.58, 0.22), // warm amber torchlight, moderate ambient so walls/textures stay readable
            fog_color: Vec4::new(self.fog_color[0], self.fog_color[1], self.fog_color[2], 0.0),
            fog_params: Vec4::new(self.fog_start, self.fog_end, 0.0, 0.0),
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
            // Distance cull: skip objects fully inside fog
            let dist_to_cam = (center - self.camera.position).length();
            if dist_to_cam - obj.bounds_radius > fog_cull_dist {
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

        // Upload particle instance data
        let particle_instances = self.particle_system.instance_data();
        self.particle_renderer.upload(
            frame,
            &self.device.device,
            &self.allocator,
            &particle_instances,
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

            // ---- Main pass ----
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
                .render_pass(self.render_pass)
                .framebuffer(self.framebuffers[image_index as usize])
                .render_area(vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: self.swapchain.extent,
                })
                .clear_values(&clear_values);

            device.cmd_begin_render_pass(cmd, &render_pass_begin, vk::SubpassContents::INLINE);

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
                    vk::ShaderStageFlags::VERTEX,
                    0,
                    model_bytes,
                );
                device.cmd_draw_indexed(cmd, draw.index_count, 1, 0, 0, 0);
            }

            // Particles (world-space, additive blending)
            self.particle_renderer.record(frame, device, cmd, self.uniforms.descriptor_sets[frame]);

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
                self.render_pass,
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
        self.particle_renderer.cleanup(&self.device.device, &self.allocator);

        unsafe {
            for &fb in &self.framebuffers {
                self.device.device.destroy_framebuffer(fb, None);
            }

            self.device.device.destroy_pipeline(self.pipeline, None);
            self.device
                .device
                .destroy_pipeline_layout(self.pipeline_layout, None);
            self.device.device.destroy_render_pass(self.render_pass, None);

            self.swapchain.cleanup(&self.device.device);
            self.surface_fn.destroy_surface(self.surface, None);
        }

        // Drop allocator before device & instance (auto-drop handles the rest)
        drop(self.allocator.lock());
    }
}

fn create_framebuffers(
    device: &ash::Device,
    swapchain: &Swapchain,
    depth: &DepthBuffer,
    render_pass: vk::RenderPass,
) -> Result<Vec<vk::Framebuffer>> {
    swapchain
        .image_views
        .iter()
        .map(|&view| {
            let attachments = [view, depth.view];
            let fb_info = vk::FramebufferCreateInfo::default()
                .render_pass(render_pass)
                .attachments(&attachments)
                .width(swapchain.extent.width)
                .height(swapchain.extent.height)
                .layers(1);
            unsafe { device.create_framebuffer(&fb_info, None).map_err(Into::into) }
        })
        .collect()
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
