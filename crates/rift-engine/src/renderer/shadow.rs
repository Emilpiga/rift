//! Directional-light shadow mapping.
//!
//! Renders the scene from the light's POV into a depth-only image, which is
//! then sampled in the main fragment shader to darken occluded surfaces.
//!
//! Single cascade, fixed orthographic frustum following the camera. Tuned for
//! a top-down dungeon: 28 m × 28 m projection, 60 m near→far. The frustum is
//! deliberately tight around the play-area so the 2k depth buffer maps to
//! ~73 texels / world-meter, which is what the 12-tap Poisson PCF in
//! `triangle.frag::sampleShadow` relies on for soft-but-defined penumbras.

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

pub const SHADOW_MAP_SIZE: u32 = 4096;
pub const SHADOW_FORMAT: vk::Format = vk::Format::D32_SFLOAT;

/// Half-extent (in world units) of the orthographic light frustum. Larger
/// values cover more area but reduce shadow resolution per texel. The 16 m
/// half-extent below produces a 32×32 m projection, which at 4096² gives
/// ~128 texels/m (~8 mm/texel) — fine enough that a shadow falling on a
/// character's torso reads as a soft cohesive shape rather than a chunky
/// 2 cm staircase along the silhouette. The frustum still extends well
/// past the third-person camera's visible radius, and the edge feather
/// in `triangle.frag::sampleShadow` lands inside the fog band at any
/// camera angle so shadow despawn at the boundary stays invisible.
pub const SHADOW_ORTHO_HALF_EXTENT: f32 = 16.0;
/// Distance behind the focus point at which to place the light camera.
pub const SHADOW_BACK_DISTANCE: f32 = 30.0;
/// Near/far planes of the orthographic light frustum.
pub const SHADOW_NEAR: f32 = 0.1;
pub const SHADOW_FAR: f32 = 60.0;

pub struct ShadowMap {
    pub image: vk::Image,
    pub view: vk::ImageView,
    pub sampler: vk::Sampler,
    pub allocation: Option<Allocation>,
    pub render_pass: vk::RenderPass,
    pub framebuffer: vk::Framebuffer,
    pub pipeline: vk::Pipeline,
    pub pipeline_layout: vk::PipelineLayout,
}

impl ShadowMap {
    pub fn new(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        descriptor_set_layout: vk::DescriptorSetLayout,
        shader_dir: &Path,
    ) -> Result<Self> {
        // ---- Depth image (sampled + depth-stencil attachment) ----
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(SHADOW_FORMAT)
            .extent(vk::Extent3D {
                width: SHADOW_MAP_SIZE,
                height: SHADOW_MAP_SIZE,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let image = unsafe { device.create_image(&image_info, None)? };
        let reqs = unsafe { device.get_image_memory_requirements(image) };
        let allocation = allocator.lock().unwrap().allocate(&AllocationCreateDesc {
            name: "shadow_map",
            requirements: reqs,
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
            .format(SHADOW_FORMAT)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::DEPTH,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        let view = unsafe { device.create_image_view(&view_info, None)? };

        // Sampler with comparison + bilinear PCF.
        let sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_BORDER)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_BORDER)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_BORDER)
            .border_color(vk::BorderColor::FLOAT_OPAQUE_WHITE)
            .compare_enable(true)
            .compare_op(vk::CompareOp::LESS_OR_EQUAL)
            .anisotropy_enable(false)
            .max_lod(1.0);
        let sampler = unsafe { device.create_sampler(&sampler_info, None)? };

        // ---- Depth-only render pass ----
        let depth_attachment = vk::AttachmentDescription::default()
            .format(SHADOW_FORMAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        let depth_ref = vk::AttachmentReference {
            attachment: 0,
            layout: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
        };
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .depth_stencil_attachment(&depth_ref);
        // External -> shadow: ensure earlier frame's sampling completed.
        // shadow -> External: ensure depth writes finish before the main pass samples.
        let deps = [
            vk::SubpassDependency::default()
                .src_subpass(vk::SUBPASS_EXTERNAL)
                .dst_subpass(0)
                .src_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
                .src_access_mask(vk::AccessFlags::SHADER_READ)
                .dst_stage_mask(vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS)
                .dst_access_mask(vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE),
            vk::SubpassDependency::default()
                .src_subpass(0)
                .dst_subpass(vk::SUBPASS_EXTERNAL)
                .src_stage_mask(vk::PipelineStageFlags::LATE_FRAGMENT_TESTS)
                .src_access_mask(vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
                .dst_access_mask(vk::AccessFlags::SHADER_READ),
        ];
        let rp_info = vk::RenderPassCreateInfo::default()
            .attachments(std::slice::from_ref(&depth_attachment))
            .subpasses(std::slice::from_ref(&subpass))
            .dependencies(&deps);
        let render_pass = unsafe { device.create_render_pass(&rp_info, None)? };

        // ---- Framebuffer ----
        let attachments = [view];
        let fb_info = vk::FramebufferCreateInfo::default()
            .render_pass(render_pass)
            .attachments(&attachments)
            .width(SHADOW_MAP_SIZE)
            .height(SHADOW_MAP_SIZE)
            .layers(1);
        let framebuffer = unsafe { device.create_framebuffer(&fb_info, None)? };

        // ---- Pipeline ----
        let (pipeline, pipeline_layout) =
            create_shadow_pipeline(device, render_pass, descriptor_set_layout, shader_dir)?;

        Ok(Self {
            image,
            view,
            sampler,
            allocation: Some(allocation),
            render_pass,
            framebuffer,
            pipeline,
            pipeline_layout,
        })
    }

    /// Recompile the shadow pipeline (e.g. on hot-reload). The render pass
    /// and depth resources are unchanged.
    pub fn recreate_pipeline(
        &mut self,
        device: &ash::Device,
        descriptor_set_layout: vk::DescriptorSetLayout,
        shader_dir: &Path,
    ) -> Result<()> {
        let (pipeline, pipeline_layout) =
            create_shadow_pipeline(device, self.render_pass, descriptor_set_layout, shader_dir)?;
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
            device.destroy_framebuffer(self.framebuffer, None);
            device.destroy_render_pass(self.render_pass, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
        }
        if let Some(alloc) = self.allocation.take() {
            allocator.lock().unwrap().free(alloc).ok();
        }
    }
}

/// Build the directional-light view-projection matrix for the current frame.
///
/// The frustum follows `focus` (typically the camera or player position) and
/// looks along `light_dir` (the world-space *direction the light rays travel*,
/// e.g. `Vec3::new(-0.4, -1.0, -0.3).normalize()` for a downward-front sun).
///
/// Caller passes `light_dir_to` = the direction *toward* the light. Same
/// convention as the main UBO's `light_dir`.
pub fn light_view_proj(focus: Vec3, light_dir_to: Vec3) -> Mat4 {
    let l = light_dir_to.normalize_or_zero();
    let l = if l.length_squared() < 0.001 {
        Vec3::Y
    } else {
        l
    };
    // Snap focus to texel size to reduce shimmering as the camera moves.
    let texel_world = (2.0 * SHADOW_ORTHO_HALF_EXTENT) / SHADOW_MAP_SIZE as f32;
    let snap = |v: f32| (v / texel_world).round() * texel_world;
    let snapped = Vec3::new(snap(focus.x), snap(focus.y), snap(focus.z));

    let eye = snapped + l * SHADOW_BACK_DISTANCE;
    // Pick an up vector that isn't parallel to the light direction.
    let up = if l.y.abs() > 0.95 { Vec3::Z } else { Vec3::Y };
    let view = Mat4::look_at_rh(eye, snapped, up);
    let h = SHADOW_ORTHO_HALF_EXTENT;
    let proj = Mat4::orthographic_rh(-h, h, -h, h, SHADOW_NEAR, SHADOW_FAR);
    proj * view
}

fn create_shadow_pipeline(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    descriptor_set_layout: vk::DescriptorSetLayout,
    shader_dir: &Path,
) -> Result<(vk::Pipeline, vk::PipelineLayout)> {
    // Vertex shader only — the depth pass produces no color output.
    // Use the same Vertex layout as the main pipeline so we can reuse all
    // existing vertex buffers without conversion.
    let vert_path = shader_dir.join("shadow.vert");
    let vert_source = std::fs::read_to_string(&vert_path)
        .map_err(|e| anyhow::anyhow!("Failed to read {:?}: {}", vert_path, e))?;
    let vert_spv =
        hot_reload::compile_glsl(&vert_source, "shadow.vert", shaderc::ShaderKind::Vertex)?;
    let vert_module = pipe::create_shader_module(device, &vert_spv)?;

    let entry_name = c"main";
    let stages = [vk::PipelineShaderStageCreateInfo::default()
        .stage(vk::ShaderStageFlags::VERTEX)
        .module(vert_module)
        .name(entry_name)];

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
        width: SHADOW_MAP_SIZE as f32,
        height: SHADOW_MAP_SIZE as f32,
        min_depth: 0.0,
        max_depth: 1.0,
    };
    let scissor = vk::Rect2D {
        offset: vk::Offset2D { x: 0, y: 0 },
        extent: vk::Extent2D {
            width: SHADOW_MAP_SIZE,
            height: SHADOW_MAP_SIZE,
        },
    };
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewports(std::slice::from_ref(&viewport))
        .scissors(std::slice::from_ref(&scissor));

    // Front-face culling for shadow pass reduces self-shadow acne on
    // back-facing surfaces (a common rasterizer trick). Falls back fine if
    // some meshes have non-watertight topology — the depth bias below
    // catches the rest.
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

    // No color attachments — the shadow pass writes only depth.
    let color_blending = vk::PipelineColorBlendStateCreateInfo::default().attachments(&[]);

    let set_layouts = [descriptor_set_layout];
    let push_constant_range = vk::PushConstantRange {
        stage_flags: vk::ShaderStageFlags::VERTEX,
        offset: 0,
        size: 64, // Mat4
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
    }

    Ok((pipeline, pipeline_layout))
}
