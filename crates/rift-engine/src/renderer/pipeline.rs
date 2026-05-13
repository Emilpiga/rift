//! Shader / pipeline lifecycle: startup default-resource provisioning,
//! shader hot-reload, and on-disk GLSL → SPIR-V → graphics-pipeline
//! compilation for the forward / scene pass.

use anyhow::Result;
use ash::vk;
use gpu_allocator::vulkan::Allocator;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::hot_reload;
use crate::renderer::blood;
use crate::renderer::forward::Renderer;
use crate::renderer::material::MaterialPool;
use crate::renderer::texture::Texture;
use crate::renderer::uniform::UniformBuffers;
use crate::vulkan::{VulkanDevice, commands, pipeline};

impl Renderer {
    /// Materialise the four placeholder/default GPU resources the
    /// forward pipeline needs bound at startup:
    ///
    ///   * `default_texture` — a 32×32 magenta checkerboard bound to
    ///     set 0 / binding 1 so any object that hasn't received a
    ///     real albedo yet still rasterises (and the missing texture
    ///     is visually obvious during dev).
    ///   * `default_blood_field` — a 1×1 R16G16_SFLOAT zero texture
    ///     bound to set 0 / binding 4 so the forward shader's
    ///     `wet * intensity` term collapses to zero until a real
    ///     floor calls `recreate_blood_field`.
    ///   * `material_pool` — set-1 material descriptor pool plus its
    ///     own 1×1 white default albedo (uploaded via the same
    ///     command pool, then ready for `set_object_shared_material`
    ///     calls).
    ///   * `blood_field` — the per-floor blood splat subsystem,
    ///     constructed in its inactive (zero `world_xform`) state.
    ///     Owns its own render pass + splat pipeline + procgen
    ///     mask atlas; the pass is a no-op until a floor binds.
    ///
    /// All four uploads share a single throwaway command pool that
    /// is created and destroyed inside this helper — keeping the
    /// `new` orchestrator free of one-shot Vulkan plumbing.
    pub(super) fn init_default_resources(
        device: &VulkanDevice,
        allocator: &Arc<Mutex<Allocator>>,
        shader_dir: &std::path::Path,
        uniforms: &UniformBuffers,
    ) -> Result<(Texture, Texture, MaterialPool, blood::BloodField)> {
        let command_pool_init =
            commands::create_command_pool(&device.device, device.graphics_queue_family)?;

        let default_texture = Texture::checkerboard(
            &device.device,
            allocator,
            device.graphics_queue,
            command_pool_init,
        )?;
        uniforms.bind_texture(
            &device.device,
            default_texture.view,
            default_texture.sampler,
        );

        // 1×1 zero-valued R16G16_SFLOAT texture (two 16-bit floats
        // encoded as zero = four zero bytes → wet=0, age=0).
        let default_blood_field = Texture::from_rgba_with_format(
            &device.device,
            allocator,
            device.graphics_queue,
            command_pool_init,
            1,
            1,
            &[0u8; 4],
            vk::Format::R16G16_SFLOAT,
        )?;
        uniforms.bind_blood_field(
            &device.device,
            default_blood_field.view,
            default_blood_field.sampler,
        );

        let material_pool = MaterialPool::new(
            &device.device,
            allocator,
            device.graphics_queue,
            command_pool_init,
        )?;

        let blood_field = blood::BloodField::new(
            &device.device,
            allocator,
            device.graphics_queue,
            command_pool_init,
            shader_dir,
        )?;

        unsafe {
            device.device.destroy_command_pool(command_pool_init, None);
        }

        Ok((
            default_texture,
            default_blood_field,
            material_pool,
            blood_field,
        ))
    }

    /// Check for shader changes and hot-reload the pipeline if needed.
    pub fn check_hot_reload(&mut self) {
        let should_reload = self
            .hot_reloader
            .as_ref()
            .map(|hr| hr.check_and_reset())
            .unwrap_or(false);

        if should_reload {
            unsafe {
                self.device.device.device_wait_idle().ok();
            }

            match Self::compile_pipeline_from_disk(
                &self.device.device,
                self.post.scene_pass,
                self.swapchain.extent,
                &[
                    self.uniforms.descriptor_set_layout,
                    self.material_pool.layout,
                ],
                &self.shader_dir,
            ) {
                Ok((new_pipeline, new_layout)) => {
                    unsafe {
                        self.device.device.destroy_pipeline(self.pipeline, None);
                        self.device
                            .device
                            .destroy_pipeline_layout(self.pipeline_layout, None);
                    }
                    self.pipeline = new_pipeline;
                    self.pipeline_layout = new_layout;
                    log::info!("Pipeline hot-reloaded successfully!");
                }
                Err(e) => {
                    log::error!("Hot-reload failed (keeping old pipeline): {}", e);
                }
            }

            // Also rebuild the post-process pipelines (bright /
            // blur / composite). The dirty flag fires for *any*
            // .frag/.vert change in the shader directory so we
            // can't tell whether forward_opaque.* or post_*.* moved —
            // just rebuild everything. Cheap relative to the
            // device wait above. Compile failures are
            // non-fatal: the existing pipelines stay live.
            if let Err(e) = self
                .post
                .reload_pipelines(&self.device.device, &self.shader_dir)
            {
                log::error!(
                    "Post-pipeline hot-reload failed (keeping old pipelines): {}",
                    e
                );
            } else {
                log::info!("Post pipelines hot-reloaded successfully!");
            }
        }
    }

    pub(super) fn compile_pipeline_from_disk(
        device: &ash::Device,
        render_pass: vk::RenderPass,
        extent: vk::Extent2D,
        descriptor_set_layouts: &[vk::DescriptorSetLayout],
        shader_dir: &std::path::Path,
    ) -> Result<(vk::Pipeline, vk::PipelineLayout)> {
        let vert_path = shader_dir.join("forward_opaque.vert");
        let frag_path = shader_dir.join("forward_opaque.frag");

        let vert_spv = hot_reload::compile_glsl_file(&vert_path, shaderc::ShaderKind::Vertex)?;
        let frag_spv = hot_reload::compile_glsl_file(&frag_path, shaderc::ShaderKind::Fragment)?;

        let vert_module = pipeline::create_shader_module(device, &vert_spv)?;
        let frag_module = pipeline::create_shader_module(device, &frag_spv)?;

        let result = pipeline::create_graphics_pipeline(
            device,
            render_pass,
            extent,
            descriptor_set_layouts,
            vert_module,
            frag_module,
        );

        unsafe {
            device.destroy_shader_module(vert_module, None);
            device.destroy_shader_module(frag_module, None);
        }

        result
    }
}

/// Find the shader directory by checking common locations.
pub(super) fn find_shader_dir() -> PathBuf {
    // Try relative to current dir (workspace root)
    let candidates = [
        PathBuf::from("assets/shaders"),
        PathBuf::from("../assets/shaders"),
        PathBuf::from("../../assets/shaders"),
    ];

    for candidate in &candidates {
        if candidate.exists() && candidate.join("forward_opaque.vert").exists() {
            return candidate
                .canonicalize()
                .unwrap_or_else(|_| candidate.clone());
        }
    }

    // Fallback
    PathBuf::from("assets/shaders")
}
