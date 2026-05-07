use glam::{Mat4, Vec2, Vec3};
use crate::renderer::mesh::{Mesh, Vertex};
use crate::Renderer;

/// Maximum blood splatters per floor (to cap GPU memory).
const MAX_DECALS: usize = 120;

/// How long a single splat takes to "spread out" once it begins.
const GROW_DURATION: f32 = 0.22;
/// How long the bright wet sheen lingers before settling to its
/// final dried tone (cosmetic; affects scale wobble only).
const SETTLE_DURATION: f32 = 0.35;

/// One animated decal entry: tracks the base transform plus a
/// per-decal stagger delay so satellite droplets and streaks land a
/// fraction of a second after the main pool, the way real splatter
/// scatters in stages.
struct DecalEntry {
    obj_idx: usize,
    base_model: Mat4,
    /// Seconds until this decal becomes visible / starts growing.
    spawn_delay: f32,
    /// Time since spawn_blood() was called.
    age: f32,
    /// Once the grow + settle animation is complete we stop touching
    /// this object's model matrix to keep per-frame cost flat.
    done: bool,
}

/// Manages persistent blood splatter decals on floors/walls.
pub struct DecalSystem {
    /// Animated decal records; old ones recycled when MAX_DECALS hit.
    decals: Vec<DecalEntry>,
    /// Simple RNG state for procedural splatter generation.
    rng: u32,
}

impl DecalSystem {
    pub fn new() -> Self {
        Self {
            decals: Vec::new(),
            rng: 7919,
        }
    }

    fn rand(&mut self) -> f32 {
        self.rng = self.rng.wrapping_mul(1664525).wrapping_add(1013904223);
        (self.rng >> 16) as f32 / 65535.0
    }

    fn rand_range(&mut self, min: f32, max: f32) -> f32 {
        min + self.rand() * (max - min)
    }

    /// Spawn blood splatters at a death position.
    /// Places a main pool plus 2-4 staggered satellite splats and an
    /// optional wall splatter.  Each piece is animated by `update()`.
    pub fn spawn_blood(
        &mut self,
        position: Vec3,
        wall_aabbs: &[rift_math::physics::Aabb],
        renderer: &mut Renderer,
    ) {
        // Recycle the oldest decal if we're at the cap.
        while self.decals.len() >= MAX_DECALS {
            let oldest = self.decals.remove(0);
            if oldest.obj_idx < renderer.objects.len() {
                renderer.objects[oldest.obj_idx].model_matrix = Mat4::ZERO;
            }
        }

        // Floor splatters: a single dominant pool plus 2-4 smaller
        // peripheral pools so kills "feel heavy" rather than uniform.
        let floor_count = 3 + (self.rand() * 3.0) as u32;
        for i in 0..floor_count {
            let offset_x = self.rand_range(-1.4, 1.4);
            let offset_z = self.rand_range(-1.4, 1.4);
            let splat_pos = Vec3::new(
                position.x + offset_x,
                0.02, // just above floor to avoid z-fighting
                position.z + offset_z,
            );
            // First splat is the big one; the rest are 25-55% size.
            let size = if i == 0 {
                self.rand_range(0.7, 1.1)
            } else {
                self.rand_range(0.25, 0.55)
            };
            let mesh = self.gen_splatter_mesh(size, false);
            let rot_angle = self.rand_range(0.0, std::f32::consts::TAU);
            let base_model = Mat4::from_translation(splat_pos)
                * Mat4::from_rotation_y(rot_angle);

            // Satellites get a small delay so they appear to land
            // after the main pool — sells the "thrown" feeling.
            let spawn_delay = if i == 0 {
                0.0
            } else {
                self.rand_range(0.04, 0.32)
            };

            // Start invisible (model = ZERO); update() will scale up
            // once spawn_delay elapses.
            if renderer.add_mesh(&mesh, Mat4::ZERO).is_ok() {
                self.decals.push(DecalEntry {
                    obj_idx: renderer.objects.len() - 1,
                    base_model,
                    spawn_delay,
                    age: 0.0,
                    done: false,
                });
            }
        }

        // Wall splatter: check if any wall is nearby
        let check_dirs = [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z];
        for dir in &check_dirs {
            let ray_origin = position + Vec3::new(0.0, 0.5, 0.0);
            let ray = rift_math::physics::Ray::new(ray_origin, *dir);
            if let Some(hit) = rift_math::physics::raycast(&ray, 2.0, wall_aabbs) {
                // Skip if ray started inside the wall (t~0)
                if hit.distance < 0.05 {
                    continue;
                }
                // Place splatter on wall (normal is opposite of ray direction)
                let wall_normal = -*dir;
                let wall_pos = hit.point + wall_normal * 0.05;
                let size = self.rand_range(0.25, 0.6);
                let mesh = self.gen_splatter_mesh(size, true);

                // Orient the decal flat against the wall:
                // Mesh is on XZ plane with +Y normal.
                // We want local +Y to map to wall_normal (outward from wall).
                // Build a right-handed basis so winding is preserved (det = +1).
                let world_up = Vec3::Y;
                let right = wall_normal.cross(world_up).normalize();
                let up = right.cross(wall_normal).normalize();

                let rot_angle = self.rand_range(0.0, std::f32::consts::TAU);
                // Rotate right/up around wall_normal
                let cos_a = rot_angle.cos();
                let sin_a = rot_angle.sin();
                let rot_right = right * cos_a + up * sin_a;
                let rot_up = -right * sin_a + up * cos_a;

                let base_model = Mat4::from_cols(
                    rot_right.extend(0.0),
                    wall_normal.extend(0.0),
                    rot_up.extend(0.0),
                    wall_pos.extend(1.0),
                );

                if renderer.add_mesh(&mesh, Mat4::ZERO).is_ok() {
                    let delay = self.rand_range(0.05, 0.15);
                    self.decals.push(DecalEntry {
                        obj_idx: renderer.objects.len() - 1,
                        base_model,
                        // Wall hit is a splash from the kill itself, so it
                        // appears almost immediately after the main pool.
                        spawn_delay: delay,
                        age: 0.0,
                        done: false,
                    });
                }
                break; // only one wall splatter per death
            }
        }
    }

    /// Per-frame update: drives the spawn-delay and grow animation
    /// for every active decal.  Each decal:
    ///   1. stays invisible (model_matrix = ZERO) until its delay elapses,
    ///   2. scales 0 → ~1.06 over GROW_DURATION (ease-out), and
    ///   3. settles back to 1.0 with a tiny wobble during SETTLE_DURATION.
    /// Once fully settled the entry is marked `done` and skipped.
    pub fn update(&mut self, dt: f32, renderer: &mut Renderer) {
        for d in self.decals.iter_mut() {
            if d.done {
                continue;
            }
            if d.obj_idx >= renderer.objects.len() {
                d.done = true;
                continue;
            }
            d.age += dt;
            let local_t = d.age - d.spawn_delay;
            if local_t < 0.0 {
                // Still waiting to "land".
                renderer.objects[d.obj_idx].model_matrix = Mat4::ZERO;
                continue;
            }

            // Phase 1 — splat & spread: scale ramps from 0 to ~1.06
            // with an ease-out for an impactful pop.
            let scale = if local_t < GROW_DURATION {
                let t = local_t / GROW_DURATION;
                // Ease-out cubic, overshoot to 1.06.
                let eased = 1.0 - (1.0 - t).powi(3);
                eased * 1.06
            } else if local_t < GROW_DURATION + SETTLE_DURATION {
                // Phase 2 — settle: ease back from 1.06 to 1.0.
                let t = (local_t - GROW_DURATION) / SETTLE_DURATION;
                let eased = 1.0 - (1.0 - t).powi(2);
                1.06 - eased * 0.06
            } else {
                d.done = true;
                1.0
            };

            // Apply scale around the decal origin.  For floor splats
            // the base_model is translation*Y-rotation, so right-
            // multiplying by Mat4::from_scale leaves the world position
            // intact and just scales the local mesh.  For the wall
            // splatter the basis matrix has unit-length columns so the
            // same trick scales the patch in place.
            renderer.objects[d.obj_idx].model_matrix =
                d.base_model * Mat4::from_scale(Vec3::splat(scale));
        }
    }

    /// Generate a procedural blood splatter mesh.
    ///
    /// We aim for a "wet, recently-spilled" look layered out of
    /// three concentric rings plus radial extras:
    ///
    /// 1. **Wet sheen** — a small inner disc using a slightly
    ///    super-bright red so the HDR bloom pass picks it up as
    ///    a faint glint. Sells the "still wet, light catches it"
    ///    micro-highlight.
    /// 2. **Viscous mid** — the main pool body in a saturated
    ///    fresh-blood red.
    /// 3. **Dry rim** — a darker, slightly desaturated outer
    ///    ring with a jagged irregular outline.
    ///
    /// Around the pool we add elongated streaks (longer + thinner
    /// than before) and a denser scatter of micro-droplets with
    /// wider size jitter.
    fn gen_splatter_mesh(&mut self, size: f32, _is_wall: bool) -> Mesh {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        // Multi-tone palette for "fresh blood on stone".
        // `sheen` is intentionally pushed slightly above the
        // standard sRGB range on R so the new HDR/bloom pipeline
        // catches it as a wet-light highlight without making the
        // splat itself look pink.
        let sheen      = Vec3::new(0.55, 0.040, 0.025);
        let core_wet   = Vec3::new(0.40, 0.028, 0.016);
        let mid_visc   = Vec3::new(0.30, 0.020, 0.012);
        let rim_dry    = Vec3::new(0.18, 0.012, 0.008);

        // ---- Outer pool: irregular jagged disk (rim_dry edge) ----
        let segments = 18 + (self.rand() * 8.0) as usize;
        let center_idx = vertices.len() as u32;
        vertices.push(Vertex {
            position: Vec3::new(0.0, 0.001, 0.0),
            normal: Vec3::Y,
            color: mid_visc,
            uv: Vec2::new(0.5, 0.5),
        });
        for i in 0..segments {
            let angle = (i as f32 / segments as f32) * std::f32::consts::TAU;
            // Jagged outline — taller spikes, occasional pinches.
            let spike = if self.rand() < 0.22 {
                self.rand_range(1.10, 1.55)
            } else if self.rand() < 0.10 {
                self.rand_range(0.70, 0.90)
            } else {
                1.0
            };
            let r = size * self.rand_range(0.55, 0.95) * spike;
            let x = angle.cos() * r;
            let z = angle.sin() * r;
            // Per-vertex jitter on the dry rim to break uniformity.
            let c = rim_dry * self.rand_range(0.80, 1.10);
            vertices.push(Vertex {
                position: Vec3::new(x, 0.001, z),
                normal: Vec3::Y,
                color: c,
                uv: Vec2::new(0.5, 0.5),
            });
        }
        for i in 0..segments {
            let next = ((i + 1) % segments) as u32;
            indices.push(center_idx);
            indices.push(center_idx + 1 + next);
            indices.push(center_idx + 1 + i as u32);
        }

        // ---- Mid ring: viscous body (core_wet inner, mid_visc outer) ----
        // A second, smaller disk sits on top of the rim disk to
        // give the pool a darker outer / brighter inner gradient
        // without needing an extra texture.
        let mid_segs = 14;
        let mid_center = vertices.len() as u32;
        vertices.push(Vertex {
            position: Vec3::new(0.0, 0.0015, 0.0),
            normal: Vec3::Y,
            color: core_wet,
            uv: Vec2::new(0.5, 0.5),
        });
        for i in 0..mid_segs {
            let angle = (i as f32 / mid_segs as f32) * std::f32::consts::TAU;
            let r = size * self.rand_range(0.40, 0.62);
            vertices.push(Vertex {
                position: Vec3::new(angle.cos() * r, 0.0015, angle.sin() * r),
                normal: Vec3::Y,
                color: mid_visc,
                uv: Vec2::new(0.5, 0.5),
            });
        }
        for i in 0..mid_segs {
            let next = ((i + 1) % mid_segs) as u32;
            indices.push(mid_center);
            indices.push(mid_center + 1 + next);
            indices.push(mid_center + 1 + i as u32);
        }

        // ---- Wet sheen: tiny bright inner disc for HDR glint ----
        let sheen_segs = 10;
        let sheen_center = vertices.len() as u32;
        // Slight off-centre offset — the highlight rarely sits
        // exactly on the geometric centre of a real splat.
        let sheen_off = Vec3::new(
            self.rand_range(-0.08, 0.08) * size,
            0.0020,
            self.rand_range(-0.08, 0.08) * size,
        );
        vertices.push(Vertex {
            position: sheen_off,
            normal: Vec3::Y,
            color: sheen,
            uv: Vec2::new(0.5, 0.5),
        });
        for i in 0..sheen_segs {
            let angle = (i as f32 / sheen_segs as f32) * std::f32::consts::TAU;
            let r = size * self.rand_range(0.12, 0.22);
            vertices.push(Vertex {
                position: sheen_off + Vec3::new(angle.cos() * r, 0.0, angle.sin() * r),
                normal: Vec3::Y,
                // Fade to core_wet at the highlight edge so it
                // blends rather than abrupt-cuts.
                color: core_wet,
                uv: Vec2::new(0.5, 0.5),
            });
        }
        for i in 0..sheen_segs {
            let next = ((i + 1) % sheen_segs) as u32;
            indices.push(sheen_center);
            indices.push(sheen_center + 1 + next);
            indices.push(sheen_center + 1 + i as u32);
        }

        // ---- Satellite droplets: denser + wider size jitter ----
        let droplet_count = 8 + (self.rand() * 8.0) as usize;
        for _ in 0..droplet_count {
            let angle = self.rand_range(0.0, std::f32::consts::TAU);
            let dist = size * self.rand_range(0.85, 2.1);
            let drop_center = Vec3::new(angle.cos() * dist, 0.002, angle.sin() * dist);
            // Wider jitter — some are hair-thin, some are pea-sized.
            let drop_r = size * self.rand_range(0.04, 0.22);
            let drop_segs = 6;
            let base_idx = vertices.len() as u32;
            // Smaller, more distant droplets dry faster (rim_dry).
            let outer_color = if drop_r < size * 0.10 { rim_dry } else { mid_visc };
            vertices.push(Vertex {
                position: drop_center,
                normal: Vec3::Y,
                color: core_wet,
                uv: Vec2::new(0.5, 0.5),
            });
            for j in 0..drop_segs {
                let a = (j as f32 / drop_segs as f32) * std::f32::consts::TAU;
                let dr = drop_r * self.rand_range(0.7, 1.0);
                vertices.push(Vertex {
                    position: drop_center
                        + Vec3::new(a.cos() * dr, 0.0, a.sin() * dr),
                    normal: Vec3::Y,
                    color: outer_color,
                    uv: Vec2::new(0.5, 0.5),
                });
            }
            for j in 0..drop_segs {
                let next = ((j + 1) % drop_segs) as u32;
                indices.push(base_idx);
                indices.push(base_idx + 1 + next);
                indices.push(base_idx + 1 + j as u32);
            }
        }

        // ---- Long thin radiating streaks (longer + tapered) ----
        let streak_count = 4 + (self.rand() * 5.0) as usize;
        for _ in 0..streak_count {
            let angle = self.rand_range(0.0, std::f32::consts::TAU);
            let length = size * self.rand_range(1.6, 3.2);
            let width  = size * self.rand_range(0.020, 0.060);
            let base_idx = vertices.len() as u32;

            let dir  = Vec3::new(angle.cos(), 0.0, angle.sin());
            let perp = Vec3::new(-angle.sin(), 0.0, angle.cos());

            // Base of streak overlaps the pool rim — wet & bright.
            let start = dir * size * 0.35;
            vertices.push(Vertex {
                position: start + perp * width + Vec3::new(0.0, 0.001, 0.0),
                normal: Vec3::Y,
                color: core_wet,
                uv: Vec2::new(0.5, 0.5),
            });
            vertices.push(Vertex {
                position: start - perp * width + Vec3::new(0.0, 0.001, 0.0),
                normal: Vec3::Y,
                color: core_wet,
                uv: Vec2::new(0.5, 0.5),
            });
            // Mid: midway tone, slightly thinner.
            let mid = dir * length * 0.55;
            let mid_w = width * 0.7;
            let mid_color = mid_visc;
            vertices.push(Vertex {
                position: mid + perp * mid_w + Vec3::new(0.0, 0.001, 0.0),
                normal: Vec3::Y,
                color: mid_color,
                uv: Vec2::new(0.5, 0.5),
            });
            vertices.push(Vertex {
                position: mid - perp * mid_w + Vec3::new(0.0, 0.001, 0.0),
                normal: Vec3::Y,
                color: mid_color,
                uv: Vec2::new(0.5, 0.5),
            });
            // Tip: tapered point, dry-rim tone.
            vertices.push(Vertex {
                position: dir * length + Vec3::new(0.0, 0.001, 0.0),
                normal: Vec3::Y,
                color: rim_dry,
                uv: Vec2::new(0.5, 0.5),
            });

            // Quad (start→mid) split into two tris, plus tri (mid→tip).
            indices.extend_from_slice(&[
                base_idx,     base_idx + 2, base_idx + 1,
                base_idx + 1, base_idx + 2, base_idx + 3,
                base_idx + 2, base_idx + 4, base_idx + 3,
            ]);
        }

        Mesh { vertices, indices }
    }

    /// Clear all decals (call on floor transition).
    pub fn clear(&mut self) {
        // Render objects get cleared by clear_objects() in floor generation
        self.decals.clear();
    }
}
