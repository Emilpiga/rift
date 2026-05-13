use ash::vk;

use super::BloomConfig;

#[repr(C)]
#[derive(Clone, Copy)]
struct CompositePush {
    bloom_intensity: f32,
    exposure: f32,
    ghost_mix: f32,
    _pad0: f32,
}
unsafe impl bytemuck::Pod for CompositePush {}
unsafe impl bytemuck::Zeroable for CompositePush {}

pub(super) const COMPOSITE_PUSH_SIZE: u32 = std::mem::size_of::<CompositePush>() as u32;

pub(super) struct CompositeRecordInfo {
    pub(super) extent: vk::Extent2D,
    pub(super) pipeline: vk::Pipeline,
    pub(super) layout: vk::PipelineLayout,
    pub(super) descriptor_set: vk::DescriptorSet,
}

pub(super) fn record(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    config: &BloomConfig,
    ghost_mix: f32,
    info: CompositeRecordInfo,
) {
    let viewport = vk::Viewport {
        x: 0.0,
        y: 0.0,
        width: info.extent.width as f32,
        height: info.extent.height as f32,
        min_depth: 0.0,
        max_depth: 1.0,
    };
    let scissor = vk::Rect2D {
        offset: vk::Offset2D::default(),
        extent: info.extent,
    };
    unsafe {
        device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, info.pipeline);
        device.cmd_set_viewport(cmd, 0, std::slice::from_ref(&viewport));
        device.cmd_set_scissor(cmd, 0, std::slice::from_ref(&scissor));
        device.cmd_bind_descriptor_sets(
            cmd,
            vk::PipelineBindPoint::GRAPHICS,
            info.layout,
            0,
            std::slice::from_ref(&info.descriptor_set),
            &[],
        );
        let push = CompositePush {
            bloom_intensity: config.intensity,
            exposure: config.exposure,
            ghost_mix: ghost_mix.clamp(0.0, 1.0),
            _pad0: 0.0,
        };
        device.cmd_push_constants(
            cmd,
            info.layout,
            vk::ShaderStageFlags::FRAGMENT,
            0,
            bytemuck::bytes_of(&push),
        );
        device.cmd_draw(cmd, 3, 1, 0, 0);
    }
}
