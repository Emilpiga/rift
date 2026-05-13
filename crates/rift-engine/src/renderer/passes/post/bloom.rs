use ash::vk;

use super::BloomConfig;

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

pub(super) const BRIGHT_PUSH_SIZE: u32 = std::mem::size_of::<BrightPush>() as u32;
pub(super) const BLUR_PUSH_SIZE: u32 = std::mem::size_of::<BlurPush>() as u32;

pub(super) struct BloomRecordInfo<'a> {
    pub(super) extent: vk::Extent2D,
    pub(super) render_pass: vk::RenderPass,
    pub(super) bright_framebuffers: &'a [vk::Framebuffer],
    pub(super) blur_h_framebuffers: &'a [vk::Framebuffer],
    pub(super) blur_v_framebuffers: &'a [vk::Framebuffer],
    pub(super) bright_pipeline: vk::Pipeline,
    pub(super) bright_layout: vk::PipelineLayout,
    pub(super) blur_pipeline: vk::Pipeline,
    pub(super) blur_layout: vk::PipelineLayout,
    pub(super) bright_in_sets: &'a [vk::DescriptorSet],
    pub(super) blur_h_in_sets: &'a [vk::DescriptorSet],
    pub(super) blur_v_in_sets: &'a [vk::DescriptorSet],
}

pub(super) fn record(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    image_index: u32,
    config: &BloomConfig,
    info: BloomRecordInfo<'_>,
) {
    let i = image_index as usize;
    let bloom_area = vk::Rect2D {
        offset: vk::Offset2D::default(),
        extent: info.extent,
    };
    let viewport = vk::Viewport {
        x: 0.0,
        y: 0.0,
        width: info.extent.width as f32,
        height: info.extent.height as f32,
        min_depth: 0.0,
        max_depth: 1.0,
    };

    unsafe {
        let begin = vk::RenderPassBeginInfo::default()
            .render_pass(info.render_pass)
            .framebuffer(info.bright_framebuffers[i])
            .render_area(bloom_area);
        device.cmd_begin_render_pass(cmd, &begin, vk::SubpassContents::INLINE);
        device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, info.bright_pipeline);
        device.cmd_set_viewport(cmd, 0, std::slice::from_ref(&viewport));
        device.cmd_set_scissor(cmd, 0, std::slice::from_ref(&bloom_area));
        device.cmd_bind_descriptor_sets(
            cmd,
            vk::PipelineBindPoint::GRAPHICS,
            info.bright_layout,
            0,
            std::slice::from_ref(&info.bright_in_sets[i]),
            &[],
        );
        let push = BrightPush {
            threshold: config.threshold,
            soft_knee: config.soft_knee,
            _pad0: 0.0,
            _pad1: 0.0,
        };
        device.cmd_push_constants(
            cmd,
            info.bright_layout,
            vk::ShaderStageFlags::FRAGMENT,
            0,
            bytemuck::bytes_of(&push),
        );
        device.cmd_draw(cmd, 3, 1, 0, 0);
        device.cmd_end_render_pass(cmd);
    }

    let texel = [
        1.0 / info.extent.width as f32,
        1.0 / info.extent.height as f32,
    ];

    record_blur(
        device,
        cmd,
        &info,
        info.blur_h_framebuffers[i],
        info.blur_h_in_sets[i],
        texel,
        [1.0, 0.0],
    );
    record_blur(
        device,
        cmd,
        &info,
        info.blur_v_framebuffers[i],
        info.blur_v_in_sets[i],
        texel,
        [0.0, 1.0],
    );
}

fn record_blur(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    info: &BloomRecordInfo<'_>,
    framebuffer: vk::Framebuffer,
    descriptor_set: vk::DescriptorSet,
    texel: [f32; 2],
    direction: [f32; 2],
) {
    let bloom_area = vk::Rect2D {
        offset: vk::Offset2D::default(),
        extent: info.extent,
    };
    let viewport = vk::Viewport {
        x: 0.0,
        y: 0.0,
        width: info.extent.width as f32,
        height: info.extent.height as f32,
        min_depth: 0.0,
        max_depth: 1.0,
    };
    unsafe {
        let begin = vk::RenderPassBeginInfo::default()
            .render_pass(info.render_pass)
            .framebuffer(framebuffer)
            .render_area(bloom_area);
        device.cmd_begin_render_pass(cmd, &begin, vk::SubpassContents::INLINE);
        device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, info.blur_pipeline);
        device.cmd_set_viewport(cmd, 0, std::slice::from_ref(&viewport));
        device.cmd_set_scissor(cmd, 0, std::slice::from_ref(&bloom_area));
        device.cmd_bind_descriptor_sets(
            cmd,
            vk::PipelineBindPoint::GRAPHICS,
            info.blur_layout,
            0,
            std::slice::from_ref(&descriptor_set),
            &[],
        );
        let push = BlurPush {
            texel_size: texel,
            direction,
        };
        device.cmd_push_constants(
            cmd,
            info.blur_layout,
            vk::ShaderStageFlags::FRAGMENT,
            0,
            bytemuck::bytes_of(&push),
        );
        device.cmd_draw(cmd, 3, 1, 0, 0);
        device.cmd_end_render_pass(cmd);
    }
}
