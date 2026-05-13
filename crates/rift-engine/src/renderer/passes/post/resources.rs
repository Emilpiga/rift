use anyhow::Result;
use ash::vk;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::hot_reload;
use crate::vulkan::pipeline as pipe;

pub(super) struct OffscreenImage {
    pub(super) image: vk::Image,
    pub(super) view: vk::ImageView,
    allocation: Option<Allocation>,
}

impl OffscreenImage {
    pub(super) fn new(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        extent: vk::Extent2D,
        format: vk::Format,
        name: &'static str,
    ) -> Result<Self> {
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width: extent.width.max(1),
                height: extent.height.max(1),
                depth: 1,
            })
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
        Ok(Self {
            image,
            view,
            allocation: Some(allocation),
        })
    }

    pub(super) fn cleanup(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        unsafe {
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
        }
        if let Some(a) = self.allocation.take() {
            allocator.lock().unwrap().free(a).ok();
        }
    }
}

pub(super) fn write_combined(
    device: &ash::Device,
    set: vk::DescriptorSet,
    binding: u32,
    view: vk::ImageView,
    sampler: vk::Sampler,
) {
    write_combined_with_layout(
        device,
        set,
        binding,
        view,
        sampler,
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
    );
}

pub(super) fn write_combined_with_layout(
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
    unsafe {
        device.update_descriptor_sets(std::slice::from_ref(&write), &[]);
    }
}

pub(super) fn create_sampled_set_layout(
    device: &ash::Device,
    input_count: u32,
) -> Result<vk::DescriptorSetLayout> {
    let bindings = (0..input_count)
        .map(|binding| {
            vk::DescriptorSetLayoutBinding::default()
                .binding(binding)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)
        })
        .collect::<Vec<_>>();
    Ok(unsafe {
        device.create_descriptor_set_layout(
            &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
            None,
        )?
    })
}

pub(super) fn create_fbs(
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

pub(super) fn create_fbs_single(
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

pub(super) fn create_post_graph_pass(
    device: &ash::Device,
    format: vk::Format,
) -> Result<vk::RenderPass> {
    let attachments = [vk::AttachmentDescription::default()
        .format(format)
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
    let info = vk::RenderPassCreateInfo::default()
        .attachments(&attachments)
        .subpasses(std::slice::from_ref(&subpass))
        .dependencies(&dependencies);
    Ok(unsafe { device.create_render_pass(&info, None)? })
}

pub(super) fn build_post_pipeline(
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
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vert_module)
            .name(entry),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(frag_module)
            .name(entry),
    ];
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic_state =
        vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
    let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .line_width(1.0)
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE);
    let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(false)
        .depth_write_enable(false)
        .stencil_test_enable(false);
    let blend_attachment = vk::PipelineColorBlendAttachmentState::default()
        .blend_enable(false)
        .color_write_mask(vk::ColorComponentFlags::RGBA);
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
        device
            .create_graphics_pipelines(vk::PipelineCache::null(), &[info], None)
            .map_err(|(_, e)| e)?[0]
    };
    unsafe {
        device.destroy_shader_module(vert_module, None);
        device.destroy_shader_module(frag_module, None);
    }
    Ok((pipeline, pipeline_layout))
}
