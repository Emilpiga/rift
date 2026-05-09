//! HDR offscreen rendering + bloom post-process.
//!
//! ## Pipeline
//!
//! 1. **Scene pass** (`scene_pass`): the main forward pass renders
//!    into `R16G16B16A16_SFLOAT` colour + the existing depth
//!    buffer. Sky, world meshes, ribbons and particles all draw
//!    here. Final layout is `SHADER_READ_ONLY_OPTIMAL` so the
//!    bright-pass can sample it immediately.
//! 2. **Bright pass** (`bloom_pass` instance, framebuffer A):
//!    samples `hdr` → outputs energy above the threshold to
//!    `bloom_a`.
//! 3. **Blur H** (`bloom_pass` instance, framebuffer B): samples
//!    `bloom_a` → writes horizontally-blurred `bloom_b`.
//! 4. **Blur V** (`bloom_pass` instance, framebuffer A): samples
//!    `bloom_b` → writes vertically-blurred `bloom_a` (final).
//! 5. **Composite pass** (`composite_pass`): samples `hdr` +
//!    `bloom_a`, tonemaps, writes to the swapchain. Overlay/UI
//!    is recorded into this same pass so it stays crisp and
//!    isn't tonemapped a second time.
//!
//! ## Resource layout
//!
//! All offscreen images are sized per-swapchain-image (3 sets
//! when MAX_FRAMES_IN_FLIGHT == 3) so frames in flight don't
//! stomp on each other. Bloom resources live at half resolution.
//!
//! ```text
//!   per swapchain image:
//!     hdr     — full-res RGBA16F, COLOR_ATTACHMENT | SAMPLED
//!     bloom_a — half-res RGBA16F, COLOR_ATTACHMENT | SAMPLED
//!     bloom_b — half-res RGBA16F, COLOR_ATTACHMENT | SAMPLED
//!   per swapchain image:
//!     scene_fb     [hdr_view, depth_view]      → scene_pass
//!     bright_fb    [bloom_a_view]              → bloom_pass
//!     blur_h_fb    [bloom_b_view]              → bloom_pass
//!     blur_v_fb    [bloom_a_view]              → bloom_pass
//!     composite_fb [swapchain_view]            → composite_pass
//! ```
//!
//! The bright/blur framebuffers are the same render pass with
//! different attachments — Vulkan only requires render-pass
//! *compatibility* (matching attachment formats), not identity.

use anyhow::Result;
use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};
use gpu_allocator::MemoryLocation;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::hot_reload;
use crate::renderer::depth::DEPTH_FORMAT;
use crate::vulkan::pipeline as pipe;
use crate::vulkan::Swapchain;

/// HDR colour format. Half-float is enough range for our
/// stylised palette (peak ~16-32) and saves bandwidth vs. F32.
pub const HDR_FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;

/// Tunable bloom parameters. Game code can mutate these per
/// biome / cinematic / debug overlay.
#[derive(Clone, Copy, Debug)]
pub struct BloomConfig {
    /// Brightness above which a pixel contributes to bloom.
    /// 1.0 ≈ standard SDR white. Lower (0.6-0.8) for a "lifted"
    /// look, higher (1.5-2.0) to gate the effect to true HDR.
    pub threshold: f32,
    /// Soft-knee width around the threshold (0..1). 0 = hard cut.
    pub soft_knee: f32,
    /// Multiplier on the blurred bloom in the composite. 0
    /// disables bloom visually but keeps the HDR pipeline running.
    pub intensity: f32,
    /// Scene exposure scalar. 1.0 leaves HDR colours alone.
    pub exposure: f32,
}

impl Default for BloomConfig {
    fn default() -> Self {
        Self {
            threshold: 1.0,
            soft_knee: 0.5,
            intensity: 0.55,
            // ACES compresses mid-tones to ~0.78 of their input
            // value at white. The renderer's existing biome
            // colours were tuned for a direct linear path, so we
            // pre-multiply by ~1.3 here to cancel out the
            // perceptual darkening. Game code can override.
            exposure: 1.3,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct BrightPush {
    threshold: f32,
    soft_knee: f32,
    _pad0: f32,
    _pad1: f32,
}
unsafe impl bytemuck::Pod for BrightPush {}
unsafe impl bytemuck::Zeroable for BrightPush {}

#[repr(C)]
#[derive(Clone, Copy)]
struct BlurPush {
    texel_size: [f32; 2],
    direction: [f32; 2],
}
unsafe impl bytemuck::Pod for BlurPush {}
unsafe impl bytemuck::Zeroable for BlurPush {}

#[repr(C)]
#[derive(Clone, Copy)]
struct CompositePush {
    bloom_intensity: f32,
    exposure: f32,
    /// 0.0 = normal scene, 1.0 = full ghost view (desaturate to
    /// luma + cool cyan tint + radial vignette). Driven by the
    /// client when the local player is in ghost mode.
    ghost_mix: f32,
    /// SSAO strength multiplier in `[0, 1]`. 0 disables the
    /// effect; 1 applies the full computed occlusion. Useful as
    /// a graphics-quality knob.
    ssao_strength: f32,
    /// Inverse projection matrix used by the inline SSAO pass to
    /// reconstruct view-space positions from the sampled depth
    /// buffer.
    inv_proj: [[f32; 4]; 4],
    /// Volumetric god-ray data:
    ///   `xy` — sun screen-space UV (NDC ½-mapped). May be
    ///          outside [0,1] (sun off-screen); the shader's
    ///          radial march still produces partial rays from
    ///          on-screen sky pixels in that case.
    ///   `z`  — strength in `[0, 1]`. 0 disables the effect.
    ///   `w`  — `1.0` if the sun is in front of the camera,
    ///          `0.0` if behind (god-rays disabled).
    sun_screen: [f32; 4],
    /// God-ray tint colour (rgb). Typically the sun's base
    /// colour scaled by sun_strength so warmer suns produce
    /// warmer rays.
    sun_color: [f32; 4],
    /// Heat-distortion source (single — brightest fire-like
    /// point light each frame, picked CPU-side).
    ///   `xy` — source screen UV.
    ///   `z`  — falloff radius in UV units (typical 0.10–0.30).
    ///   `w`  — strength in `[0, 1]`. 0 disables the effect.
    heat_source: [f32; 4],
}
unsafe impl bytemuck::Pod for CompositePush {}
unsafe impl bytemuck::Zeroable for CompositePush {}

/// One owned image + view + memory allocation. Used for HDR
/// scene and bloom ping-pong buffers.
struct OffscreenImage {
    image: vk::Image,
    view: vk::ImageView,
    allocation: Option<Allocation>,
}

impl OffscreenImage {
    fn new(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        extent: vk::Extent2D,
        format: vk::Format,
        name: &'static str,
    ) -> Result<Self> {
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D { width: extent.width.max(1), height: extent.height.max(1), depth: 1 })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let image = unsafe { device.create_image(&image_info, None)? };
        let requirements = unsafe { device.get_image_memory_requirements(image) };
        let allocation = allocator.lock().unwrap().allocate(&AllocationCreateDesc {
            name,
            requirements,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })?;
        unsafe {
            device.bind_image_memory(image, allocation.memory(), allocation.offset())?;
        }

        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(format)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        let view = unsafe { device.create_image_view(&view_info, None)? };
        Ok(Self { image, view, allocation: Some(allocation) })
    }

    fn cleanup(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        unsafe {
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
        }
        if let Some(a) = self.allocation.take() {
            allocator.lock().unwrap().free(a).ok();
        }
    }
}

pub struct PostProcessing {
    /// Render pass for the main forward scene (HDR + depth).
    /// Sky and opaque mesh pipelines are built against this
    /// pass. Ribbons and particles use `translucent_pass`.
    pub scene_pass: vk::RenderPass,
    /// Render pass for translucent draws (ribbons + particles).
    /// Loads the HDR + depth from the scene pass; depth is
    /// bound as a read-only attachment AND simultaneously
    /// available to the fragment shader as a combined-image-
    /// sampler (binding via `translucent_set_layout`) so soft
    /// particles can fade as they approach world geometry.
    pub translucent_pass: vk::RenderPass,
    /// Render pass for any bloom ping-pong step (single HDR
    /// colour attachment, no depth). Used by the bright-pass
    /// and both blur passes — render-pass *compatibility* is
    /// what matters for pipelines, not identity, so all three
    /// reuse the same pass object with different framebuffers.
    pub bloom_pass: vk::RenderPass,
    /// Render pass for the swapchain composite. Overlay/UI
    /// pipelines must be built against this pass.
    pub composite_pass: vk::RenderPass,

    pub extent: vk::Extent2D,
    bloom_extent: vk::Extent2D,

    hdr: Vec<OffscreenImage>,
    bloom_a: Vec<OffscreenImage>,
    bloom_b: Vec<OffscreenImage>,

    pub scene_framebuffers: Vec<vk::Framebuffer>,
    pub translucent_framebuffers: Vec<vk::Framebuffer>,
    bright_framebuffers: Vec<vk::Framebuffer>,
    blur_h_framebuffers: Vec<vk::Framebuffer>,
    blur_v_framebuffers: Vec<vk::Framebuffer>,
    pub composite_framebuffers: Vec<vk::Framebuffer>,

    sampler: vk::Sampler,
    /// Nearest-neighbour sampler for the depth buffer used by
    /// the inline SSAO in the composite pass.
    depth_sampler: vk::Sampler,
    /// Cached depth view bound to every composite descriptor
    /// set. Single shared depth attachment, so one view is
    /// correct for every set.
    depth_view: vk::ImageView,

    // Descriptor plumbing. Two layouts: a single combined-image-
    // sampler layout (used by bright + both blur passes), and a
    // dual-binding layout used by the composite to read HDR and
    // bloom in one pipeline.
    descriptor_pool: vk::DescriptorPool,
    single_set_layout: vk::DescriptorSetLayout,
    composite_set_layout: vk::DescriptorSetLayout,
    /// Descriptor set layout used by ribbon + particle shaders
    /// to read the scene depth buffer (binding 0,
    /// COMBINED_IMAGE_SAMPLER). One set per swapchain image,
    /// allocated in `translucent_in_sets`.
    pub translucent_set_layout: vk::DescriptorSetLayout,

    bright_in_sets: Vec<vk::DescriptorSet>,    // bright reads HDR
    blur_h_in_sets: Vec<vk::DescriptorSet>,    // blur_h reads bloom_a
    blur_v_in_sets: Vec<vk::DescriptorSet>,    // blur_v reads bloom_b
    composite_in_sets: Vec<vk::DescriptorSet>, // composite reads HDR + bloom_a
    /// Per-image descriptor set bound at set=1 by ribbon +
    /// particle pipelines so their fragment shaders can sample
    /// the scene depth buffer.
    pub translucent_in_sets: Vec<vk::DescriptorSet>,

    bright_pipeline: vk::Pipeline,
    bright_layout: vk::PipelineLayout,
    blur_pipeline: vk::Pipeline,
    blur_layout: vk::PipelineLayout,
    composite_pipeline: vk::Pipeline,
    composite_layout: vk::PipelineLayout,
}

impl PostProcessing {
    pub fn new(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        swapchain: &Swapchain,
        depth_view: vk::ImageView,
        shader_dir: &Path,
    ) -> Result<Self> {
        let extent = swapchain.extent;
        // Bloom at full screen resolution. Half-res was making
        // small particle sprites read as chunky 2-pixel blobs
        // once their bright contribution was upsampled back
        // into the composite — the HDR halo from the bloom
        // pass dominates the visible footprint of a spark or
        // ember, so any blur kernel at half-res is what the
        // player perceives as "pixelated". Native res keeps
        // the bloom soft + smooth and the cost is bounded by
        // the 4-tap separable blur (still very cheap).
        let bloom_extent = extent;
        let image_count = swapchain.image_views.len();

        let scene_pass = create_scene_pass(device)?;
        let translucent_pass = create_translucent_pass(device)?;
        let bloom_pass = create_bloom_pass(device)?;
        let composite_pass = create_composite_pass(device, swapchain.format.format)?;

        // ---- Offscreen images ----
        let mut hdr = Vec::with_capacity(image_count);
        let mut bloom_a = Vec::with_capacity(image_count);
        let mut bloom_b = Vec::with_capacity(image_count);
        for _ in 0..image_count {
            hdr.push(OffscreenImage::new(device, allocator, extent, HDR_FORMAT, "post_hdr")?);
            bloom_a.push(OffscreenImage::new(device, allocator, bloom_extent, HDR_FORMAT, "post_bloom_a")?);
            bloom_b.push(OffscreenImage::new(device, allocator, bloom_extent, HDR_FORMAT, "post_bloom_b")?);
        }

        // ---- Framebuffers ----
        let scene_framebuffers = create_fbs(device, scene_pass, extent,
            hdr.iter().map(|h| [h.view, depth_view]).collect::<Vec<_>>().as_slice())?;
        // Translucent pass shares the same hdr+depth framebuffer
        // pair — it loads what scene_pass stored.
        let translucent_framebuffers = create_fbs(device, translucent_pass, extent,
            hdr.iter().map(|h| [h.view, depth_view]).collect::<Vec<_>>().as_slice())?;
        let bright_framebuffers = create_fbs_single(device, bloom_pass, bloom_extent,
            &bloom_a.iter().map(|i| i.view).collect::<Vec<_>>())?;
        let blur_h_framebuffers = create_fbs_single(device, bloom_pass, bloom_extent,
            &bloom_b.iter().map(|i| i.view).collect::<Vec<_>>())?;
        let blur_v_framebuffers = create_fbs_single(device, bloom_pass, bloom_extent,
            &bloom_a.iter().map(|i| i.view).collect::<Vec<_>>())?;
        let composite_framebuffers = create_fbs_single(device, composite_pass, extent,
            &swapchain.image_views)?;

        // ---- Sampler ----
        let sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .anisotropy_enable(false)
            .max_lod(0.0)
            .min_lod(0.0)
            .border_color(vk::BorderColor::FLOAT_OPAQUE_BLACK);
        let sampler = unsafe { device.create_sampler(&sampler_info, None)? };

        // Nearest-neighbour clamp sampler for depth. Linear
        // filtering across depth edges would corrupt the SSAO
        // position reconstruction.
        let depth_sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::NEAREST)
            .min_filter(vk::Filter::NEAREST)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .anisotropy_enable(false)
            .max_lod(0.0)
            .min_lod(0.0)
            .border_color(vk::BorderColor::FLOAT_OPAQUE_BLACK);
        let depth_sampler = unsafe { device.create_sampler(&depth_sampler_info, None)? };

        // ---- Descriptor layouts + pool + sets ----
        let single_binding = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
        let single_set_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&single_binding),
                None,
            )?
        };
        let composite_bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            // Depth buffer — sampled by inline SSAO.
            vk::DescriptorSetLayoutBinding::default()
                .binding(2)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];
        let composite_set_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&composite_bindings),
                None,
            )?
        };
        // Translucent set: single binding (depth sampler) at
        // set=1 of ribbon + particle pipelines.
        let translucent_bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
        let translucent_set_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&translucent_bindings),
                None,
            )?
        };

        // 3 single-binding sets per image + 1 composite set per
        // image + 1 translucent set per image.
        let max_sets = (image_count * 5) as u32;
        let pool_sizes = [vk::DescriptorPoolSize {
            ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
            // 3*1 (single) + 1*3 (composite hdr+bloom+depth) +
            // 1*1 (translucent depth) = 7 per image.
            descriptor_count: (image_count * 7) as u32,
        }];
        let descriptor_pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(max_sets)
                    .pool_sizes(&pool_sizes),
                None,
            )?
        };

        // Allocate sets.
        let single_layouts = vec![single_set_layout; image_count];
        let composite_layouts = vec![composite_set_layout; image_count];
        let bright_in_sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(descriptor_pool)
                    .set_layouts(&single_layouts),
            )?
        };
        let blur_h_in_sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(descriptor_pool)
                    .set_layouts(&single_layouts),
            )?
        };
        let blur_v_in_sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(descriptor_pool)
                    .set_layouts(&single_layouts),
            )?
        };
        let composite_in_sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(descriptor_pool)
                    .set_layouts(&composite_layouts),
            )?
        };
        let translucent_layouts = vec![translucent_set_layout; image_count];
        let translucent_in_sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(descriptor_pool)
                    .set_layouts(&translucent_layouts),
            )?
        };

        // Wire descriptors → image views. Each is a
        // COMBINED_IMAGE_SAMPLER with the linear-clamp sampler we
        // just built. Layout in the descriptor is
        // SHADER_READ_ONLY_OPTIMAL because every sampling pass
        // reads after a render pass that ends with that layout.
        for i in 0..image_count {
            write_combined(device, bright_in_sets[i], 0, hdr[i].view, sampler);
            write_combined(device, blur_h_in_sets[i], 0, bloom_a[i].view, sampler);
            write_combined(device, blur_v_in_sets[i], 0, bloom_b[i].view, sampler);
            write_combined(device, composite_in_sets[i], 0, hdr[i].view, sampler);
            write_combined(device, composite_in_sets[i], 1, bloom_a[i].view, sampler);
            write_combined(device, composite_in_sets[i], 2, depth_view, depth_sampler);
            // Translucent set: same depth buffer, sampled by
            // ribbon/particle frag shaders for soft fade. The
            // descriptor's layout must match the image's actual
            // layout *while the translucent pass is recording*
            // — the subpass attachment ref keeps depth in
            // DEPTH_STENCIL_READ_ONLY_OPTIMAL, so the descriptor
            // must use the same.
            write_combined_with_layout(
                device, translucent_in_sets[i], 0,
                depth_view, depth_sampler,
                vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL,
            );
        }

        // ---- Pipelines ----
        let (bright_pipeline, bright_layout) = build_post_pipeline(
            device, bloom_pass, shader_dir, "post.vert", "post_bright.frag",
            single_set_layout, std::mem::size_of::<BrightPush>() as u32,
        )?;
        let (blur_pipeline, blur_layout) = build_post_pipeline(
            device, bloom_pass, shader_dir, "post.vert", "post_blur.frag",
            single_set_layout, std::mem::size_of::<BlurPush>() as u32,
        )?;
        let (composite_pipeline, composite_layout) = build_post_pipeline(
            device, composite_pass, shader_dir, "post.vert", "post_composite.frag",
            composite_set_layout, std::mem::size_of::<CompositePush>() as u32,
        )?;

        Ok(Self {
            scene_pass, translucent_pass, bloom_pass, composite_pass,
            extent, bloom_extent,
            hdr, bloom_a, bloom_b,
            scene_framebuffers, translucent_framebuffers,
            bright_framebuffers, blur_h_framebuffers,
            blur_v_framebuffers, composite_framebuffers,
            sampler,
            depth_sampler,
            depth_view,
            descriptor_pool, single_set_layout, composite_set_layout,
            translucent_set_layout,
            bright_in_sets, blur_h_in_sets, blur_v_in_sets, composite_in_sets,
            translucent_in_sets,
            bright_pipeline, bright_layout,
            blur_pipeline, blur_layout,
            composite_pipeline, composite_layout,
        })
    }

    /// Tear down everything that depends on swapchain extent /
    /// images (framebuffers, offscreen images, descriptor sets,
    /// composite framebuffers). Called by `recreate_swapchain`
    /// before rebuilding via `recreate`.
    pub fn cleanup_swapchain_dependent(
        &mut self,
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
    ) {
        unsafe {
            for &fb in self.scene_framebuffers.iter()
                .chain(self.translucent_framebuffers.iter())
                .chain(self.bright_framebuffers.iter())
                .chain(self.blur_h_framebuffers.iter())
                .chain(self.blur_v_framebuffers.iter())
                .chain(self.composite_framebuffers.iter())
            {
                device.destroy_framebuffer(fb, None);
            }
        }
        self.scene_framebuffers.clear();
        self.translucent_framebuffers.clear();
        self.bright_framebuffers.clear();
        self.blur_h_framebuffers.clear();
        self.blur_v_framebuffers.clear();
        self.composite_framebuffers.clear();

        for img in self.hdr.iter_mut() { img.cleanup(device, allocator); }
        for img in self.bloom_a.iter_mut() { img.cleanup(device, allocator); }
        for img in self.bloom_b.iter_mut() { img.cleanup(device, allocator); }
        self.hdr.clear();
        self.bloom_a.clear();
        self.bloom_b.clear();

        // Reset descriptor pool — frees all sets so we can
        // re-allocate against new image views.
        unsafe { device.reset_descriptor_pool(self.descriptor_pool, vk::DescriptorPoolResetFlags::empty()).ok(); }
        self.bright_in_sets.clear();
        self.blur_h_in_sets.clear();
        self.blur_v_in_sets.clear();
        self.composite_in_sets.clear();
        self.translucent_in_sets.clear();
    }

    /// Rebuild the offscreen images, framebuffers and descriptor
    /// sets for the new swapchain. Render passes and pipelines
    /// are kept (their layouts are extent-independent because
    /// every post pipeline uses dynamic viewport+scissor).
    pub fn recreate(
        &mut self,
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        swapchain: &Swapchain,
        depth_view: vk::ImageView,
    ) -> Result<()> {
        self.extent = swapchain.extent;
        // Match the constructor: full-res bloom for crisp
        // particles. See `new` for rationale.
        self.bloom_extent = self.extent;
        // Cache the new depth view — the depth attachment is
        // recreated when the swapchain is, so the SSAO sampler
        // binding has to point at the new image.
        self.depth_view = depth_view;
        let image_count = swapchain.image_views.len();

        for _ in 0..image_count {
            self.hdr.push(OffscreenImage::new(device, allocator, self.extent, HDR_FORMAT, "post_hdr")?);
            self.bloom_a.push(OffscreenImage::new(device, allocator, self.bloom_extent, HDR_FORMAT, "post_bloom_a")?);
            self.bloom_b.push(OffscreenImage::new(device, allocator, self.bloom_extent, HDR_FORMAT, "post_bloom_b")?);
        }

        self.scene_framebuffers = create_fbs(device, self.scene_pass, self.extent,
            self.hdr.iter().map(|h| [h.view, depth_view]).collect::<Vec<_>>().as_slice())?;
        self.translucent_framebuffers = create_fbs(device, self.translucent_pass, self.extent,
            self.hdr.iter().map(|h| [h.view, depth_view]).collect::<Vec<_>>().as_slice())?;
        self.bright_framebuffers = create_fbs_single(device, self.bloom_pass, self.bloom_extent,
            &self.bloom_a.iter().map(|i| i.view).collect::<Vec<_>>())?;
        self.blur_h_framebuffers = create_fbs_single(device, self.bloom_pass, self.bloom_extent,
            &self.bloom_b.iter().map(|i| i.view).collect::<Vec<_>>())?;
        self.blur_v_framebuffers = create_fbs_single(device, self.bloom_pass, self.bloom_extent,
            &self.bloom_a.iter().map(|i| i.view).collect::<Vec<_>>())?;
        self.composite_framebuffers = create_fbs_single(device, self.composite_pass, self.extent,
            &swapchain.image_views)?;

        let single_layouts = vec![self.single_set_layout; image_count];
        let composite_layouts = vec![self.composite_set_layout; image_count];
        self.bright_in_sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(self.descriptor_pool).set_layouts(&single_layouts))?
        };
        self.blur_h_in_sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(self.descriptor_pool).set_layouts(&single_layouts))?
        };
        self.blur_v_in_sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(self.descriptor_pool).set_layouts(&single_layouts))?
        };
        self.composite_in_sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(self.descriptor_pool).set_layouts(&composite_layouts))?
        };
        let translucent_layouts = vec![self.translucent_set_layout; image_count];
        self.translucent_in_sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(self.descriptor_pool).set_layouts(&translucent_layouts))?
        };
        for i in 0..image_count {
            write_combined(device, self.bright_in_sets[i], 0, self.hdr[i].view, self.sampler);
            write_combined(device, self.blur_h_in_sets[i], 0, self.bloom_a[i].view, self.sampler);
            write_combined(device, self.blur_v_in_sets[i], 0, self.bloom_b[i].view, self.sampler);
            write_combined(device, self.composite_in_sets[i], 0, self.hdr[i].view, self.sampler);
            write_combined(device, self.composite_in_sets[i], 1, self.bloom_a[i].view, self.sampler);
            write_combined(device, self.composite_in_sets[i], 2, self.depth_view, self.depth_sampler);
            write_combined_with_layout(
                device, self.translucent_in_sets[i], 0,
                self.depth_view, self.depth_sampler,
                vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL,
            );
        }
        Ok(())
    }

    /// Bright-pass + both blur passes. Caller has just ended the
    /// scene render pass, so the HDR image is in
    /// SHADER_READ_ONLY_OPTIMAL layout (forced by scene_pass's
    /// final_layout). We run three back-to-back render passes;
    /// each pass's external dependencies handle the
    /// sampler-read → colour-write → sampler-read transitions.
    pub fn record_bloom(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        image_index: u32,
        config: &BloomConfig,
    ) {
        let i = image_index as usize;
        let bloom_area = vk::Rect2D { offset: vk::Offset2D::default(), extent: self.bloom_extent };
        let viewport = vk::Viewport {
            x: 0.0, y: 0.0,
            width: self.bloom_extent.width as f32,
            height: self.bloom_extent.height as f32,
            min_depth: 0.0, max_depth: 1.0,
        };

        // ---- Bright pass ----
        unsafe {
            let begin = vk::RenderPassBeginInfo::default()
                .render_pass(self.bloom_pass)
                .framebuffer(self.bright_framebuffers[i])
                .render_area(bloom_area);
            device.cmd_begin_render_pass(cmd, &begin, vk::SubpassContents::INLINE);
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.bright_pipeline);
            device.cmd_set_viewport(cmd, 0, std::slice::from_ref(&viewport));
            device.cmd_set_scissor(cmd, 0, std::slice::from_ref(&bloom_area));
            device.cmd_bind_descriptor_sets(
                cmd, vk::PipelineBindPoint::GRAPHICS, self.bright_layout,
                0, std::slice::from_ref(&self.bright_in_sets[i]), &[],
            );
            let push = BrightPush {
                threshold: config.threshold,
                soft_knee: config.soft_knee,
                _pad0: 0.0, _pad1: 0.0,
            };
            device.cmd_push_constants(cmd, self.bright_layout,
                vk::ShaderStageFlags::FRAGMENT, 0, bytemuck::bytes_of(&push));
            device.cmd_draw(cmd, 3, 1, 0, 0);
            device.cmd_end_render_pass(cmd);
        }

        let texel = [
            1.0 / self.bloom_extent.width as f32,
            1.0 / self.bloom_extent.height as f32,
        ];

        // ---- Blur horizontal ----
        unsafe {
            let begin = vk::RenderPassBeginInfo::default()
                .render_pass(self.bloom_pass)
                .framebuffer(self.blur_h_framebuffers[i])
                .render_area(bloom_area);
            device.cmd_begin_render_pass(cmd, &begin, vk::SubpassContents::INLINE);
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.blur_pipeline);
            device.cmd_set_viewport(cmd, 0, std::slice::from_ref(&viewport));
            device.cmd_set_scissor(cmd, 0, std::slice::from_ref(&bloom_area));
            device.cmd_bind_descriptor_sets(
                cmd, vk::PipelineBindPoint::GRAPHICS, self.blur_layout,
                0, std::slice::from_ref(&self.blur_h_in_sets[i]), &[],
            );
            let push = BlurPush { texel_size: texel, direction: [1.0, 0.0] };
            device.cmd_push_constants(cmd, self.blur_layout,
                vk::ShaderStageFlags::FRAGMENT, 0, bytemuck::bytes_of(&push));
            device.cmd_draw(cmd, 3, 1, 0, 0);
            device.cmd_end_render_pass(cmd);
        }

        // ---- Blur vertical ----
        unsafe {
            let begin = vk::RenderPassBeginInfo::default()
                .render_pass(self.bloom_pass)
                .framebuffer(self.blur_v_framebuffers[i])
                .render_area(bloom_area);
            device.cmd_begin_render_pass(cmd, &begin, vk::SubpassContents::INLINE);
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.blur_pipeline);
            device.cmd_set_viewport(cmd, 0, std::slice::from_ref(&viewport));
            device.cmd_set_scissor(cmd, 0, std::slice::from_ref(&bloom_area));
            device.cmd_bind_descriptor_sets(
                cmd, vk::PipelineBindPoint::GRAPHICS, self.blur_layout,
                0, std::slice::from_ref(&self.blur_v_in_sets[i]), &[],
            );
            let push = BlurPush { texel_size: texel, direction: [0.0, 1.0] };
            device.cmd_push_constants(cmd, self.blur_layout,
                vk::ShaderStageFlags::FRAGMENT, 0, bytemuck::bytes_of(&push));
            device.cmd_draw(cmd, 3, 1, 0, 0);
            device.cmd_end_render_pass(cmd);
        }
    }

    /// Composite HDR + bloom into the swapchain. Caller is
    /// responsible for beginning the composite render pass (so
    /// that overlay drawing can happen in the same pass after
    /// us). Just records the fullscreen draw with proper
    /// pipeline / descriptors.
    ///
    /// `ghost_mix` in `[0.0, 1.0]` blends in the ghost-view
    /// post effect (desaturate-to-luma + cool tint + radial
    /// vignette). `0.0` is the default no-op.
    ///
    /// `inv_proj` is the inverse of the camera projection matrix
    /// (NOT the view-projection — SSAO works in view space). It
    /// is uploaded as a push constant so the composite shader can
    /// reconstruct view-space positions from sampled depth.
    ///
    /// `ssao_strength` in `[0.0, 1.0]` scales the inline SSAO
    /// contribution. 0 disables it.
    pub fn record_composite(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        image_index: u32,
        config: &BloomConfig,
        ghost_mix: f32,
        inv_proj: [[f32; 4]; 4],
        ssao_strength: f32,
        sun_screen: [f32; 4],
        sun_color: [f32; 4],
        heat_source: [f32; 4],
    ) {
        let i = image_index as usize;
        let viewport = vk::Viewport {
            x: 0.0, y: 0.0,
            width: self.extent.width as f32,
            height: self.extent.height as f32,
            min_depth: 0.0, max_depth: 1.0,
        };
        let scissor = vk::Rect2D { offset: vk::Offset2D::default(), extent: self.extent };
        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.composite_pipeline);
            device.cmd_set_viewport(cmd, 0, std::slice::from_ref(&viewport));
            device.cmd_set_scissor(cmd, 0, std::slice::from_ref(&scissor));
            device.cmd_bind_descriptor_sets(
                cmd, vk::PipelineBindPoint::GRAPHICS, self.composite_layout,
                0, std::slice::from_ref(&self.composite_in_sets[i]), &[],
            );
            let push = CompositePush {
                bloom_intensity: config.intensity,
                exposure: config.exposure,
                ghost_mix: ghost_mix.clamp(0.0, 1.0),
                ssao_strength: ssao_strength.clamp(0.0, 1.0),
                inv_proj,
                sun_screen,
                sun_color,
                heat_source,
            };
            device.cmd_push_constants(cmd, self.composite_layout,
                vk::ShaderStageFlags::FRAGMENT, 0, bytemuck::bytes_of(&push));
            device.cmd_draw(cmd, 3, 1, 0, 0);
        }
    }

    /// Recompile the bright / blur / composite pipelines from
    /// the on-disk shader sources and atomically swap them in.
    /// Used by the editor hot-reload path so that edits to
    /// `post_bright.frag`, `post_blur.frag`, `post_composite.frag`
    /// (or the shared `post.vert`) take effect without a
    /// process restart. The descriptor set layouts, render
    /// passes, and push-constant ranges are unchanged, so
    /// the existing descriptor sets and recorded command
    /// buffers remain valid.
    ///
    /// Caller is responsible for ensuring the device is idle
    /// before invoking this (the old pipelines are destroyed
    /// in place). On compile failure the existing pipelines
    /// are kept and an error is returned.
    pub fn reload_pipelines(
        &mut self,
        device: &ash::Device,
        shader_dir: &Path,
    ) -> Result<()> {
        let (new_bright, new_bright_layout) = build_post_pipeline(
            device, self.bloom_pass, shader_dir, "post.vert", "post_bright.frag",
            self.single_set_layout, std::mem::size_of::<BrightPush>() as u32,
        )?;
        let (new_blur, new_blur_layout) = build_post_pipeline(
            device, self.bloom_pass, shader_dir, "post.vert", "post_blur.frag",
            self.single_set_layout, std::mem::size_of::<BlurPush>() as u32,
        )?;
        let (new_composite, new_composite_layout) = build_post_pipeline(
            device, self.composite_pass, shader_dir, "post.vert", "post_composite.frag",
            self.composite_set_layout, std::mem::size_of::<CompositePush>() as u32,
        )?;
        unsafe {
            device.destroy_pipeline(self.bright_pipeline, None);
            device.destroy_pipeline_layout(self.bright_layout, None);
            device.destroy_pipeline(self.blur_pipeline, None);
            device.destroy_pipeline_layout(self.blur_layout, None);
            device.destroy_pipeline(self.composite_pipeline, None);
            device.destroy_pipeline_layout(self.composite_layout, None);
        }
        self.bright_pipeline = new_bright;
        self.bright_layout = new_bright_layout;
        self.blur_pipeline = new_blur;
        self.blur_layout = new_blur_layout;
        self.composite_pipeline = new_composite;
        self.composite_layout = new_composite_layout;
        Ok(())
    }

    pub fn cleanup(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        self.cleanup_swapchain_dependent(device, allocator);
        unsafe {
            device.destroy_pipeline(self.bright_pipeline, None);
            device.destroy_pipeline_layout(self.bright_layout, None);
            device.destroy_pipeline(self.blur_pipeline, None);
            device.destroy_pipeline_layout(self.blur_layout, None);
            device.destroy_pipeline(self.composite_pipeline, None);
            device.destroy_pipeline_layout(self.composite_layout, None);
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.single_set_layout, None);
            device.destroy_descriptor_set_layout(self.composite_set_layout, None);
            device.destroy_descriptor_set_layout(self.translucent_set_layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_sampler(self.depth_sampler, None);
            device.destroy_render_pass(self.scene_pass, None);
            device.destroy_render_pass(self.translucent_pass, None);
            device.destroy_render_pass(self.bloom_pass, None);
            device.destroy_render_pass(self.composite_pass, None);
        }
    }
}

// ---------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------

fn write_combined(
    device: &ash::Device,
    set: vk::DescriptorSet,
    binding: u32,
    view: vk::ImageView,
    sampler: vk::Sampler,
) {
    write_combined_with_layout(device, set, binding, view, sampler,
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
}

/// Variant of `write_combined` that lets the caller pick the
/// descriptor's image layout. Required for the translucent
/// pass's depth sampler binding: while particles draw, the
/// depth image is in `DEPTH_STENCIL_READ_ONLY_OPTIMAL` (so the
/// render pass can both depth-test against it AND let the
/// shader sample it). Using `SHADER_READ_ONLY_OPTIMAL` for
/// that binding triggers VUID-vkCmdDrawIndexed-imageLayout-00344
/// — Vulkan requires the descriptor layout to match the image's
/// actual layout when the descriptor is accessed.
fn write_combined_with_layout(
    device: &ash::Device,
    set: vk::DescriptorSet,
    binding: u32,
    view: vk::ImageView,
    sampler: vk::Sampler,
    layout: vk::ImageLayout,
) {
    let image_info = [vk::DescriptorImageInfo::default()
        .image_view(view)
        .sampler(sampler)
        .image_layout(layout)];
    let write = vk::WriteDescriptorSet::default()
        .dst_set(set)
        .dst_binding(binding)
        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
        .image_info(&image_info);
    unsafe { device.update_descriptor_sets(std::slice::from_ref(&write), &[]); }
}

fn create_fbs(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    extent: vk::Extent2D,
    attachments_per_fb: &[[vk::ImageView; 2]],
) -> Result<Vec<vk::Framebuffer>> {
    let mut out = Vec::with_capacity(attachments_per_fb.len());
    for atts in attachments_per_fb {
        let info = vk::FramebufferCreateInfo::default()
            .render_pass(render_pass)
            .attachments(atts)
            .width(extent.width)
            .height(extent.height)
            .layers(1);
        out.push(unsafe { device.create_framebuffer(&info, None)? });
    }
    Ok(out)
}

fn create_fbs_single(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    extent: vk::Extent2D,
    views: &[vk::ImageView],
) -> Result<Vec<vk::Framebuffer>> {
    let mut out = Vec::with_capacity(views.len());
    for view in views {
        let atts = [*view];
        let info = vk::FramebufferCreateInfo::default()
            .render_pass(render_pass)
            .attachments(&atts)
            .width(extent.width)
            .height(extent.height)
            .layers(1);
        out.push(unsafe { device.create_framebuffer(&info, None)? });
    }
    Ok(out)
}

fn create_scene_pass(device: &ash::Device) -> Result<vk::RenderPass> {
    let attachments = [
        // HDR colour. Translucent pass loads this so we hand off
        // in COLOR_ATTACHMENT_OPTIMAL, not SHADER_READ_ONLY.
        vk::AttachmentDescription::default()
            .format(HDR_FORMAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL),
        // Depth. Final layout SHADER_READ_ONLY_OPTIMAL so any
        // pass that samples depth (translucent particles, the
        // composite SSAO) sees a stable, sampleable layout. The
        // translucent pass re-binds it as a read-only depth
        // attachment (DEPTH_STENCIL_READ_ONLY_OPTIMAL via the
        // subpass attachment ref) and transitions back at the
        // end — no manual barrier needed.
        vk::AttachmentDescription::default()
            .format(DEPTH_FORMAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL),
    ];
    let color_ref = vk::AttachmentReference {
        attachment: 0,
        layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
    };
    let depth_ref = vk::AttachmentReference {
        attachment: 1,
        layout: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
    };
    let subpass = vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(std::slice::from_ref(&color_ref))
        .depth_stencil_attachment(&depth_ref);
    let dependencies = [
        // Wait for previous-frame sampling of the HDR image to
        // finish before we overwrite it. (Also handles the
        // initial UNDEFINED → COLOR_ATTACHMENT_OPTIMAL on first
        // use.)
        vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(
                vk::PipelineStageFlags::FRAGMENT_SHADER
                    | vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
            )
            .src_access_mask(vk::AccessFlags::SHADER_READ)
            .dst_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
            )
            .dst_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            ),
        // Make scene writes visible to the bright pass that
        // samples the HDR image AND to the composite pass that
        // samples depth for SSAO. Includes both COLOR_ATTACHMENT
        // (HDR write) and LATE_FRAGMENT_TESTS (depth write +
        // implicit layout transition to SHADER_READ_ONLY_OPTIMAL).
        vk::SubpassDependency::default()
            .src_subpass(0)
            .dst_subpass(vk::SUBPASS_EXTERNAL)
            .src_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS,
            )
            .src_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            )
            .dst_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
            .dst_access_mask(vk::AccessFlags::SHADER_READ),
    ];
    let info = vk::RenderPassCreateInfo::default()
        .attachments(&attachments)
        .subpasses(std::slice::from_ref(&subpass))
        .dependencies(&dependencies);
    Ok(unsafe { device.create_render_pass(&info, None)? })
}

/// Render pass for the translucent layer (ribbons + particles).
/// Loads HDR colour from the scene pass (initial layout
/// COLOR_ATTACHMENT_OPTIMAL) and the depth buffer in
/// DEPTH_STENCIL_READ_ONLY_OPTIMAL — depth test still works
/// (with `depth_write = false` baked into the particle/ribbon
/// pipelines), AND the same depth image can be sampled
/// simultaneously by the fragment shader for soft-particle
/// fade. Final layouts hand off to the bright-pass + composite:
/// HDR → SHADER_READ_ONLY_OPTIMAL, depth → SHADER_READ_ONLY_OPTIMAL
/// (composite SSAO samples it).
fn create_translucent_pass(device: &ash::Device) -> Result<vk::RenderPass> {
    let attachments = [
        vk::AttachmentDescription::default()
            .format(HDR_FORMAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::LOAD)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .final_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL),
        vk::AttachmentDescription::default()
            .format(DEPTH_FORMAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::LOAD)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            // Scene pass left depth in SHADER_READ_ONLY_OPTIMAL
            // so descriptors that sample it (this pass + the
            // composite) all agree on layout. The subpass
            // attachment reference below specifies
            // DEPTH_STENCIL_READ_ONLY_OPTIMAL so the render pass
            // transitions for depth-test access on entry and
            // back to SHADER_READ_ONLY_OPTIMAL on exit.
            .initial_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .final_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL),
    ];
    let color_ref = vk::AttachmentReference {
        attachment: 0,
        layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
    };
    // Read-only depth attachment — pipeline state controls
    // depth-test (enabled) and depth-write (disabled).
    let depth_ref = vk::AttachmentReference {
        attachment: 1,
        layout: vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL,
    };
    let subpass = vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(std::slice::from_ref(&color_ref))
        .depth_stencil_attachment(&depth_ref);
    let dependencies = [
        // Wait for the scene pass to finish writing colour AND
        // for the depth-write to be visible as a sampler read.
        vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS,
            )
            .src_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            )
            .dst_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS
                    | vk::PipelineStageFlags::FRAGMENT_SHADER,
            )
            .dst_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags::COLOR_ATTACHMENT_READ
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_READ
                    | vk::AccessFlags::SHADER_READ,
            ),
        // Make our colour write visible to the bright-pass
        // sampler downstream.
        vk::SubpassDependency::default()
            .src_subpass(0)
            .dst_subpass(vk::SUBPASS_EXTERNAL)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
            .dst_access_mask(vk::AccessFlags::SHADER_READ),
    ];
    let info = vk::RenderPassCreateInfo::default()
        .attachments(&attachments)
        .subpasses(std::slice::from_ref(&subpass))
        .dependencies(&dependencies);
    Ok(unsafe { device.create_render_pass(&info, None)? })
}

fn create_bloom_pass(device: &ash::Device) -> Result<vk::RenderPass> {
    let attachments = [vk::AttachmentDescription::default()
        .format(HDR_FORMAT)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::DONT_CARE)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
    let color_ref = vk::AttachmentReference {
        attachment: 0,
        layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
    };
    let subpass = vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(std::slice::from_ref(&color_ref));
    let dependencies = [
        // Previous sampling of this image must finish before we
        // start writing.
        vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
            .src_access_mask(vk::AccessFlags::SHADER_READ)
            .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE),
        // Make our write visible to the next sampler.
        vk::SubpassDependency::default()
            .src_subpass(0)
            .dst_subpass(vk::SUBPASS_EXTERNAL)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
            .dst_access_mask(vk::AccessFlags::SHADER_READ),
    ];
    let info = vk::RenderPassCreateInfo::default()
        .attachments(&attachments)
        .subpasses(std::slice::from_ref(&subpass))
        .dependencies(&dependencies);
    Ok(unsafe { device.create_render_pass(&info, None)? })
}

fn create_composite_pass(device: &ash::Device, swapchain_format: vk::Format) -> Result<vk::RenderPass> {
    let attachments = [vk::AttachmentDescription::default()
        .format(swapchain_format)
        .samples(vk::SampleCountFlags::TYPE_1)
        // We cover every pixel with the composite triangle; no
        // need to load the previous swapchain contents.
        .load_op(vk::AttachmentLoadOp::DONT_CARE)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::PRESENT_SRC_KHR)];
    let color_ref = vk::AttachmentReference {
        attachment: 0,
        layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
    };
    let subpass = vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(std::slice::from_ref(&color_ref));
    let dependencies = [vk::SubpassDependency::default()
        .src_subpass(vk::SUBPASS_EXTERNAL)
        .dst_subpass(0)
        // Wait for the swapchain image to be available (semaphore
        // signals at COLOR_ATTACHMENT_OUTPUT) AND for upstream
        // post passes to finish writing the bloom we sample.
        .src_stage_mask(
            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                | vk::PipelineStageFlags::FRAGMENT_SHADER,
        )
        .src_access_mask(vk::AccessFlags::SHADER_READ)
        .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
        .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)];
    let info = vk::RenderPassCreateInfo::default()
        .attachments(&attachments)
        .subpasses(std::slice::from_ref(&subpass))
        .dependencies(&dependencies);
    Ok(unsafe { device.create_render_pass(&info, None)? })
}

/// Build a fullscreen-triangle post-process pipeline. All three
/// post pipelines share the same vertex input (none),
/// rasterisation, depth-stencil (off) and dynamic state setup —
/// only the fragment shader, target render pass, descriptor
/// layout and push-constant size differ.
fn build_post_pipeline(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    shader_dir: &Path,
    vert_name: &str,
    frag_name: &str,
    set_layout: vk::DescriptorSetLayout,
    push_size: u32,
) -> Result<(vk::Pipeline, vk::PipelineLayout)> {
    let vert_src = std::fs::read_to_string(shader_dir.join(vert_name))
        .map_err(|e| anyhow::anyhow!("read {vert_name}: {e}"))?;
    let frag_src = std::fs::read_to_string(shader_dir.join(frag_name))
        .map_err(|e| anyhow::anyhow!("read {frag_name}: {e}"))?;
    let vert_spv = hot_reload::compile_glsl(&vert_src, vert_name, shaderc::ShaderKind::Vertex)?;
    let frag_spv = hot_reload::compile_glsl(&frag_src, frag_name, shaderc::ShaderKind::Fragment)?;
    let vert_module = pipe::create_shader_module(device, &vert_spv)?;
    let frag_module = pipe::create_shader_module(device, &frag_spv)?;

    let entry = c"main";
    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX).module(vert_module).name(entry),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT).module(frag_module).name(entry),
    ];
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1).scissor_count(1);
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic_state = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
    let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL).line_width(1.0)
        .cull_mode(vk::CullModeFlags::NONE).front_face(vk::FrontFace::COUNTER_CLOCKWISE);
    let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(false).depth_write_enable(false).stencil_test_enable(false);
    let blend_attachment = vk::PipelineColorBlendAttachmentState::default()
        .blend_enable(false).color_write_mask(vk::ColorComponentFlags::RGBA);
    let color_blend = vk::PipelineColorBlendStateCreateInfo::default()
        .attachments(std::slice::from_ref(&blend_attachment));

    let push_range = vk::PushConstantRange {
        stage_flags: vk::ShaderStageFlags::FRAGMENT,
        offset: 0,
        size: push_size,
    };
    let layout_info = vk::PipelineLayoutCreateInfo::default()
        .set_layouts(std::slice::from_ref(&set_layout))
        .push_constant_ranges(std::slice::from_ref(&push_range));
    let pipeline_layout = unsafe { device.create_pipeline_layout(&layout_info, None)? };

    let info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&rasterizer)
        .multisample_state(&multisampling)
        .depth_stencil_state(&depth_stencil)
        .color_blend_state(&color_blend)
        .dynamic_state(&dynamic_state)
        .layout(pipeline_layout)
        .render_pass(render_pass)
        .subpass(0);

    let pipeline = unsafe {
        device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None)
            .map_err(|(_, e)| e)?[0]
    };
    unsafe {
        device.destroy_shader_module(vert_module, None);
        device.destroy_shader_module(frag_module, None);
    }
    Ok((pipeline, pipeline_layout))
}
