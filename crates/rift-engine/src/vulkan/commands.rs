use anyhow::Result;
use ash::{vk, Device};
use glam::Mat4;

pub fn create_command_pool(device: &Device, queue_family: u32) -> Result<vk::CommandPool> {
    let pool_info = vk::CommandPoolCreateInfo::default()
        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
        .queue_family_index(queue_family);
    let pool = unsafe { device.create_command_pool(&pool_info, None)? };
    Ok(pool)
}

pub fn allocate_command_buffers(
    device: &Device,
    pool: vk::CommandPool,
    count: u32,
) -> Result<Vec<vk::CommandBuffer>> {
    let alloc_info = vk::CommandBufferAllocateInfo::default()
        .command_pool(pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(count);
    let buffers = unsafe { device.allocate_command_buffers(&alloc_info)? };
    Ok(buffers)
}

#[derive(Clone)]
pub struct DrawCommand {
    pub vertex_buffer: vk::Buffer,
    pub index_buffer: vk::Buffer,
    pub index_count: u32,
    pub descriptor_set: vk::DescriptorSet,
    /// Per-object material descriptor set (set=1). Bound *after* the
    /// uniform set.
    pub material_set: vk::DescriptorSet,
    pub model_matrix: Mat4,
    /// World-space bounding-sphere radius copied from the source
    /// `RenderObject` after frustum culling. Carried through here
    /// so the shadow passes (which iterate `&draws` rather than
    /// `&objects`) can run their own per-light spatial cull
    /// without having to look anything up on the renderer.
    pub bounds_radius: f32,
    /// RGBA tint pushed at offset 64 in the vertex push-constant
    /// range. RGB multiplies the lit fragment colour, A is the
    /// output alpha. `[1.0; 4]` is the default no-op opaque
    /// path; ghost-mode avatars push a pale cyan-white with
    /// reduced alpha.
    pub tint: [f32; 4],
    /// Per-object PBR / sampling tweaks pushed at offset 80.
    /// Layout: `(uv_scale, parallax_scale, flags, _reserved)`.
    /// `flags` bit 0 = enable PBR + normal mapping (otherwise the
    /// shader stays on the legacy cel-shaded diffuse path).
    pub material_params: [f32; 4],
    /// True when this draw's vertex buffer contents are
    /// regenerated every frame (CPU dynamic ring or GPU
    /// skinning compute output). The model matrix and
    /// bounding sphere stay constant for these objects but
    /// their silhouette can change every frame, so the
    /// shadow-slot cache must treat them as always-dirty.
    pub dynamic_vertices: bool,
}

pub fn record_render_pass(
    device: &Device,
    command_buffer: vk::CommandBuffer,
    render_pass: vk::RenderPass,
    framebuffer: vk::Framebuffer,
    extent: vk::Extent2D,
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    draws: &[DrawCommand],
) -> Result<()> {
    let begin_info = vk::CommandBufferBeginInfo::default();
    unsafe { device.begin_command_buffer(command_buffer, &begin_info)? };

    let clear_values = [
        vk::ClearValue {
            color: vk::ClearColorValue {
                float32: [0.01, 0.01, 0.02, 1.0],
            },
        },
        vk::ClearValue {
            depth_stencil: vk::ClearDepthStencilValue {
                depth: 1.0,
                stencil: 0,
            },
        },
    ];

    let render_pass_begin = vk::RenderPassBeginInfo::default()
        .render_pass(render_pass)
        .framebuffer(framebuffer)
        .render_area(vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent,
        })
        .clear_values(&clear_values);

    unsafe {
        device.cmd_begin_render_pass(
            command_buffer,
            &render_pass_begin,
            vk::SubpassContents::INLINE,
        );
        device.cmd_bind_pipeline(command_buffer, vk::PipelineBindPoint::GRAPHICS, pipeline);

        for draw in draws {
            device.cmd_bind_vertex_buffers(command_buffer, 0, &[draw.vertex_buffer], &[0]);
            device.cmd_bind_index_buffer(
                command_buffer,
                draw.index_buffer,
                0,
                vk::IndexType::UINT32,
            );
            device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline_layout,
                0,
                &[draw.descriptor_set],
                &[],
            );

            // Push model matrix (offset 0) + tint (offset 64).
            // Both stages can see them — vert uses model, frag
            // uses tint.
            let model_bytes: &[u8] = bytemuck::bytes_of(&draw.model_matrix);
            device.cmd_push_constants(
                command_buffer,
                pipeline_layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                0,
                model_bytes,
            );
            let tint_bytes: &[u8] = bytemuck::bytes_of(&draw.tint);
            device.cmd_push_constants(
                command_buffer,
                pipeline_layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                64,
                tint_bytes,
            );
            let mp_bytes: &[u8] = bytemuck::bytes_of(&draw.material_params);
            device.cmd_push_constants(
                command_buffer,
                pipeline_layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                80,
                mp_bytes,
            );

            device.cmd_draw_indexed(command_buffer, draw.index_count, 1, 0, 0, 0);
        }

        device.cmd_end_render_pass(command_buffer);
        device.end_command_buffer(command_buffer)?;
    }

    Ok(())
}
