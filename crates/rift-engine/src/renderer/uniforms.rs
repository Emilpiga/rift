//! Per-frame light + uniform building: directional `KeyLight`,
//! dynamic `PointLight`s, `HeatSource`s, point-shadow dirty-tracking,
//! and `UniformData` assembly for the camera/lighting/fog UBO.

use glam::{Mat4, Vec3, Vec4};

use crate::renderer::forward::Renderer;
use crate::renderer::passes::{shadow, shadow_point};
use crate::renderer::uniform::UniformData;

/// Directional key light + ambient floor. The forward shader
/// reads `direction` as the light vector, `color` as its tint
/// (multiplied into diffuse + specular + rim), and `ambient`
/// as the unconditional floor added to every fragment.
#[derive(Clone, Copy, Debug)]
pub struct KeyLight {
    /// World-space direction *toward* the light (will be
    /// normalised before upload).
    pub direction: Vec3,
    /// RGB tint of the directional contribution. Treat as
    /// linear, pre-tonemap. ~0.2 reads as moonlight, ~1.0 as
    /// midday sun.
    pub color: Vec3,
    /// Unconditional ambient floor. ~0.05 = cave-dark, ~0.30 =
    /// outdoor overcast.
    pub ambient: f32,
}

impl KeyLight {
    /// Default rift / dungeon mood: very dim cool moonlight
    /// with low ambient so torches carry the warmth.
    pub const DUNGEON: Self = Self {
        direction: Vec3::new(0.4, 0.8, 0.3),
        color: Vec3::new(0.18, 0.20, 0.28),
        ambient: 0.05,
    };

    /// Sunny outdoor hub: warm bright key + lifted ambient so
    /// the open meadow reads as midday rather than dusk.
    pub const SUNLIT: Self = Self {
        direction: Vec3::new(0.4, 0.8, 0.3),
        color: Vec3::new(1.10, 1.00, 0.85),
        ambient: 0.35,
    };

    /// Brooding crimson stormlight for the abyss hub. Cooler-than-
    /// sunlit, slightly biased red on the directional, with a
    /// dim warm ambient so the platform reads as lit by the
    /// distant fire-storm horizon rather than a sun.
    pub const STORMLIT: Self = Self {
        direction: Vec3::new(0.2, 0.7, 0.5),
        color: Vec3::new(0.65, 0.30, 0.28),
        ambient: 0.18,
    };

    /// Diffuse warm sandstorm light. A single strong sun-like
    /// directional aimed to match the sandstorm sky's hot
    /// spot, lifted ambient so the dust-scattered fill
    /// bathes the whole platform, and the warm tan tint
    /// pre-bakes the dust scattering into the directional
    /// contribution. Combined with a sky-anchored point light
    /// in the hub, this gives the platform a dramatic
    /// "sunbeam through the dust" key/fill split rather than
    /// the flat overcast a high-ambient sandstorm would
    /// otherwise produce.
    ///
    /// The directional is intentionally HDR-bright (>1.0 on
    /// red/green) so the dunes' lit faces punch through the
    /// dust horizon and the platform reads as midday-veiled
    /// rather than dusk; the matching ambient lift keeps the
    /// shaded side from going muddy.
    ///
    /// Ambient sits high (`0.55`) on purpose: a real
    /// sandstorm has so much airborne dust that *every*
    /// surface picks up a strong omni-directional warm fill,
    /// not just the sun-facing side. Without this, props
    /// directly opposite the sun (the chest, the player
    /// standing with their back to the sun) read as
    /// near-black silhouettes against the lifted sky/fog —
    /// fine for a dungeon but wrong for a midday hub. The
    /// directional is bumped in lock-step so the lit side
    /// still has clear contrast against the shaded side.
    pub const SANDSTORM: Self = Self {
        // Matches `SkyConfig::sandstorm_hub`'s `sun_dir`
        // (normalised) so the shadow map lays the platform's
        // shadow opposite the visible sun in the sky.
        direction: Vec3::new(0.70, 0.32, 0.65),
        color: Vec3::new(2.10, 1.55, 0.95),
        ambient: 0.55,
    };
}

impl Default for KeyLight {
    fn default() -> Self {
        Self::DUNGEON
    }
}

/// Maximum number of point lights uploaded to the camera UBO
/// per frame. Kept in sync with the `[16]` array sizes in every
/// shader that binds the camera UBO (forward opaque frag, particle.vert,
/// ribbon.vert, shadow*.{vert,frag}). The first
/// `point_shadow_count` slots are shadow-casters, then VFX
/// additive lights, then any additional ambient/torch lights;
/// see `Renderer::merge_per_frame_lights`.
pub(super) const MAX_POINT_LIGHTS: usize = 16;

/// A dynamic point light source.
#[derive(Clone, Copy)]
pub struct PointLight {
    pub position: Vec3,
    pub color: Vec3,
    pub radius: f32,
    pub intensity: f32,
}

/// Per-slot dirty-tracking state for the point-shadow atlas.
/// Captured immediately after a slot's 6 cube faces are
/// rendered; on the next frame the renderer recomputes the
/// would-be value and skips the render entirely if it matches.
///
/// The hash collapses (a) the light's pose & radius and (b)
/// the bit pattern of every shadow-caster's translation +
/// bounds_radius within the light's effective range. That's
/// cheap to compute (a single FNV-style fold per slot) and
/// stable bit-for-bit across frames as long as the inputs are
/// genuinely unchanged. False positives (skip when shouldn't)
/// require a 64-bit hash collision *and* a coincidence in the
/// recorded light pose — both vanishingly rare; the worst
/// observable artefact would be a one-frame stale shadow.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct PointShadowSlotState {
    pub(super) light_bits: [u32; 4], // pos.x/y/z + radius, all to_bits()
    pub(super) caster_hash: u64,
}

/// One screen-space heat-distortion source. The composite pass
/// picks the strongest of these each frame and applies a
/// noise-driven UV warp to the HDR sample. Pushed only by VFX
/// effects whose attached light has `heat_haze: true` —
/// passive scene lights (torches, ambient flames) are
/// excluded by design so the world doesn't shimmer
/// permanently around them.
#[derive(Clone, Copy, Debug)]
pub struct HeatSource {
    /// World-space origin (the same point as the source
    /// light). Projected to screen UV in the composite path.
    pub position: Vec3,
    /// Falloff radius in metres. Drives the on-screen extent
    /// of the warp via a perspective projection.
    pub radius: f32,
    /// Strength in `[0, 1]`. Drives both the warp amplitude
    /// and the noise scroll rate. Should fade to 0 alongside
    /// the source effect's animation.
    pub strength: f32,
}

impl Renderer {
    /// Merge `point_lights` and `vfx_lights` into a single
    /// `[PointLight; MAX_POINT_LIGHTS]` with a deliberate
    /// ordering:
    ///
    ///   slots [0..n_shadow)     shadow-casting torches
    ///                           (`point_lights`, capped at
    ///                           `MAX_POINT_SHADOWS = 8`)
    ///   slots [n_shadow..M)     VFX lights — additive only,
    ///                           never cast shadows. A
    ///                           projectile-trail light sits
    ///                           *inside* the projectile mesh, so
    ///                           any cube shadow rendered for it
    ///                           would be occluded in every
    ///                           outward direction (back-faces of
    ///                           the fireball) and the world
    ///                           would go pitch black around it.
    ///   slots [M..16)           remaining torches as additive
    ///                           lights.
    ///
    /// The shader uses `lightIdx < pointShadowMeta.x` as
    /// the shadow-test, so shadow-casters MUST occupy the leading
    /// prefix. VFX lives just past that prefix, which also
    /// reserves it a slot even in dense torch rooms (worst
    /// case: 8 shadowed + 2 VFX + 6 plain = 16).
    ///
    /// Returns `(merged, light_count, n_shadow)`.
    pub(super) fn merge_per_frame_lights(&self) -> ([PointLight; MAX_POINT_LIGHTS], usize, usize) {
        let n_shadow = self.point_lights.len().min(shadow_point::MAX_POINT_SHADOWS);
        // Build the merged light list directly into a stack
        // array — saves the per-frame heap allocation that
        // a `.chain().take(N).collect()` version would do.
        // The default-init `PointLight` value never ships to
        // the GPU because `light_count` bounds every consumer.
        const DEFAULT_LIGHT: PointLight = PointLight {
            position: Vec3::ZERO,
            color: Vec3::ZERO,
            radius: 0.0,
            intensity: 0.0,
        };
        let mut merged = [DEFAULT_LIGHT; MAX_POINT_LIGHTS];
        let mut count = 0usize;
        let mut push = |src: PointLight, count: &mut usize| {
            if *count < MAX_POINT_LIGHTS {
                merged[*count] = src;
                *count += 1;
            }
        };
        for pl in self.point_lights.iter().take(n_shadow) {
            push(*pl, &mut count);
        }
        for pl in self.vfx_lights.iter() {
            push(*pl, &mut count);
        }
        for pl in self.point_lights.iter().skip(n_shadow) {
            push(*pl, &mut count);
        }
        (merged, count, n_shadow)
    }

    /// Build the per-face VPs for the point-light cube shadow atlas.
    /// Only the first `point_shadow_count` slots are populated; the
    /// rest stay identity (the shader only samples the active range
    /// via `pointShadowMeta.x`).
    pub(super) fn build_point_shadow_face_vp(
        &self,
        point_shadow_count: usize,
        merged_lights: &[PointLight; MAX_POINT_LIGHTS],
    ) -> [Mat4; shadow_point::MAX_POINT_SHADOWS * 6] {
        let mut face_vp = [Mat4::IDENTITY; shadow_point::MAX_POINT_SHADOWS * 6];
        for (i, pl) in merged_lights.iter().take(point_shadow_count).enumerate() {
            let faces = shadow_point::cube_face_view_projs(pl.position, pl.radius.max(0.1));
            for (f, m) in faces.iter().enumerate() {
                face_vp[i * 6 + f] = *m;
            }
        }
        face_vp
    }

    /// Build the camera/lighting/fog UBO from current renderer
    /// state plus the merged per-frame light list.
    pub(super) fn build_uniform_data(
        &self,
        merged_lights: &[PointLight; MAX_POINT_LIGHTS],
        light_count: usize,
        point_shadow_count: usize,
        point_shadow_face_vp: [Mat4; shadow_point::MAX_POINT_SHADOWS * 6],
    ) -> UniformData {
        let mut point_light_pos = [Vec4::ZERO; MAX_POINT_LIGHTS];
        let mut point_light_color = [Vec4::ZERO; MAX_POINT_LIGHTS];
        for (i, pl) in merged_lights.iter().take(light_count).enumerate() {
            point_light_pos[i] = Vec4::new(pl.position.x, pl.position.y, pl.position.z, pl.radius);
            point_light_color[i] = Vec4::new(pl.color.x, pl.color.y, pl.color.z, pl.intensity);
        }

        let light_dir_world = Vec4::new(
            self.key_light.direction.x,
            self.key_light.direction.y,
            self.key_light.direction.z,
            0.0,
        );
        let light_dir_normalized = light_dir_world.normalize();
        // Snap the shadow focus to the camera *target* (the
        // player / look-at point) projected onto y=0 — NOT the
        // camera position. The camera sits behind+above the
        // player, so anchoring the 28 m ortho box on the camera
        // makes the shadow frustum extend mostly behind the
        // player; the in-front cutoff lands only a few metres
        // past the player and reads as a square that tracks
        // the camera. Using `target` re-centres the box on the
        // player so the cutoff is symmetric and far enough out
        // in every direction that the edge feather in
        // `sampleShadow` hides it. The shadow module further
        // snaps to texel size to suppress shimmering.
        let shadow_focus = Vec3::new(self.camera.target.x, 0.0, self.camera.target.z);
        let light_vp = shadow::light_view_proj(
            shadow_focus,
            Vec3::new(
                light_dir_normalized.x,
                light_dir_normalized.y,
                light_dir_normalized.z,
            ),
        );

        UniformData {
            view: self.camera.view_matrix(),
            proj: self.camera.projection_matrix(),
            camera_pos: Vec4::new(
                self.camera.position.x,
                self.camera.position.y,
                self.camera.position.z,
                0.0,
            ),
            light_dir: light_dir_normalized,
            light_color: Vec4::new(
                self.key_light.color.x,
                self.key_light.color.y,
                self.key_light.color.z,
                self.key_light.ambient,
            ),
            fog_color: Vec4::new(self.fog_color[0], self.fog_color[1], self.fog_color[2], 0.0),
            fog_params: Vec4::new(self.fog_start, self.fog_end, 0.0, 0.0),
            fog_origin: Vec4::new(
                self.fog_origin.x,
                self.fog_origin.y,
                self.fog_origin.z,
                self.wall_xray_strength,
            ),
            point_light_pos,
            point_light_color,
            point_light_count: Vec4::new(light_count as f32, 0.0, 0.0, 0.0),
            light_vp,
            point_shadow_face_vp,
            point_shadow_meta: Vec4::new(point_shadow_count as f32, 0.0, 0.0, 0.0),
            // `time` packs scalar globals consumed by the
            // forward fragment shader:
            //   x = elapsed seconds (used by VFX time-driven
            //       hashes and the blood splat age curve)
            //   y = floor_y_min   (lowest walkable plane Y)
            //   z = floor_y_max   (highest walkable plane Y)
            //   w = unused
            // The blood-field shader gate accepts fragments
            // whose Y is within a small epsilon of
            // `[floor_y_min, floor_y_max]` so dungeons with
            // raised platforms / lowered pits still receive
            // splats on every walkable surface.
            time: Vec4::new(
                self.start_time.elapsed().as_secs_f32(),
                self.blood_field.floor_y,
                self.blood_field.floor_y_max,
                0.0,
            ),
            blood_field_xform: self.blood_field.world_xform,
            player_room_aabb: self.player_room_aabb,
        }
    }

    /// Project the sun direction into screen UV for the godrays
    /// post pass. Returns `(sun_screen, sun_color)` where
    /// `sun_screen = [u, v, strength, _]`. Both go zero when
    /// the sky is disabled or the sun is behind the camera.
    pub(super) fn compute_sun_screen_uv(&self) -> ([f32; 4], [f32; 4]) {
        if !self.sky.enabled || self.sky.sun_strength <= 0.001 {
            return ([0.0; 4], [0.0; 4]);
        }
        let view = self.camera.view_matrix();
        let proj = self.camera.projection_matrix();
        let sd = self.sky.sun_dir.normalize();
        // Direction in view space (w=0 → infinitely far).
        let view_dir = view.transform_vector3(sd);
        if view_dir.z >= -0.05 {
            return ([0.0; 4], [0.0; 4]);
        }
        // Sun in front of camera. Project a point far along
        // that direction (distance has no effect on UV under
        // perspective when the point is treated as on a ray
        // from the eye, but using a finite distance gives
        // well-behaved w).
        let world_pt = self.camera.position + sd * 1000.0;
        let clip = proj * view * Vec4::new(world_pt.x, world_pt.y, world_pt.z, 1.0);
        if clip.w <= 0.0 {
            return ([0.0; 4], [0.0; 4]);
        }
        let ndc = Vec3::new(clip.x, clip.y, clip.z) / clip.w;
        // Vulkan/GLSL UV: (ndc.xy * 0.5 + 0.5).
        // The renderer flips Y in proj so this matches the
        // depth-sample UV convention already in the composite
        // shader.
        let uv = Vec3::new(ndc.x, ndc.y, 0.0) * 0.5 + Vec3::new(0.5, 0.5, 0.0);
        // Strength scales with how centred the sun is in view
        // (cosine of angle to camera forward) so off-screen
        // rays still appear but fade as the sun leaves the
        // frustum.
        let cam_fwd = -Vec3::new(view.row(2).x, view.row(2).y, view.row(2).z).normalize();
        let cosine = sd.dot(cam_fwd).max(0.0);
        // Bumped from `0.6` to `1.0` so the sandstorm hub's
        // sun (sun_strength = 1.4) drives the post pass at
        // ~1.4 instead of ~0.84 — the rays read clearly
        // through the dust without the sun disc itself going
        // blowout. Clamped to 2.0 so future skies with
        // sun_strength > 2 don't over-saturate the composite.
        let strength = (self.sky.sun_strength * 1.0 * cosine).clamp(0.0, 2.0);
        (
            [uv.x, uv.y, strength, 1.0],
            [
                self.sky.cloud_flash_color.x.max(1.0),
                self.sky.cloud_flash_color.y.max(0.95),
                self.sky.cloud_flash_color.z.max(0.85),
                1.0,
            ],
        )
    }

    /// Project the strongest active VFX-published heat source to
    /// screen UV for the composite pass's heat-haze warp. Only one
    /// source is forwarded per frame; additional bursts take over
    /// when the strongest fades.
    pub(super) fn compute_heat_source_uv(&self) -> [f32; 4] {
        let view = self.camera.view_matrix();
        let proj = self.camera.projection_matrix();
        let mut best: Option<(f32, [f32; 4])> = None;
        for hs in self.heat_sources.iter() {
            if hs.strength < 1e-3 {
                continue;
            }

            let world = Vec4::new(hs.position.x, hs.position.y, hs.position.z, 1.0);
            let view_p = view * world;
            if view_p.z >= -0.05 {
                continue;
            }
            let clip = proj * view_p;
            if clip.w <= 0.0 {
                continue;
            }
            let ndc = Vec3::new(clip.x, clip.y, clip.z) / clip.w;
            let uv = Vec3::new(ndc.x, ndc.y, 0.0) * 0.5 + Vec3::new(0.5, 0.5, 0.0);
            if uv.x < -0.2 || uv.x > 1.2 || uv.y < -0.2 || uv.y > 1.2 {
                continue;
            }
            let dist = (-view_p.z).max(0.1);
            // proj[1][1] is the y-focal term; with the
            // renderer's flipped-Y projection it's negative,
            // but we only want magnitude.
            let focal_y = proj.col(1).y.abs();
            let radius_uv = (hs.radius / dist) * focal_y * 0.5;
            if radius_uv < 0.02 {
                continue;
            }
            let s = hs.strength.clamp(0.0, 1.0);
            if best.map(|(prev, _)| s > prev).unwrap_or(true) {
                best = Some((s, [uv.x, uv.y, radius_uv.min(0.6), s]));
            }
        }
        best.map(|(_, v)| v).unwrap_or([0.0; 4])
    }
}
