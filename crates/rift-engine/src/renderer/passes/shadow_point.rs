//! Point-light shadow atlas (omnidirectional cube shadows).
//!
//! Single image of format `R32_SFLOAT`, dimensions `512 × 512 × (6 ×
//! MAX_POINT_SHADOWS)`, with `CUBE_COMPATIBLE` flags so it can be sampled
//! as a `samplerCubeArray` in the main fragment shader. Each face stores
//! a *linear distance* from the light, normalized to the light's radius
//! (so the atlas is independent of per-light units and PCF math is the
//! same for every light).
//!
//! Rendering: for each active point light, six render-pass invocations
//! (one per cube face) replay the visible scene depth-only into the
//! corresponding face's framebuffer. Per-face view-projection matrices
//! are precomputed CPU-side and shipped in the main UBO so the vertex
//! shader can index them by push-constant face slot. The fragment shader
//! writes `length(worldPos - lightPos) / radius` to the color attachment.
//!
//! Sampling (in `forward/shadow_sampling.glsl`): a plain `samplerCubeArray` (no
//! comparison sampler) returns the stored normalized distance for any
//! direction. The receiver computes its own normalized distance to the
//! light and compares with a small bias + 4-tap stochastic PCF for soft
//! contact shadows around chests, walls, and props.

use anyhow::Result;
use ash::vk;
use glam::{Mat4, Vec3};
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};
use gpu_allocator::MemoryLocation;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::hot_reload;
use crate::renderer::mesh::Vertex;
use crate::vulkan::pipeline as pipe;

/// Maximum number of simultaneous shadow-casting point lights. The atlas
/// allocates `6 * MAX_POINT_SHADOWS` cube-face layers, so increasing this
/// scales memory and worst-case render cost linearly. At 512² R32 + D32,
/// 8 lights uses ~48 MiB color + 1 MiB depth.
///
/// Sized to comfortably fit every torch within the typical dungeon
/// fog radius (~24 m × ~6 m torch spacing). Below this number the
/// closest-N selection in the renderer would otherwise pop shadows on
/// and off as the player walks past sconces.
pub const MAX_POINT_SHADOWS: usize = 8;

/// Per-face resolution of the cube atlas. 512² is the sweet
/// spot for our gameplay camera: each texel covers ~10 mm at 5 m
/// with the 90° per-face FoV, which is well below a screen pixel
/// at any reasonable display resolution. The single-tap PCF in
/// `samplePointShadow` plus the per-pixel basis-rotation jitter
/// hide the residual stepping at the silhouette, so visually the
/// 512² atlas is indistinguishable from 1024² but renders **4×
/// fewer fragments per cube face**. Skinned characters force-
/// re-render all 6 faces every frame for every shadow-casting
/// light they're in range of, so this is the single biggest win
/// for in-dungeon framerate.
///
/// Memory cost: 8 lights × 6 faces × 512² × 4 bytes ≈ 48 MiB
/// color + 1 MiB depth.
pub const POINT_SHADOW_SIZE: u32 = 512;

/// Color attachment format. R32_SFLOAT gives floating-point precision
/// for normalized distances and is universally supported as a color
/// attachment with `BLEND_OP::ADD` (which we don't use, but keeps it
/// trivially compatible across drivers).
pub const POINT_SHADOW_COLOR_FORMAT: vk::Format = vk::Format::R32_SFLOAT;

/// Depth attachment format used during the shadow pass. Reused across
/// all faces — we clear it at the start of each face render pass.
pub const POINT_SHADOW_DEPTH_FORMAT: vk::Format = vk::Format::D32_SFLOAT;

/// Near plane of each face's perspective projection. Matches the main
/// shadow pass.
pub const POINT_SHADOW_NEAR: f32 = 0.1;

pub struct PointShadowAtlas {
    /// Color image (linear normalized distance). Layered, cube-compatible.
    pub color_image: vk::Image,
    pub color_allocation: Option<Allocation>,
    /// Per-face 2D image views used as render-pass attachments. Indexed
    /// as `light_idx * 6 + face_idx`.
    pub face_views: Vec<vk::ImageView>,
    /// Cube-array view bound for sampling in the main fragment shader.
    pub cube_array_view: vk::ImageView,
    /// Sampler used in the main pass. Linear filtering, edge-clamp; no
    /// comparison op (we compare manually).
    pub sampler: vk::Sampler,

    /// Shared depth attachment for the shadow render pass. Cleared each
    /// face. One layer is sufficient because faces render sequentially.
    depth_image: vk::Image,
    depth_view: vk::ImageView,
    depth_allocation: Option<Allocation>,

    pub render_pass: vk::RenderPass,
    /// One framebuffer per face slot (`MAX_POINT_SHADOWS * 6`).
    pub framebuffers: Vec<vk::Framebuffer>,
    pub pipeline: vk::Pipeline,
    pub pipeline_layout: vk::PipelineLayout,
}

impl PointShadowAtlas {
    pub fn new(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        descriptor_set_layout: vk::DescriptorSetLayout,
        shader_dir: &Path,
    ) -> Result<Self> {
        let layer_count = (MAX_POINT_SHADOWS * 6) as u32;

        // ---- Color image (cube array, linear distance) ----
        let color_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(POINT_SHADOW_COLOR_FORMAT)
            .extent(vk::Extent3D {
                width: POINT_SHADOW_SIZE,
                height: POINT_SHADOW_SIZE,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(layer_count)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .flags(vk::ImageCreateFlags::CUBE_COMPATIBLE);
        let color_image = unsafe { device.create_image(&color_info, None)? };
        let reqs = unsafe { device.get_image_memory_requirements(color_image) };
        let color_allocation = allocator.lock().unwrap().allocate(&AllocationCreateDesc {
            name: "point_shadow_atlas_color",
            requirements: reqs,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })?;
        unsafe {
            device.bind_image_memory(
                color_image,
                color_allocation.memory(),
                color_allocation.offset(),
            )?;
        }

        // Per-face 2D views for use as framebuffer attachments. Each one
        // exposes a single layer of the cube-array image so the shadow
        // pass can render to that face directly.
        let mut face_views = Vec::with_capacity(layer_count as usize);
        for layer in 0..layer_count {
            let view_info = vk::ImageViewCreateInfo::default()
                .image(color_image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(POINT_SHADOW_COLOR_FORMAT)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: layer,
                    layer_count: 1,
                });
            face_views.push(unsafe { device.create_image_view(&view_info, None)? });
        }

        // Cube-array view exposing all `MAX_POINT_SHADOWS` cubes. Bound
        // in the main fragment shader as `samplerCubeArray`.
        let cube_array_info = vk::ImageViewCreateInfo::default()
            .image(color_image)
            .view_type(vk::ImageViewType::CUBE_ARRAY)
            .format(POINT_SHADOW_COLOR_FORMAT)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count,
            });
        let cube_array_view = unsafe { device.create_image_view(&cube_array_info, None)? };

        let sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .anisotropy_enable(false)
            .max_lod(1.0);
        let sampler = unsafe { device.create_sampler(&sampler_info, None)? };

        // ---- Depth image (single layer, shared across faces) ----
        let depth_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(POINT_SHADOW_DEPTH_FORMAT)
            .extent(vk::Extent3D {
                width: POINT_SHADOW_SIZE,
                height: POINT_SHADOW_SIZE,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let depth_image = unsafe { device.create_image(&depth_info, None)? };
        let dreqs = unsafe { device.get_image_memory_requirements(depth_image) };
        let depth_allocation = allocator.lock().unwrap().allocate(&AllocationCreateDesc {
            name: "point_shadow_atlas_depth",
            requirements: dreqs,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })?;
        unsafe {
            device.bind_image_memory(
                depth_image,
                depth_allocation.memory(),
                depth_allocation.offset(),
            )?;
        }
        let depth_view_info = vk::ImageViewCreateInfo::default()
            .image(depth_image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(POINT_SHADOW_DEPTH_FORMAT)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::DEPTH,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        let depth_view = unsafe { device.create_image_view(&depth_view_info, None)? };

        // ---- Render pass: color (linear distance) + depth ----
        let color_attachment = vk::AttachmentDescription::default()
            .format(POINT_SHADOW_COLOR_FORMAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        let depth_attachment = vk::AttachmentDescription::default()
            .format(POINT_SHADOW_DEPTH_FORMAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::DONT_CARE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        let attachments = [color_attachment, depth_attachment];
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
        let deps = [
            vk::SubpassDependency::default()
                .src_subpass(vk::SUBPASS_EXTERNAL)
                .dst_subpass(0)
                .src_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
                .src_access_mask(vk::AccessFlags::SHADER_READ)
                .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE),
            vk::SubpassDependency::default()
                .src_subpass(0)
                .dst_subpass(vk::SUBPASS_EXTERNAL)
                .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
                .dst_access_mask(vk::AccessFlags::SHADER_READ),
        ];
        let rp_info = vk::RenderPassCreateInfo::default()
            .attachments(&attachments)
            .subpasses(std::slice::from_ref(&subpass))
            .dependencies(&deps);
        let render_pass = unsafe { device.create_render_pass(&rp_info, None)? };

        // ---- Framebuffers (one per face slot) ----
        let mut framebuffers = Vec::with_capacity(face_views.len());
        for &face_view in &face_views {
            let attach = [face_view, depth_view];
            let fb_info = vk::FramebufferCreateInfo::default()
                .render_pass(render_pass)
                .attachments(&attach)
                .width(POINT_SHADOW_SIZE)
                .height(POINT_SHADOW_SIZE)
                .layers(1);
            framebuffers.push(unsafe { device.create_framebuffer(&fb_info, None)? });
        }

        // ---- Pipeline ----
        let (pipeline, pipeline_layout) =
            create_point_shadow_pipeline(device, render_pass, descriptor_set_layout, shader_dir)?;

        Ok(Self {
            color_image,
            color_allocation: Some(color_allocation),
            face_views,
            cube_array_view,
            sampler,
            depth_image,
            depth_view,
            depth_allocation: Some(depth_allocation),
            render_pass,
            framebuffers,
            pipeline,
            pipeline_layout,
        })
    }

    pub fn recreate_pipeline(
        &mut self,
        device: &ash::Device,
        descriptor_set_layout: vk::DescriptorSetLayout,
        shader_dir: &Path,
    ) -> Result<()> {
        let (pipeline, pipeline_layout) = create_point_shadow_pipeline(
            device,
            self.render_pass,
            descriptor_set_layout,
            shader_dir,
        )?;
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
        }
        self.pipeline = pipeline;
        self.pipeline_layout = pipeline_layout;
        Ok(())
    }

    pub fn cleanup(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            for fb in self.framebuffers.drain(..) {
                device.destroy_framebuffer(fb, None);
            }
            device.destroy_render_pass(self.render_pass, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.cube_array_view, None);
            for v in self.face_views.drain(..) {
                device.destroy_image_view(v, None);
            }
            device.destroy_image_view(self.depth_view, None);
            device.destroy_image(self.depth_image, None);
            device.destroy_image(self.color_image, None);
        }
        if let Some(alloc) = self.color_allocation.take() {
            allocator.lock().unwrap().free(alloc).ok();
        }
        if let Some(alloc) = self.depth_allocation.take() {
            allocator.lock().unwrap().free(alloc).ok();
        }
    }
}

/// Compute the six per-face view-projection matrices for a point light at
/// `light_pos` with effective range `radius`. Returned in the canonical
/// Vulkan cube-face order: +X, -X, +Y, -Y, +Z, -Z. Caller should write
/// these into UBO slots `[6*light_slot .. 6*light_slot + 6]`.
///
/// Each face uses a 90° perspective with `near = POINT_SHADOW_NEAR` and
/// `far = radius`, matching the normalization performed by the shadow
/// fragment shader (which divides world-space distance by the same
/// radius). The look-at "up" vectors follow the GL/Vulkan cube-map
/// convention; getting them wrong manifests as shadows mirrored across
/// the wrong axis on one face only, so they are documented inline.
pub fn cube_face_view_projs(light_pos: Vec3, radius: f32) -> [Mat4; 6] {
    // 90° vertical FoV, square aspect — covers exactly one cube face.
    // We do *not* apply the Vulkan Y-flip we use for the main camera
    // here: cube maps are sampled with directions, not framebuffer UVs,
    // and the GLSL cube-array sampler expects the GL-convention face
    // orientation. The look-at "up" vectors below already encode the
    // expected face orientation, so a plain RH perspective is correct.
    let proj = Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1.0, POINT_SHADOW_NEAR, radius);

    // Vulkan/GL cube-face orientations. Each pair = (target offset from
    // eye, up vector). These match the conventions documented in the
    // Vulkan spec section "Cube Map Face Selection" so a direction `D`
    // sampled via `samplerCubeArray` resolves to the same face we
    // rendered into.
    let faces = [
        (Vec3::X, -Vec3::Y),  // +X
        (-Vec3::X, -Vec3::Y), // -X
        (Vec3::Y, Vec3::Z),   // +Y
        (-Vec3::Y, -Vec3::Z), // -Y
        (Vec3::Z, -Vec3::Y),  // +Z
        (-Vec3::Z, -Vec3::Y), // -Z
    ];

    let mut result = [Mat4::IDENTITY; 6];
    for (i, (dir, up)) in faces.iter().enumerate() {
        let view = Mat4::look_at_rh(light_pos, light_pos + *dir, *up);
        result[i] = proj * view;
    }
    result
}

fn create_point_shadow_pipeline(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    descriptor_set_layout: vk::DescriptorSetLayout,
    shader_dir: &Path,
) -> Result<(vk::Pipeline, vk::PipelineLayout)> {
    let vert_path = shader_dir.join("shadow_point.vert");
    let vert_source = std::fs::read_to_string(&vert_path)
        .map_err(|e| anyhow::anyhow!("Failed to read {:?}: {}", vert_path, e))?;
    let vert_spv = hot_reload::compile_glsl(
        &vert_source,
        "shadow_point.vert",
        shaderc::ShaderKind::Vertex,
    )?;
    let vert_module = pipe::create_shader_module(device, &vert_spv)?;

    let frag_path = shader_dir.join("shadow_point.frag");
    let frag_source = std::fs::read_to_string(&frag_path)
        .map_err(|e| anyhow::anyhow!("Failed to read {:?}: {}", frag_path, e))?;
    let frag_spv = hot_reload::compile_glsl(
        &frag_source,
        "shadow_point.frag",
        shaderc::ShaderKind::Fragment,
    )?;
    let frag_module = pipe::create_shader_module(device, &frag_spv)?;

    let entry_name = c"main";
    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vert_module)
            .name(entry_name),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(frag_module)
            .name(entry_name),
    ];

    let binding_desc = [Vertex::binding_description()];
    let attr_descs = Vertex::attribute_descriptions();
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
        .vertex_binding_descriptions(&binding_desc)
        .vertex_attribute_descriptions(&attr_descs);

    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);

    let viewport = vk::Viewport {
        x: 0.0,
        y: 0.0,
        width: POINT_SHADOW_SIZE as f32,
        height: POINT_SHADOW_SIZE as f32,
        min_depth: 0.0,
        max_depth: 1.0,
    };
    let scissor = vk::Rect2D {
        offset: vk::Offset2D { x: 0, y: 0 },
        extent: vk::Extent2D {
            width: POINT_SHADOW_SIZE,
            height: POINT_SHADOW_SIZE,
        },
    };
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewports(std::slice::from_ref(&viewport))
        .scissors(std::slice::from_ref(&scissor));

    // Front-face culling: same self-shadow-acne mitigation as the
    // directional pass. The depth bias adds a small constant + slope-
    // scaled offset to catch grazing geometry. Bias values tuned for
    // 90° perspective, near=0.1, far≈8m typical torch radius.
    let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .line_width(1.0)
        .cull_mode(vk::CullModeFlags::FRONT)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .depth_bias_enable(true)
        .depth_bias_constant_factor(1.5)
        .depth_bias_slope_factor(2.0);

    let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);

    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(true)
        .depth_write_enable(true)
        .depth_compare_op(vk::CompareOp::LESS)
        .depth_bounds_test_enable(false)
        .stencil_test_enable(false);

    // Single color attachment, no blend (we want the closest fragment's
    // distance, which depth test already enforces).
    let blend_attachment = vk::PipelineColorBlendAttachmentState::default()
        .color_write_mask(vk::ColorComponentFlags::R)
        .blend_enable(false);
    let color_blending = vk::PipelineColorBlendStateCreateInfo::default()
        .attachments(std::slice::from_ref(&blend_attachment));

    let set_layouts = [descriptor_set_layout];
    // Push: mat4 model (64) + uvec4 indices (16) = 80 bytes. Indices.x
    // is the global face slot (0 .. MAX_POINT_SHADOWS*6) used by the
    // vertex shader to fetch the right face VP from the UBO; indices.y
    // is the light index used by the fragment shader to fetch the
    // corresponding light position+radius.
    let push_constant_range = vk::PushConstantRange {
        stage_flags: vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
        offset: 0,
        size: 80,
    };
    let layout_info = vk::PipelineLayoutCreateInfo::default()
        .set_layouts(&set_layouts)
        .push_constant_ranges(std::slice::from_ref(&push_constant_range));
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
