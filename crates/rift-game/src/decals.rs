use glam::{Mat4, Vec2, Vec3};
use rift_engine::renderer::mesh::{Mesh, Vertex};
use rift_engine::Renderer;

/// Maximum blood splatters per floor (to cap GPU memory).
const MAX_DECALS: usize = 120;

/// Manages persistent blood splatter decals on floors/walls.
pub struct DecalSystem {
    /// Render object indices for active decals.
    decal_indices: Vec<usize>,
    /// Simple RNG state for procedural splatter generation.
    rng: u32,
}

impl DecalSystem {
    pub fn new() -> Self {
        Self {
            decal_indices: Vec::new(),
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
    /// Places 2-4 floor decals and 0-1 wall decal near the position.
    pub fn spawn_blood(
        &mut self,
        position: Vec3,
        wall_aabbs: &[rift_engine::physics::Aabb],
        renderer: &mut Renderer,
    ) {
        if self.decal_indices.len() >= MAX_DECALS {
            // Recycle oldest decal
            let oldest = self.decal_indices.remove(0);
            if oldest < renderer.objects.len() {
                renderer.objects[oldest].model_matrix = Mat4::ZERO;
            }
        }

        // Floor splatters (2-4 around death position)
        let floor_count = 2 + (self.rand() * 3.0) as u32;
        for _ in 0..floor_count {
            let offset_x = self.rand_range(-1.2, 1.2);
            let offset_z = self.rand_range(-1.2, 1.2);
            let splat_pos = Vec3::new(
                position.x + offset_x,
                0.02, // just above floor to avoid z-fighting
                position.z + offset_z,
            );
            let size = self.rand_range(0.3, 0.9);
            let mesh = self.gen_splatter_mesh(size, false);
            let rot_angle = self.rand_range(0.0, std::f32::consts::TAU);
            let model = Mat4::from_translation(splat_pos)
                * Mat4::from_rotation_y(rot_angle);

            if renderer.add_mesh(&mesh, model).is_ok() {
                self.decal_indices.push(renderer.objects.len() - 1);
            }
        }

        // Wall splatter: check if any wall is nearby
        let check_dirs = [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z];
        for dir in &check_dirs {
            let ray_origin = position + Vec3::new(0.0, 0.5, 0.0);
            let ray = rift_engine::physics::Ray::new(ray_origin, *dir);
            if let Some(hit) = rift_engine::physics::raycast(&ray, 2.0, wall_aabbs) {
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

                let model = Mat4::from_cols(
                    rot_right.extend(0.0),
                    wall_normal.extend(0.0),
                    rot_up.extend(0.0),
                    wall_pos.extend(1.0),
                );

                if renderer.add_mesh(&mesh, model).is_ok() {
                    self.decal_indices.push(renderer.objects.len() - 1);
                }
                break; // only one wall splatter per death
            }
        }
    }

    /// Generate a procedural blood splatter mesh (irregular polygon).
    fn gen_splatter_mesh(&mut self, size: f32, _is_wall: bool) -> Mesh {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        // Main splatter: irregular radial polygon
        let segments = 8 + (self.rand() * 5.0) as usize;
        let center_color = Vec3::new(0.4, 0.02, 0.02); // dark blood red
        let edge_color = Vec3::new(0.55, 0.05, 0.03);  // slightly brighter edge

        // Center vertex
        vertices.push(Vertex {
            position: Vec3::ZERO,
            normal: Vec3::Y,
            color: center_color,
            uv: Vec2::new(0.5, 0.5),
        });

        // Edge vertices with random radii for organic look
        for i in 0..segments {
            let angle = (i as f32 / segments as f32) * std::f32::consts::TAU;
            let r = size * self.rand_range(0.5, 1.0);
            let x = angle.cos() * r;
            let z = angle.sin() * r;
            vertices.push(Vertex {
                position: Vec3::new(x, 0.0, z),
                normal: Vec3::Y,
                color: edge_color,
                uv: Vec2::new(0.5, 0.5),
            });
        }

        // Fan triangles from center (CCW winding when viewed from +Y)
        for i in 0..segments {
            let next = (i + 1) % segments;
            indices.push(0);
            indices.push(1 + next as u32);
            indices.push(1 + i as u32);
        }

        // Add 2-4 smaller satellite droplets around the main splatter
        let droplet_count = 2 + (self.rand() * 3.0) as usize;
        for _ in 0..droplet_count {
            let angle = self.rand_range(0.0, std::f32::consts::TAU);
            let dist = size * self.rand_range(0.8, 1.6);
            let drop_center = Vec3::new(angle.cos() * dist, 0.0, angle.sin() * dist);
            let drop_r = size * self.rand_range(0.1, 0.25);
            let drop_segs = 5;
            let base_idx = vertices.len() as u32;

            let drop_color = Vec3::new(
                self.rand_range(0.35, 0.5),
                0.02,
                0.02,
            );

            vertices.push(Vertex {
                position: drop_center,
                normal: Vec3::Y,
                color: drop_color,
                uv: Vec2::new(0.5, 0.5),
            });

            for j in 0..drop_segs {
                let a = (j as f32 / drop_segs as f32) * std::f32::consts::TAU;
                let dr = drop_r * self.rand_range(0.6, 1.0);
                vertices.push(Vertex {
                    position: drop_center + Vec3::new(a.cos() * dr, 0.0, a.sin() * dr),
                    normal: Vec3::Y,
                    color: drop_color,
                    uv: Vec2::new(0.5, 0.5),
                });
            }

            for j in 0..drop_segs {
                let next = (j + 1) % drop_segs;
                indices.push(base_idx);
                indices.push(base_idx + 1 + next as u32);
                indices.push(base_idx + 1 + j as u32);
            }
        }

        // Add elongated streaks (thin triangles radiating outward)
        let streak_count = 1 + (self.rand() * 3.0) as usize;
        for _ in 0..streak_count {
            let angle = self.rand_range(0.0, std::f32::consts::TAU);
            let length = size * self.rand_range(1.0, 2.0);
            let width = size * self.rand_range(0.04, 0.1);
            let base_idx = vertices.len() as u32;

            let streak_color = Vec3::new(0.5, 0.03, 0.02);
            let dir = Vec3::new(angle.cos(), 0.0, angle.sin());
            let perp = Vec3::new(-angle.sin(), 0.0, angle.cos());

            // Base (near center)
            let start = dir * size * 0.3;
            vertices.push(Vertex {
                position: start + perp * width,
                normal: Vec3::Y,
                color: streak_color,
                uv: Vec2::new(0.5, 0.5),
            });
            vertices.push(Vertex {
                position: start - perp * width,
                normal: Vec3::Y,
                color: streak_color,
                uv: Vec2::new(0.5, 0.5),
            });
            // Tip
            vertices.push(Vertex {
                position: dir * length,
                normal: Vec3::Y,
                color: Vec3::new(0.3, 0.01, 0.01),
                uv: Vec2::new(0.5, 0.5),
            });

            indices.push(base_idx);
            indices.push(base_idx + 2);
            indices.push(base_idx + 1);
        }

        Mesh { vertices, indices }
    }

    /// Clear all decals (call on floor transition).
    pub fn clear(&mut self) {
        // Render objects get cleared by clear_objects() in floor generation
        self.decal_indices.clear();
    }
}
