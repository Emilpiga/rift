//! GPU mesh skinning — replaces the per-frame CPU `skin_to` loop.
//!
//! For each skinned mesh we keep three immutable device-local buffers
//! (rest-pose vertices, per-vertex skin weights, the output vertex
//! buffer the graphics pipelines bind) plus one small host-visible
//! bone-palette UBO per in-flight frame. Each frame the ECS skinning
//! system writes the freshly evaluated bone palette into the current
//! frame's UBO and we dispatch `skin.comp` once per visible skinned
//! object before the shadow pass. The compute shader fills the
//! output VB with fully transformed vertices that the existing
//! shadow + forward + point-shadow pipelines consume completely
//! unchanged.
//!
//! This module is intentionally self-contained: the only thing the
//! `Renderer` has to do is hold a `SkinningSystem`, call
//! `record_dispatches` near the top of `draw_frame`, and route the
//! bound vertex buffer through `output_vertex_buffer(handle)` for
//! skinned objects (replacing the old host-visible
//! `dynamic_vertex_buffers` ring).

use anyhow::{Context, Result};
use ash::vk;
use glam::Mat4;
use gpu_allocator::vulkan::Allocator;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::hot_reload;
use crate::renderer::mesh::{Vertex, VertexSkin};
use crate::vulkan::buffer::{self, GpuBuffer};
use crate::vulkan::pipeline;
use crate::vulkan::sync::MAX_FRAMES_IN_FLIGHT;

/// Hard cap on bones per skeleton. Sized large enough for the meanest
/// rig in the asset pack with comfortable headroom; bumping this just
/// changes the palette UBO size (16 KiB ÷ 64 B per mat4 = 256, so we
/// stay well under any UBO size limit even at 128).
pub const MAX_PALETTE_JOINTS: usize = 128;

const PALETTE_BYTES: vk::DeviceSize =
    (MAX_PALETTE_JOINTS * std::mem::size_of::<Mat4>()) as vk::DeviceSize;

const COMPUTE_LOCAL_X: u32 = 64;

/// Opaque handle returned by `register_mesh`. The `Renderer` stores
/// one of these on each skinned `RenderObject` and uses
/// `output_vertex_buffer(handle)` at draw time + `update_palette` /
/// `mark_active` per frame.
#[derive(Clone, Copy, Debug)]
pub struct SkinHandle(pub usize);

/// All GPU resources for one skinned mesh. Owned by `SkinSlot`
/// while the mesh is registered, then moved into a `PendingFree`
/// retirement entry on `free_mesh` so the GPU has a few frames
/// to finish reading the buffers before we destroy them.
struct SkinnedMeshResources {
    /// Number of vertices in `rest_vb` and `output_vb`.
    vertex_count: u32,
    /// Rest-pose vertex stream — uploaded once at registration and
    /// never touched again. Bound at compute set 0 binding 0.
    rest_vb: GpuBuffer,
    /// Per-vertex `(joints, weights)` — uploaded once. Bound at
    /// compute set 0 binding 1. The on-GPU layout matches the host
    /// `repr(C)` `VertexSkin`: 8 bytes joints + 16 bytes weights.
    skin_buf: GpuBuffer,
    /// Per-frame palette UBOs, host-visible. Index by `current_frame`.
    palette_ubos: [GpuBuffer; MAX_FRAMES_IN_FLIGHT],
    /// Output vertex buffer — device-local, written by `skin.comp`,
    /// then bound as a regular `VERTEX_BUFFER` by the graphics
    /// passes. We allocate a single output buffer (not one per
    /// in-flight frame) because we re-skin every frame and protect
    /// the read-then-write hazard with the in-frame compute→vertex
    /// barrier already issued before the shadow pass.
    output_vb: GpuBuffer,
    /// Optional outfit-shell push along the skinned normal. Mirrors
    /// the old `skin_to_inflated` behaviour for outfit pieces so
    /// they sit just outside the body and don't z-fight.
    inflate: f32,
    /// Set true when the ECS marks this mesh as needing a re-skin
    /// for the upcoming frame. Cleared after the dispatch is
    /// recorded. Lets distant / culled monsters skip the dispatch
    /// entirely without losing their last good output VB.
    active_this_frame: bool,
}

/// One slot in the `SkinningSystem`. Descriptor sets live with
/// the slot, not the mesh resources, so a slot can be recycled
/// without re-allocating descriptors — we just rewire the
/// existing sets to point at the new buffers via
/// `update_descriptor_sets`. This avoids needing
/// `FREE_DESCRIPTOR_SET` on the pool (which fragments) and means
/// per-mesh free is essentially free of pool churn.
struct SkinSlot {
    /// `None` when the slot is on the free list, `Some` when a
    /// `SkinHandle` is live for it. `SkinHandle(i)` always refers
    /// to slot `i` whether occupied or not — callers can hold a
    /// stale handle without UB; `update_palette` / draw routing
    /// just no-op silently because the entity that owned it has
    /// already cleared `RenderObject.skin_handle`.
    resources: Option<SkinnedMeshResources>,
    /// Persistent descriptor sets, one per in-flight frame. Stay
    /// allocated for the slot's whole lifetime even across
    /// recycle; rewired in-place when a new mesh moves in.
    descriptor_sets: [vk::DescriptorSet; MAX_FRAMES_IN_FLIGHT],
}

/// A mesh that's been logically freed but whose GPU buffers may
/// still be referenced by an in-flight frame's compute dispatch
/// or shadow/forward pass. Destroyed once `frames_remaining` hits
/// zero (decremented by `record_dispatches`, which is called once
/// per frame).
struct PendingFree {
    rest_vb: GpuBuffer,
    skin_buf: GpuBuffer,
    palette_ubos: [GpuBuffer; MAX_FRAMES_IN_FLIGHT],
    output_vb: GpuBuffer,
    frames_remaining: u32,
}

/// Owns the compute pipeline and every per-mesh resource bundle.
pub struct SkinningSystem {
    descriptor_set_layout: vk::DescriptorSetLayout,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    /// Chain of descriptor pools. We allocate from the back; when
    /// it returns `OUT_OF_POOL_MEMORY` we push another pool sized
    /// for `POOL_CHUNK` more meshes. There is no registration cap
    /// — the only ceiling is host VRAM. Per-frame work still
    /// scales with the *visible* skinned-mesh count, which the
    /// ECS culls separately.
    descriptor_pools: Vec<vk::DescriptorPool>,
    /// All slots ever allocated, in handle order. Vacant slots
    /// (mesh freed) keep their descriptor sets alive so the next
    /// `register_mesh` can reuse them.
    slots: Vec<SkinSlot>,
    /// Indices into `slots` that are currently vacant. LIFO so
    /// recently freed slots are reused first — keeps the active
    /// dispatch range compact under steady-state churn.
    free_slots: Vec<usize>,
    /// Mesh buffers retired but not yet destroyed. Drained by
    /// `record_dispatches` once their `frames_remaining` countdown
    /// hits zero, guaranteeing the GPU is no longer reading them.
    pending_free: Vec<PendingFree>,
}

/// How many skinned-mesh slots each descriptor pool covers.
/// Picked so the very first pool comfortably handles the player
/// + a normal floor's monster count without growth, but small
/// enough that late-floor growth happens in modest steps rather
/// than one giant up-front allocation.
const POOL_CHUNK: usize = 128;

#[repr(C)]
#[derive(Clone, Copy)]
struct PushConsts {
    vertex_count: u32,
    inflate: f32,
    _pad0: u32,
    _pad1: u32,
}

impl SkinningSystem {
    pub fn new(device: &ash::Device, shader_dir: &Path) -> Result<Self> {
        // ---- Descriptor set layout: 4 storage + 1 uniform binding ------
        // Slots match `assets/shaders/skin.comp`:
        //   binding 0 : rest vertices  (storage, read-only)
        //   binding 1 : skin influences (storage, read-only)
        //   binding 2 : bone palette    (storage, read-only)  -- see note
        //   binding 3 : output vertices (storage, write-only)
        //
        // The palette is declared as a storage buffer in the shader
        // (not a UBO) to keep the descriptor set homogeneous and so
        // we never have to worry about the 16 KiB UBO limit if the
        // joint cap ever grows past 256.
        let bindings = [
            descriptor_binding(0, vk::DescriptorType::STORAGE_BUFFER),
            descriptor_binding(1, vk::DescriptorType::STORAGE_BUFFER),
            descriptor_binding(2, vk::DescriptorType::STORAGE_BUFFER),
            descriptor_binding(3, vk::DescriptorType::STORAGE_BUFFER),
        ];
        let layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        let descriptor_set_layout =
            unsafe { device.create_descriptor_set_layout(&layout_info, None)? };

        // ---- Pipeline layout: one descriptor set + push constants ------
        let push_range = vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .offset(0)
            .size(std::mem::size_of::<PushConsts>() as u32);
        let pl_info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(std::slice::from_ref(&descriptor_set_layout))
            .push_constant_ranges(std::slice::from_ref(&push_range));
        let pipeline_layout = unsafe { device.create_pipeline_layout(&pl_info, None)? };

        // ---- Compile + create compute pipeline -------------------------
        let pipeline = build_compute_pipeline(device, shader_dir, pipeline_layout)
            .context("build skin.comp compute pipeline")?;

        // ---- First descriptor pool. We grow on demand from there. ------
        let descriptor_pools = vec![create_pool(device)?];

        Ok(Self {
            descriptor_set_layout,
            pipeline_layout,
            pipeline,
            descriptor_pools,
            slots: Vec::with_capacity(POOL_CHUNK),
            free_slots: Vec::new(),
            pending_free: Vec::new(),
        })
    }

    /// Upload a skinned mesh's immutable data and allocate its
    /// per-frame palette UBOs + output VB. Returns a `SkinHandle`
    /// the caller stores on the corresponding `RenderObject`.
    ///
    /// `inflate` is applied along the post-skin normal in the
    /// compute shader — pass `0.0` for body, ~`0.022` for outfit
    /// shells.
    pub fn register_mesh(
        &mut self,
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        rest_vertices: &[Vertex],
        skin_data: &[VertexSkin],
        inflate: f32,
    ) -> Result<SkinHandle> {
        if rest_vertices.len() != skin_data.len() {
            anyhow::bail!(
                "SkinningSystem::register_mesh: rest ({}) and skin ({}) length mismatch",
                rest_vertices.len(),
                skin_data.len(),
            );
        }
        let vertex_count = rest_vertices.len() as u32;

        // Rest VB — needs STORAGE for the compute shader to read it.
        let rest_vb = buffer::create_device_local_buffer(
            device,
            allocator,
            queue,
            command_pool,
            rest_vertices,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            "skinned_rest_vb",
        )?;

        // Skin influences SSBO. The on-disk struct is 24 bytes
        // (`[u16; 4]` + `[f32; 4]`); we ship it verbatim and the
        // compute shader unpacks the joint pairs from u32 lanes.
        let skin_buf = buffer::create_device_local_buffer(
            device,
            allocator,
            queue,
            command_pool,
            skin_data,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            "skinned_skin_data",
        )?;

        // Output VB — device-local, sized to match the rest stream.
        // Needs STORAGE (compute writes it) + VERTEX_BUFFER (graphics
        // passes bind it) and TRANSFER_DST so we can prime it with a
        // copy of the rest pose for the very first frame, before any
        // dispatch has run (otherwise a freshly registered mesh
        // would draw garbage on frame 0).
        let vb_size = (rest_vertices.len() * std::mem::size_of::<Vertex>()) as vk::DeviceSize;
        let output_vb = GpuBuffer::new(
            device,
            allocator,
            vb_size,
            vk::BufferUsageFlags::STORAGE_BUFFER
                | vk::BufferUsageFlags::VERTEX_BUFFER
                | vk::BufferUsageFlags::TRANSFER_DST,
            gpu_allocator::MemoryLocation::GpuOnly,
            "skinned_output_vb",
        )?;
        prime_output_vb(
            device,
            allocator,
            queue,
            command_pool,
            &output_vb,
            rest_vertices,
        )?;

        // Per-frame palette UBOs (held in a STORAGE buffer for binding
        // simplicity — see set-layout note above). Initialise with
        // identity matrices so a never-updated palette still skins to
        // bind pose instead of producing NaNs.
        let identity_palette = vec![Mat4::IDENTITY; MAX_PALETTE_JOINTS];
        let palette_ubos: [GpuBuffer; MAX_FRAMES_IN_FLIGHT] = std::array::from_fn(|i| {
            buffer::create_host_buffer(
                device,
                allocator,
                &identity_palette,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                &format!("skinned_palette_ubo[{}]", i),
            )
            .expect("alloc skinned palette ubo")
        });

        // Pick a slot: reuse a freed one if available (avoids
        // allocating fresh descriptor sets), otherwise extend.
        let (slot_idx, descriptor_sets) = if let Some(idx) = self.free_slots.pop() {
            let sets = self.slots[idx].descriptor_sets;
            (idx, sets)
        } else {
            // Allocate one descriptor set per in-flight frame from
            // the most recent pool; if it's exhausted grow the
            // chain by one pool and retry. Vulkan returns
            // OUT_OF_POOL_MEMORY (or, on some drivers,
            // FRAGMENTED_POOL) when a pool is full — both are
            // recoverable by allocating from a fresh pool.
            let layouts = vec![self.descriptor_set_layout; MAX_FRAMES_IN_FLIGHT];
            let raw_sets = self.allocate_sets_growing(device, &layouts)?;
            let mut sets = [vk::DescriptorSet::null(); MAX_FRAMES_IN_FLIGHT];
            sets.copy_from_slice(&raw_sets);
            let idx = self.slots.len();
            self.slots.push(SkinSlot {
                resources: None,
                descriptor_sets: sets,
            });
            (idx, sets)
        };

        // (Re)wire the slot's descriptor sets to point at this
        // mesh's buffers. Safe whether the sets are fresh or
        // recycled — the GPU has had `MAX_FRAMES_IN_FLIGHT` frames
        // of dispatch quiet on this slot since `free_mesh`
        // retired the previous occupant's buffers.
        for (frame, set) in descriptor_sets.iter().enumerate() {
            write_set(
                device,
                *set,
                &rest_vb,
                &skin_buf,
                &palette_ubos[frame],
                &output_vb,
            );
        }

        self.slots[slot_idx].resources = Some(SkinnedMeshResources {
            vertex_count,
            rest_vb,
            skin_buf,
            palette_ubos,
            output_vb,
            inflate,
            active_this_frame: false,
        });
        Ok(SkinHandle(slot_idx))
    }

    /// Free the GPU resources of the mesh registered under `handle`.
    /// Buffers are deferred for `MAX_FRAMES_IN_FLIGHT` frames so any
    /// in-flight compute / shadow / forward pass that already
    /// recorded a reference to them finishes safely. The slot's
    /// descriptor sets are *kept* and recycled by the next
    /// `register_mesh` call — cheaper than freeing & re-allocating.
    /// Stale handles are silently ignored.
    pub fn free_mesh(&mut self, handle: SkinHandle) {
        let slot = match self.slots.get_mut(handle.0) {
            Some(s) => s,
            None => return,
        };
        let res = match slot.resources.take() {
            Some(r) => r,
            None => return, // already freed
        };
        // Defer destruction by exactly the in-flight frame depth.
        // `record_dispatches` decrements once per frame, so after
        // MAX_FRAMES_IN_FLIGHT ticks every queued frame that could
        // still be reading these buffers has retired.
        self.pending_free.push(PendingFree {
            rest_vb: res.rest_vb,
            skin_buf: res.skin_buf,
            palette_ubos: res.palette_ubos,
            output_vb: res.output_vb,
            frames_remaining: MAX_FRAMES_IN_FLIGHT as u32 + 1,
        });
        self.free_slots.push(handle.0);
    }

    /// Update the bone palette for `handle` for the frame currently
    /// being prepared. Pads / truncates to `MAX_PALETTE_JOINTS`.
    /// Marks the mesh active so its compute dispatch will be
    /// recorded by the next `record_dispatches`. No-op for stale
    /// or freed handles.
    pub fn update_palette(&mut self, current_frame: usize, handle: SkinHandle, palette: &[Mat4]) {
        let mesh = match self
            .slots
            .get_mut(handle.0)
            .and_then(|s| s.resources.as_mut())
        {
            Some(m) => m,
            None => return,
        };
        let buf = &mut mesh.palette_ubos[current_frame];
        let n = palette.len().min(MAX_PALETTE_JOINTS);
        // Write the first `n` matrices; trailing slots keep whatever
        // was there before (irrelevant — the compute shader only
        // indexes `joints[k]` which the loader has already clamped to
        // the joint count of this mesh).
        if n > 0 {
            buf.write(&palette[..n]);
        }
        mesh.active_this_frame = true;
    }

    /// Returns the GPU buffer the graphics pipelines should bind for
    /// this skinned mesh's vertices (i.e. the compute shader's
    /// output target). Stable across frames — safe to cache on the
    /// `RenderObject`.
    pub fn output_vertex_buffer(&self, handle: SkinHandle) -> Option<vk::Buffer> {
        self.slots
            .get(handle.0)
            .and_then(|s| s.resources.as_ref())
            .map(|m| m.output_vb.buffer)
    }

    /// Record a compute dispatch for every mesh whose palette was
    /// updated this frame, followed by a single barrier publishing
    /// the writes to subsequent vertex-input reads. Call once near
    /// the top of `draw_frame`, before the shadow pass begins.
    pub fn record_dispatches(
        &mut self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        current_frame: usize,
        allocator: &Arc<Mutex<Allocator>>,
    ) {
        // Tick the deferred-destruction queue first — any pending
        // free that has aged past the in-flight depth is safe to
        // destroy now (no queued frame can still reference it).
        // Done before recording dispatches so we never destroy a
        // buffer the dispatch loop is about to bind for.
        self.pending_free.retain_mut(|p| {
            if p.frames_remaining == 0 {
                p.rest_vb.cleanup(device, allocator);
                p.skin_buf.cleanup(device, allocator);
                p.output_vb.cleanup(device, allocator);
                for buf in p.palette_ubos.iter_mut() {
                    buf.cleanup(device, allocator);
                }
                false
            } else {
                p.frames_remaining -= 1;
                true
            }
        });

        // Collect indices of active meshes first so we know whether
        // we need to issue the post-dispatch barrier at all.
        let mut any = false;
        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, self.pipeline);
            for slot in self.slots.iter_mut() {
                let mesh = match slot.resources.as_mut() {
                    Some(m) if m.active_this_frame => m,
                    _ => continue,
                };
                mesh.active_this_frame = false;
                any = true;

                let set = slot.descriptor_sets[current_frame];
                device.cmd_bind_descriptor_sets(
                    cmd,
                    vk::PipelineBindPoint::COMPUTE,
                    self.pipeline_layout,
                    0,
                    std::slice::from_ref(&set),
                    &[],
                );
                let pc = PushConsts {
                    vertex_count: mesh.vertex_count,
                    inflate: mesh.inflate,
                    _pad0: 0,
                    _pad1: 0,
                };
                let pc_bytes = std::slice::from_raw_parts(
                    &pc as *const _ as *const u8,
                    std::mem::size_of::<PushConsts>(),
                );
                device.cmd_push_constants(
                    cmd,
                    self.pipeline_layout,
                    vk::ShaderStageFlags::COMPUTE,
                    0,
                    pc_bytes,
                );
                let groups = (mesh.vertex_count + COMPUTE_LOCAL_X - 1) / COMPUTE_LOCAL_X;
                device.cmd_dispatch(cmd, groups, 1, 1);
            }

            if any {
                // One global barrier covers every output VB —
                // cheaper than per-buffer barriers and just as
                // correct because the shadow pass that follows will
                // sample any of them.
                let barrier = vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                    .dst_access_mask(vk::AccessFlags::VERTEX_ATTRIBUTE_READ);
                device.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::PipelineStageFlags::VERTEX_INPUT,
                    vk::DependencyFlags::empty(),
                    std::slice::from_ref(&barrier),
                    &[],
                    &[],
                );
            }
        }
    }

    /// Recompile + swap the compute pipeline. Called from the
    /// renderer's hot-reload check so editing `skin.comp` updates
    /// without a restart.
    pub fn reload_pipeline(&mut self, device: &ash::Device, shader_dir: &Path) -> Result<()> {
        let new_pipeline = build_compute_pipeline(device, shader_dir, self.pipeline_layout)?;
        let old = std::mem::replace(&mut self.pipeline, new_pipeline);
        unsafe {
            // Safe: the previous pipeline can't be in use because we
            // reload between frames after `device_wait_idle`.
            device.destroy_pipeline(old, None);
        }
        Ok(())
    }

    pub fn cleanup(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        self.clear(device, allocator);
        unsafe {
            for pool in self.descriptor_pools.drain(..) {
                device.destroy_descriptor_pool(pool, None);
            }
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_descriptor_set_layout(self.descriptor_set_layout, None);
        }
    }

    /// Free every registered mesh's GPU resources and reset every
    /// descriptor pool back to empty so the next floor starts from
    /// a clean slot table. Pool *objects* are kept around (cheap to
    /// reset, expensive to recreate) — this is hot-path code
    /// between floors. Caller (`Renderer::clear_objects`) has
    /// already issued `device_wait_idle` so destroying buffers is
    /// safe.
    pub fn clear(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        // Drop any live mesh resources.
        for slot in self.slots.drain(..) {
            if let Some(res) = slot.resources {
                let SkinnedMeshResources {
                    mut rest_vb,
                    mut skin_buf,
                    mut output_vb,
                    mut palette_ubos,
                    ..
                } = res;
                rest_vb.cleanup(device, allocator);
                skin_buf.cleanup(device, allocator);
                output_vb.cleanup(device, allocator);
                for buf in palette_ubos.iter_mut() {
                    buf.cleanup(device, allocator);
                }
            }
        }
        self.free_slots.clear();
        // Drain any deferred frees too. `clear` is only called
        // after a `device_wait_idle`, so it's safe to skip the
        // frame countdown and destroy them right now.
        for mut p in self.pending_free.drain(..) {
            p.rest_vb.cleanup(device, allocator);
            p.skin_buf.cleanup(device, allocator);
            p.output_vb.cleanup(device, allocator);
            for buf in p.palette_ubos.iter_mut() {
                buf.cleanup(device, allocator);
            }
        }
        // Reset every pool so the descriptor sets we handed out
        // (which referenced the buffers we just freed) are recycled.
        // Then collapse the chain back to a single pool — late-floor
        // growth shouldn't permanently inflate our descriptor-pool
        // commit if the next floor turns out lighter.
        unsafe {
            let extras = self.descriptor_pools.split_off(1);
            for pool in extras {
                device.destroy_descriptor_pool(pool, None);
            }
            for pool in &self.descriptor_pools {
                device
                    .reset_descriptor_pool(*pool, vk::DescriptorPoolResetFlags::empty())
                    .expect("reset skin descriptor pool");
            }
        }
    }

    /// Try to allocate `layouts.len()` descriptor sets from the
    /// last pool in the chain; if it's full, push a new pool and
    /// retry. Returns the allocated sets (length matches
    /// `layouts`).
    fn allocate_sets_growing(
        &mut self,
        device: &ash::Device,
        layouts: &[vk::DescriptorSetLayout],
    ) -> Result<Vec<vk::DescriptorSet>> {
        for _ in 0..2 {
            let pool = *self.descriptor_pools.last().expect("at least one pool");
            let info = vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(pool)
                .set_layouts(layouts);
            match unsafe { device.allocate_descriptor_sets(&info) } {
                Ok(sets) => return Ok(sets),
                Err(vk::Result::ERROR_OUT_OF_POOL_MEMORY)
                | Err(vk::Result::ERROR_FRAGMENTED_POOL) => {
                    // Pool full — grow the chain and try again.
                    self.descriptor_pools.push(create_pool(device)?);
                    continue;
                }
                Err(e) => return Err(anyhow::anyhow!("allocate skin descriptor sets: {:?}", e)),
            }
        }
        anyhow::bail!("SkinningSystem: failed to allocate descriptor sets after pool grow");
    }
}

// ---------- helpers ---------------------------------------------------------

fn create_pool(device: &ash::Device) -> Result<vk::DescriptorPool> {
    // Sized for `POOL_CHUNK` skinned meshes × `MAX_FRAMES_IN_FLIGHT`
    // sets per mesh × 4 storage-buffer descriptors per set.
    let pool_sizes = [vk::DescriptorPoolSize {
        ty: vk::DescriptorType::STORAGE_BUFFER,
        descriptor_count: (4 * MAX_FRAMES_IN_FLIGHT * POOL_CHUNK) as u32,
    }];
    let info = vk::DescriptorPoolCreateInfo::default()
        .pool_sizes(&pool_sizes)
        .max_sets((MAX_FRAMES_IN_FLIGHT * POOL_CHUNK) as u32);
    Ok(unsafe { device.create_descriptor_pool(&info, None)? })
}

fn descriptor_binding(
    binding: u32,
    ty: vk::DescriptorType,
) -> vk::DescriptorSetLayoutBinding<'static> {
    vk::DescriptorSetLayoutBinding::default()
        .binding(binding)
        .descriptor_type(ty)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::COMPUTE)
}

fn build_compute_pipeline(
    device: &ash::Device,
    shader_dir: &Path,
    pipeline_layout: vk::PipelineLayout,
) -> Result<vk::Pipeline> {
    let comp_path = shader_dir.join("skin.comp");
    let source = std::fs::read_to_string(&comp_path)
        .with_context(|| format!("read {}", comp_path.display()))?;
    let spv = hot_reload::compile_glsl(&source, "skin.comp", shaderc::ShaderKind::Compute)?;
    let module = pipeline::create_shader_module(device, &spv)?;
    let entry = std::ffi::CString::new("main").unwrap();
    let stage = vk::PipelineShaderStageCreateInfo::default()
        .stage(vk::ShaderStageFlags::COMPUTE)
        .module(module)
        .name(&entry);
    let info = vk::ComputePipelineCreateInfo::default()
        .stage(stage)
        .layout(pipeline_layout);
    let pipeline = unsafe {
        device
            .create_compute_pipelines(vk::PipelineCache::null(), std::slice::from_ref(&info), None)
            .map_err(|(_, e)| anyhow::anyhow!("create_compute_pipelines: {:?}", e))?[0]
    };
    unsafe { device.destroy_shader_module(module, None) };
    Ok(pipeline)
}

fn write_set(
    device: &ash::Device,
    set: vk::DescriptorSet,
    rest: &GpuBuffer,
    skin: &GpuBuffer,
    palette: &GpuBuffer,
    out: &GpuBuffer,
) {
    let infos = [
        vk::DescriptorBufferInfo {
            buffer: rest.buffer,
            offset: 0,
            range: rest.size,
        },
        vk::DescriptorBufferInfo {
            buffer: skin.buffer,
            offset: 0,
            range: skin.size,
        },
        vk::DescriptorBufferInfo {
            buffer: palette.buffer,
            offset: 0,
            range: PALETTE_BYTES.min(palette.size),
        },
        vk::DescriptorBufferInfo {
            buffer: out.buffer,
            offset: 0,
            range: out.size,
        },
    ];
    let writes = [
        write(set, 0, &infos[0]),
        write(set, 1, &infos[1]),
        write(set, 2, &infos[2]),
        write(set, 3, &infos[3]),
    ];
    unsafe { device.update_descriptor_sets(&writes, &[]) };
}

fn write<'a>(
    set: vk::DescriptorSet,
    binding: u32,
    info: &'a vk::DescriptorBufferInfo,
) -> vk::WriteDescriptorSet<'a> {
    vk::WriteDescriptorSet::default()
        .dst_set(set)
        .dst_binding(binding)
        .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
        .buffer_info(std::slice::from_ref(info))
}

/// One-shot copy that fills the output VB with the rest pose, so a
/// freshly registered skinned mesh has valid vertices to draw on
/// frame 0 (before its first compute dispatch has run).
fn prime_output_vb(
    device: &ash::Device,
    allocator: &Arc<Mutex<Allocator>>,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    output_vb: &GpuBuffer,
    rest_vertices: &[Vertex],
) -> Result<()> {
    let mut staging = buffer::create_host_buffer(
        device,
        allocator,
        rest_vertices,
        vk::BufferUsageFlags::TRANSFER_SRC,
        "skinned_output_prime_staging",
    )?;
    let alloc_info = vk::CommandBufferAllocateInfo::default()
        .command_pool(command_pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    let cmd = unsafe { device.allocate_command_buffers(&alloc_info)?[0] };
    let begin =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    unsafe {
        device.begin_command_buffer(cmd, &begin)?;
        device.cmd_copy_buffer(
            cmd,
            staging.buffer,
            output_vb.buffer,
            &[vk::BufferCopy {
                src_offset: 0,
                dst_offset: 0,
                size: staging.size,
            }],
        );
        device.end_command_buffer(cmd)?;
        let submit = vk::SubmitInfo::default().command_buffers(std::slice::from_ref(&cmd));
        device.queue_submit(queue, &[submit], vk::Fence::null())?;
        device.queue_wait_idle(queue)?;
        device.free_command_buffers(command_pool, &[cmd]);
    }
    staging.cleanup(device, allocator);
    Ok(())
}
