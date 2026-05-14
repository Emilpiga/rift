//! Per-frame orchestration: `prepare_frame`, `build_draw_lists`,
//! `draw_frame`, and the private `record_*` helpers used by
//! `draw_frame` to lay down shadow / scene / translucent / composite
//! render passes.

use anyhow::Result;
use ash::vk;
use glam::Mat4;

use crate::renderer::forward::Renderer;
use crate::renderer::passes::{shadow, shadow_point};
use crate::renderer::uniforms::{PointLight, PointShadowSlotState, MAX_POINT_LIGHTS};
use crate::vulkan::{commands::DrawCommand, sync::MAX_FRAMES_IN_FLIGHT};

impl Renderer {
    /// Wait until the GPU has finished any prior submission that used the
    /// resources for the upcoming frame. Call this BEFORE writing into any
    /// per-frame host-visible buffer (e.g. dynamic vertex buffers for CPU
    /// skinning) and then call `draw_frame` afterwards. Calling `draw_frame`
    /// without `prepare_frame` is still safe — it does the same wait
    /// internally — but then per-frame writes done before `draw_frame` may
    /// race with in-flight GPU reads.
    pub fn prepare_frame(&mut self) -> Result<()> {
        if self.window_extent[0] == 0 || self.window_extent[1] == 0 {
            return Ok(());
        }
        let frame = self.current_frame;
        unsafe {
            self.device.device.wait_for_fences(
                &[self.frame_sync.in_flight[frame]],
                true,
                u64::MAX,
            )?;
        }
        Ok(())
    }

    /// Build this frame's two draw lists by walking `self.objects` once,
    /// applying frustum + fog culling for the visible-draw list and only
    /// fog culling for the shadow-caster list (off-screen casters can
    /// still project shadows onto visible floor).
    ///
    /// Reuses the per-renderer scratch Vecs via `mem::take` so the
    /// hot loop allocates zero heap per frame; the caller must restore
    /// the Vecs into `self.draw_scratch` / `self.shadow_draw_scratch`
    /// before the next frame.
    pub(super) fn build_draw_lists(
        &mut self,
        frame: usize,
    ) -> (Vec<DrawCommand>, Vec<DrawCommand>) {
        let frustum = self.camera.frustum_planes();
        let fog_cull_dist = self.fog_end + 2.0; // small margin beyond fog end
        let mut draws = std::mem::take(&mut self.draw_scratch);
        let mut shadow_draws = std::mem::take(&mut self.shadow_draw_scratch);
        draws.clear();
        shadow_draws.clear();
        for obj in &self.objects {
            // Skip hidden objects (dead entities set matrix to zero).
            if obj.model_matrix == Mat4::ZERO {
                continue;
            }
            // Frustum cull: extract world-space center from model matrix col 3.
            let center = obj.model_matrix.w_axis.truncate();
            // Distance cull: skip objects fully outside the
            // fog volume. Anchored on `fog_origin` (player) to
            // match the shader's fog math — otherwise zooming
            // the camera out would pop in geometry the player
            // can still see.
            let fog_limit = fog_cull_dist + obj.bounds_radius;
            if (center - self.fog_origin).length_squared() > fog_limit * fog_limit {
                continue;
            }
            // Pick the GPU-skinner output VB if present, then fall
            // back to the legacy host-visible dynamic ring, then to
            // the static device-local VB. The skin handle wins over
            // dynamic_vertex_buffers because a freshly converted
            // mesh may briefly carry both during transition.
            let vb = match obj
                .skin_handle
                .and_then(|h| self.skin_system.output_vertex_buffer(h))
            {
                Some(b) => b,
                None => match obj.dynamic_vertex_buffers.as_ref() {
                    Some(bufs) => bufs[frame].buffer,
                    None => obj.vertex_buffer.buffer,
                },
            };
            let is_dynamic = obj.skin_handle.is_some() || obj.dynamic_vertex_buffers.is_some();
            let cmd = DrawCommand {
                vertex_buffer: vb,
                index_buffer: obj.index_buffer.buffer,
                index_count: obj.index_count,
                descriptor_set: self.uniforms.descriptor_sets[frame],
                material_set: obj.material_set,
                model_matrix: obj.model_matrix,
                center,
                bounds_radius: obj.bounds_radius,
                tint: obj.tint,
                material_params: obj.material_params,
                dynamic_vertices: is_dynamic,
            };
            // Shadow casters must include geometry outside the
            // camera frustum: a wall / prop behind the camera
            // can still project a shadow that falls on visible
            // floor in front of the camera. Cull only by the
            // player-anchored fog distance for the shadow list.
            // `casts_shadow=false` opts giant flat receivers
            // (hub platform, dune ring, dungeon floor) out of
            // every shadow pass entirely \u2014 they're a major
            // chunk of triangles and contribute no visible
            // cast shadows worth the GPU time.
            let height_shadow_surface = self.height_shadows_enabled
                && obj.material_params[1] > 0.001
                && (obj.material_params[2].to_bits() & 1) != 0;
            if obj.casts_shadow || height_shadow_surface {
                shadow_draws.push(cmd);
            }
            // Visible-draw list: also gate on the camera
            // frustum so we don't rasterise off-screen geometry
            // into the forward pass.
            if !self
                .camera
                .sphere_in_frustum(&frustum, center, obj.bounds_radius + 1.0)
            {
                continue;
            }
            draws.push(cmd);
        }
        (draws, shadow_draws)
    }

    /// Record the directional (sun/key-light) shadow pass.
    /// Renders scene depth from the light's POV into the single
    /// orthographic shadow map.
    ///
    /// SAFETY: caller must have an active command buffer recording
    /// (i.e. between `begin_command_buffer` and `end_command_buffer`).
    unsafe fn record_dir_shadow_pass(&self, cmd: vk::CommandBuffer, shadow_draws: &[DrawCommand]) {
        let device = &self.device.device;
        let shadow_clear = [vk::ClearValue {
            depth_stencil: vk::ClearDepthStencilValue {
                depth: 1.0,
                stencil: 0,
            },
        }];
        let shadow_rp_begin = vk::RenderPassBeginInfo::default()
            .render_pass(self.shadow_map.render_pass)
            .framebuffer(self.shadow_map.framebuffer)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: vk::Extent2D {
                    width: shadow::SHADOW_MAP_SIZE,
                    height: shadow::SHADOW_MAP_SIZE,
                },
            })
            .clear_values(&shadow_clear);
        device.cmd_begin_render_pass(cmd, &shadow_rp_begin, vk::SubpassContents::INLINE);
        device.cmd_bind_pipeline(
            cmd,
            vk::PipelineBindPoint::GRAPHICS,
            self.shadow_map.pipeline,
        );
        // The shadow pipeline reads only the global UBO (set 0),
        // and every draw uses the same per-frame descriptor set.
        // Bind it once for the whole pass instead of once per
        // draw — saves ~draws.len() command buffer entries with
        // no behavioural change.
        if let Some(first) = shadow_draws.first() {
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.shadow_map.pipeline_layout,
                0,
                &[first.descriptor_set],
                &[],
            );
        }
        for draw in shadow_draws.iter() {
            device.cmd_bind_vertex_buffers(cmd, 0, &[draw.vertex_buffer], &[0]);
            device.cmd_bind_index_buffer(cmd, draw.index_buffer, 0, vk::IndexType::UINT32);
            let model_bytes: &[u8] = bytemuck::bytes_of(&draw.model_matrix);
            device.cmd_push_constants(
                cmd,
                self.shadow_map.pipeline_layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                model_bytes,
            );
            device.cmd_draw_indexed(cmd, draw.index_count, 1, 0, 0, 0);
        }
        device.cmd_end_render_pass(cmd);
    }

    /// Record the point-light cube shadow pass: 6 cube faces × N
    /// active shadow lights, with per-slot dirty-tracking that
    /// skips re-rendering when no caster has moved within range.
    /// Also transitions any unused atlas slots to
    /// SHADER_READ_ONLY_OPTIMAL so the main fragment shader's
    /// cube-array sampler is layout-legal.
    ///
    /// SAFETY: caller must have an active command buffer recording.
    unsafe fn record_point_shadow_pass(
        &mut self,
        cmd: vk::CommandBuffer,
        point_shadow_count: usize,
        merged_lights: &[PointLight; MAX_POINT_LIGHTS],
        shadow_draws: &[DrawCommand],
    ) {
        let device = &self.device.device;
        // Defined-layout pre-pass: any cube face slot we
        // *don't* render into this frame stays in
        // VK_IMAGE_LAYOUT_UNDEFINED, but the main
        // fragment shader still samples those layers via
        // the cube-array view (the conditional is in the
        // shader, not in descriptor binding). Vulkan
        // requires every subresource the descriptor
        // covers to be in SHADER_READ_ONLY_OPTIMAL at
        // submit time, so we transition the unused slots
        // here. UNDEFINED → SHADER_READ_ONLY_OPTIMAL is a
        // discard-and-set, so it's safe to issue every
        // frame: the render pass that follows will
        // implicitly transition any used slot back to
        // SHADER_READ_ONLY_OPTIMAL via its own attachment
        // descriptions.
        if point_shadow_count < shadow_point::MAX_POINT_SHADOWS {
            let unused_base = (point_shadow_count * 6) as u32;
            let unused_count = (shadow_point::MAX_POINT_SHADOWS * 6) as u32 - unused_base;
            let barrier = vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::SHADER_READ)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(self.point_shadow_atlas.color_image)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: unused_base,
                    layer_count: unused_count,
                });
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
        }
        if point_shadow_count == 0 {
            return;
        }
        let psh_clear = [
            vk::ClearValue {
                color: vk::ClearColorValue {
                    // Default to "fully unoccluded" (>= 1.0)
                    // so directions that were never rasterised
                    // sample as lit, not as black walls.
                    float32: [1.0, 1.0, 1.0, 1.0],
                },
            },
            vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            },
        ];
        device.cmd_bind_pipeline(
            cmd,
            vk::PipelineBindPoint::GRAPHICS,
            self.point_shadow_atlas.pipeline,
        );
        // Take the per-light scratch list out of `self`
        // (mirrors the trick used for `draws`) so we can
        // hold a `&self.point_shadow_atlas` borrow and
        // mutate the scratch Vec in the same scope.
        let mut light_draws = std::mem::take(&mut self.point_shadow_draw_scratch);
        for light_idx in 0..point_shadow_count {
            let pl = &merged_lights[light_idx];
            let lpos = pl.position;
            let lrad = pl.radius.max(0.1);
            // Cull once per light against the per-draw
            // bounding sphere. Reused for all 6 cube faces
            // so a light that touches `K` draws does
            // `K` sphere tests + `6*K` draws instead of
            // `6*K` sphere tests + `6*K` draws as before.
            light_draws.clear();
            for draw in shadow_draws.iter() {
                let light_limit = lrad + draw.bounds_radius;
                if (draw.center - lpos).length_squared() > light_limit * light_limit {
                    continue;
                }
                light_draws.push(*draw);
            }
            // ---- Dirty check ----
            // Hash the current frame's slot inputs (light
            // pose + every relevant caster's translation &
            // bounds_radius). If it matches the cached
            // value from when we last rendered into this
            // slot, skip the 6-face render entirely — the
            // atlas contents are still valid because no
            // caster moved within range. FNV-1a 64 over
            // the bit patterns is cheap and stable.
            let new_state = {
                const FNV_OFFSET: u64 = 0xcbf29ce484222325;
                const FNV_PRIME: u64 = 0x100000001b3;
                let mut h: u64 = FNV_OFFSET;
                // If any caster has dynamic vertex contents
                // (CPU ring or GPU skin output), force the
                // slot to re-render every frame by mixing
                // the frame counter into the hash. The model
                // matrix is invariant across pure pose
                // changes (spine twist while standing still,
                // animation cycles in place), so without
                // this the cube atlas freezes for skinned
                // characters and the cast shadow visibly
                // lags the silhouette until the entity
                // translates.
                let mut force_dirty = false;
                for d in light_draws.iter() {
                    let m = d.model_matrix;
                    for col in [m.x_axis, m.y_axis, m.z_axis, m.w_axis] {
                        for word in [
                            col.x.to_bits(),
                            col.y.to_bits(),
                            col.z.to_bits(),
                            col.w.to_bits(),
                        ] {
                            h ^= word as u64;
                            h = h.wrapping_mul(FNV_PRIME);
                        }
                    }
                    h ^= d.bounds_radius.to_bits() as u64;
                    h = h.wrapping_mul(FNV_PRIME);
                    for word in d.material_params.iter().map(|v| v.to_bits()) {
                        h ^= word as u64;
                        h = h.wrapping_mul(FNV_PRIME);
                    }
                    if d.dynamic_vertices {
                        force_dirty = true;
                    }
                }
                h ^= u64::from(self.height_shadows_enabled);
                h = h.wrapping_mul(FNV_PRIME);
                if force_dirty {
                    // Stagger shadow updates across frames.
                    // Each dynamic-caster light re-renders
                    // every *other* frame, and even-indexed
                    // lights refresh on opposite frames
                    // from odd-indexed lights so the work
                    // is spread evenly instead of spiking
                    // every two frames.
                    //
                    // With 8 shadow lights + 12 skinned
                    // monsters wandering between them, the
                    // previous behaviour refreshed every
                    // light every frame — even though a
                    // single skinned pose changes by
                    // sub-pixel amounts in 16 ms. Halving
                    // the refresh rate of dynamic shadows
                    // is invisible at 60+ FPS (the shadow
                    // follows the silhouette exactly one
                    // frame late, less than the monitor
                    // refresh) and roughly halves the
                    // per-frame shadow-pass GPU cost.
                    //
                    // `epoch = (frame_count + light_idx) >> 1`
                    // is constant across two adjacent
                    // frames per-light (so the cache hits),
                    // and adjacent lights have offset
                    // epochs (so half refresh on even
                    // frames, half on odd).
                    let epoch = (self.frame_count.wrapping_add(light_idx as u64)) >> 1;
                    h ^= epoch;
                    h = h.wrapping_mul(FNV_PRIME);
                }
                PointShadowSlotState {
                    light_bits: [
                        lpos.x.to_bits(),
                        lpos.y.to_bits(),
                        lpos.z.to_bits(),
                        lrad.to_bits(),
                    ],
                    caster_hash: h,
                }
            };
            if self.point_shadow_state[light_idx] == Some(new_state) {
                // Slot is clean: previous frame's atlas
                // contents are still valid and the image is
                // already in SHADER_READ_ONLY_OPTIMAL (left
                // there by the prior render pass's final
                // attachment layout). Nothing to do.
                continue;
            }
            self.point_shadow_state[light_idx] = Some(new_state);
            // Note: when `light_draws` is empty we still
            // run the 6 render passes — they hit the
            // LOAD_OP::CLEAR path with zero draws,
            // which paints the slot fully unoccluded.
            // Skipping the passes outright would leave any
            // previous frame's shadow content in the atlas
            // (visible as stale shadows for a frame after a
            // caster leaves the light's radius). The
            // descriptor-set bind below is guarded so an
            // empty `light_draws` is safe.
            for face_idx in 0..6 {
                let face_slot = light_idx * 6 + face_idx;
                // Per-face cone cull. Each cube face is a
                // 90° FOV view aligned with one of the six
                // cardinal axes. A caster (sphere `C`,
                // radius `r`) only appears in this face when
                // it intersects that view cone — testing
                // against the 5 frustum planes (skip far)
                // lets us skip ~5/6 of the per-face draws
                // when casters are clustered around the
                // light. With 12 monsters across 8 shadow
                // lights this turns ~576 per-frame shadow
                // draw calls into ~96, cutting both the
                // command-buffer submission cost and the
                // vertex-shader work for skinned meshes.
                //
                // Face axes: 0:+X 1:-X 2:+Y 3:-Y 4:+Z 5:-Z
                let face_axis: glam::Vec3 = match face_idx {
                    0 => glam::Vec3::X,
                    1 => glam::Vec3::NEG_X,
                    2 => glam::Vec3::Y,
                    3 => glam::Vec3::NEG_Y,
                    4 => glam::Vec3::Z,
                    _ => glam::Vec3::NEG_Z,
                };
                let rp_begin = vk::RenderPassBeginInfo::default()
                    .render_pass(self.point_shadow_atlas.render_pass)
                    .framebuffer(self.point_shadow_atlas.framebuffers[face_slot])
                    .render_area(vk::Rect2D {
                        offset: vk::Offset2D { x: 0, y: 0 },
                        extent: vk::Extent2D {
                            width: shadow_point::POINT_SHADOW_SIZE,
                            height: shadow_point::POINT_SHADOW_SIZE,
                        },
                    })
                    .clear_values(&psh_clear);
                device.cmd_begin_render_pass(cmd, &rp_begin, vk::SubpassContents::INLINE);
                for draw in light_draws.iter() {
                    // Sphere-vs-cube-face cone test. For a
                    // 90° FOV view down `face_axis`, a point
                    // is inside the view if its component
                    // along `face_axis` exceeds the magnitude
                    // of its perpendicular components. For a
                    // sphere, we extend the test by `r` to
                    // get a conservative include. Skip if the
                    // entire sphere is outside the cone.
                    let d = draw.center - lpos;
                    let along = d.dot(face_axis);
                    let r = draw.bounds_radius;
                    if along + r < 0.0 {
                        continue; // entirely behind face
                    }
                    let perp_sq = (d.length_squared() - along * along).max(0.0);
                    // Cone half-angle is 45° → tan = 1, so
                    // sphere fits inside cone when `perp <=
                    // along + r * sqrt(2)`. The sqrt(2)
                    // factor is the conservative inflation
                    // for a sphere-vs-plane test on the
                    // 45° side planes.
                    let cone_limit = along + r * std::f32::consts::SQRT_2;
                    if cone_limit < 0.0 || perp_sq > cone_limit * cone_limit {
                        continue;
                    }
                    device.cmd_bind_vertex_buffers(cmd, 0, &[draw.vertex_buffer], &[0]);
                    device.cmd_bind_index_buffer(cmd, draw.index_buffer, 0, vk::IndexType::UINT32);
                    device.cmd_bind_descriptor_sets(
                        cmd,
                        vk::PipelineBindPoint::GRAPHICS,
                        self.point_shadow_atlas.pipeline_layout,
                        0,
                        &[draw.descriptor_set, draw.material_set],
                        &[],
                    );
                    // Push the model + indices + material payload as
                    // a single 96-byte block. The vert
                    // shader reads `mat4 model` at offset
                    // 0; the frag reads `uvec4 indices` at
                    // offset 64 and material params at 80. One push call instead of
                    // two saves a command-buffer entry per
                    // draw.
                    let mut bytes = [0u8; 96];
                    bytes[..64].copy_from_slice(bytemuck::bytes_of(&draw.model_matrix));
                    let indices: [u32; 4] = [face_slot as u32, light_idx as u32, 0, 0];
                    bytes[64..80].copy_from_slice(bytemuck::bytes_of(&indices));
                    bytes[80..].copy_from_slice(bytemuck::bytes_of(&draw.material_params));
                    device.cmd_push_constants(
                        cmd,
                        self.point_shadow_atlas.pipeline_layout,
                        vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                        0,
                        &bytes,
                    );
                    device.cmd_draw_indexed(cmd, draw.index_count, 1, 0, 0, 0);
                }
                device.cmd_end_render_pass(cmd);
            }
        }
        // Restore for next frame.
        self.point_shadow_draw_scratch = light_draws;
    }

    /// Record the main HDR scene pass: sky dome, then opaque
    /// 3D draws. Renders into the post-processing HDR colour
    /// target (not the swapchain). Overlay/UI moves to the
    /// composite pass.
    ///
    /// SAFETY: caller must have an active command buffer recording.
    unsafe fn record_scene_pass(
        &self,
        cmd: vk::CommandBuffer,
        image_index: u32,
        frame: usize,
        draws: &[DrawCommand],
    ) {
        let device = &self.device.device;
        let clear_values = [
            vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: self.clear_color,
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
            .render_pass(self.post.scene_pass)
            .framebuffer(self.post.scene_framebuffers[image_index as usize])
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.swapchain.extent,
            })
            .clear_values(&clear_values);

        device.cmd_begin_render_pass(cmd, &render_pass_begin, vk::SubpassContents::INLINE);

        // Sky dome — drawn first inside the main pass with
        // depth test/write disabled so subsequent scene
        // geometry occludes it naturally. No-op when
        // `sky.enabled` is false (indoor dungeons).
        self.sky_renderer.record(
            device,
            cmd,
            self.swapchain.extent,
            self.camera.view_matrix(),
            self.camera.projection_matrix(),
            &self.sky,
            self.start_time.elapsed().as_secs_f32(),
        );

        // 3D scene
        device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
        for draw in draws {
            device.cmd_bind_vertex_buffers(cmd, 0, &[draw.vertex_buffer], &[0]);
            device.cmd_bind_index_buffer(cmd, draw.index_buffer, 0, vk::IndexType::UINT32);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[draw.descriptor_set, draw.material_set],
                &[],
            );
            let model_bytes: &[u8] = bytemuck::bytes_of(&draw.model_matrix);
            device.cmd_push_constants(
                cmd,
                self.pipeline_layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                0,
                model_bytes,
            );
            let tint_bytes: &[u8] = bytemuck::bytes_of(&draw.tint);
            device.cmd_push_constants(
                cmd,
                self.pipeline_layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                64,
                tint_bytes,
            );
            let mp_bytes: &[u8] = bytemuck::bytes_of(&draw.material_params);
            device.cmd_push_constants(
                cmd,
                self.pipeline_layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                80,
                mp_bytes,
            );
            device.cmd_draw_indexed(cmd, draw.index_count, 1, 0, 0, 0);
        }

        self.record_selected_outline_pass(cmd, draws);

        self.record_portrait_draws(cmd, frame);

        // End the opaque scene pass. Depth is now in
        // DEPTH_STENCIL_READ_ONLY_OPTIMAL — translucent
        // pipelines can both depth-test against it and
        // sample it as a combined-image-sampler for soft-
        // particle fade.
        device.cmd_end_render_pass(cmd);
    }

    unsafe fn record_selected_outline_pass(&self, cmd: vk::CommandBuffer, draws: &[DrawCommand]) {
        const SELECTED_FLAG: u32 = 16;
        const OUTLINE_PASS_FLAG: u32 = 128;

        let selected_draws = draws
            .iter()
            .filter(|draw| (draw.material_params[2].to_bits() & SELECTED_FLAG) != 0);
        let device = &self.device.device;
        device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.outline_pipeline);
        for draw in selected_draws {
            device.cmd_bind_vertex_buffers(cmd, 0, &[draw.vertex_buffer], &[0]);
            device.cmd_bind_index_buffer(cmd, draw.index_buffer, 0, vk::IndexType::UINT32);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.outline_pipeline_layout,
                0,
                &[draw.descriptor_set, draw.material_set],
                &[],
            );
            let model_bytes: &[u8] = bytemuck::bytes_of(&draw.model_matrix);
            device.cmd_push_constants(
                cmd,
                self.outline_pipeline_layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                0,
                model_bytes,
            );
            let tint = [1.0_f32, 1.0, 1.0, 0.92];
            let tint_bytes: &[u8] = bytemuck::bytes_of(&tint);
            device.cmd_push_constants(
                cmd,
                self.outline_pipeline_layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                64,
                tint_bytes,
            );
            let mut params = draw.material_params;
            params[2] = f32::from_bits(params[2].to_bits() | OUTLINE_PASS_FLAG);
            let mp_bytes: &[u8] = bytemuck::bytes_of(&params);
            device.cmd_push_constants(
                cmd,
                self.outline_pipeline_layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                80,
                mp_bytes,
            );
            device.cmd_draw_indexed(cmd, draw.index_count, 1, 0, 0, 0);
        }
    }

    unsafe fn record_portrait_draws(&self, cmd: vk::CommandBuffer, frame: usize) {
        if self.portrait_draws.is_empty() {
            return;
        }
        let device = &self.device.device;
        let forward = (self.camera.target - self.camera.position).normalize_or_zero();
        if forward.length_squared() <= 0.001 {
            return;
        }
        let right = forward.cross(self.camera.up).normalize_or_zero();
        let up = right.cross(forward).normalize_or_zero();
        let d = 2.2_f32;
        let tan_half = (self.camera.fov_y * 0.5).tan();
        let screen_w = self.window_extent[0].max(1) as f32;
        let screen_h = self.window_extent[1].max(1) as f32;
        let world_h = 2.0 * d * tan_half;
        let world_per_px = world_h / screen_h;
        let head_center_y = 1.55_f32;

        for portrait in &self.portrait_draws {
            let Some(obj) = self.objects.get(portrait.object_index) else {
                continue;
            };
            if obj.model_matrix == Mat4::ZERO {
                continue;
            }
            let [x0, y0, x1, y1] = portrait.rect_px_bl;
            let h = (y1 - y0).max(1.0);
            let center_x = (x0 + x1) * 0.5;
            let center_y_top = (y0 + y1) * 0.5;
            let ndc_x = (center_x / screen_w) * 2.0 - 1.0;
            let ndc_y = (center_y_top / screen_h) * 2.0 - 1.0;
            let view_x = ndc_x * tan_half * self.camera.aspect * d;
            let view_y = ndc_y * tan_half * d;
            let desired_head_h = h * 0.88 * world_per_px;
            let scale = (desired_head_h / 0.72).clamp(0.12, 1.4);
            let head_world_center =
                self.camera.position + forward * d + right * view_x - up * view_y;
            let basis = Mat4::from_cols(
                (right * scale).extend(0.0),
                (up * scale).extend(0.0),
                (-forward * scale).extend(0.0),
                (head_world_center - up * head_center_y * scale).extend(1.0),
            );

            let vb = match obj
                .skin_handle
                .and_then(|h| self.skin_system.output_vertex_buffer(h))
            {
                Some(b) => b,
                None => match obj.dynamic_vertex_buffers.as_ref() {
                    Some(bufs) => bufs[frame].buffer,
                    None => obj.vertex_buffer.buffer,
                },
            };
            device.cmd_bind_vertex_buffers(cmd, 0, &[vb], &[0]);
            device.cmd_bind_index_buffer(cmd, obj.index_buffer.buffer, 0, vk::IndexType::UINT32);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[self.uniforms.descriptor_sets[frame], obj.material_set],
                &[],
            );
            let model_bytes: &[u8] = bytemuck::bytes_of(&basis);
            device.cmd_push_constants(
                cmd,
                self.pipeline_layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                0,
                model_bytes,
            );
            let rect_bytes: &[u8] = bytemuck::bytes_of(&portrait.rect_px_bl);
            device.cmd_push_constants(
                cmd,
                self.pipeline_layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                64,
                rect_bytes,
            );
            let mut params = obj.material_params;
            params[2] = f32::from_bits(params[2].to_bits() | 32u32);
            params[3] = 0.0;
            let mp_bytes: &[u8] = bytemuck::bytes_of(&params);
            device.cmd_push_constants(
                cmd,
                self.pipeline_layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                80,
                mp_bytes,
            );
            device.cmd_draw_indexed(cmd, obj.index_count, 1, 0, 0, 0);
        }
    }

    /// Record the translucent pass: ribbons + particles. Loads
    /// the HDR colour the opaque pass just wrote; depth is
    /// read-only so this pass can't write to it.
    ///
    /// SAFETY: caller must have an active command buffer recording.
    unsafe fn record_translucent_pass(
        &self,
        cmd: vk::CommandBuffer,
        image_index: u32,
        frame: usize,
    ) {
        let device = &self.device.device;
        let translucent_begin = vk::RenderPassBeginInfo::default()
            .render_pass(self.post.translucent_pass)
            .framebuffer(self.post.translucent_framebuffers[image_index as usize])
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.swapchain.extent,
            });
        device.cmd_begin_render_pass(cmd, &translucent_begin, vk::SubpassContents::INLINE);

        // VFX ribbons (world-space, premultiplied additive).
        // Drawn before particles so the spark/glow trails
        // composite on top of the beam core.
        self.vfx_ribbon_renderer.record(
            frame,
            device,
            cmd,
            self.uniforms.descriptor_sets[frame],
            self.post.translucent_in_sets[image_index as usize],
        );
        // VFX particles (world-space, two pipelines).
        self.vfx_particle_renderer.record(
            frame,
            device,
            cmd,
            self.uniforms.descriptor_sets[frame],
            self.post.translucent_in_sets[image_index as usize],
        );

        device.cmd_end_render_pass(cmd);
    }

    /// Record the final composite + overlay pass: tonemap HDR +
    /// bloom + graph outputs into the swapchain and draw UI on
    /// top so it stays at native sRGB crispness.
    ///
    /// SAFETY: caller must have an active command buffer recording.
    unsafe fn record_composite_and_overlay(
        &self,
        cmd: vk::CommandBuffer,
        image_index: u32,
        frame: usize,
    ) {
        let device = &self.device.device;
        let composite_begin = vk::RenderPassBeginInfo::default()
            .render_pass(self.post.composite_pass)
            .framebuffer(self.post.composite_framebuffers[image_index as usize])
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.swapchain.extent,
            });
        device.cmd_begin_render_pass(cmd, &composite_begin, vk::SubpassContents::INLINE);

        let mut bloom = self.bloom;
        if !self.bloom_enabled {
            bloom.intensity = 0.0;
        }
        let ao_strength = if self.ssao_enabled { 1.0 } else { 0.0 };
        let volumetrics_intensity = if self.volumetrics_enabled { 1.0 } else { 0.0 };
        self.post.record_composite(
            device,
            cmd,
            image_index,
            &bloom,
            self.ghost_mix,
            ao_strength,
            volumetrics_intensity,
        );

        // Overlay (HUD)
        self.overlay.record(frame, device, cmd);

        device.cmd_end_render_pass(cmd);
    }

    pub fn draw_frame(&mut self) -> Result<()> {
        // Skip rendering when minimized.
        if self.window_extent[0] == 0 || self.window_extent[1] == 0 {
            return Ok(());
        }

        self.frame_count += 1;
        let frame = self.current_frame;

        unsafe {
            self.device.device.wait_for_fences(
                &[self.frame_sync.in_flight[frame]],
                true,
                u64::MAX,
            )?;
        }
        // Validation requires the cmd buffer to be reset before its
        // referenced buffers are destroyed.
        let cmd = self.command_buffers[frame];
        unsafe {
            self.device
                .device
                .reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())?;
        }
        self.flush_deletions();

        // ---- Acquire next swapchain image ----
        let image_index = unsafe {
            match self.swapchain.swapchain_fn.acquire_next_image(
                self.swapchain.swapchain,
                u64::MAX,
                self.frame_sync.image_available[frame],
                vk::Fence::null(),
            ) {
                Ok((index, _suboptimal)) => index,
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                    self.recreate_swapchain(self.window_extent[0], self.window_extent[1])?;
                    return Ok(());
                }
                Err(e) => return Err(e.into()),
            }
        };
        unsafe {
            self.device
                .device
                .reset_fences(&[self.frame_sync.in_flight[frame]])?;
        }

        // ---- Build per-frame light list, shadow VPs, UBO ----
        let (merged_lights, light_count, point_shadow_count) = self.merge_per_frame_lights();
        let point_shadow_face_vp =
            self.build_point_shadow_face_vp(point_shadow_count, &merged_lights);
        let ubo = self.build_uniform_data(
            &merged_lights,
            light_count,
            point_shadow_count,
            point_shadow_face_vp,
        );
        self.uniforms.update(frame, &ubo);

        // ---- Build per-frame draw lists ----
        let (draws, shadow_draws) = self.build_draw_lists(frame);

        // ---- Upload per-frame instance data ----
        self.overlay.upload(
            frame,
            &self.device.device,
            &self.allocator,
            self.device.graphics_queue,
            self.command_pool,
            &self.overlay_batch,
        )?;
        self.vfx_ribbon_renderer.upload(
            frame,
            &self.device.device,
            &self.allocator,
            self.vfx_system.ribbon_instances(),
        )?;
        self.vfx_particle_renderer.upload(
            frame,
            &self.device.device,
            &self.allocator,
            self.vfx_system.particle_instances(),
        )?;

        // ---- Compute screen-space post inputs (CPU math) ----
        let (sun_screen, sun_color) = self.compute_sun_screen_uv();
        // Inverse projection matrix is needed by the SSAO graph
        // node to reconstruct view-space positions from sampled
        // depth. Inverting on CPU once per frame is essentially
        // free vs. doing it per pixel.
        let inv_proj = self.camera.projection_matrix().inverse().to_cols_array_2d();
        // The final composite applies AO multiplicatively to
        // the shaded HDR. Gameplay uses a moderate default,
        // while preview scenes can reduce this to avoid visible
        // low-sample screen-space noise on smooth surfaces.
        let ssao_strength = if self.ssao_enabled {
            self.ssao_strength
        } else {
            0.0
        };

        // ---- Record command buffer ----
        unsafe {
            let begin_info = vk::CommandBufferBeginInfo::default();
            self.device.device.begin_command_buffer(cmd, &begin_info)?;

            // GPU mesh skinning compute dispatches + a single
            // COMPUTE_SHADER_WRITE -> VERTEX_ATTRIBUTE_READ
            // barrier so the shadow + forward passes see the
            // new vertices. No-op if nothing's active.
            self.skin_system.record_dispatches(
                &self.device.device,
                cmd,
                self.current_frame,
                &self.allocator,
            );

            if self.shadows_enabled {
                self.record_dir_shadow_pass(cmd, &shadow_draws);
                self.record_point_shadow_pass(
                    cmd,
                    point_shadow_count,
                    &merged_lights,
                    &shadow_draws,
                );
            } else {
                self.point_shadow_state = [None; shadow_point::MAX_POINT_SHADOWS];
            }

            // Blood-field splat pass: drains kill splats queued
            // during the gameplay frame into this frame's instance
            // buffer and renders into the per-floor blood field.
            // Also handles the initial clear when a new floor is
            // bound. No-op when no floor is active or no splats
            // are pending.
            self.blood_field
                .record(&self.device.device, cmd, frame, self.elapsed_secs());

            self.record_scene_pass(cmd, image_index, frame, &draws);
            self.record_translucent_pass(cmd, image_index, frame);
            if self.ssao_enabled || self.volumetrics_enabled {
                self.post.record_post_graph(
                    &self.device.device,
                    cmd,
                    image_index,
                    inv_proj,
                    ssao_strength,
                    sun_screen,
                    sun_color,
                );
            }
            if self.bloom_enabled {
                self.post
                    .record_bloom(&self.device.device, cmd, image_index, &self.bloom);
            }
            self.record_composite_and_overlay(cmd, image_index, frame);

            self.device.device.end_command_buffer(cmd)?;
        }

        // ---- Submit ----
        let wait_semaphores = [self.frame_sync.image_available[frame]];
        let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        let signal_semaphores = [self.frame_sync.render_finished[frame]];
        let submit_info = vk::SubmitInfo::default()
            .wait_semaphores(&wait_semaphores)
            .wait_dst_stage_mask(&wait_stages)
            .command_buffers(std::slice::from_ref(&cmd))
            .signal_semaphores(&signal_semaphores);
        unsafe {
            self.device.device.queue_submit(
                self.device.graphics_queue,
                &[submit_info],
                self.frame_sync.in_flight[frame],
            )?;
        }

        // ---- Present ----
        let swapchains = [self.swapchain.swapchain];
        let image_indices = [image_index];
        let present_info = vk::PresentInfoKHR::default()
            .wait_semaphores(&signal_semaphores)
            .swapchains(&swapchains)
            .image_indices(&image_indices);
        let present_result = unsafe {
            self.swapchain
                .swapchain_fn
                .queue_present(self.device.present_queue, &present_info)
        };

        // Restore scratch buffers so next frame reuses the same
        // allocation. Done before any potential early return so a
        // swapchain rebuild doesn't drop the scratch capacity.
        self.draw_scratch = draws;
        self.shadow_draw_scratch = shadow_draws;

        match present_result {
            Ok(_) => {}
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR | vk::Result::SUBOPTIMAL_KHR) => {
                self.framebuffer_resized = false;
                self.recreate_swapchain(self.window_extent[0], self.window_extent[1])?;
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        }

        if self.framebuffer_resized {
            self.framebuffer_resized = false;
            self.recreate_swapchain(self.window_extent[0], self.window_extent[1])?;
            return Ok(());
        }

        self.current_frame = (self.current_frame + 1) % MAX_FRAMES_IN_FLIGHT;
        Ok(())
    }
}
