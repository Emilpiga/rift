//! Per-floor blood field.
//!
//! A single `R16G16_SFLOAT` 2-D texture mapped across the floor's
//! world-space XZ extent. The forward fragment shader samples it
//! while shading floor / wall fragments to composite wet-and-drying
//! blood as a real material substitution (PBR roughness + albedo +
//! a mild bevel from the field's gradient), so torchlight glints
//! off fresh puddles and old ones go matte without any per-decal
//! geometry.
//!
//! Layout per texel:
//!   - `R` = wet intensity in `[0, 1]`. Zero when the floor is
//!     untouched. Splats write the splat's intensity multiplied by
//!     the silhouette mask.
//!   - `G` = spawn time in seconds (renderer clock). The shader
//!     computes `age = u_time - G` and drives the wet→dry tween
//!     from that, so old pools fade out without a CPU-side decay
//!     pass.
//!
//! Splats are queued on the CPU (`queue_splat`), uploaded into a
//! ring of per-frame instance buffers (one per frame in flight so
//! we never overwrite data the GPU is still reading), and rendered
//! as instanced quads against a small mask atlas using a dedicated
//! pipeline that runs between the shadow passes and the main
//! forward pass.
//!
//! The default placeholder bound by the renderer at startup is a
//! 1×1 zero texture, so until a floor is built the forward shader
//! sees `(0, 0)` everywhere and skips the blood composite.

use anyhow::Result;
use ash::vk;
use glam::{Vec2, Vec3, Vec4};
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};
use gpu_allocator::MemoryLocation;
use std::path::Path;
use std::sync::{Arc, Mutex};

use rift_math::physics::Aabb;

use crate::hot_reload;
use crate::renderer::texture::Texture;
use crate::vulkan::buffer::GpuBuffer;
use crate::vulkan::pipeline as pipe;
use crate::vulkan::sync::MAX_FRAMES_IN_FLIGHT;

/// Side length of the blood field in texels.
pub const FIELD_RESOLUTION: u32 = 1024;
/// Storage format. Floats give us a clean `(wet, time)` pair without
/// needing to encode time as a UNORM ramp.
pub const FIELD_FORMAT: vk::Format = vk::Format::R16G16_SFLOAT;

/// Procgen mask atlas — 4 silhouettes packed in a 2×2 grid. The shader
/// picks a slice via the per-instance `atlas_slice` attribute.
pub const MASK_RESOLUTION: u32 = 512;
pub const MASK_SLICE_COUNT: u32 = 4;

/// Maximum splats per frame. Plenty for a kill burst (~20 splats per
/// kill) plus several queued kills overlapping; sized so the instance
/// buffer is small (`MAX_INSTANCES * 32 B` ≈ 8 KiB).
pub const MAX_INSTANCES: usize = 256;

/// Per-instance vertex attributes. Mirrored in `blood_splat.vert`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SplatInstance {
    /// xy = uv center in `[0, 1]`, z = spawn time (seconds), w = wet
    /// intensity at spawn `[0, 1]`.
    pub center_time_intensity: [f32; 4],
    /// x = uv half-size along major axis, y = aspect (minor / major),
    /// z = rotation in radians (XZ orientation), w = atlas slice (cast
    /// to int via `floor(.+0.5)` in the shader).
    pub size_rot_slice: [f32; 4],
}

/// Description of a kill, used to drive the layered splat emission.
/// Mirrors the previous decal-system `KillContext` exactly so the
/// callsite in `main.rs` doesn't need to change shape.
#[derive(Clone, Copy, Debug)]
pub struct KillContext {
    /// World-space position the body fell at. Y is used for the
    /// "is the body at floor level" check; XZ drives splat placement.
    pub pos: Vec3,
    /// Horizontal projection of the killing-blow / body-impulse
    /// direction. Length is treated as a hint of throw strength —
    /// near-zero falls back to a deterministic per-position angle
    /// so stationary kills still get asymmetric character.
    pub dir: Vec3,
    /// 0..=1 scalar describing how violent the kill was. Boss
    /// finishers approach 1.0; trash kills sit around 0.2-0.4.
    /// Drives pool size, droplet count, and throw distance.
    pub power: f32,
}

impl KillContext {
    /// Convenience constructor for callers that don't track a
    /// direction. Picks a stable per-position pseudo-random angle
    /// so the resulting pool still has asymmetry.
    pub fn isotropic(pos: Vec3, power: f32) -> Self {
        Self {
            pos,
            dir: Vec3::ZERO,
            power,
        }
    }
}

pub struct BloodField {
    // ---- Field render target ----
    pub field_image: vk::Image,
    pub field_view: vk::ImageView,
    pub field_sampler: vk::Sampler,
    field_allocation: Option<Allocation>,

    // ---- Splat render pass + framebuffer ----
    render_pass: vk::RenderPass,
    framebuffer: vk::Framebuffer,
    needs_clear: bool,

    // ---- Splat pipeline ----
    splat_pipeline: vk::Pipeline,
    splat_pipeline_layout: vk::PipelineLayout,

    // ---- Mask atlas (sampled inside the splat fragment) ----
    mask: Texture,
    mask_set_layout: vk::DescriptorSetLayout,
    mask_pool: vk::DescriptorPool,
    mask_set: vk::DescriptorSet,

    // ---- Per-frame streaming instance buffers ----
    instance_buffers: Vec<GpuBuffer>,
    /// Splats queued by gameplay this frame, drained into the next
    /// frame's instance buffer at record time.
    pending: Vec<SplatInstance>,
    /// Number of instances actually uploaded for the most recent
    /// frame. Read by `record_splat_pass`.
    pub frame_instance_counts: [u32; MAX_FRAMES_IN_FLIGHT],

    // ---- World-to-UV transform ----
    /// Set when a floor binds a real field. `Vec4::ZERO` means no
    /// active field; the forward shader treats this as a no-op.
    pub world_xform: Vec4,
    /// World-Y of the lowest floor plane bound by the active
    /// field. Combined with [`Self::floor_y_max`] gives the
    /// fragment-acceptance band the forward shader uses to
    /// reject wall-top samples that share an XZ projection
    /// with a splat below. A single-elevation level has
    /// `floor_y_min == floor_y_max`.
    pub floor_y: f32,
    /// World-Y of the highest floor plane bound by the
    /// active field. See [`Self::floor_y`].
    pub floor_y_max: f32,
    /// `true` once a floor has been bound. Drives whether the splat
    /// pass runs at all.
    pub active: bool,

    /// xorshift32 state used by `splat_for_kill` to jitter sub-splat
    /// placement, rotation, and atlas-slice selection.
    rng: u32,

    // ---- Footprint tracking ----
    /// Recent kill XZ + soak radius + spawn time. The spawn time
    /// is what footprints picked up from this stain inherit, so
    /// stepping through old (dried) blood lays down dried-looking
    /// footprints rather than bright fresh ones.
    recent_kills: Vec<(Vec2, f32, f32)>,
    /// Per-player footprint trackers, keyed by an opaque token
    /// chosen by the caller (typically `0` for the local player).
    trackers: Vec<(u32, FootprintTracker)>,
}

/// Per-player tracking used to lay footprint splats. Distance
/// since last print accumulates as the player walks; once it
/// crosses a threshold, a footprint splat is stamped at the foot
/// position (alternating L/R) and `charge` is drained. Charge
/// refills whenever the foot enters a recent kill stain.
struct FootprintTracker {
    last_pos: Vec3,
    step_accum: f32,
    charge: f32,
    next_foot_left: bool,
    /// Spawn time of the most recent kill stain the foot was
    /// inside. Footprint splats inherit this so prints stamped
    /// from old blood read as old (dried) rather than fresh.
    /// Zero / negative means "unsoaked".
    soak_source_time: f32,
}

impl FootprintTracker {
    fn new(pos: Vec3) -> Self {
        Self {
            last_pos: pos,
            step_accum: 0.0,
            charge: 0.0,
            next_foot_left: true,
            soak_source_time: 0.0,
        }
    }
}

/// Distance the player must walk between footprints — roughly one
/// human stride. Anything shorter and prints overlap into a smear;
/// anything longer reads as skipped frames.
const FOOTPRINT_STEP_DISTANCE: f32 = 0.55;
/// Charge a single fresh kill-stain refill grants. Bounded to 1.0
/// in the tracker so multiple refills don't extend the print trail
/// indefinitely.
const FOOTPRINT_CHARGE_PER_SOAK: f32 = 1.0;
/// Charge consumed per footprint stamp. With 0.04 the player gets
/// ~25 prints from a fully-soaked boot, which feels like "you
/// definitely tracked that" without painting the whole dungeon red.
const FOOTPRINT_CHARGE_PER_STEP: f32 = 0.04;
/// Max recent kills tracked for charge-refill checks. Older entries
/// fall off the front of the ring as new ones arrive.
const RECENT_KILLS_CAP: usize = 32;

impl BloodField {
    pub fn new(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        shader_dir: &Path,
    ) -> Result<Self> {
        // ---- Field image (R16G16 float, color attachment + sampled) ----
        let img_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(FIELD_FORMAT)
            .extent(vk::Extent3D {
                width: FIELD_RESOLUTION,
                height: FIELD_RESOLUTION,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(
                vk::ImageUsageFlags::COLOR_ATTACHMENT
                    | vk::ImageUsageFlags::SAMPLED
                    // Required for the per-floor `cmd_clear_color_image`
                    // path that wipes the field on level transitions.
                    | vk::ImageUsageFlags::TRANSFER_DST,
            )
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let field_image = unsafe { device.create_image(&img_info, None)? };
        let reqs = unsafe { device.get_image_memory_requirements(field_image) };
        let field_allocation = allocator.lock().unwrap().allocate(&AllocationCreateDesc {
            name: "blood_field",
            requirements: reqs,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })?;
        unsafe {
            device.bind_image_memory(
                field_image,
                field_allocation.memory(),
                field_allocation.offset(),
            )?;
        }

        let view_info = vk::ImageViewCreateInfo::default()
            .image(field_image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(FIELD_FORMAT)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        let field_view = unsafe { device.create_image_view(&view_info, None)? };

        let sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .anisotropy_enable(false)
            .max_lod(1.0);
        let field_sampler = unsafe { device.create_sampler(&sampler_info, None)? };

        // ---- Render pass: single R16G16 color attachment, no depth.
        // First time we render into the field we clear; on subsequent
        // frames we LOAD so accumulated splats survive. The
        // `needs_clear` bool toggles which attachment description
        // we use; both share the same render pass via two separate
        // passes would double the API surface, so we instead keep
        // a single LOAD pass and clear via cmd_clear_color_image
        // when we need to wipe. Simpler.
        let attach = vk::AttachmentDescription::default()
            .format(FIELD_FORMAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::LOAD)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            // We always keep the field in SHADER_READ between frames
            // (the forward pass samples it), and the layout is
            // restored to that on subpass exit so subsequent frames
            // can begin with `LOAD` from `SHADER_READ_ONLY_OPTIMAL`
            // → `COLOR_ATTACHMENT_OPTIMAL` via the subpass dep.
            .initial_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .final_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        let color_ref = vk::AttachmentReference {
            attachment: 0,
            layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        };
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(std::slice::from_ref(&color_ref));
        let deps = [
            vk::SubpassDependency::default()
                .src_subpass(vk::SUBPASS_EXTERNAL)
                .dst_subpass(0)
                .src_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
                .src_access_mask(vk::AccessFlags::SHADER_READ)
                .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                .dst_access_mask(
                    vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                        | vk::AccessFlags::COLOR_ATTACHMENT_READ,
                ),
            vk::SubpassDependency::default()
                .src_subpass(0)
                .dst_subpass(vk::SUBPASS_EXTERNAL)
                .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
                .dst_access_mask(vk::AccessFlags::SHADER_READ),
        ];
        let rp_info = vk::RenderPassCreateInfo::default()
            .attachments(std::slice::from_ref(&attach))
            .subpasses(std::slice::from_ref(&subpass))
            .dependencies(&deps);
        let render_pass = unsafe { device.create_render_pass(&rp_info, None)? };

        // ---- Framebuffer ----
        let attach_views = [field_view];
        let fb_info = vk::FramebufferCreateInfo::default()
            .render_pass(render_pass)
            .attachments(&attach_views)
            .width(FIELD_RESOLUTION)
            .height(FIELD_RESOLUTION)
            .layers(1);
        let framebuffer = unsafe { device.create_framebuffer(&fb_info, None)? };

        // ---- Mask atlas ----
        let mask_pixels = generate_mask_atlas();
        let mask = Texture::from_r8(
            device,
            allocator,
            queue,
            command_pool,
            MASK_RESOLUTION,
            MASK_RESOLUTION,
            &mask_pixels,
        )?;

        // Mask descriptor set (set 0 of the splat pipeline).
        let mask_binding = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
        let mask_layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&mask_binding);
        let mask_set_layout =
            unsafe { device.create_descriptor_set_layout(&mask_layout_info, None)? };
        let mask_pool_size = [vk::DescriptorPoolSize {
            ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
            descriptor_count: 1,
        }];
        let mask_pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(1)
            .pool_sizes(&mask_pool_size);
        let mask_pool = unsafe { device.create_descriptor_pool(&mask_pool_info, None)? };
        let alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(mask_pool)
            .set_layouts(std::slice::from_ref(&mask_set_layout));
        let mask_set = unsafe { device.allocate_descriptor_sets(&alloc_info)?[0] };
        let img_info = vk::DescriptorImageInfo {
            sampler: mask.sampler,
            image_view: mask.view,
            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        };
        let write = vk::WriteDescriptorSet::default()
            .dst_set(mask_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(std::slice::from_ref(&img_info));
        unsafe { device.update_descriptor_sets(&[write], &[]) };

        // ---- Splat pipeline ----
        let (splat_pipeline, splat_pipeline_layout) =
            create_splat_pipeline(device, render_pass, mask_set_layout, shader_dir)?;

        // ---- Per-frame instance buffers (CpuToGpu, persistently mapped) ----
        let mut instance_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            let buf = GpuBuffer::new(
                device,
                allocator,
                (MAX_INSTANCES * std::mem::size_of::<SplatInstance>()) as vk::DeviceSize,
                vk::BufferUsageFlags::VERTEX_BUFFER,
                MemoryLocation::CpuToGpu,
                &format!("blood_splat_instances_{}", i),
            )?;
            instance_buffers.push(buf);
        }

        // ---- Initial layout transition: UNDEFINED → SHADER_READ_ONLY ----
        // The render pass expects `initial_layout =
        // SHADER_READ_ONLY_OPTIMAL`, so we have to bring the image
        // there once at creation. We also need it in that layout for
        // the forward shader's first sample if no splats happen
        // before the first composite.
        transition_to_shader_read(device, queue, command_pool, field_image)?;

        Ok(Self {
            field_image,
            field_view,
            field_sampler,
            field_allocation: Some(field_allocation),
            render_pass,
            framebuffer,
            needs_clear: true,
            splat_pipeline,
            splat_pipeline_layout,
            mask,
            mask_set_layout,
            mask_pool,
            mask_set,
            instance_buffers,
            pending: Vec::with_capacity(MAX_INSTANCES),
            frame_instance_counts: [0; MAX_FRAMES_IN_FLIGHT],
            world_xform: Vec4::ZERO,
            floor_y: 0.0,
            floor_y_max: 0.0,
            active: false,
            rng: 0xC0FF_EE13,
            recent_kills: Vec::with_capacity(RECENT_KILLS_CAP),
            trackers: Vec::new(),
        })
    }

    /// Bind a new floor's world AABB. `min` / `max` are the floor's
    /// XZ extents in world space; `floor_y_min` and `floor_y_max`
    /// bracket the elevation range of the playable floor surface
    /// (lowest pit ↔ highest dais). The forward shader uses the
    /// band `[floor_y_min - eps, floor_y_max + eps]` to decide which
    /// fragments are eligible to sample the blood field, so a
    /// rift floor with raised platforms / lowered pits still
    /// accepts splats on every walkable surface. For a flat floor
    /// pass the same value for both. Wipes any pending splats and
    /// marks the field for clear on the next splat pass. Caller is
    /// responsible for re-binding the field view to set 0 /
    /// binding 4 of the main descriptor sets via
    /// `UniformBuffers::bind_blood_field`.
    pub fn bind_floor(
        &mut self,
        min_xz: glam::Vec2,
        max_xz: glam::Vec2,
        floor_y_min: f32,
        floor_y_max: f32,
    ) {
        let extent = max_xz - min_xz;
        // Pad slightly so splats near the edge of the floor don't
        // wrap or clip — half a metre at each side.
        let pad = glam::Vec2::splat(0.5);
        let origin = min_xz - pad;
        let size = extent + pad * 2.0;
        let inv_size = glam::Vec2::new(
            if size.x > 1e-3 { 1.0 / size.x } else { 0.0 },
            if size.y > 1e-3 { 1.0 / size.y } else { 0.0 },
        );
        self.world_xform = Vec4::new(origin.x, origin.y, inv_size.x, inv_size.y);
        self.floor_y = floor_y_min;
        self.floor_y_max = floor_y_max;
        self.active = true;
        self.needs_clear = true;
        self.pending.clear();
        // New floor — stains from the previous floor are stale.
        // Trackers keep their charge so a player walking out of one
        // floor with bloody boots still leaves prints in the next.
        self.recent_kills.clear();
    }

    /// Disable the field (e.g. on entering the hub). The forward
    /// shader will see zeros until a new floor binds a field.
    pub fn unbind(&mut self) {
        self.world_xform = Vec4::ZERO;
        self.floor_y = 0.0;
        self.floor_y_max = 0.0;
        self.active = false;
        self.pending.clear();
        self.recent_kills.clear();
    }

    /// Convert a world-space XZ point into field UV space. Returns
    /// `None` if no floor is bound or the point falls outside the
    /// padded extent.
    pub fn world_to_uv(&self, world_xz: glam::Vec2) -> Option<glam::Vec2> {
        if !self.active {
            return None;
        }
        let origin = glam::Vec2::new(self.world_xform.x, self.world_xform.y);
        let inv = glam::Vec2::new(self.world_xform.z, self.world_xform.w);
        let uv = (world_xz - origin) * inv;
        if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 {
            return None;
        }
        Some(uv)
    }

    /// Inverse of `world_to_uv` — returns the UV-space size of a
    /// world-space size in metres (along whichever axis is largest).
    pub fn world_size_to_uv(&self, world_size_m: f32) -> f32 {
        // Use the larger of the two inv components so the UV size
        // stays correct on non-square floors.
        let inv = self.world_xform.z.max(self.world_xform.w);
        world_size_m * inv
    }

    /// Queue a splat for the next frame. Silently dropped if the
    /// field isn't bound or the queue is full.
    pub fn queue_splat(&mut self, instance: SplatInstance) {
        if !self.active || self.pending.len() >= MAX_INSTANCES {
            return;
        }
        self.pending.push(instance);
    }

    /// Emit the layered splat pattern for a kill.
    ///
    /// Layers (all written into the floor blood field — wall blood
    /// will land in a follow-up vertical-field pass):
    ///
    /// 1. **Corpse pool** — one large oblong splat at `ctx.pos`,
    ///    back-offset slightly along `-dir` so the body settles into
    ///    the pool. Aspect 1.45×, atlas slice 2 (long-drip pool).
    ///    Size scales with `power`.
    /// 2. **Forward spray fan** — 2-3 medium splats clustered in a
    ///    forward cone (lateral ±0.3 m, forward 0.4-1.1 m), each
    ///    elongated along the impact axis. Atlas slice 1 (dense
    ///    splatter).
    /// 3. **Scatter droplets** — 6-12+ tiny round splats in a wide
    ///    cone (~±70°), distributed 0.6-2.4 m out. Atlas slice 0
    ///    (small spray) or 3 (round splat) at random.
    ///
    /// `time_secs` is the renderer's current elapsed time. Caller
    /// should pass `renderer.elapsed_secs()`. The MAX-blend ensures
    /// later splats overwrite older `G` values for any overlapping
    /// texel, so a fresh kill on top of an old pool re-wets the
    /// region.
    pub fn splat_for_kill(&mut self, ctx: KillContext, time_secs: f32, wall_aabbs: &[Aabb]) {
        if !self.active {
            return;
        }

        // Resolve direction. Project to XZ; fall back to a stable
        // per-position hashed angle if the velocity was negligible.
        let dir_xz = Vec2::new(ctx.dir.x, ctx.dir.z);
        let (dir, fallback) = if dir_xz.length_squared() > 1e-4 {
            (dir_xz.normalize(), false)
        } else {
            // Hash position bits → angle. Stable for any given
            // position so two stationary kills at different points
            // produce different signatures, but the same kill always
            // looks the same.
            let bits = ctx.pos.x.to_bits() ^ ctx.pos.z.to_bits().rotate_left(13);
            let theta = (bits as f32 / u32::MAX as f32).fract() * std::f32::consts::TAU;
            (Vec2::new(theta.cos(), theta.sin()), true)
        };
        let _ = fallback;
        let angle = dir.y.atan2(dir.x); // atan2(z, x) since dir = (x, z)
        let perp = Vec2::new(-dir.y, dir.x);
        let power = ctx.power.clamp(0.0, 1.0);
        let pos_xz = Vec2::new(ctx.pos.x, ctx.pos.z);

        // ---- Layer 1: Corpse pool (~70 % visual weight) ----
        // Stamped twice (slightly offset + co-aligned) so the centre
        // saturates the field with overlapping coverage; visually this
        // reads as a thick puddle rather than a single faint disc.
        // The pool now sits slightly *forward* of impact for any kill
        // with real velocity — the body carries momentum, so the
        // pool follows the impulse rather than landing dead-centre.
        // For zero-velocity (hashed-direction) kills it stays behind
        // the impact point so the kill still has an asymmetry tied
        // to its hashed angle.
        let pool_size_m;
        let pool_center;
        {
            // For real kill velocity, push the pool forward of
            // impact by 30–60 cm scaled with power; for hashed
            // fallback, keep the legacy back-offset.
            let along_offset = if dir_xz.length_squared() > 1e-4 {
                0.30 + 0.30 * power
            } else {
                -(0.05 + 0.13 * power)
            };
            pool_center = pos_xz + dir * along_offset;
            pool_size_m = 0.95 + 0.55 * power;
            let intensity = 1.0;
            // Stronger forward stretch — pool reads as oblong slick
            // pointing along impulse, not a circle.
            let pool_aspect = 1.65 + 0.45 * power;
            let pool_jitter = self.signed_jitter(0.12);
            self.emit_at(
                pool_center,
                pool_size_m,
                pool_aspect,
                angle + pool_jitter,
                2,
                intensity,
                time_secs,
            );
            // Inner core stamp — smaller, fully saturated, slot 1.
            // Offset slightly forward inside the pool so the bright
            // centre is at the leading edge of the slick.
            let core_jit_x = self.signed_jitter(0.05);
            let core_jit_z = self.signed_jitter(0.05);
            let core_rot_jit = self.signed_jitter(0.20);
            self.emit_at(
                pool_center + dir * 0.10 + Vec2::new(core_jit_x, core_jit_z),
                pool_size_m * 0.62,
                1.30,
                angle + core_rot_jit,
                1,
                1.0,
                time_secs,
            );
        }
        // Register the pool as a soak stain so any player walking
        // through it will pick up a footprint-charge refill.
        self.push_kill_stain(pool_center, pool_size_m * 0.7, time_secs);

        // ---- Layer 2: Forward spray (~20 % visual weight) ----
        // Tight forward cone, longer reach. Each splat is elongated
        // along the impulse axis and stamped with a small forward
        // stagger so the spray reads as motion blur — leading edge
        // pushes furthest, trailing splats sit nearer the body.
        let fan_count = 3 + (power * 3.0) as i32;
        for i in 0..fan_count {
            // Distance scales hard with power: a snipe / fireball
            // throws blood metres forward; a melee tap drops it
            // half a metre out.
            let forward_dist = 0.55 + self.rand01() * (1.40 + 1.20 * power);
            // Tight ~±15° lateral cone (was ±25°+).
            let lateral = (self.rand01() * 2.0 - 1.0) * 0.30;
            let center = pos_xz + dir * forward_dist + perp * lateral;
            // Bigger leading splats — the elongated spray needs to
            // read at a glance, not as a sea of speckle.
            let size_m = 0.36 + self.rand01() * 0.26 + 0.20 * power;
            let intensity = 0.90 + 0.10 * self.rand01();
            // Stretched along the impulse axis.
            let aspect = 1.40 + self.rand01() * 0.40;
            let rot_jit = self.signed_jitter(0.25);
            // Stagger gives a motion-blur read.
            let stagger = 0.04 + i as f32 * 0.05;
            self.emit_at(
                center,
                size_m,
                aspect,
                angle + rot_jit,
                1,
                intensity,
                time_secs + stagger,
            );
        }

        // ---- Layer 3: Distant droplets (~10 % visual weight) ----
        // Strongly forward-biased: 80 % of droplets land in a tight
        // ±25° forward cone, the remaining 20 % scatter wider for
        // chaos. Power lengthens throw distance — a fireball kill
        // throws droplets 4 m+, a knife strike a metre or so.
        let drop_count = 6 + (self.rand01() * 4.0) as i32 + (5.0 * power) as i32;
        for _ in 0..drop_count {
            // 80 % ±25° forward cone, 20 % wider ±60°.
            let tight = self.rand01() < 0.80;
            let cone_amp = if tight { 0.44 } else { 1.05 };
            let cone_t = self.rand01() * 2.0 - 1.0;
            // cone_t squared keeps mass near the axis.
            let cone_angle = cone_t * cone_t * cone_amp * cone_t.signum();
            let throw_mul = if tight { 1.0 } else { 0.6 };
            let dist = (0.85 + self.rand01() * 2.60 * (1.0 + 1.0 * power)) * throw_mul;
            let theta = angle + cone_angle;
            let dropdir = Vec2::new(theta.cos(), theta.sin());
            let center = pos_xz + dropdir * dist;
            let size_m = 0.10 + self.rand01() * 0.14;
            let intensity = 0.70 + 0.20 * self.rand01();
            // Mostly small-spray atlas slices (0 or 3); occasionally
            // 1 for variety.
            let slice_pick = self.rand01();
            let slice_alt = self.rand01();
            let slice = if slice_pick < 0.7 {
                if slice_alt < 0.5 {
                    0
                } else {
                    3
                }
            } else {
                1
            };
            // Forward droplets are slightly elongated along impulse;
            // wide-cone scatter stays roughly round.
            let aspect = if tight {
                1.05 + self.rand01() * 0.35
            } else {
                0.85 + self.rand01() * 0.35
            };
            // Forward droplets orient along impulse; scatter rotate
            // freely.
            let rot = if tight {
                angle + self.signed_jitter(0.45)
            } else {
                self.rand01() * std::f32::consts::TAU
            };
            self.emit_at(center, size_m, aspect, rot, slice, intensity, time_secs);
        }

        // ---- Layer 4: Wall arcs ----
        // Cast a tight forward fan of rays against wall AABBs
        // (projected onto XZ). The cone is narrow (~±20°) and
        // weighted toward the centre line so wall hits land where
        // the impulse actually pointed, not scattered around the
        // body.
        let wall_ray_count = 5;
        let wall_max_dist = 3.5 + 2.0 * power;
        for i in 0..wall_ray_count {
            let t =
                (i as f32 - (wall_ray_count - 1) as f32 * 0.5) / (wall_ray_count - 1) as f32 * 2.0;
            // Cubic taper: ray indices near the centre stay close to
            // angle 0; outer rays fan out only mildly. Range ~±20°.
            let cone_a = (t * t * t.signum()) * 0.35;
            let theta = angle + cone_a;
            let rd = Vec2::new(theta.cos(), theta.sin());
            let Some((hit_xz, _hit_dist)) =
                ray_first_aabb_xz(pos_xz, rd, wall_max_dist, wall_aabbs)
            else {
                continue;
            };
            // Wall normal in XZ — perpendicular to the ray hit
            // edge, opposite to ray direction. We don't currently
            // know which AABB face was hit precisely, so use ray
            // direction as a proxy; tangent along the wall is
            // perpendicular to the spray.
            let wall_tan = Vec2::new(-rd.y, rd.x);
            // Big main impact stamp.
            let main_size = 0.55 + 0.40 * power;
            let main_jit = self.signed_jitter(0.25);
            let core_jit = self.signed_jitter(0.40);
            self.emit_at(hit_xz, main_size, 1.30, theta + main_jit, 2, 1.0, time_secs);
            self.emit_at(
                hit_xz,
                main_size * 0.55,
                1.10,
                theta + core_jit,
                1,
                1.0,
                time_secs,
            );
            // Lateral satellites along the wall to imply spread.
            let sat_rot_a = self.rand01() * std::f32::consts::TAU;
            let sat_rot_b = self.rand01() * std::f32::consts::TAU;
            self.emit_at(
                hit_xz + wall_tan * (0.35 + 0.15 * power),
                main_size * 0.40,
                0.95,
                sat_rot_a,
                3,
                0.85,
                time_secs + 0.02,
            );
            self.emit_at(
                hit_xz - wall_tan * (0.35 + 0.15 * power),
                main_size * 0.40,
                0.95,
                sat_rot_b,
                3,
                0.85,
                time_secs + 0.04,
            );
        }
    }

    // -------------------------------------------------------------------
    // Footprint trail
    // -------------------------------------------------------------------
    //
    // Footprints are stamped as small directional splats into the same
    // accumulation field. Because they share the field they automatically
    // inherit the wet→dry curve, the tile-aware composite, and the
    // ragged-edge mask shader — no separate decal system needed.

    /// Re-anchor a tracker without emitting any prints. Call when the
    /// player teleports / dodge-rolls / jumps so the next grounded
    /// frame doesn't see the airborne delta as one giant step.
    /// Charge is preserved.
    pub fn reset_step_tracker(&mut self, token: u32, pos: Vec3) {
        let idx = match self.trackers.iter().position(|(t, _)| *t == token) {
            Some(i) => i,
            None => {
                self.trackers.push((token, FootprintTracker::new(pos)));
                return;
            }
        };
        let t = &mut self.trackers[idx].1;
        t.last_pos = pos;
        t.step_accum = 0.0;
    }

    /// Per-frame hook for the local player's foot position. The
    /// tracker derives a movement-aligned facing from the position
    /// delta, refills boot-blood charge whenever the foot enters a
    /// recent kill stain, and stamps a footprint splat every
    /// [`FOOTPRINT_STEP_DISTANCE`] metres of travel until charge runs
    /// out. No-op when the field is unbound.
    pub fn track_player_step(&mut self, token: u32, pos: Vec3, time_secs: f32) {
        if !self.active {
            return;
        }
        // Locate or create a tracker for this player.
        let idx = match self.trackers.iter().position(|(t, _)| *t == token) {
            Some(i) => i,
            None => {
                self.trackers.push((token, FootprintTracker::new(pos)));
                self.trackers.len() - 1
            }
        };

        // Snapshot frame delta + facing.
        let facing = {
            let t = &mut self.trackers[idx].1;
            // First-frame guard / teleport guard.
            if t.last_pos == Vec3::ZERO || (pos - t.last_pos).length() > 4.0 {
                t.last_pos = pos;
                t.step_accum = 0.0;
                return;
            }
            let delta = pos - t.last_pos;
            let dxz = Vec3::new(delta.x, 0.0, delta.z);
            let d = dxz.length();
            t.step_accum += d;
            t.last_pos = pos;
            if d > 1e-3 {
                dxz / d
            } else {
                Vec3::Z
            }
        };

        // Soak refill: if the foot is inside any recent kill stain,
        // top up the boot charge AND record the source's spawn time
        // so the footprints stamped from this soak inherit the
        // source's age. Walking through old, dried blood lays down
        // dried-looking prints rather than bright fresh ones.
        let foot_xz = Vec2::new(pos.x, pos.z);
        let mut soak_time: Option<f32> = None;
        for (s_pos, s_radius, s_time) in &self.recent_kills {
            if (foot_xz - *s_pos).length_squared() <= s_radius * s_radius {
                // Pick the freshest source under the foot — if a
                // fresh kill overlaps an old one, the new blood is
                // sitting on top so the boot picks up that tone.
                soak_time = Some(soak_time.map_or(*s_time, |t| t.max(*s_time)));
            }
        }
        if let Some(src_time) = soak_time {
            let t = &mut self.trackers[idx].1;
            t.charge = (t.charge + FOOTPRINT_CHARGE_PER_SOAK).min(1.0);
            t.soak_source_time = src_time;
        }

        // Step trigger: stamp a print every FOOTPRINT_STEP_DISTANCE
        // metres of accumulated travel while charge > 0.
        loop {
            let (foot_left, charge_after, src_time) = {
                let t = &mut self.trackers[idx].1;
                if t.step_accum < FOOTPRINT_STEP_DISTANCE || t.charge <= 0.0 {
                    break;
                }
                t.step_accum -= FOOTPRINT_STEP_DISTANCE;
                let foot_left = t.next_foot_left;
                t.next_foot_left = !t.next_foot_left;
                t.charge = (t.charge - FOOTPRINT_CHARGE_PER_STEP).max(0.0);
                (foot_left, t.charge, t.soak_source_time)
            };
            // Stamp the print at the source blood's spawn time so
            // it reads as the same age as whatever the foot was
            // last in. Falls back to "now" if we never soaked
            // (shouldn't happen in practice — charge requires a
            // soak — but keeps the system robust if a future
            // path grants charge differently).
            let stamp_time = if src_time > 0.0 { src_time } else { time_secs };
            self.emit_footprint(pos, facing, foot_left, charge_after, stamp_time);
        }
    }

    /// Stamp a single footprint as two small directional splats —
    /// a sole patch + a smaller heel patch behind it, laterally
    /// offset by ~10 cm from the player's centreline so prints
    /// alternate left / right around the locomotion axis.
    fn emit_footprint(
        &mut self,
        pos: Vec3,
        facing: Vec3,
        foot_left: bool,
        charge: f32,
        time_secs: f32,
    ) {
        // Project facing to XZ. Fall back to +Z so a stationary
        // hit still produces a sensibly-oriented print.
        let mut fwd = Vec2::new(facing.x, facing.z);
        if fwd.length_squared() < 1e-4 {
            fwd = Vec2::new(0.0, 1.0);
        }
        let fwd = fwd.normalize();
        let right = Vec2::new(fwd.y, -fwd.x);
        let lateral = if foot_left { -0.10 } else { 0.10 };
        let foot_pos = Vec2::new(pos.x, pos.z) + right * lateral;
        let angle = fwd.y.atan2(fwd.x);

        // Boot-charge fade. A near-empty boot leaves only the
        // faintest trace; a full boot stamps a punchy print.
        let intensity = (0.45 + 0.55 * charge).clamp(0.0, 1.0);

        // Sole — slightly forward of foot centre.
        self.emit_at(
            foot_pos + fwd * 0.05,
            0.18,
            1.55,
            angle,
            1,
            intensity,
            time_secs,
        );
        // Heel — smaller, offset back along facing.
        self.emit_at(
            foot_pos - fwd * 0.10,
            0.11,
            1.20,
            angle,
            3,
            intensity * 0.85,
            time_secs,
        );
    }

    /// Push a recent kill XZ + soak radius into the ring used by
    /// the footprint tracker. Bounded; oldest entries fall off the
    /// front.
    fn push_kill_stain(&mut self, center_xz: Vec2, radius: f32, time_secs: f32) {
        if self.recent_kills.len() >= RECENT_KILLS_CAP {
            self.recent_kills.remove(0);
        }
        self.recent_kills.push((center_xz, radius, time_secs));
    }

    /// Convert a world-space splat (centre + size in metres) into a
    /// `SplatInstance` and queue it. Skipped if the centre falls
    /// outside the floor's padded extent.
    fn emit_at(
        &mut self,
        center_xz: Vec2,
        size_m: f32,
        aspect: f32,
        rotation: f32,
        atlas_slice: u32,
        intensity: f32,
        time_secs: f32,
    ) {
        let Some(uv) = self.world_to_uv(center_xz) else {
            return;
        };
        let half_size_uv = self.world_size_to_uv(size_m * 0.5);
        // Keep the recorded aspect within sane bounds; >2.5 produces
        // pencil-streak splats that don't read as blood.
        let aspect = aspect.clamp(0.5, 2.5);
        // A splat smaller than ~2 texels would alias badly; clamp so
        // tiny droplets at least cover a few texels.
        let min_uv = 2.0 / FIELD_RESOLUTION as f32;
        let half_size_uv = half_size_uv.max(min_uv);
        self.queue_splat(SplatInstance {
            center_time_intensity: [uv.x, uv.y, time_secs, intensity.clamp(0.0, 1.0)],
            size_rot_slice: [half_size_uv, aspect, rotation, atlas_slice as f32],
        });
    }

    /// xorshift32 in [0, 1).
    fn rand01(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        // Avoid the 1.0 boundary to keep callers' clamp logic simple.
        ((self.rng >> 8) as f32) / ((1u32 << 24) as f32)
    }

    /// Random value in `[-amp, amp]`.
    fn signed_jitter(&mut self, amp: f32) -> f32 {
        (self.rand01() * 2.0 - 1.0) * amp
    }

    /// Drain the pending queue into this frame's instance buffer
    /// and record the splat pass. No-op when the field isn't bound
    /// or the queue is empty (and no clear is pending).
    pub fn record(&mut self, device: &ash::Device, cmd: vk::CommandBuffer, frame: usize) {
        if !self.active {
            self.pending.clear();
            self.frame_instance_counts[frame] = 0;
            return;
        }

        let instance_count = self.pending.len() as u32;
        if instance_count > 0 {
            self.instance_buffers[frame].write(&self.pending);
        }
        self.frame_instance_counts[frame] = instance_count;

        let needs_anything = self.needs_clear || instance_count > 0;
        if !needs_anything {
            self.pending.clear();
            return;
        }

        unsafe {
            // Optional clear: cmd_clear_color_image. We have to
            // transition out of SHADER_READ → TRANSFER_DST first.
            if self.needs_clear {
                let to_transfer = vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(self.field_image)
                    .subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    })
                    .src_access_mask(vk::AccessFlags::SHADER_READ)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
                device.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::FRAGMENT_SHADER,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[to_transfer],
                );

                let clear = vk::ClearColorValue {
                    float32: [0.0, 0.0, 0.0, 0.0],
                };
                device.cmd_clear_color_image(
                    cmd,
                    self.field_image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &clear,
                    &[vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    }],
                );

                let to_shader = vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(self.field_image)
                    .subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    })
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ);
                device.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::FRAGMENT_SHADER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[to_shader],
                );

                self.needs_clear = false;
            }

            if instance_count > 0 {
                let rp_begin = vk::RenderPassBeginInfo::default()
                    .render_pass(self.render_pass)
                    .framebuffer(self.framebuffer)
                    .render_area(vk::Rect2D {
                        offset: vk::Offset2D { x: 0, y: 0 },
                        extent: vk::Extent2D {
                            width: FIELD_RESOLUTION,
                            height: FIELD_RESOLUTION,
                        },
                    });
                device.cmd_begin_render_pass(cmd, &rp_begin, vk::SubpassContents::INLINE);
                device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.splat_pipeline);
                device.cmd_bind_descriptor_sets(
                    cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.splat_pipeline_layout,
                    0,
                    &[self.mask_set],
                    &[],
                );
                device.cmd_bind_vertex_buffers(
                    cmd,
                    0,
                    &[self.instance_buffers[frame].buffer],
                    &[0],
                );
                // 4 vertices (a triangle strip), `instance_count`
                // instances. The vertex shader builds the unit quad
                // from `gl_VertexIndex`.
                device.cmd_draw(cmd, 4, instance_count, 0, 0);
                device.cmd_end_render_pass(cmd);
            }
        }

        self.pending.clear();
    }

    pub fn cleanup(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        for buf in &mut self.instance_buffers {
            buf.cleanup(device, allocator);
        }
        unsafe {
            device.destroy_pipeline(self.splat_pipeline, None);
            device.destroy_pipeline_layout(self.splat_pipeline_layout, None);
            device.destroy_descriptor_pool(self.mask_pool, None);
            device.destroy_descriptor_set_layout(self.mask_set_layout, None);
            device.destroy_framebuffer(self.framebuffer, None);
            device.destroy_render_pass(self.render_pass, None);
            device.destroy_sampler(self.field_sampler, None);
            device.destroy_image_view(self.field_view, None);
            device.destroy_image(self.field_image, None);
        }
        if let Some(alloc) = self.field_allocation.take() {
            allocator.lock().unwrap().free(alloc).ok();
        }
        self.mask.cleanup(device, allocator);
    }
}

fn transition_to_shader_read(
    device: &ash::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    image: vk::Image,
) -> Result<()> {
    let alloc_info = vk::CommandBufferAllocateInfo::default()
        .command_pool(command_pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    let cmd = unsafe { device.allocate_command_buffers(&alloc_info)?[0] };
    let begin =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    unsafe {
        device.begin_command_buffer(cmd, &begin)?;
        let barrier = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            })
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(vk::AccessFlags::SHADER_READ);
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        );
        device.end_command_buffer(cmd)?;
        let submit = vk::SubmitInfo::default().command_buffers(std::slice::from_ref(&cmd));
        device.queue_submit(queue, &[submit], vk::Fence::null())?;
        device.queue_wait_idle(queue)?;
        device.free_command_buffers(command_pool, &[cmd]);
    }
    Ok(())
}

fn create_splat_pipeline(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    mask_set_layout: vk::DescriptorSetLayout,
    shader_dir: &Path,
) -> Result<(vk::Pipeline, vk::PipelineLayout)> {
    let vert_path = shader_dir.join("blood_splat.vert");
    let vert_source = std::fs::read_to_string(&vert_path)
        .map_err(|e| anyhow::anyhow!("Failed to read {:?}: {}", vert_path, e))?;
    let vert_spv = hot_reload::compile_glsl(
        &vert_source,
        "blood_splat.vert",
        shaderc::ShaderKind::Vertex,
    )?;
    let vert_module = pipe::create_shader_module(device, &vert_spv)?;

    let frag_path = shader_dir.join("blood_splat.frag");
    let frag_source = std::fs::read_to_string(&frag_path)
        .map_err(|e| anyhow::anyhow!("Failed to read {:?}: {}", frag_path, e))?;
    let frag_spv = hot_reload::compile_glsl(
        &frag_source,
        "blood_splat.frag",
        shaderc::ShaderKind::Fragment,
    )?;
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

    // Per-instance vertex bindings. Two `vec4` attributes packed in a
    // single `SplatInstance` struct.
    let bindings = [vk::VertexInputBindingDescription {
        binding: 0,
        stride: std::mem::size_of::<SplatInstance>() as u32,
        input_rate: vk::VertexInputRate::INSTANCE,
    }];
    let attrs = [
        vk::VertexInputAttributeDescription {
            location: 0,
            binding: 0,
            format: vk::Format::R32G32B32A32_SFLOAT,
            offset: 0,
        },
        vk::VertexInputAttributeDescription {
            location: 1,
            binding: 0,
            format: vk::Format::R32G32B32A32_SFLOAT,
            offset: 16,
        },
    ];
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
        .vertex_binding_descriptions(&bindings)
        .vertex_attribute_descriptions(&attrs);

    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_STRIP);

    let viewport = vk::Viewport {
        x: 0.0,
        y: 0.0,
        width: FIELD_RESOLUTION as f32,
        height: FIELD_RESOLUTION as f32,
        min_depth: 0.0,
        max_depth: 1.0,
    };
    let scissor = vk::Rect2D {
        offset: vk::Offset2D { x: 0, y: 0 },
        extent: vk::Extent2D {
            width: FIELD_RESOLUTION,
            height: FIELD_RESOLUTION,
        },
    };
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewports(std::slice::from_ref(&viewport))
        .scissors(std::slice::from_ref(&scissor));

    // No culling — the splat quads use triangle-strip and may be
    // emitted with either winding depending on rotation.
    let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .line_width(1.0)
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .depth_bias_enable(false);

    let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);

    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(false)
        .depth_write_enable(false)
        .stencil_test_enable(false);

    // Blend op: MAX on both R and G. `MAX(src, dst)` per channel.
    // R = max wet intensity, G = max spawn time (most recent splat
    // wins for any texel it overlaps). Effectively layers splats
    // without darkening, and "newest splat" wins age-wise.
    let blend_attachment = vk::PipelineColorBlendAttachmentState::default()
        .color_write_mask(vk::ColorComponentFlags::R | vk::ColorComponentFlags::G)
        .blend_enable(true)
        .src_color_blend_factor(vk::BlendFactor::ONE)
        .dst_color_blend_factor(vk::BlendFactor::ONE)
        .color_blend_op(vk::BlendOp::MAX)
        .src_alpha_blend_factor(vk::BlendFactor::ONE)
        .dst_alpha_blend_factor(vk::BlendFactor::ONE)
        .alpha_blend_op(vk::BlendOp::MAX);
    let color_blending = vk::PipelineColorBlendStateCreateInfo::default()
        .attachments(std::slice::from_ref(&blend_attachment));

    let set_layouts = [mask_set_layout];
    let layout_info = vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts);
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

/// Procgen 4-silhouette mask atlas. 2×2 grid of organic blood-splatter
/// shapes built from metaballs + radial drips so that splats don't
/// look like geometric circles.
fn generate_mask_atlas() -> Vec<u8> {
    let mut pixels = vec![0u8; (MASK_RESOLUTION * MASK_RESOLUTION) as usize];
    let cell = MASK_RESOLUTION / 2;
    for slice in 0..MASK_SLICE_COUNT {
        let cx_off = (slice & 1) * cell;
        let cy_off = ((slice >> 1) & 1) * cell;
        // Per-slice variation: different blob count + drip count +
        // RNG seed so the four cells read as distinct silhouettes.
        let (blob_count, drip_count, seed) = match slice {
            0 => (5, 6, 0x9e37_79b9_u32),  // small spray
            1 => (8, 12, 0xa1b2_c3d4_u32), // dense splatter
            2 => (3, 16, 0x1234_5678_u32), // long-drip pool
            _ => (6, 4, 0xface_cafe_u32),  // round splat
        };
        let mut rng = seed;
        let mut next = || {
            // xorshift32
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            (rng as f32 / u32::MAX as f32).fract()
        };
        // Generate metaball centres in the central [0.25, 0.75]² of
        // the cell so the silhouette doesn't bleed into neighbours.
        let mut blobs = Vec::with_capacity(blob_count);
        for _ in 0..blob_count {
            let x = 0.30 + next() * 0.40;
            let y = 0.30 + next() * 0.40;
            let r = 0.10 + next() * 0.10;
            blobs.push((x, y, r));
        }
        // Radial drips emanate outward from the centroid with random
        // length/thickness for the spray feel.
        let cx = 0.5;
        let cy = 0.5;
        let mut drips = Vec::with_capacity(drip_count);
        for _ in 0..drip_count {
            let theta = next() * std::f32::consts::TAU;
            let len = 0.18 + next() * 0.30;
            let thick = 0.012 + next() * 0.025;
            drips.push((theta, len, thick));
        }
        for py in 0..cell {
            for px in 0..cell {
                let u = px as f32 / cell as f32;
                let v = py as f32 / cell as f32;
                // Metaball field: sum of 1/d² lobes, threshold at
                // ~1.0 for a rounded silhouette boundary.
                let mut field = 0.0_f32;
                for &(bx, by, br) in &blobs {
                    let dx = u - bx;
                    let dy = v - by;
                    let d2 = dx * dx + dy * dy + 1e-4;
                    field += (br * br) / d2;
                }
                let mut mask = ((field - 1.0) * 4.0).clamp(0.0, 1.0);
                // Drips: distance to a line segment from centroid
                // outward. Add the closest drip's contribution.
                let mut drip_contrib = 0.0_f32;
                for &(theta, len, thick) in &drips {
                    let dx = u - cx;
                    let dy = v - cy;
                    // Project (dx,dy) onto the drip direction.
                    let dirx = theta.cos();
                    let diry = theta.sin();
                    let t = (dx * dirx + dy * diry).clamp(0.0, len);
                    let projx = t * dirx;
                    let projy = t * diry;
                    let nx = dx - projx;
                    let ny = dy - projy;
                    // Drip thickness tapers along its length.
                    let taper = 1.0 - (t / len).clamp(0.0, 1.0).powf(1.4);
                    let r = thick * taper + 0.002;
                    let dist = (nx * nx + ny * ny).sqrt();
                    let c = (1.0 - (dist / r)).clamp(0.0, 1.0);
                    drip_contrib = drip_contrib.max(c);
                }
                mask = mask.max(drip_contrib);
                // Soft outer falloff so the mask edge doesn't snap.
                let dx = u - cx;
                let dy = v - cy;
                let d = (dx * dx + dy * dy).sqrt();
                let outer = (1.0 - ((d - 0.45) * 4.0)).clamp(0.0, 1.0);
                mask *= outer;
                let dst_x = cx_off + px;
                let dst_y = cy_off + py;
                pixels[(dst_y * MASK_RESOLUTION + dst_x) as usize] =
                    (mask.clamp(0.0, 1.0) * 255.0) as u8;
            }
        }
    }
    pixels
}

/// 2D ray vs. axis-aligned box (XZ projection of an `Aabb`) — slab
/// method. Returns the first hit point in XZ + the distance along
/// the ray, or `None` if no AABB is hit within `max_dist`.
///
/// Used by the wall-arc layer of `BloodField::splat_for_kill` to
/// find which wall a kill's spray cone actually hits, so the splat
/// system can paint blood at the wall's XZ rather than scattering
/// freely in open space.
fn ray_first_aabb_xz(
    origin: Vec2,
    dir: Vec2,
    max_dist: f32,
    aabbs: &[Aabb],
) -> Option<(Vec2, f32)> {
    let mut best: Option<(Vec2, f32)> = None;
    for aabb in aabbs {
        let lo = Vec2::new(aabb.min.x, aabb.min.z);
        let hi = Vec2::new(aabb.max.x, aabb.max.z);
        // Slab intersect, guarded against divide-by-zero on rays
        // parallel to an axis.
        let inv_x = if dir.x.abs() > 1e-6 {
            1.0 / dir.x
        } else {
            f32::INFINITY
        };
        let inv_y = if dir.y.abs() > 1e-6 {
            1.0 / dir.y
        } else {
            f32::INFINITY
        };
        let t1 = (lo.x - origin.x) * inv_x;
        let t2 = (hi.x - origin.x) * inv_x;
        let t3 = (lo.y - origin.y) * inv_y;
        let t4 = (hi.y - origin.y) * inv_y;
        let tmin = t1.min(t2).max(t3.min(t4));
        let tmax = t1.max(t2).min(t3.max(t4));
        if tmax < 0.0 || tmin > tmax || tmin > max_dist {
            continue;
        }
        // Use the entry-point distance (front face of the slab),
        // skipping rays that originate inside the box.
        let t = if tmin >= 0.0 { tmin } else { continue };
        if best.map_or(true, |(_, td)| t < td) {
            best = Some((origin + dir * t, t));
        }
    }
    best
}
