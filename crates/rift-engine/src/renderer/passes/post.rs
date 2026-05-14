//! HDR offscreen rendering + bloom post-process.
//!
//! ## Pipeline
//!
//! 1. **Scene pass** (`scene_pass`): the main forward pass renders
//!    into `R16G16B16A16_SFLOAT` colour + the existing depth
//!    buffer. Sky and opaque world meshes draw here, then the
//!    translucent pass loads the same HDR/depth attachments.
//! 2. **Post graph**: fullscreen intermediate effects write
//!    small, focused render targets. `post_ssao.frag` writes a
//!    single-channel AO term from depth; `post_volumetrics.frag`
//!    writes HDR god-rays from HDR + depth.
//! 3. **Bright pass** (`bloom_pass` instance, framebuffer A):
//!    samples `hdr` → outputs energy above the threshold to
//!    `bloom_a`.
//! 4. **Blur H** (`bloom_pass` instance, framebuffer B): samples
//!    `bloom_a` → writes horizontally-blurred `bloom_b`.
//! 5. **Blur V** (`bloom_pass` instance, framebuffer A): samples
//!    `bloom_b` → writes vertically-blurred `bloom_a` (final).
//! 6. **Composite pass** (`composite_pass`): samples HDR, AO,
//!    volumetrics and `bloom_a`, tonemaps, writes to the
//!    swapchain. Overlay/UI is recorded into this same pass so
//!    it stays crisp and isn't tonemapped a second time.
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
//!     ssao    — full-res R8,      COLOR_ATTACHMENT | SAMPLED
//!     rays    — half-res RGBA16F, COLOR_ATTACHMENT | SAMPLED
//!     bloom_a — half-res RGBA16F, COLOR_ATTACHMENT | SAMPLED
//!     bloom_b — half-res RGBA16F, COLOR_ATTACHMENT | SAMPLED
//!   per swapchain image:
//!     scene_fb     [hdr_view, depth_view]      → scene_pass
//!     ssao_fb      [ssao_view]                 → post graph
//!     rays_fb      [rays_view]                 → post graph
//!     bright_fb    [bloom_a_view]              → bloom_pass
//!     blur_h_fb    [bloom_b_view]              → bloom_pass
//!     blur_v_fb    [bloom_a_view]              → bloom_pass
//!     composite_fb [swapchain_view]            → composite_pass
//! ```
//!
//! The bright/blur framebuffers are the same render pass with
//! different attachments — Vulkan only requires render-pass
//! *compatibility* (matching attachment formats), not identity.
//!
//! ## Fixed Stack vs. Graph
//!
//! This module intentionally has two post systems:
//!
//! - The **fixed stack** owns scene/translucent handoff, bloom
//!   ping-pong, and final swapchain composite. These passes are
//!   order-critical and tightly coupled to frame presentation.
//! - The **post graph** owns optional fullscreen effects that
//!   produce intermediate textures consumed by the fixed stack.
//!   Today that is SSAO and volumetrics.
//!
//! If the final image looks wrong, debug in that order: first
//! verify graph outputs, then verify bloom, then verify the final
//! composite bindings and push constants.

use anyhow::Result;
use ash::vk;
use gpu_allocator::vulkan::Allocator;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::vulkan::Swapchain;

mod bloom;
mod composite;
mod graph;
mod passes;
mod resources;

use graph::{
    DescriptorAllocationPlan, PostGraph, PostGraphDescriptorViews, PostGraphRecord,
    PostGraphSamplers, PostGraphViews, SwapchainResourceState,
};
use passes::{
    create_bloom_pass, create_composite_pass, create_scene_pass, create_translucent_pass,
};
use resources::{
    build_post_pipeline, create_fbs, create_fbs_single, write_combined, write_combined_with_layout,
    OffscreenImage,
};

/// HDR colour format. Half-float is enough range for our
/// stylised palette (peak ~16-32) and saves bandwidth vs. F32.
pub const HDR_FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;

/// Single-channel ambient-occlusion target. SSAO writes a
/// multiplicative visibility term in `[0, 1]`; R8 is plenty of
/// precision once it is linearly filtered in the final composite.
pub const AO_FORMAT: vk::Format = vk::Format::R8_UNORM;

/// Shared layout contract for the depth image after scene and
/// translucent passes complete. Any post descriptor that samples
/// depth must use this same layout.
pub(super) const DEPTH_SAMPLED_LAYOUT: vk::ImageLayout =
    vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL;

const BLOOM_DESCRIPTOR_SETS_PER_IMAGE: u32 = 3;
const BLOOM_COMBINED_IMAGE_SAMPLERS_PER_IMAGE: u32 = 3;
const COMPOSITE_DESCRIPTOR_SETS_PER_IMAGE: u32 = 1;
const COMPOSITE_COMBINED_IMAGE_SAMPLERS_PER_IMAGE: u32 = 4;
const TRANSLUCENT_DESCRIPTOR_SETS_PER_IMAGE: u32 = 1;
const TRANSLUCENT_COMBINED_IMAGE_SAMPLERS_PER_IMAGE: u32 = 1;

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

fn post_descriptor_allocation_plan_per_image(post_graph: &PostGraph) -> DescriptorAllocationPlan {
    let graph = post_graph.descriptor_allocation_plan_per_image();
    // Fixed-stack descriptor allocations per swapchain image:
    // bright, blur_h, blur_v, composite, and translucent. The
    // graph contributes one set per registered node and one
    // combined-image-sampler descriptor per declared input.
    DescriptorAllocationPlan {
        sets: BLOOM_DESCRIPTOR_SETS_PER_IMAGE
            + COMPOSITE_DESCRIPTOR_SETS_PER_IMAGE
            + TRANSLUCENT_DESCRIPTOR_SETS_PER_IMAGE
            + graph.sets,
        combined_image_samplers: BLOOM_COMBINED_IMAGE_SAMPLERS_PER_IMAGE
            + COMPOSITE_COMBINED_IMAGE_SAMPLERS_PER_IMAGE
            + TRANSLUCENT_COMBINED_IMAGE_SAMPLERS_PER_IMAGE
            + graph.combined_image_samplers,
    }
}

fn create_post_descriptor_pool(
    device: &ash::Device,
    image_count: usize,
    plan_per_image: DescriptorAllocationPlan,
) -> Result<vk::DescriptorPool> {
    let max_sets = (image_count as u32) * plan_per_image.sets;
    let pool_sizes = [vk::DescriptorPoolSize {
        ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
        descriptor_count: (image_count as u32) * plan_per_image.combined_image_samplers,
    }];
    Ok(unsafe {
        device.create_descriptor_pool(
            &vk::DescriptorPoolCreateInfo::default()
                .max_sets(max_sets)
                .pool_sizes(&pool_sizes),
            None,
        )?
    })
}

fn merge_swapchain_state(state: SwapchainResourceState, len: usize) -> SwapchainResourceState {
    match (state, len) {
        (SwapchainResourceState::Partial, _) => SwapchainResourceState::Partial,
        (SwapchainResourceState::TornDown, 0) => SwapchainResourceState::TornDown,
        (SwapchainResourceState::TornDown, image_count) => {
            SwapchainResourceState::Ready { image_count }
        }
        (SwapchainResourceState::Ready { image_count }, len) if image_count == len => {
            SwapchainResourceState::Ready { image_count }
        }
        _ => SwapchainResourceState::Partial,
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
    ssao: Vec<OffscreenImage>,
    volumetrics: Vec<OffscreenImage>,
    bloom_a: Vec<OffscreenImage>,
    bloom_b: Vec<OffscreenImage>,

    /// Registered fullscreen effects that feed the fixed post
    /// stack. Bloom and final composite intentionally remain
    /// outside this graph because they are presentation-coupled.
    post_graph: PostGraph,

    pub scene_framebuffers: Vec<vk::Framebuffer>,
    pub translucent_framebuffers: Vec<vk::Framebuffer>,
    bright_framebuffers: Vec<vk::Framebuffer>,
    blur_h_framebuffers: Vec<vk::Framebuffer>,
    blur_v_framebuffers: Vec<vk::Framebuffer>,
    pub composite_framebuffers: Vec<vk::Framebuffer>,

    /// Default linear clamp sampler for colour-like post targets
    /// (HDR, bloom, volumetrics). If an effect needs custom
    /// filtering, bias, or mips, promote this to a small sampler
    /// table keyed by graph resource/effect.
    sampler: vk::Sampler,
    /// Nearest-neighbour sampler for depth reconstruction. Linear
    /// filtering across depth edges corrupts SSAO/soft-particle
    /// depth reads.
    depth_sampler: vk::Sampler,
    /// Cached depth view bound to every composite descriptor
    /// set. Single shared depth attachment, so one view is
    /// correct for every set.
    depth_view: vk::ImageView,

    // Descriptor plumbing. The bloom passes share a single
    // combined-image-sampler layout; graph nodes own their
    // effect-specific layouts; final composite reads the post
    // stack outputs in one pipeline.
    descriptor_pool: vk::DescriptorPool,
    single_set_layout: vk::DescriptorSetLayout,
    composite_set_layout: vk::DescriptorSetLayout,
    /// Descriptor set layout used by ribbon + particle shaders
    /// to read the scene depth buffer (binding 0,
    /// COMBINED_IMAGE_SAMPLER). One set per swapchain image,
    /// allocated in `translucent_in_sets`.
    pub translucent_set_layout: vk::DescriptorSetLayout,

    bright_in_sets: Vec<vk::DescriptorSet>, // bright reads HDR
    blur_h_in_sets: Vec<vk::DescriptorSet>, // blur_h reads bloom_a
    blur_v_in_sets: Vec<vk::DescriptorSet>, // blur_v reads bloom_b
    composite_in_sets: Vec<vk::DescriptorSet>, // composite reads post-stack outputs
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
        // Bloom runs at half resolution. The HDR bright pass +
        // the two separable Gaussian blurs are some of the most
        // fragment-heavy work in the frame; halving each
        // dimension cuts those three passes' pixel count to a
        // quarter, which is the single biggest fragment-bound
        // win available short of dropping bloom entirely. Visual
        // quality is essentially unchanged because bloom is by
        // construction a low-frequency blur — the LINEAR sampler
        // both downsamples (bright reads from full-res HDR) and
        // upsamples (composite reads from half-res bloom) for
        // free, with no aliasing on the eventual screen.
        let bloom_extent = vk::Extent2D {
            width: extent.width.max(2) / 2,
            height: extent.height.max(2) / 2,
        };
        let image_count = swapchain.image_views.len();

        let scene_pass = create_scene_pass(device)?;
        let translucent_pass = create_translucent_pass(device)?;
        let bloom_pass = create_bloom_pass(device)?;
        let composite_pass = create_composite_pass(device, swapchain.format.format)?;

        // ---- Offscreen images ----
        let mut hdr = Vec::with_capacity(image_count);
        let mut ssao = Vec::with_capacity(image_count);
        let mut volumetrics = Vec::with_capacity(image_count);
        let mut bloom_a = Vec::with_capacity(image_count);
        let mut bloom_b = Vec::with_capacity(image_count);
        for _ in 0..image_count {
            hdr.push(OffscreenImage::new(
                device, allocator, extent, HDR_FORMAT, "post_hdr",
            )?);
            ssao.push(OffscreenImage::new(
                device,
                allocator,
                extent,
                AO_FORMAT,
                "post_ssao",
            )?);
            volumetrics.push(OffscreenImage::new(
                device,
                allocator,
                bloom_extent,
                HDR_FORMAT,
                "post_volumetrics",
            )?);
            bloom_a.push(OffscreenImage::new(
                device,
                allocator,
                bloom_extent,
                HDR_FORMAT,
                "post_bloom_a",
            )?);
            bloom_b.push(OffscreenImage::new(
                device,
                allocator,
                bloom_extent,
                HDR_FORMAT,
                "post_bloom_b",
            )?);
        }

        // ---- Framebuffers ----
        let scene_framebuffers = create_fbs(
            device,
            scene_pass,
            extent,
            hdr.iter()
                .map(|h| [h.view, depth_view])
                .collect::<Vec<_>>()
                .as_slice(),
        )?;
        // Translucent pass shares the same hdr+depth framebuffer
        // pair — it loads what scene_pass stored.
        let translucent_framebuffers = create_fbs(
            device,
            translucent_pass,
            extent,
            hdr.iter()
                .map(|h| [h.view, depth_view])
                .collect::<Vec<_>>()
                .as_slice(),
        )?;
        let bright_framebuffers = create_fbs_single(
            device,
            bloom_pass,
            bloom_extent,
            &bloom_a.iter().map(|i| i.view).collect::<Vec<_>>(),
        )?;
        let blur_h_framebuffers = create_fbs_single(
            device,
            bloom_pass,
            bloom_extent,
            &bloom_b.iter().map(|i| i.view).collect::<Vec<_>>(),
        )?;
        let blur_v_framebuffers = create_fbs_single(
            device,
            bloom_pass,
            bloom_extent,
            &bloom_a.iter().map(|i| i.view).collect::<Vec<_>>(),
        )?;
        let composite_framebuffers =
            create_fbs_single(device, composite_pass, extent, &swapchain.image_views)?;

        // ---- Samplers ----
        // Baseline post policy: colour-like targets use linear
        // clamp, depth uses nearest clamp. More specialized
        // effects can extend `PostGraphSamplers` without changing
        // descriptor ownership.
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
            vk::DescriptorSetLayoutBinding::default()
                .binding(2)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            // Volumetric light target produced by the post graph.
            vk::DescriptorSetLayoutBinding::default()
                .binding(3)
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

        let mut post_graph = PostGraph::new(
            device,
            shader_dir,
            extent,
            PostGraphViews {
                ssao: &ssao.iter().map(|i| i.view).collect::<Vec<_>>(),
                volumetrics: &volumetrics.iter().map(|i| i.view).collect::<Vec<_>>(),
            },
            bloom_extent,
        )?;

        let descriptor_plan = post_descriptor_allocation_plan_per_image(&post_graph);
        let descriptor_pool = create_post_descriptor_pool(device, image_count, descriptor_plan)?;

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
        post_graph.allocate_descriptor_sets(device, descriptor_pool, image_count)?;

        // Wire descriptors → image views. Colour targets sample
        // in SHADER_READ_ONLY_OPTIMAL; depth targets use the
        // depth/stencil read-only layout while graph nodes sample
        // the depth attachment.
        for i in 0..image_count {
            write_combined(device, bright_in_sets[i], 0, hdr[i].view, sampler);
            write_combined(device, blur_h_in_sets[i], 0, bloom_a[i].view, sampler);
            write_combined(device, blur_v_in_sets[i], 0, bloom_b[i].view, sampler);
            write_combined(device, composite_in_sets[i], 0, hdr[i].view, sampler);
            write_combined(device, composite_in_sets[i], 1, bloom_a[i].view, sampler);
            write_combined(device, composite_in_sets[i], 2, ssao[i].view, sampler);
            write_combined(
                device,
                composite_in_sets[i],
                3,
                volumetrics[i].view,
                sampler,
            );
            post_graph.write_descriptors(
                device,
                i,
                PostGraphDescriptorViews {
                    hdr: hdr[i].view,
                    depth: depth_view,
                },
                PostGraphSamplers {
                    linear: sampler,
                    depth: depth_sampler,
                },
            );
            // Translucent set: same depth buffer, sampled by
            // ribbon/particle frag shaders for soft fade. The
            // descriptor's layout must match the image's actual
            // layout *while the translucent pass is recording*
            // — the subpass attachment ref keeps depth in
            // `DEPTH_SAMPLED_LAYOUT`, so the descriptor must use
            // the same.
            write_combined_with_layout(
                device,
                translucent_in_sets[i],
                0,
                depth_view,
                depth_sampler,
                DEPTH_SAMPLED_LAYOUT,
            );
        }

        // ---- Pipelines ----
        let (bright_pipeline, bright_layout) = build_post_pipeline(
            device,
            bloom_pass,
            shader_dir,
            "post.vert",
            "post_bright.frag",
            single_set_layout,
            bloom::BRIGHT_PUSH_SIZE,
        )?;
        let (blur_pipeline, blur_layout) = build_post_pipeline(
            device,
            bloom_pass,
            shader_dir,
            "post.vert",
            "post_blur.frag",
            single_set_layout,
            bloom::BLUR_PUSH_SIZE,
        )?;
        let (composite_pipeline, composite_layout) = build_post_pipeline(
            device,
            composite_pass,
            shader_dir,
            "post.vert",
            "post_composite.frag",
            composite_set_layout,
            composite::COMPOSITE_PUSH_SIZE,
        )?;

        Ok(Self {
            scene_pass,
            translucent_pass,
            bloom_pass,
            composite_pass,
            extent,
            bloom_extent,
            hdr,
            ssao,
            volumetrics,
            bloom_a,
            bloom_b,
            post_graph,
            scene_framebuffers,
            translucent_framebuffers,
            bright_framebuffers,
            blur_h_framebuffers,
            blur_v_framebuffers,
            composite_framebuffers,
            sampler,
            depth_sampler,
            depth_view,
            descriptor_pool,
            single_set_layout,
            composite_set_layout,
            translucent_set_layout,
            bright_in_sets,
            blur_h_in_sets,
            blur_v_in_sets,
            composite_in_sets,
            translucent_in_sets,
            bright_pipeline,
            bright_layout,
            blur_pipeline,
            blur_layout,
            composite_pipeline,
            composite_layout,
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
        debug_assert_ne!(
            self.swapchain_resource_state(),
            SwapchainResourceState::Partial,
            "post-processing swapchain resources are partially initialized before cleanup"
        );

        unsafe {
            for &fb in self
                .scene_framebuffers
                .iter()
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
        self.post_graph.cleanup_swapchain_dependent(device);

        for img in self.hdr.iter_mut() {
            img.cleanup(device, allocator);
        }
        for img in self.ssao.iter_mut() {
            img.cleanup(device, allocator);
        }
        for img in self.volumetrics.iter_mut() {
            img.cleanup(device, allocator);
        }
        for img in self.bloom_a.iter_mut() {
            img.cleanup(device, allocator);
        }
        for img in self.bloom_b.iter_mut() {
            img.cleanup(device, allocator);
        }
        self.hdr.clear();
        self.ssao.clear();
        self.volumetrics.clear();
        self.bloom_a.clear();
        self.bloom_b.clear();

        // Reset descriptor pool — frees all sets so we can
        // re-allocate against new image views.
        unsafe {
            device
                .reset_descriptor_pool(self.descriptor_pool, vk::DescriptorPoolResetFlags::empty())
                .ok();
        }
        self.bright_in_sets.clear();
        self.blur_h_in_sets.clear();
        self.blur_v_in_sets.clear();
        self.composite_in_sets.clear();
        self.translucent_in_sets.clear();

        debug_assert_eq!(
            self.swapchain_resource_state(),
            SwapchainResourceState::TornDown,
            "post-processing swapchain cleanup did not fully tear down resources"
        );
    }

    fn swapchain_resource_state(&self) -> SwapchainResourceState {
        let mut state = self.post_graph.swapchain_resource_state();
        for len in [
            self.scene_framebuffers.len(),
            self.translucent_framebuffers.len(),
            self.bright_framebuffers.len(),
            self.blur_h_framebuffers.len(),
            self.blur_v_framebuffers.len(),
            self.composite_framebuffers.len(),
            self.hdr.len(),
            self.ssao.len(),
            self.volumetrics.len(),
            self.bloom_a.len(),
            self.bloom_b.len(),
            self.bright_in_sets.len(),
            self.blur_h_in_sets.len(),
            self.blur_v_in_sets.len(),
            self.composite_in_sets.len(),
            self.translucent_in_sets.len(),
        ] {
            state = merge_swapchain_state(state, len);
            if state == SwapchainResourceState::Partial {
                break;
            }
        }
        state
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
        let image_count = swapchain.image_views.len();
        match self.swapchain_resource_state() {
            SwapchainResourceState::TornDown => {}
            SwapchainResourceState::Ready { .. } => {
                self.cleanup_swapchain_dependent(device, allocator);
            }
            SwapchainResourceState::Partial => {
                anyhow::bail!(
                    "post-processing swapchain resources are partially initialized before recreate"
                );
            }
        }

        self.extent = swapchain.extent;
        // See `new` for the rationale on half-res bloom; we have
        // to mirror that decision here so swapchain recreation
        // (window resize) keeps bloom at the right size.
        self.bloom_extent = vk::Extent2D {
            width: self.extent.width.max(2) / 2,
            height: self.extent.height.max(2) / 2,
        };
        // Cache the new depth view — the depth attachment is
        // recreated when the swapchain is, so the SSAO sampler
        // binding has to point at the new image.
        self.depth_view = depth_view;

        for _ in 0..image_count {
            self.hdr.push(OffscreenImage::new(
                device,
                allocator,
                self.extent,
                HDR_FORMAT,
                "post_hdr",
            )?);
            self.ssao.push(OffscreenImage::new(
                device,
                allocator,
                self.extent,
                AO_FORMAT,
                "post_ssao",
            )?);
            self.volumetrics.push(OffscreenImage::new(
                device,
                allocator,
                self.bloom_extent,
                HDR_FORMAT,
                "post_volumetrics",
            )?);
            self.bloom_a.push(OffscreenImage::new(
                device,
                allocator,
                self.bloom_extent,
                HDR_FORMAT,
                "post_bloom_a",
            )?);
            self.bloom_b.push(OffscreenImage::new(
                device,
                allocator,
                self.bloom_extent,
                HDR_FORMAT,
                "post_bloom_b",
            )?);
        }

        self.scene_framebuffers = create_fbs(
            device,
            self.scene_pass,
            self.extent,
            self.hdr
                .iter()
                .map(|h| [h.view, depth_view])
                .collect::<Vec<_>>()
                .as_slice(),
        )?;
        self.translucent_framebuffers = create_fbs(
            device,
            self.translucent_pass,
            self.extent,
            self.hdr
                .iter()
                .map(|h| [h.view, depth_view])
                .collect::<Vec<_>>()
                .as_slice(),
        )?;
        self.bright_framebuffers = create_fbs_single(
            device,
            self.bloom_pass,
            self.bloom_extent,
            &self.bloom_a.iter().map(|i| i.view).collect::<Vec<_>>(),
        )?;
        self.blur_h_framebuffers = create_fbs_single(
            device,
            self.bloom_pass,
            self.bloom_extent,
            &self.bloom_b.iter().map(|i| i.view).collect::<Vec<_>>(),
        )?;
        self.blur_v_framebuffers = create_fbs_single(
            device,
            self.bloom_pass,
            self.bloom_extent,
            &self.bloom_a.iter().map(|i| i.view).collect::<Vec<_>>(),
        )?;
        self.composite_framebuffers = create_fbs_single(
            device,
            self.composite_pass,
            self.extent,
            &swapchain.image_views,
        )?;
        self.post_graph.recreate_framebuffers(
            device,
            self.extent,
            PostGraphViews {
                ssao: &self.ssao.iter().map(|i| i.view).collect::<Vec<_>>(),
                volumetrics: &self.volumetrics.iter().map(|i| i.view).collect::<Vec<_>>(),
            },
            self.bloom_extent,
        )?;

        let single_layouts = vec![self.single_set_layout; image_count];
        let composite_layouts = vec![self.composite_set_layout; image_count];
        self.bright_in_sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(self.descriptor_pool)
                    .set_layouts(&single_layouts),
            )?
        };
        self.blur_h_in_sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(self.descriptor_pool)
                    .set_layouts(&single_layouts),
            )?
        };
        self.blur_v_in_sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(self.descriptor_pool)
                    .set_layouts(&single_layouts),
            )?
        };
        self.composite_in_sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(self.descriptor_pool)
                    .set_layouts(&composite_layouts),
            )?
        };
        let translucent_layouts = vec![self.translucent_set_layout; image_count];
        self.translucent_in_sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(self.descriptor_pool)
                    .set_layouts(&translucent_layouts),
            )?
        };
        self.post_graph
            .allocate_descriptor_sets(device, self.descriptor_pool, image_count)?;
        for i in 0..image_count {
            write_combined(
                device,
                self.bright_in_sets[i],
                0,
                self.hdr[i].view,
                self.sampler,
            );
            write_combined(
                device,
                self.blur_h_in_sets[i],
                0,
                self.bloom_a[i].view,
                self.sampler,
            );
            write_combined(
                device,
                self.blur_v_in_sets[i],
                0,
                self.bloom_b[i].view,
                self.sampler,
            );
            write_combined(
                device,
                self.composite_in_sets[i],
                0,
                self.hdr[i].view,
                self.sampler,
            );
            write_combined(
                device,
                self.composite_in_sets[i],
                1,
                self.bloom_a[i].view,
                self.sampler,
            );
            write_combined(
                device,
                self.composite_in_sets[i],
                2,
                self.ssao[i].view,
                self.sampler,
            );
            write_combined(
                device,
                self.composite_in_sets[i],
                3,
                self.volumetrics[i].view,
                self.sampler,
            );
            self.post_graph.write_descriptors(
                device,
                i,
                PostGraphDescriptorViews {
                    hdr: self.hdr[i].view,
                    depth: self.depth_view,
                },
                PostGraphSamplers {
                    linear: self.sampler,
                    depth: self.depth_sampler,
                },
            );
            write_combined_with_layout(
                device,
                self.translucent_in_sets[i],
                0,
                self.depth_view,
                self.depth_sampler,
                DEPTH_SAMPLED_LAYOUT,
            );
        }

        debug_assert_eq!(
            self.swapchain_resource_state(),
            SwapchainResourceState::Ready { image_count },
            "post-processing swapchain recreate produced inconsistent resource counts"
        );
        Ok(())
    }

    /// Record generic post graph nodes that run between the HDR
    /// scene and final composite. Each node writes an intermediate
    /// texture that the final composite samples cheaply.
    pub fn record_post_graph(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        image_index: u32,
        inv_proj: [[f32; 4]; 4],
        ssao_strength: f32,
        sun_screen: [f32; 4],
        sun_color: [f32; 4],
    ) {
        self.post_graph.record(
            device,
            cmd,
            image_index,
            self.extent,
            self.bloom_extent,
            PostGraphRecord {
                inv_proj,
                ssao_strength,
                sun_screen,
                sun_color,
            },
        );
    }

    pub fn set_post_node_enabled(&mut self, name: &str, enabled: bool) -> bool {
        self.post_graph.set_enabled(name, enabled)
    }

    /// Bright-pass + both blur passes. Caller has just ended the
    /// translucent render pass, so the HDR image is in
    /// SHADER_READ_ONLY_OPTIMAL layout. We run three back-to-back render passes;
    /// each pass's external dependencies handle the
    /// sampler-read → colour-write → sampler-read transitions.
    pub fn record_bloom(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        image_index: u32,
        config: &BloomConfig,
    ) {
        bloom::record(
            device,
            cmd,
            image_index,
            config,
            bloom::BloomRecordInfo {
                extent: self.bloom_extent,
                render_pass: self.bloom_pass,
                bright_framebuffers: &self.bright_framebuffers,
                blur_h_framebuffers: &self.blur_h_framebuffers,
                blur_v_framebuffers: &self.blur_v_framebuffers,
                bright_pipeline: self.bright_pipeline,
                bright_layout: self.bright_layout,
                blur_pipeline: self.blur_pipeline,
                blur_layout: self.blur_layout,
                bright_in_sets: &self.bright_in_sets,
                blur_h_in_sets: &self.blur_h_in_sets,
                blur_v_in_sets: &self.blur_v_in_sets,
            },
        );
    }

    /// Composite HDR + bloom + post-graph outputs into the swapchain. Caller is
    /// responsible for beginning the composite render pass (so
    /// that overlay drawing can happen in the same pass after
    /// us). Just records the fullscreen draw with proper
    /// pipeline / descriptors.
    ///
    /// `ghost_mix` in `[0.0, 1.0]` blends in the ghost-view
    /// post effect (desaturate-to-luma + cool tint + radial
    /// vignette). `0.0` is the default no-op.
    pub fn record_composite(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        image_index: u32,
        config: &BloomConfig,
        ghost_mix: f32,
    ) {
        let i = image_index as usize;
        composite::record(
            device,
            cmd,
            config,
            ghost_mix,
            composite::CompositeRecordInfo {
                extent: self.extent,
                pipeline: self.composite_pipeline,
                layout: self.composite_layout,
                descriptor_set: self.composite_in_sets[i],
            },
        );
    }

    /// Recompile the bright / blur / composite pipelines from
    /// the on-disk shader sources and atomically swap them in.
    /// Used by the editor hot-reload path so that edits to
    /// `post_ssao.frag`, `post_volumetrics.frag`,
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
    pub fn reload_pipelines(&mut self, device: &ash::Device, shader_dir: &Path) -> Result<()> {
        self.post_graph.reload_pipelines(device, shader_dir)?;
        let (new_bright, new_bright_layout) = build_post_pipeline(
            device,
            self.bloom_pass,
            shader_dir,
            "post.vert",
            "post_bright.frag",
            self.single_set_layout,
            bloom::BRIGHT_PUSH_SIZE,
        )?;
        let (new_blur, new_blur_layout) = build_post_pipeline(
            device,
            self.bloom_pass,
            shader_dir,
            "post.vert",
            "post_blur.frag",
            self.single_set_layout,
            bloom::BLUR_PUSH_SIZE,
        )?;
        let (new_composite, new_composite_layout) = build_post_pipeline(
            device,
            self.composite_pass,
            shader_dir,
            "post.vert",
            "post_composite.frag",
            self.composite_set_layout,
            composite::COMPOSITE_PUSH_SIZE,
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
        self.post_graph.cleanup(device);
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
