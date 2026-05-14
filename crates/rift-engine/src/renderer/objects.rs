//! `Renderer` object-CRUD: meshes, dynamic / skinned meshes, materials,
//! per-object textures, blood-field binding, deferred buffer destruction,
//! glTF loading, and icon-streaming progress accessors.

use anyhow::Result;
use ash::vk;
use glam::Mat4;
use gpu_allocator::vulkan::Allocator;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::renderer::forward::Renderer;
use crate::renderer::gpu_skin::SkinHandle;
use crate::renderer::mesh::{Mesh, Vertex, VertexSkin};
use crate::renderer::texture::{PbrSource, Texture, TextureSource};
use crate::vulkan::{
    buffer::{self, GpuBuffer},
    sync::MAX_FRAMES_IN_FLIGHT,
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

struct TextureUploadGuard<'a> {
    textures: Vec<Texture>,
    device: &'a ash::Device,
    allocator: &'a Arc<Mutex<Allocator>>,
    armed: bool,
}

impl<'a> TextureUploadGuard<'a> {
    fn new(device: &'a ash::Device, allocator: &'a Arc<Mutex<Allocator>>) -> Self {
        Self {
            textures: Vec::new(),
            device,
            allocator,
            armed: true,
        }
    }

    fn with_capacity(
        device: &'a ash::Device,
        allocator: &'a Arc<Mutex<Allocator>>,
        capacity: usize,
    ) -> Self {
        Self {
            textures: Vec::with_capacity(capacity),
            device,
            allocator,
            armed: true,
        }
    }

    fn finish(mut self) -> Vec<Texture> {
        self.armed = false;
        std::mem::take(&mut self.textures)
    }
}

impl Drop for TextureUploadGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            for tex in &mut self.textures {
                tex.cleanup(self.device, self.allocator);
            }
        }
    }
}

impl Renderer {
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

    /// Seed a GPU-skinned object's output buffer with CPU-skinned vertices.
    pub fn prime_skinned_mesh_output(&mut self, obj_idx: usize, vertices: &[Vertex]) -> Result<()> {
        let handle = match self.objects.get(obj_idx).and_then(|o| o.skin_handle) {
            Some(h) => h,
            None => return Ok(()),
        };
        self.skin_system.prime_output_vertices(
            &self.device.device,
            &self.allocator,
            self.device.graphics_queue,
            self.command_pool,
            handle,
            vertices,
        )
    }

    /// Monotonic render-frame counter. Gameplay-side render systems use
    /// this for deterministic visual LOD staggering without owning their
    /// own global frame state.
    pub fn frame_count(&self) -> u64 {
        self.frame_count
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
        let mut texture = Texture::load(
            &self.device.device,
            &self.allocator,
            self.device.graphics_queue,
            self.command_pool,
            src,
        )?;
        let set = match self.material_pool.alloc_set(&self.device.device, &texture) {
            Ok(set) => set,
            Err(e) => {
                texture.cleanup(&self.device.device, &self.allocator);
                return Err(e);
            }
        };
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
        let mut texture = Texture::load(
            &self.device.device,
            &self.allocator,
            self.device.graphics_queue,
            self.command_pool,
            src,
        )?;
        let set = match self.material_pool.alloc_set(&self.device.device, &texture) {
            Ok(set) => set,
            Err(e) => {
                texture.cleanup(&self.device.device, &self.allocator);
                return Err(e);
            }
        };
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
        let (mut owned, normal_idx, mr_idx, ao_idx, height_idx) = match src {
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
                let mut guard = TextureUploadGuard::new(&self.device.device, &self.allocator);
                guard.textures.push(basecolor);
                let mut load_linear = |path: Option<&Path>| -> Result<Option<usize>> {
                    let Some(p) = path else { return Ok(None) };
                    let t = Texture::from_file_linear(
                        &self.device.device,
                        &self.allocator,
                        self.device.graphics_queue,
                        self.command_pool,
                        p,
                    )?;
                    guard.textures.push(t);
                    Ok(Some(guard.textures.len() - 1))
                };
                let normal_idx = load_linear(normal)?;
                let mr_idx = load_linear(metallic_roughness)?;
                let ao_idx = load_linear(ao)?;
                let height_idx = load_linear(height)?;
                (guard.finish(), normal_idx, mr_idx, ao_idx, height_idx)
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
                let mut guard = TextureUploadGuard::new(&self.device.device, &self.allocator);
                guard.textures.push(basecolor);
                let mut load_linear = |path: Option<&Path>| -> Result<Option<usize>> {
                    let Some(p) = path else { return Ok(None) };
                    let t = Texture::from_file_linear(
                        &self.device.device,
                        &self.allocator,
                        self.device.graphics_queue,
                        self.command_pool,
                        p,
                    )?;
                    guard.textures.push(t);
                    Ok(Some(guard.textures.len() - 1))
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
                    guard.textures.push(tex);
                    Some(guard.textures.len() - 1)
                } else {
                    None
                };
                (guard.finish(), normal_idx, mr_idx, ao_idx, height_idx)
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
                let mut guard =
                    TextureUploadGuard::with_capacity(&self.device.device, &self.allocator, 5);
                guard.textures.push(upload(self, basecolor)?);
                let push_opt =
                    |opt: Option<_>, owned: &mut TextureUploadGuard<'_>| -> Result<Option<usize>> {
                        if let Some(d) = opt {
                            owned.textures.push(upload(self, d)?);
                            Ok(Some(owned.textures.len() - 1))
                        } else {
                            Ok(None)
                        }
                    };
                let normal_idx = push_opt(normal, &mut guard)?;
                let mr_idx = push_opt(mr, &mut guard)?;
                let ao_idx = push_opt(ao, &mut guard)?;
                let height_idx = push_opt(height, &mut guard)?;
                (guard.finish(), normal_idx, mr_idx, ao_idx, height_idx)
            }
        };

        let basecolor_ref = &owned[0];
        let set = match self.material_pool.alloc_pbr_set(
            &self.device.device,
            basecolor_ref,
            normal_idx.map(|i| &owned[i]),
            mr_idx.map(|i| &owned[i]),
            ao_idx.map(|i| &owned[i]),
            height_idx.map(|i| &owned[i]),
        ) {
            Ok(set) => set,
            Err(e) => {
                for tex in &mut owned {
                    tex.cleanup(&self.device.device, &self.allocator);
                }
                return Err(e);
            }
        };
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
    pub(super) fn flush_deletions(&mut self) {
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
}
