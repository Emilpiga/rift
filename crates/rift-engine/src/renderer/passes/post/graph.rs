use anyhow::Result;
use ash::vk;
use std::path::Path;

use super::resources::{
    build_post_pipeline, create_fbs_single, create_post_graph_pass, create_sampled_set_layout,
    write_combined, write_combined_with_layout,
};
use super::{AO_FORMAT, DEPTH_SAMPLED_LAYOUT, HDR_FORMAT};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PostResource {
    Hdr,
    Depth,
    Ssao,
    Volumetrics,
    Heat,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SsaoPush {
    inv_proj: [[f32; 4]; 4],
    strength: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}
unsafe impl bytemuck::Pod for SsaoPush {}
unsafe impl bytemuck::Zeroable for SsaoPush {}

#[repr(C)]
#[derive(Clone, Copy)]
struct VolumetricPush {
    sun_screen: [f32; 4],
    sun_color: [f32; 4],
}
unsafe impl bytemuck::Pod for VolumetricPush {}
unsafe impl bytemuck::Zeroable for VolumetricPush {}

#[repr(C)]
#[derive(Clone, Copy)]
struct HeatPush {
    heat_source: [f32; 4],
}
unsafe impl bytemuck::Pod for HeatPush {}
unsafe impl bytemuck::Zeroable for HeatPush {}

pub(super) struct PostGraphNode {
    render_pass: vk::RenderPass,
    framebuffers: Vec<vk::Framebuffer>,
    pub(super) descriptor_sets: Vec<vk::DescriptorSet>,
    pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    set_layout: vk::DescriptorSetLayout,
    frag_name: &'static str,
    push_size: u32,
}

impl PostGraphNode {
    fn new(
        device: &ash::Device,
        shader_dir: &Path,
        output_format: vk::Format,
        extent: vk::Extent2D,
        output_views: &[vk::ImageView],
        input_count: u32,
        frag_name: &'static str,
        push_size: u32,
    ) -> Result<Self> {
        let render_pass = create_post_graph_pass(device, output_format)?;
        let set_layout = create_sampled_set_layout(device, input_count)?;
        let framebuffers = create_fbs_single(device, render_pass, extent, output_views)?;
        let (pipeline, layout) = build_post_pipeline(
            device,
            render_pass,
            shader_dir,
            "post.vert",
            frag_name,
            set_layout,
            push_size,
        )?;
        Ok(Self {
            render_pass,
            framebuffers,
            descriptor_sets: Vec::new(),
            pipeline,
            layout,
            set_layout,
            frag_name,
            push_size,
        })
    }

    pub(super) fn allocate_descriptor_sets(
        &mut self,
        device: &ash::Device,
        descriptor_pool: vk::DescriptorPool,
        image_count: usize,
    ) -> Result<()> {
        let layouts = vec![self.set_layout; image_count];
        self.descriptor_sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(descriptor_pool)
                    .set_layouts(&layouts),
            )?
        };
        Ok(())
    }

    pub(super) fn recreate_framebuffers(
        &mut self,
        device: &ash::Device,
        extent: vk::Extent2D,
        output_views: &[vk::ImageView],
    ) -> Result<()> {
        self.framebuffers = create_fbs_single(device, self.render_pass, extent, output_views)?;
        Ok(())
    }

    fn record(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        image_index: u32,
        extent: vk::Extent2D,
        push_bytes: &[u8],
    ) {
        debug_assert_eq!(push_bytes.len(), self.push_size as usize);
        let i = image_index as usize;
        let area = vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent,
        };
        let viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: extent.width as f32,
            height: extent.height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        unsafe {
            let begin = vk::RenderPassBeginInfo::default()
                .render_pass(self.render_pass)
                .framebuffer(self.framebuffers[i])
                .render_area(area);
            device.cmd_begin_render_pass(cmd, &begin, vk::SubpassContents::INLINE);
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_set_viewport(cmd, 0, std::slice::from_ref(&viewport));
            device.cmd_set_scissor(cmd, 0, std::slice::from_ref(&area));
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.layout,
                0,
                std::slice::from_ref(&self.descriptor_sets[i]),
                &[],
            );
            if !push_bytes.is_empty() {
                device.cmd_push_constants(
                    cmd,
                    self.layout,
                    vk::ShaderStageFlags::FRAGMENT,
                    0,
                    push_bytes,
                );
            }
            device.cmd_draw(cmd, 3, 1, 0, 0);
            device.cmd_end_render_pass(cmd);
        }
    }

    pub(super) fn reload_pipeline(
        &mut self,
        device: &ash::Device,
        shader_dir: &Path,
    ) -> Result<()> {
        let (new_pipeline, new_layout) = build_post_pipeline(
            device,
            self.render_pass,
            shader_dir,
            "post.vert",
            self.frag_name,
            self.set_layout,
            self.push_size,
        )?;
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
        }
        self.pipeline = new_pipeline;
        self.layout = new_layout;
        Ok(())
    }

    pub(super) fn cleanup_swapchain_dependent(&mut self, device: &ash::Device) {
        unsafe {
            for &fb in &self.framebuffers {
                device.destroy_framebuffer(fb, None);
            }
        }
        self.framebuffers.clear();
        self.descriptor_sets.clear();
    }

    pub(super) fn cleanup(&mut self, device: &ash::Device) {
        self.cleanup_swapchain_dependent(device);
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_render_pass(self.render_pass, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SwapchainResourceState {
    TornDown,
    Ready { image_count: usize },
    Partial,
}

#[derive(Clone, Copy, Debug)]
enum PostNodeKind {
    Ssao,
    Volumetrics,
    Heat,
}

struct PostNode {
    name: &'static str,
    inputs: Vec<PostResource>,
    output: PostResource,
    enabled: bool,
    kind: PostNodeKind,
    node: PostGraphNode,
}

impl PostNode {
    fn new(
        device: &ash::Device,
        shader_dir: &Path,
        name: &'static str,
        inputs: Vec<PostResource>,
        output: PostResource,
        kind: PostNodeKind,
        output_format: vk::Format,
        extent: vk::Extent2D,
        output_views: &[vk::ImageView],
        frag_name: &'static str,
        push_size: u32,
    ) -> Result<Self> {
        let input_count = inputs.len() as u32;
        Ok(Self {
            name,
            inputs,
            output,
            enabled: true,
            kind,
            node: PostGraphNode::new(
                device,
                shader_dir,
                output_format,
                extent,
                output_views,
                input_count,
                frag_name,
                push_size,
            )?,
        })
    }
}

pub(super) struct PostGraphViews<'a> {
    pub(super) ssao: &'a [vk::ImageView],
    pub(super) volumetrics: &'a [vk::ImageView],
    pub(super) heat: &'a [vk::ImageView],
}

pub(super) struct PostGraphDescriptorViews {
    pub(super) hdr: vk::ImageView,
    pub(super) depth: vk::ImageView,
}

/// Current graph sampler policy is intentionally small: linear
/// clamp for colour resources, nearest clamp for depth. If heat,
/// volumetrics, or future graph nodes need custom filtering/mip
/// behavior, extend this into per-resource or per-node samplers.
pub(super) struct PostGraphSamplers {
    pub(super) linear: vk::Sampler,
    pub(super) depth: vk::Sampler,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct DescriptorAllocationPlan {
    pub(super) sets: u32,
    pub(super) combined_image_samplers: u32,
}

pub(super) struct PostGraph {
    nodes: Vec<PostNode>,
}

impl PostGraph {
    pub(super) fn new(
        device: &ash::Device,
        shader_dir: &Path,
        extent: vk::Extent2D,
        views: PostGraphViews<'_>,
        volumetric_extent: vk::Extent2D,
    ) -> Result<Self> {
        let mut graph = Self { nodes: Vec::new() };
        graph.add(PostNode::new(
            device,
            shader_dir,
            "ssao",
            vec![PostResource::Depth],
            PostResource::Ssao,
            PostNodeKind::Ssao,
            AO_FORMAT,
            extent,
            views.ssao,
            "post_ssao.frag",
            std::mem::size_of::<SsaoPush>() as u32,
        )?);
        graph.add(PostNode::new(
            device,
            shader_dir,
            "volumetrics",
            vec![PostResource::Hdr, PostResource::Depth],
            PostResource::Volumetrics,
            PostNodeKind::Volumetrics,
            HDR_FORMAT,
            volumetric_extent,
            views.volumetrics,
            "post_volumetrics.frag",
            std::mem::size_of::<VolumetricPush>() as u32,
        )?);
        graph.add(PostNode::new(
            device,
            shader_dir,
            "heat",
            vec![PostResource::Hdr],
            PostResource::Heat,
            PostNodeKind::Heat,
            HDR_FORMAT,
            extent,
            views.heat,
            "post_heat.frag",
            std::mem::size_of::<HeatPush>() as u32,
        )?);
        Ok(graph)
    }

    fn add(&mut self, node: PostNode) {
        self.nodes.push(node);
    }

    pub(super) fn write_descriptors(
        &self,
        device: &ash::Device,
        image_index: usize,
        views: PostGraphDescriptorViews,
        samplers: PostGraphSamplers,
    ) {
        for node in &self.nodes {
            let set = node.node.descriptor_sets[image_index];
            for (binding, input) in node.inputs.iter().enumerate() {
                let (view, sampler) = match input {
                    PostResource::Hdr => (views.hdr, samplers.linear),
                    PostResource::Depth => (views.depth, samplers.depth),
                    resource => panic!("post node {} cannot sample {resource:?}", node.name),
                };
                if *input == PostResource::Depth {
                    write_combined_with_layout(
                        device,
                        set,
                        binding as u32,
                        view,
                        sampler,
                        DEPTH_SAMPLED_LAYOUT,
                    );
                } else {
                    write_combined(device, set, binding as u32, view, sampler);
                }
            }
        }
    }

    pub(super) fn descriptor_allocation_plan_per_image(&self) -> DescriptorAllocationPlan {
        DescriptorAllocationPlan {
            sets: self.nodes.len() as u32,
            combined_image_samplers: self.nodes.iter().map(|node| node.inputs.len() as u32).sum(),
        }
    }

    fn output_views<'a>(
        output: PostResource,
        views: &'a PostGraphViews<'_>,
    ) -> &'a [vk::ImageView] {
        match output {
            PostResource::Ssao => views.ssao,
            PostResource::Volumetrics => views.volumetrics,
            PostResource::Heat => views.heat,
            resource => panic!("post node output cannot be {resource:?}"),
        }
    }

    fn output_extent(
        output: PostResource,
        extent: vk::Extent2D,
        volumetric_extent: vk::Extent2D,
    ) -> vk::Extent2D {
        match output {
            PostResource::Volumetrics => volumetric_extent,
            _ => extent,
        }
    }

    fn record_node(
        node: &PostNode,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        image_index: u32,
        extent: vk::Extent2D,
        volumetric_extent: vk::Extent2D,
        record: &PostGraphRecord,
    ) {
        let output_extent = Self::output_extent(node.output, extent, volumetric_extent);
        match node.kind {
            PostNodeKind::Ssao => {
                let push = SsaoPush {
                    inv_proj: record.inv_proj,
                    strength: record.ssao_strength.clamp(0.0, 1.0),
                    _pad0: 0.0,
                    _pad1: 0.0,
                    _pad2: 0.0,
                };
                node.node.record(
                    device,
                    cmd,
                    image_index,
                    output_extent,
                    bytemuck::bytes_of(&push),
                );
            }
            PostNodeKind::Volumetrics => {
                let push = VolumetricPush {
                    sun_screen: record.sun_screen,
                    sun_color: record.sun_color,
                };
                node.node.record(
                    device,
                    cmd,
                    image_index,
                    output_extent,
                    bytemuck::bytes_of(&push),
                );
            }
            PostNodeKind::Heat => {
                let push = HeatPush {
                    heat_source: record.heat_source,
                };
                node.node.record(
                    device,
                    cmd,
                    image_index,
                    output_extent,
                    bytemuck::bytes_of(&push),
                );
            }
        }
    }

    pub(super) fn set_enabled(&mut self, name: &str, enabled: bool) -> bool {
        if let Some(node) = self.nodes.iter_mut().find(|node| node.name == name) {
            node.enabled = enabled;
            true
        } else {
            false
        }
    }

    pub(super) fn allocate_descriptor_sets(
        &mut self,
        device: &ash::Device,
        descriptor_pool: vk::DescriptorPool,
        image_count: usize,
    ) -> Result<()> {
        for node in &mut self.nodes {
            node.node
                .allocate_descriptor_sets(device, descriptor_pool, image_count)?;
        }
        Ok(())
    }

    pub(super) fn recreate_framebuffers(
        &mut self,
        device: &ash::Device,
        extent: vk::Extent2D,
        views: PostGraphViews<'_>,
        volumetric_extent: vk::Extent2D,
    ) -> Result<()> {
        for node in &mut self.nodes {
            let output_extent = Self::output_extent(node.output, extent, volumetric_extent);
            let output_views = Self::output_views(node.output, &views);
            node.node
                .recreate_framebuffers(device, output_extent, output_views)?;
        }
        Ok(())
    }

    pub(super) fn record(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        image_index: u32,
        extent: vk::Extent2D,
        bloom_extent: vk::Extent2D,
        record: PostGraphRecord,
    ) {
        for node in &self.nodes {
            if node.enabled {
                Self::record_node(
                    node,
                    device,
                    cmd,
                    image_index,
                    extent,
                    bloom_extent,
                    &record,
                );
            }
        }
    }

    pub(super) fn reload_pipelines(
        &mut self,
        device: &ash::Device,
        shader_dir: &Path,
    ) -> Result<()> {
        for node in &mut self.nodes {
            node.node.reload_pipeline(device, shader_dir)?;
        }
        Ok(())
    }

    pub(super) fn cleanup_swapchain_dependent(&mut self, device: &ash::Device) {
        for node in &mut self.nodes {
            node.node.cleanup_swapchain_dependent(device);
        }
    }

    pub(super) fn swapchain_resource_state(&self) -> SwapchainResourceState {
        let mut expected = None;
        for node in &self.nodes {
            for len in [
                node.node.framebuffers.len(),
                node.node.descriptor_sets.len(),
            ] {
                match (expected, len) {
                    (None, 0) => {}
                    (None, n) => expected = Some(n),
                    (Some(expected), n) if expected == n => {}
                    _ => return SwapchainResourceState::Partial,
                }
            }
        }

        match expected {
            Some(image_count) => SwapchainResourceState::Ready { image_count },
            None => SwapchainResourceState::TornDown,
        }
    }

    pub(super) fn cleanup(&mut self, device: &ash::Device) {
        for node in &mut self.nodes {
            node.node.cleanup(device);
        }
    }
}

pub(super) struct PostGraphRecord {
    pub(super) inv_proj: [[f32; 4]; 4],
    pub(super) ssao_strength: f32,
    pub(super) sun_screen: [f32; 4],
    pub(super) sun_color: [f32; 4],
    pub(super) heat_source: [f32; 4],
}
