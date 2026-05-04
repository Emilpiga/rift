use ash::vk;
use glam::{Vec2, Vec3};

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Vertex {
    pub position: Vec3,
    pub normal: Vec3,
    pub color: Vec3,
    pub uv: Vec2,
}

impl Vertex {
    pub fn binding_description() -> vk::VertexInputBindingDescription {
        vk::VertexInputBindingDescription {
            binding: 0,
            stride: std::mem::size_of::<Self>() as u32,
            input_rate: vk::VertexInputRate::VERTEX,
        }
    }

    pub fn attribute_descriptions() -> [vk::VertexInputAttributeDescription; 4] {
        [
            // position
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 0,
                format: vk::Format::R32G32B32_SFLOAT,
                offset: 0,
            },
            // normal
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 1,
                format: vk::Format::R32G32B32_SFLOAT,
                offset: std::mem::size_of::<Vec3>() as u32,
            },
            // color
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 2,
                format: vk::Format::R32G32B32_SFLOAT,
                offset: (std::mem::size_of::<Vec3>() * 2) as u32,
            },
            // uv
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 3,
                format: vk::Format::R32G32_SFLOAT,
                offset: (std::mem::size_of::<Vec3>() * 3) as u32,
            },
        ]
    }
}

/// A simple indexed mesh with vertex and index data.
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl Mesh {
    /// A colored cube centered at origin with side length 1.
    pub fn cube() -> Self {
        Self::player()
    }

    /// Player — wispy hooded wraith. Single tapered body (wide at top, narrows
    /// to a wisp tail) + glowing eyes. Floats slightly off the ground.
    /// (Arms are rendered separately by PlayerArms.)
    pub fn player() -> Self {
        let body = Vec3::new(0.35, 0.65, 1.30);     // bright spectral blue HDR
        let eye  = Vec3::new(1.60, 1.40, 0.50);     // gold (HDR)
        Self::wraith(body, body * 0.5, eye, 0.32, 1.45, 0.15)
    }

    /// A flat grid on the XZ plane (floor).
    pub fn grid(size: f32, divisions: u32) -> Self {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        let step = size / divisions as f32;
        let half = size / 2.0;

        for z in 0..=divisions {
            for x in 0..=divisions {
                let px = -half + x as f32 * step;
                let pz = -half + z as f32 * step;
                let checker = ((x + z) % 2 == 0) as u32 as f32;
                let gray = 0.15 + checker * 0.1;
                vertices.push(Vertex {
                    position: Vec3::new(px, 0.0, pz),
                    normal: Vec3::Y,
                    color: Vec3::new(gray, gray, gray),
                    uv: Vec2::new(x as f32 / divisions as f32, z as f32 / divisions as f32),
                });
            }
        }

        for z in 0..divisions {
            for x in 0..divisions {
                let tl = z * (divisions + 1) + x;
                let tr = tl + 1;
                let bl = tl + (divisions + 1);
                let br = bl + 1;
                indices.extend_from_slice(&[tl, bl, tr, tr, bl, br]);
            }
        }

        Self { vertices, indices }
    }

    /// Batch a mesh at multiple positions into a single combined mesh.
    /// Dramatically reduces draw calls for repeated static geometry (walls, floor tiles).
    pub fn batch_at_positions(base: &Mesh, positions: &[Vec3]) -> Self {
        let verts_per = base.vertices.len();
        let idxs_per = base.indices.len();
        let mut vertices = Vec::with_capacity(verts_per * positions.len());
        let mut indices = Vec::with_capacity(idxs_per * positions.len());

        for (i, pos) in positions.iter().enumerate() {
            let base_idx = (i * verts_per) as u32;
            for v in &base.vertices {
                let world_pos = v.position + *pos;
                // Use world-space UVs for seamless texture tiling
                vertices.push(Vertex {
                    position: world_pos,
                    normal: v.normal,
                    color: v.color,
                    uv: Vec2::new(v.uv.x + pos.x + pos.z, v.uv.y),
                });
            }
            for idx in &base.indices {
                indices.push(idx + base_idx);
            }
        }

        Self { vertices, indices }
    }

    /// A player arm — a small cuboid extending along +Z from the shoulder (origin)
    /// to the hand at z=1.0. Designed to be scaled (e.g. (0.10, 0.10, 0.55)) and
    /// rotated to point in the player's aim direction.
    pub fn player_arm() -> Self {
        let skin = Vec3::new(0.55, 0.42, 0.32);  // muted leather/skin tone
        let cuff = Vec3::new(0.30, 0.22, 0.16);  // darker at the hand
        let v = |pos: [f32; 3], normal: [f32; 3], color: Vec3| Vertex {
            position: Vec3::from(pos),
            normal: Vec3::from(normal),
            color,
            uv: Vec2::new(0.5, 0.5),
        };

        // Cuboid: x = ±0.5, y = ±0.5, z = 0..1
        // Shoulder end (z=0) uses `skin`, hand end (z=1) uses `cuff` for a subtle gradient.
        let vertices = vec![
            // Front face (z = 1, hand)
            v([-0.5, -0.5, 1.0], [0.0, 0.0, 1.0], cuff),
            v([ 0.5, -0.5, 1.0], [0.0, 0.0, 1.0], cuff),
            v([ 0.5,  0.5, 1.0], [0.0, 0.0, 1.0], cuff),
            v([-0.5,  0.5, 1.0], [0.0, 0.0, 1.0], cuff),
            // Back face (z = 0, shoulder)
            v([ 0.5, -0.5, 0.0], [0.0, 0.0, -1.0], skin),
            v([-0.5, -0.5, 0.0], [0.0, 0.0, -1.0], skin),
            v([-0.5,  0.5, 0.0], [0.0, 0.0, -1.0], skin),
            v([ 0.5,  0.5, 0.0], [0.0, 0.0, -1.0], skin),
            // Top face (y = 0.5)
            v([-0.5, 0.5, 0.0], [0.0, 1.0, 0.0], skin),
            v([ 0.5, 0.5, 0.0], [0.0, 1.0, 0.0], skin),
            v([ 0.5, 0.5, 1.0], [0.0, 1.0, 0.0], cuff),
            v([-0.5, 0.5, 1.0], [0.0, 1.0, 0.0], cuff),
            // Bottom face (y = -0.5)
            v([-0.5, -0.5, 1.0], [0.0, -1.0, 0.0], cuff),
            v([ 0.5, -0.5, 1.0], [0.0, -1.0, 0.0], cuff),
            v([ 0.5, -0.5, 0.0], [0.0, -1.0, 0.0], skin),
            v([-0.5, -0.5, 0.0], [0.0, -1.0, 0.0], skin),
            // Right face (x = 0.5)
            v([0.5, -0.5, 0.0], [1.0, 0.0, 0.0], skin),
            v([0.5, -0.5, 1.0], [1.0, 0.0, 0.0], cuff),
            v([0.5,  0.5, 1.0], [1.0, 0.0, 0.0], cuff),
            v([0.5,  0.5, 0.0], [1.0, 0.0, 0.0], skin),
            // Left face (x = -0.5)
            v([-0.5, -0.5, 1.0], [-1.0, 0.0, 0.0], cuff),
            v([-0.5, -0.5, 0.0], [-1.0, 0.0, 0.0], skin),
            v([-0.5,  0.5, 0.0], [-1.0, 0.0, 0.0], skin),
            v([-0.5,  0.5, 1.0], [-1.0, 0.0, 0.0], cuff),
        ];

        let indices = vec![
            0,  1,  2,  2,  3,  0,
            4,  5,  6,  6,  7,  4,
            8,  9,  10, 10, 11, 8,
            12, 13, 14, 14, 15, 12,
            16, 17, 18, 18, 19, 16,
            20, 21, 22, 22, 23, 20,
        ];

        Self { vertices, indices }
    }

    /// A wall segment (1x5x1 — tall enough that the camera can't see over it).
    pub fn wall() -> Self {
        Self::wall_colored(Vec3::new(0.35, 0.3, 0.28))
    }

    /// A wall segment with a custom color (for per-floor theme).
    pub fn wall_colored(color: Vec3) -> Self {
        let h = 5.0_f32; // Wall height
        let v = |pos: [f32; 3], normal: [f32; 3], uv: [f32; 2]| Vertex {
            position: Vec3::from(pos),
            normal: Vec3::from(normal),
            color,
            uv: Vec2::from(uv),
        };

        let vertices = vec![
            // Front face (z+)
            v([-0.5, 0.0,  0.5], [0.0, 0.0, 1.0], [0.0, 0.0]),
            v([ 0.5, 0.0,  0.5], [0.0, 0.0, 1.0], [1.0, 0.0]),
            v([ 0.5, h,    0.5], [0.0, 0.0, 1.0], [1.0, h]),
            v([-0.5, h,    0.5], [0.0, 0.0, 1.0], [0.0, h]),
            // Back face (z-)
            v([ 0.5, 0.0, -0.5], [0.0, 0.0, -1.0], [0.0, 0.0]),
            v([-0.5, 0.0, -0.5], [0.0, 0.0, -1.0], [1.0, 0.0]),
            v([-0.5, h,   -0.5], [0.0, 0.0, -1.0], [1.0, h]),
            v([ 0.5, h,   -0.5], [0.0, 0.0, -1.0], [0.0, h]),
            // Top face (y+)
            v([-0.5, h,  0.5], [0.0, 1.0, 0.0], [0.0, 0.0]),
            v([ 0.5, h,  0.5], [0.0, 1.0, 0.0], [1.0, 0.0]),
            v([ 0.5, h, -0.5], [0.0, 1.0, 0.0], [1.0, 1.0]),
            v([-0.5, h, -0.5], [0.0, 1.0, 0.0], [0.0, 1.0]),
            // Right face (x+)
            v([ 0.5, 0.0,  0.5], [1.0, 0.0, 0.0], [0.0, 0.0]),
            v([ 0.5, 0.0, -0.5], [1.0, 0.0, 0.0], [1.0, 0.0]),
            v([ 0.5, h,   -0.5], [1.0, 0.0, 0.0], [1.0, h]),
            v([ 0.5, h,    0.5], [1.0, 0.0, 0.0], [0.0, h]),
            // Left face (x-)
            v([-0.5, 0.0, -0.5], [-1.0, 0.0, 0.0], [0.0, 0.0]),
            v([-0.5, 0.0,  0.5], [-1.0, 0.0, 0.0], [1.0, 0.0]),
            v([-0.5, h,    0.5], [-1.0, 0.0, 0.0], [1.0, h]),
            v([-0.5, h,   -0.5], [-1.0, 0.0, 0.0], [0.0, h]),
        ];

        let indices = vec![
            0,  1,  2,  2,  3,  0,   // front
            4,  5,  6,  6,  7,  4,   // back
            8,  9,  10, 10, 11, 8,   // top
            12, 13, 14, 14, 15, 12,  // right
            16, 17, 18, 18, 19, 16,  // left
        ];

        Self { vertices, indices }
    }

    /// A red-tinted enemy — spiky/angular shape to distinguish from player.
    pub fn enemy() -> Self {
        // Brute wraith: wide, heavy, low-floating ghost. Blood red HDR.
        let body = Vec3::new(2.40, 0.20, 0.15);
        let eye  = Vec3::new(3.20, 0.50, 0.30);
        Self::wraith(body, body * 0.4, eye, 0.55, 1.10, 0.25)
    }

    /// Stalker variant: tall thin wisp wraith. Magenta-violet HDR.
    pub fn enemy_stalker() -> Self {
        let body = Vec3::new(1.80, 0.40, 2.80);
        let eye  = Vec3::new(2.40, 0.60, 3.20);
        Self::wraith(body, body * 0.4, eye, 0.26, 1.55, 0.18)
    }

    /// Caster variant: hooded mage wraith. Toxic green HDR.
    pub fn enemy_caster() -> Self {
        let body = Vec3::new(0.30, 2.60, 0.55);
        let eye  = Vec3::new(0.40, 3.40, 0.70);
        Self::wraith(body, body * 0.4, eye, 0.36, 1.40, 0.20)
    }

    /// Wraith template — one continuous lathed surface (revolved profile) with
    /// a tattered, wavy hem at the bottom and two glowing eyes. The profile is
    /// hand-tuned to read as: rounded head -> shoulders -> tapered body ->
    /// flaring torn hem. All enemies and the player share this silhouette;
    /// only colors and proportions vary.
    ///
    /// - `body`/`hood`: surface colors (hood used for the upper third of the body).
    /// - `eye`: HDR color of the two glowing eyes.
    /// - `radius`: max body half-width.
    /// - `height`: total tip-to-base height.
    /// - `float_h`: how far the bottom hem floats off the ground.
    pub fn wraith(body: Vec3, hood: Vec3, eye: Vec3, radius: f32, height: f32, float_h: f32) -> Self {
        // Profile curve: list of (t, r) pairs where t is normalized height
        // [0..1] (0 = bottom hem, 1 = top of head) and r is the radius at that
        // height as a fraction of `radius`. This is the silhouette of one half
        // of the ghost — the rest is generated by revolving it around Y.
        //
        // Drawn here from bottom up so it's easy to read:
        //   bottom hem — flares out a bit (the tattered skirt opens up)
        //   waist     — narrows
        //   shoulders — wide
        //   neck      — slight pinch
        //   head      — round
        //   top       — closes to a point
        let profile: &[(f32, f32)] = &[
            (0.00, 0.85), // hem outer flare
            (0.05, 0.95), // hem widest
            (0.12, 0.78), // pinch above hem
            (0.22, 0.70), // waist
            (0.40, 0.92), // shoulders
            (0.55, 0.95), // upper chest (widest)
            (0.68, 0.85), // neck pinch
            (0.78, 0.82), // jaw
            (0.88, 0.74), // crown
            (0.96, 0.45), // top of head
            (1.00, 0.00), // pole
        ];

        let azimuth_segments = 32u32;
        let mut m = Self::empty();
        m.append_lathe(profile, body, hood, radius, height, float_h, azimuth_segments);

        // Eyes — placed on the front of the head where the profile says
        // r ≈ 0.78. Use the head's t-range center.
        let eye_t = 0.83;
        let eye_y = float_h + height * eye_t;
        let eye_r_at = radius * 0.78;
        let eye_r = (radius * 0.10).clamp(0.025, 0.07);
        let eye_x = eye_r_at * 0.32;
        let eye_z = eye_r_at * 0.95;
        m.append_ellipsoid(Vec3::new(eye_r, eye_r, eye_r * 0.7), Vec3::new(-eye_x, eye_y, eye_z), eye, 6, 4);
        m.append_ellipsoid(Vec3::new(eye_r, eye_r, eye_r * 0.7), Vec3::new( eye_x, eye_y, eye_z), eye, 6, 4);
        m
    }

    /// Append a surface of revolution (lathe) to this mesh. The 2D profile is
    /// `(t, r_factor)` pairs where `t` is normalized [0..1] from bottom to top
    /// and `r_factor` is multiplied by `radius`. Y position is
    /// `float_h + height * t`. The bottom rings (t < 0.18) get a per-azimuth
    /// radial wobble to create a tattered ghostly hem. The upper third
    /// (t > 0.65) is shaded with `hood_color`, the rest with `body_color`.
    pub fn append_lathe(
        &mut self,
        profile: &[(f32, f32)],
        body_color: Vec3,
        hood_color: Vec3,
        radius: f32,
        height: f32,
        float_h: f32,
        segments: u32,
    ) {
        if profile.len() < 2 || segments < 3 { return; }

        let base = self.vertices.len() as u32;
        let stacks = profile.len() as u32;
        let row = segments + 1;

        // Hash for cheap deterministic noise (tattered hem azimuthal wobble).
        fn hash(i: u32) -> f32 {
            let mut x = i.wrapping_mul(0x27d4_eb2d);
            x ^= x >> 15;
            x = x.wrapping_mul(0x85eb_ca6b);
            x ^= x >> 13;
            x = x.wrapping_mul(0xc2b2_ae35);
            x ^= x >> 16;
            (x as f32 / u32::MAX as f32) * 2.0 - 1.0
        }

        for (i, &(t, rf)) in profile.iter().enumerate() {
            let y = float_h + height * t;
            // Tattered hem: only the bottom of the body gets per-azimuth wobble.
            // Smoothly fades out by t = 0.18.
            let hem_amount = (1.0 - (t / 0.18).clamp(0.0, 1.0)).powi(2);
            // Surface tangent in the profile plane (used to estimate normals).
            let (t_prev, rf_prev) = if i == 0 { profile[0] } else { profile[i - 1] };
            let (t_next, rf_next) = if i + 1 == profile.len() { profile[profile.len() - 1] } else { profile[i + 1] };
            let dy = (t_next - t_prev) * height;
            let dr = (rf_next - rf_prev) * radius;
            // Profile-plane normal (pointing outward from axis): rotate tangent
            // by 90° in the (r, y) plane: (dr, dy) -> (dy, -dr) and normalize.
            let prof_nlen = (dy * dy + dr * dr).sqrt().max(1e-5);
            let n_r = dy / prof_nlen;
            let n_y = -dr / prof_nlen;

            // Color blend: body -> hood across the upper third.
            let hood_blend = ((t - 0.62) / 0.20).clamp(0.0, 1.0);
            let color = body_color * (1.0 - hood_blend) + hood_color * hood_blend;

            for j in 0..=segments {
                let u = j as f32 / segments as f32;
                let theta = u * std::f32::consts::TAU;
                let (s, c) = (theta.sin(), theta.cos());

                // Per-azimuth wobble at the hem (only affects radius).
                let wobble = if hem_amount > 0.0 {
                    // Two octaves of cheap hash noise around the ring; multiply by hem_amount.
                    let n0 = hash(j * 13 + 7);
                    let n1 = hash(j * 29 + 113) * 0.5;
                    (n0 + n1) * 0.18 * hem_amount
                } else { 0.0 };
                let r = (rf + wobble).max(0.0) * radius;

                let pos = Vec3::new(s * r, y, c * r);
                let normal = Vec3::new(s * n_r, n_y, c * n_r).normalize_or_zero();
                self.vertices.push(Vertex {
                    position: pos,
                    normal: if normal == Vec3::ZERO { Vec3::Y } else { normal },
                    color,
                    uv: Vec2::new(0.5, 0.5),
                });
            }
        }

        for i in 0..(stacks - 1) {
            for j in 0..segments {
                let a = base + i * row + j;
                let b = base + i * row + j + 1;
                let c = base + (i + 1) * row + j;
                let d = base + (i + 1) * row + j + 1;
                // CCW (cull=BACK, front=CCW)
                self.indices.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }
    }

    /// Empty mesh — useful as a starting point for compositional builders.
    pub fn empty() -> Self {
        Self { vertices: Vec::new(), indices: Vec::new() }
    }

    /// Append an ellipsoid to this mesh. The ellipsoid is a unit UV sphere
    /// non-uniformly scaled by `scale` and translated to `offset`. Normals are
    /// rescaled by `1/scale` to remain (approximately) correct under the
    /// non-uniform deformation.
    pub fn append_ellipsoid(&mut self, scale: Vec3, offset: Vec3, color: Vec3, slices: u32, stacks: u32) {
        let base = self.vertices.len() as u32;
        let inv_scale = Vec3::new(1.0 / scale.x.max(1e-4), 1.0 / scale.y.max(1e-4), 1.0 / scale.z.max(1e-4));

        // Generate a UV sphere. Stacks span [0, PI] (south pole to north pole),
        // slices span [0, TAU].
        for i in 0..=stacks {
            let v = i as f32 / stacks as f32;
            let phi = v * std::f32::consts::PI; // 0 .. PI
            let (sp, cp) = (phi.sin(), phi.cos());
            for j in 0..=slices {
                let u = j as f32 / slices as f32;
                let theta = u * std::f32::consts::TAU;
                let (st, ct) = (theta.sin(), theta.cos());
                // Unit sphere vertex
                let nx = sp * ct;
                let ny = cp;
                let nz = sp * st;
                let pos = Vec3::new(nx * scale.x, ny * scale.y, nz * scale.z) + offset;
                // Approx correct normal under non-uniform scale.
                let normal = (Vec3::new(nx, ny, nz) * inv_scale).normalize_or_zero();
                self.vertices.push(Vertex {
                    position: pos,
                    normal: if normal == Vec3::ZERO { Vec3::Y } else { normal },
                    color,
                    uv: Vec2::new(0.5, 0.5),
                });
            }
        }

        // Indices (two tris per quad on the grid)
        let row = slices + 1;
        for i in 0..stacks {
            for j in 0..slices {
                let a = base + i * row + j;
                let b = base + i * row + j + 1;
                let c = base + (i + 1) * row + j;
                let d = base + (i + 1) * row + j + 1;
                // CCW winding (matches engine: cull=BACK, front=CCW)
                self.indices.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }
    }

    /// Configurable enemy mesh — body color, spike color, and shape proportions.
    /// (Legacy octagonal-prism builder, kept for any callers still using it.)
    #[allow(dead_code)]
    pub fn enemy_shape(body_color: Vec3, spike_color: Vec3, radius: f32, height: f32, spike_h: f32) -> Self {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        // Body: octagonal prism with the given proportions.
        let segments = 8u32;
        let top_radius = radius * 0.8;

        // Side faces
        for i in 0..segments {
            let angle0 = (i as f32 / segments as f32) * std::f32::consts::TAU;
            let angle1 = ((i + 1) as f32 / segments as f32) * std::f32::consts::TAU;
            let (s0, c0) = (angle0.sin(), angle0.cos());
            let (s1, c1) = (angle1.sin(), angle1.cos());

            let base_idx = vertices.len() as u32;
            let normal = Vec3::new((s0 + s1) * 0.5, 0.0, (c0 + c1) * 0.5).normalize();

            vertices.push(Vertex { position: Vec3::new(s0 * radius, 0.0, c0 * radius), normal, color: body_color, uv: Vec2::new(0.5, 0.5) });
            vertices.push(Vertex { position: Vec3::new(s1 * radius, 0.0, c1 * radius), normal, color: body_color, uv: Vec2::new(0.5, 0.5) });
            vertices.push(Vertex { position: Vec3::new(s1 * top_radius, height, c1 * top_radius), normal, color: body_color, uv: Vec2::new(0.5, 0.5) });
            vertices.push(Vertex { position: Vec3::new(s0 * top_radius, height, c0 * top_radius), normal, color: body_color, uv: Vec2::new(0.5, 0.5) });

            indices.extend_from_slice(&[base_idx, base_idx+1, base_idx+2, base_idx+2, base_idx+3, base_idx]);
        }

        // Top cap (flat disc to close the body)
        let cap_center_idx = vertices.len() as u32;
        vertices.push(Vertex { position: Vec3::new(0.0, height, 0.0), normal: Vec3::Y, color: spike_color, uv: Vec2::new(0.5, 0.5) });
        for i in 0..segments {
            let angle0 = (i as f32 / segments as f32) * std::f32::consts::TAU;
            let angle1 = ((i + 1) as f32 / segments as f32) * std::f32::consts::TAU;
            let idx = vertices.len() as u32;
            vertices.push(Vertex { position: Vec3::new(angle0.sin() * top_radius, height, angle0.cos() * top_radius), normal: Vec3::Y, color: spike_color, uv: Vec2::new(0.5, 0.5) });
            vertices.push(Vertex { position: Vec3::new(angle1.sin() * top_radius, height, angle1.cos() * top_radius), normal: Vec3::Y, color: spike_color, uv: Vec2::new(0.5, 0.5) });
            indices.extend_from_slice(&[cap_center_idx, idx, idx+1]);
        }

        // Top spike cone (solid, on top of the cap)
        let spike_tip_idx = vertices.len() as u32;
        vertices.push(Vertex { position: Vec3::new(0.0, height + spike_h, 0.0), normal: Vec3::Y, color: spike_color, uv: Vec2::new(0.5, 0.5) });
        for i in 0..segments {
            let angle0 = (i as f32 / segments as f32) * std::f32::consts::TAU;
            let angle1 = ((i + 1) as f32 / segments as f32) * std::f32::consts::TAU;
            let idx = vertices.len() as u32;
            let normal = Vec3::new(angle0.sin(), 0.7, angle0.cos()).normalize();
            vertices.push(Vertex { position: Vec3::new(angle0.sin() * top_radius * 0.6, height, angle0.cos() * top_radius * 0.6), normal, color: spike_color, uv: Vec2::new(0.5, 0.5) });
            vertices.push(Vertex { position: Vec3::new(angle1.sin() * top_radius * 0.6, height, angle1.cos() * top_radius * 0.6), normal, color: spike_color, uv: Vec2::new(0.5, 0.5) });
            indices.extend_from_slice(&[spike_tip_idx, idx, idx+1]);
        }

        Self { vertices, indices }
    }

    /// An elite enemy — bigger, more menacing wraith. Steel-blue HDR.
    pub fn elite_enemy() -> Self {
        let body = Vec3::new(0.40, 0.70, 1.80);
        let eye  = Vec3::new(0.80, 1.40, 2.40);
        Self::wraith(body, body * 0.4, eye, 0.65, 1.70, 0.30)
    }

    /// Boss — towering archfiend wraith. Deep purple HDR.
    pub fn boss() -> Self {
        let body = Vec3::new(0.90, 0.20, 1.80);
        let eye  = Vec3::new(2.40, 0.40, 2.80);
        Self::wraith(body, body * 0.4, eye, 0.95, 2.80, 0.40)
    }

    /// Generate a batched dungeon floor from tile positions.
    /// Uses checker pattern with slight color variation for visual interest.
    pub fn dungeon_floor(positions: &[Vec3], floor_num: u32) -> Self {
        let mut vertices = Vec::with_capacity(positions.len() * 4);
        let mut indices = Vec::with_capacity(positions.len() * 6);

        // Color palette changes per floor for visual variety
        // These tint the stone texture, so keep them brighter (texture darkens them)
        let base_color = match floor_num % 4 {
            0 => Vec3::new(0.55, 0.50, 0.45), // dark stone
            1 => Vec3::new(0.45, 0.55, 0.38), // mossy dungeon
            2 => Vec3::new(0.60, 0.35, 0.30), // infernal
            _ => Vec3::new(0.38, 0.48, 0.60), // ice cavern
        };

        for (i, pos) in positions.iter().enumerate() {
            let base_idx = (i * 4) as u32;
            let ix = pos.x as u32;
            let iz = pos.z as u32;
            // Subtle variation using position hash
            let hash = ((ix.wrapping_mul(7) ^ iz.wrapping_mul(13)) % 100) as f32 / 800.0;
            let color = base_color + Vec3::splat(hash);

            // Use world-space UVs for seamless tiling across tiles
            let u0 = pos.x - 0.5;
            let v0 = pos.z - 0.5;
            let u1 = pos.x + 0.5;
            let v1 = pos.z + 0.5;

            vertices.push(Vertex {
                position: *pos + Vec3::new(-0.5, 0.0, -0.5),
                normal: Vec3::Y,
                color,
                uv: Vec2::new(u0, v0),
            });
            vertices.push(Vertex {
                position: *pos + Vec3::new(0.5, 0.0, -0.5),
                normal: Vec3::Y,
                color,
                uv: Vec2::new(u1, v0),
            });
            vertices.push(Vertex {
                position: *pos + Vec3::new(0.5, 0.0, 0.5),
                normal: Vec3::Y,
                color,
                uv: Vec2::new(u1, v1),
            });
            vertices.push(Vertex {
                position: *pos + Vec3::new(-0.5, 0.0, 0.5),
                normal: Vec3::Y,
                color,
                uv: Vec2::new(u0, v1),
            });

            indices.extend_from_slice(&[
                base_idx, base_idx + 2, base_idx + 1,
                base_idx, base_idx + 3, base_idx + 2,
            ]);
        }

        Self { vertices, indices }
    }

    /// A small glowing loot orb (diamond-shaped, colored by rarity).
    pub fn loot_orb(color: [f32; 3]) -> Self {
        let v = |pos: [f32; 3], normal: [f32; 3]| Vertex {
            position: Vec3::from(pos),
            normal: Vec3::from(normal),
            color: Vec3::from(color),
            uv: Vec2::ZERO,
        };

        // Diamond shape: 6 vertices (top, bottom, 4 equatorial)
        let s = 0.15_f32;
        let h = 0.25_f32;

        let vertices = vec![
            v([0.0, h, 0.0], [0.0, 1.0, 0.0]),   // 0: top
            v([0.0, -h, 0.0], [0.0, -1.0, 0.0]),  // 1: bottom
            v([s, 0.0, 0.0], [1.0, 0.0, 0.0]),    // 2: +x
            v([-s, 0.0, 0.0], [-1.0, 0.0, 0.0]),  // 3: -x
            v([0.0, 0.0, s], [0.0, 0.0, 1.0]),    // 4: +z
            v([0.0, 0.0, -s], [0.0, 0.0, -1.0]),  // 5: -z
        ];

        let indices = vec![
            // Top pyramid
            0, 2, 4,
            0, 4, 3,
            0, 3, 5,
            0, 5, 2,
            // Bottom pyramid
            1, 4, 2,
            1, 3, 4,
            1, 5, 3,
            1, 2, 5,
        ];

        Self { vertices, indices }
    }

    /// Ground targeting circle — thick hollow ring, rendered face-up on the floor.
    pub fn targeting_circle(color: [f32; 3]) -> Self {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        let segments = 64u32;
        let y = 0.08; // safely above floor (y=0) to avoid z-fighting
        // Boost color far above 1.0 so it survives texture multiplication and lighting.
        let bright = Vec3::new(color[0] * 8.0, color[1] * 8.0, color[2] * 8.0);
        let dim = Vec3::new(color[0] * 4.0, color[1] * 4.0, color[2] * 4.0);

        // Thick hollow ring: 75%–100% of unit radius. Hollow center.
        let inner_r = 0.75_f32;
        let outer_r = 1.0_f32;
        for i in 0..segments {
            let a0 = (i as f32 / segments as f32) * std::f32::consts::TAU;
            let a1 = ((i + 1) as f32 / segments as f32) * std::f32::consts::TAU;
            let base_idx = vertices.len() as u32;

            // UV (0.5, 0.5) samples bright center of stone texture (avoids mortar lines).
            let uv = Vec2::new(0.5, 0.5);

            // 0 = inner_a0, 1 = outer_a0, 2 = outer_a1, 3 = inner_a1
            vertices.push(Vertex { position: Vec3::new(a0.cos() * inner_r, y, a0.sin() * inner_r), normal: Vec3::Y, color: dim, uv });
            vertices.push(Vertex { position: Vec3::new(a0.cos() * outer_r, y, a0.sin() * outer_r), normal: Vec3::Y, color: bright, uv });
            vertices.push(Vertex { position: Vec3::new(a1.cos() * outer_r, y, a1.sin() * outer_r), normal: Vec3::Y, color: bright, uv });
            vertices.push(Vertex { position: Vec3::new(a1.cos() * inner_r, y, a1.sin() * inner_r), normal: Vec3::Y, color: dim, uv });

            // Render double-sided so it's visible regardless of camera angle / winding.
            // Front faces (normal up): 0,3,2 and 0,2,1
            indices.extend_from_slice(&[base_idx, base_idx+3, base_idx+2,  base_idx, base_idx+2, base_idx+1]);
            // Back faces (normal down): 0,1,2 and 0,2,3
            indices.extend_from_slice(&[base_idx, base_idx+1, base_idx+2,  base_idx, base_idx+2, base_idx+3]);
        }

        Self { vertices, indices }
    }

    /// A vertical light beam (tall thin prism, fading upward).
    /// Height is 1.0 — scale via model matrix to desired beam height.
    pub fn light_beam(color: [f32; 3]) -> Self {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        // D3-style beam: multiple intersecting vertical planes (cross/star shape)
        // creates a volumetric appearance from any angle
        let height = 1.0_f32; // scaled by model matrix externally
        let num_planes = 3; // 3 planes at 60° intervals = 6 "blades"
        let base_width = 0.12_f32; // wider for visibility
        let top_width = 0.04_f32;  // tapers at top

        // Bright at base, fading upward
        let base_color = Vec3::new(color[0], color[1], color[2]);
        let mid_color = Vec3::new(color[0] * 0.7, color[1] * 0.7, color[2] * 0.7);
        let top_color = Vec3::new(color[0] * 0.2, color[1] * 0.2, color[2] * 0.2);

        for plane in 0..num_planes {
            let angle = (plane as f32 / num_planes as f32) * std::f32::consts::PI;
            let (sin_a, cos_a) = (angle.sin(), angle.cos());

            // Each plane has 3 vertical segments for color gradient
            let segments = 3u32;
            for seg in 0..segments {
                let t0 = seg as f32 / segments as f32;
                let t1 = (seg + 1) as f32 / segments as f32;
                let y0 = t0 * height;
                let y1 = t1 * height;
                let w0 = base_width * (1.0 - t0) + top_width * t0;
                let w1 = base_width * (1.0 - t1) + top_width * t1;

                let c0 = if t0 < 0.5 {
                    let f = t0 * 2.0;
                    Vec3::new(
                        base_color.x * (1.0 - f) + mid_color.x * f,
                        base_color.y * (1.0 - f) + mid_color.y * f,
                        base_color.z * (1.0 - f) + mid_color.z * f,
                    )
                } else {
                    let f = (t0 - 0.5) * 2.0;
                    Vec3::new(
                        mid_color.x * (1.0 - f) + top_color.x * f,
                        mid_color.y * (1.0 - f) + top_color.y * f,
                        mid_color.z * (1.0 - f) + top_color.z * f,
                    )
                };
                let c1 = if t1 < 0.5 {
                    let f = t1 * 2.0;
                    Vec3::new(
                        base_color.x * (1.0 - f) + mid_color.x * f,
                        base_color.y * (1.0 - f) + mid_color.y * f,
                        base_color.z * (1.0 - f) + mid_color.z * f,
                    )
                } else {
                    let f = (t1 - 0.5) * 2.0;
                    Vec3::new(
                        mid_color.x * (1.0 - f) + top_color.x * f,
                        mid_color.y * (1.0 - f) + top_color.y * f,
                        mid_color.z * (1.0 - f) + top_color.z * f,
                    )
                };

                let normal = Vec3::new(-cos_a, 0.0, sin_a); // perpendicular to plane

                let base_idx = vertices.len() as u32;
                // Front side
                vertices.push(Vertex { position: Vec3::new(-sin_a * w0, y0, -cos_a * w0), normal, color: c0, uv: Vec2::ZERO });
                vertices.push(Vertex { position: Vec3::new( sin_a * w0, y0,  cos_a * w0), normal, color: c0, uv: Vec2::ZERO });
                vertices.push(Vertex { position: Vec3::new( sin_a * w1, y1,  cos_a * w1), normal, color: c1, uv: Vec2::ZERO });
                vertices.push(Vertex { position: Vec3::new(-sin_a * w1, y1, -cos_a * w1), normal, color: c1, uv: Vec2::ZERO });
                indices.extend_from_slice(&[base_idx, base_idx+1, base_idx+2, base_idx+2, base_idx+3, base_idx]);

                // Back side (so visible from both directions)
                let base_idx2 = vertices.len() as u32;
                let normal2 = -normal;
                vertices.push(Vertex { position: Vec3::new( sin_a * w0, y0,  cos_a * w0), normal: normal2, color: c0, uv: Vec2::ZERO });
                vertices.push(Vertex { position: Vec3::new(-sin_a * w0, y0, -cos_a * w0), normal: normal2, color: c0, uv: Vec2::ZERO });
                vertices.push(Vertex { position: Vec3::new(-sin_a * w1, y1, -cos_a * w1), normal: normal2, color: c1, uv: Vec2::ZERO });
                vertices.push(Vertex { position: Vec3::new( sin_a * w1, y1,  cos_a * w1), normal: normal2, color: c1, uv: Vec2::ZERO });
                indices.extend_from_slice(&[base_idx2, base_idx2+1, base_idx2+2, base_idx2+2, base_idx2+3, base_idx2]);
            }
        }

        // Base glow ring (horizontal disc at Y=0.05 for ground highlight)
        let ring_segments = 12u32;
        let ring_inner = 0.05_f32;
        let ring_outer = 0.25_f32;
        let ring_y = 0.05;
        for i in 0..ring_segments {
            let a0 = (i as f32 / ring_segments as f32) * std::f32::consts::TAU;
            let a1 = ((i + 1) as f32 / ring_segments as f32) * std::f32::consts::TAU;
            let base_idx = vertices.len() as u32;

            vertices.push(Vertex { position: Vec3::new(a0.cos() * ring_inner, ring_y, a0.sin() * ring_inner), normal: Vec3::Y, color: base_color, uv: Vec2::ZERO });
            vertices.push(Vertex { position: Vec3::new(a1.cos() * ring_inner, ring_y, a1.sin() * ring_inner), normal: Vec3::Y, color: base_color, uv: Vec2::ZERO });
            vertices.push(Vertex { position: Vec3::new(a1.cos() * ring_outer, ring_y, a1.sin() * ring_outer), normal: Vec3::Y, color: top_color, uv: Vec2::ZERO });
            vertices.push(Vertex { position: Vec3::new(a0.cos() * ring_outer, ring_y, a0.sin() * ring_outer), normal: Vec3::Y, color: top_color, uv: Vec2::ZERO });

            indices.extend_from_slice(&[base_idx, base_idx+1, base_idx+2, base_idx+2, base_idx+3, base_idx]);
        }

        Self { vertices, indices }
    }

    /// Hostile bolt projectile — a glowing octahedron with bright HDR color.
    /// Pointing along -Z so the rotation logic from `arrow` reuses cleanly.
    pub fn enemy_bolt(color: [f32; 3]) -> Self {
        let c = Vec3::new(color[0] * 4.0, color[1] * 4.0, color[2] * 4.0);
        // Octahedron vertices: tip at -Z, tail at +Z, four equatorial points.
        let len = 0.55_f32;
        let r = 0.18_f32;
        let v = |p: [f32; 3], n: [f32; 3]| Vertex {
            position: Vec3::from(p),
            normal: Vec3::from(n),
            color: c,
            uv: Vec2::new(0.5, 0.5),
        };
        let tip   = v([0.0, 0.0, -len], [0.0, 0.0, -1.0]);
        let tail  = v([0.0, 0.0,  len], [0.0, 0.0,  1.0]);
        let east  = v([ r, 0.0, 0.0], [ 1.0, 0.0, 0.0]);
        let west  = v([-r, 0.0, 0.0], [-1.0, 0.0, 0.0]);
        let up    = v([0.0,  r, 0.0], [0.0,  1.0, 0.0]);
        let down  = v([0.0, -r, 0.0], [0.0, -1.0, 0.0]);

        let vertices = vec![tip, tail, east, west, up, down];
        // Index layout:
        // 0=tip, 1=tail, 2=east, 3=west, 4=up, 5=down
        let indices = vec![
            // Front cone (toward tip)
            0, 4, 2,  0, 2, 5,  0, 5, 3,  0, 3, 4,
            // Back cone (toward tail)
            1, 2, 4,  1, 5, 2,  1, 3, 5,  1, 4, 3,
        ];
        Self { vertices, indices }
    }

    /// Arrow projectile: elongated diamond shape pointing in -Z direction.
    pub fn arrow() -> Self {
        let v = |pos: [f32; 3], normal: [f32; 3], color: [f32; 3]| Vertex {
            position: Vec3::from(pos),
            normal: Vec3::from(normal),
            color: Vec3::from(color),
            uv: Vec2::new(0.5, 0.5),
        };

        let shaft_color = [1.0, 0.85, 0.3]; // bright golden shaft
        let tip_color = [1.0, 1.0, 1.0];    // white-hot tip
        let fletch_color = [1.0, 0.5, 0.1]; // orange fletching (glow)

        // Shaft: visible box along -Z axis
        let w = 0.08; // half-width (doubled from before)
        let len = 1.0; // shaft length (longer)

        let mut vertices = vec![
            // Shaft top face
            v([-w, w, 0.0], [0.0, 1.0, 0.0], shaft_color),
            v([w, w, 0.0], [0.0, 1.0, 0.0], shaft_color),
            v([w, w, -len], [0.0, 1.0, 0.0], shaft_color),
            v([-w, w, -len], [0.0, 1.0, 0.0], shaft_color),
            // Shaft bottom face
            v([-w, -w, -len], [0.0, -1.0, 0.0], shaft_color),
            v([w, -w, -len], [0.0, -1.0, 0.0], shaft_color),
            v([w, -w, 0.0], [0.0, -1.0, 0.0], shaft_color),
            v([-w, -w, 0.0], [0.0, -1.0, 0.0], shaft_color),
            // Shaft left face
            v([-w, -w, 0.0], [-1.0, 0.0, 0.0], shaft_color),
            v([-w, w, 0.0], [-1.0, 0.0, 0.0], shaft_color),
            v([-w, w, -len], [-1.0, 0.0, 0.0], shaft_color),
            v([-w, -w, -len], [-1.0, 0.0, 0.0], shaft_color),
            // Shaft right face
            v([w, -w, -len], [1.0, 0.0, 0.0], shaft_color),
            v([w, w, -len], [1.0, 0.0, 0.0], shaft_color),
            v([w, w, 0.0], [1.0, 0.0, 0.0], shaft_color),
            v([w, -w, 0.0], [1.0, 0.0, 0.0], shaft_color),
            // Tip: large pointed head at the front (-Z end)
            v([0.0, 0.0, -len - 0.4], [0.0, 0.0, -1.0], tip_color), // tip point
            v([-0.14, 0.14, -len], [0.0, 1.0, -0.5], tip_color),
            v([0.14, 0.14, -len], [0.0, 1.0, -0.5], tip_color),
            v([0.14, -0.14, -len], [0.0, -1.0, -0.5], tip_color),
            v([-0.14, -0.14, -len], [0.0, -1.0, -0.5], tip_color),
        ];

        let mut indices = vec![
            // Shaft top
            0, 1, 2, 2, 3, 0,
            // Shaft bottom
            4, 5, 6, 6, 7, 4,
            // Shaft left
            8, 9, 10, 10, 11, 8,
            // Shaft right
            12, 13, 14, 14, 15, 12,
            // Tip (4 triangular faces)
            16, 17, 18,  // top
            16, 18, 19,  // right
            16, 19, 20,  // bottom
            16, 20, 17,  // left
        ];

        // Fletching: two crossed diamond fins at the back
        let fin_w = 0.18;
        let fin_len = 0.25;
        let base_idx = vertices.len() as u32;
        // Vertical fin
        vertices.push(v([0.0, -fin_w, 0.0], [1.0, 0.0, 0.0], fletch_color));
        vertices.push(v([0.0, fin_w, 0.0], [1.0, 0.0, 0.0], fletch_color));
        vertices.push(v([0.0, fin_w, -fin_len], [1.0, 0.0, 0.0], fletch_color));
        vertices.push(v([0.0, -fin_w, -fin_len], [1.0, 0.0, 0.0], fletch_color));
        // Horizontal fin
        vertices.push(v([-fin_w, 0.0, 0.0], [0.0, 1.0, 0.0], fletch_color));
        vertices.push(v([fin_w, 0.0, 0.0], [0.0, 1.0, 0.0], fletch_color));
        vertices.push(v([fin_w, 0.0, -fin_len], [0.0, 1.0, 0.0], fletch_color));
        vertices.push(v([-fin_w, 0.0, -fin_len], [0.0, 1.0, 0.0], fletch_color));
        indices.extend_from_slice(&[
            base_idx, base_idx+1, base_idx+2, base_idx+2, base_idx+3, base_idx,
            base_idx+4, base_idx+5, base_idx+6, base_idx+6, base_idx+7, base_idx+4,
        ]);

        Self { vertices, indices }
    }

    /// Fireball: a glowing emissive sphere built as a low-poly UV sphere.
    /// Vertex colors interpolate from a hot white core to orange edges so it
    /// reads as a fireball even without point lights. Diameter ≈ 0.45 units.
    pub fn fireball() -> Self {
        let radius = 0.22_f32;
        let stacks = 8usize;
        let sectors = 12usize;

        let core = glam::Vec3::new(1.0, 1.0, 0.85);   // white-hot core hint
        let edge = glam::Vec3::new(1.0, 0.45, 0.05);  // orange flame

        let mut vertices: Vec<Vertex> = Vec::with_capacity((stacks + 1) * (sectors + 1));
        for i in 0..=stacks {
            let v = i as f32 / stacks as f32;
            let phi = v * std::f32::consts::PI; // 0 .. PI
            let y = phi.cos();
            let r = phi.sin();
            for j in 0..=sectors {
                let u = j as f32 / sectors as f32;
                let theta = u * std::f32::consts::TAU;
                let x = r * theta.cos();
                let z = r * theta.sin();
                let n = glam::Vec3::new(x, y, z).normalize_or_zero();
                let pos = n * radius;
                // Brighter near the equator (bands), darker at the poles —
                // a cheap fake shading so it reads as a flame even unlit.
                let fade = (1.0 - (y.abs() * 0.6)).max(0.4);
                let color = edge.lerp(core, fade * 0.5);
                vertices.push(Vertex {
                    position: pos,
                    normal: n,
                    color,
                    uv: glam::Vec2::new(u, v),
                });
            }
        }

        let stride = sectors + 1;
        let mut indices: Vec<u32> = Vec::with_capacity(stacks * sectors * 6);
        for i in 0..stacks {
            for j in 0..sectors {
                let a = (i * stride + j) as u32;
                let b = ((i + 1) * stride + j) as u32;
                let c = ((i + 1) * stride + j + 1) as u32;
                let d = (i * stride + j + 1) as u32;
                indices.extend_from_slice(&[a, b, c, c, d, a]);
            }
        }

        Self { vertices, indices }
    }

    /// Portal: a vertical ring/torus-like shape with glowing inner surface.
    pub fn portal() -> Self {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        let segments = 24;
        let ring_radius = 1.0_f32;
        let tube_radius = 0.12_f32;
        let height = 1.8_f32;

        // Outer ring (vertical ellipse)
        let outer_color = Vec3::new(0.2, 0.5, 1.0);
        let inner_color = Vec3::new(0.6, 0.9, 1.0);

        for i in 0..segments {
            let angle = (i as f32 / segments as f32) * std::f32::consts::TAU;
            let next_angle = ((i + 1) as f32 / segments as f32) * std::f32::consts::TAU;

            let cx = angle.cos() * ring_radius;
            let cy = angle.sin() * (height * 0.5) + height * 0.5;
            let nx = next_angle.cos() * ring_radius;
            let ny = next_angle.sin() * (height * 0.5) + height * 0.5;

            // Tube cross-section (4 verts per segment)
            let normal_out = Vec3::new(angle.cos(), angle.sin(), 0.0).normalize();
            let base_idx = vertices.len() as u32;

            // Outer face of tube
            vertices.push(Vertex { position: Vec3::new(cx - tube_radius * angle.cos(), cy - tube_radius * angle.sin(), -tube_radius), normal: normal_out, color: outer_color, uv: Vec2::new(0.5, 0.5) });
            vertices.push(Vertex { position: Vec3::new(cx + tube_radius * angle.cos(), cy + tube_radius * angle.sin(), -tube_radius), normal: normal_out, color: outer_color, uv: Vec2::new(0.5, 0.5) });
            vertices.push(Vertex { position: Vec3::new(nx + tube_radius * next_angle.cos(), ny + tube_radius * next_angle.sin(), -tube_radius), normal: normal_out, color: outer_color, uv: Vec2::new(0.5, 0.5) });
            vertices.push(Vertex { position: Vec3::new(nx - tube_radius * next_angle.cos(), ny - tube_radius * next_angle.sin(), -tube_radius), normal: normal_out, color: outer_color, uv: Vec2::new(0.5, 0.5) });

            indices.extend_from_slice(&[base_idx, base_idx+1, base_idx+2, base_idx+2, base_idx+3, base_idx]);
        }

        // Inner disc (the "portal surface" — glowing flat disc)
        let disc_segments = 16;
        let center_idx = vertices.len() as u32;
        vertices.push(Vertex { position: Vec3::new(0.0, height * 0.5, 0.0), normal: Vec3::Z, color: inner_color, uv: Vec2::new(0.5, 0.5) });

        for i in 0..disc_segments {
            let angle = (i as f32 / disc_segments as f32) * std::f32::consts::TAU;
            let r = ring_radius * 0.85;
            vertices.push(Vertex {
                position: Vec3::new(angle.cos() * r, angle.sin() * (height * 0.45) + height * 0.5, 0.0),
                normal: Vec3::Z,
                color: inner_color,
                uv: Vec2::new(0.5, 0.5),
            });
        }
        for i in 0..disc_segments {
            let next = (i + 1) % disc_segments;
            indices.extend_from_slice(&[center_idx, center_idx + 1 + i as u32, center_idx + 1 + next as u32]);
        }

        Self { vertices, indices }
    }

    /// Load a static mesh from a glTF / .glb file. Merges every primitive of
    /// every mesh in the scene into one Mesh, applying each node's world
    /// transform so the result sits in the model's bind-pose space.
    ///
    /// The path is tried as-is first, then prefixed with `..`, `../..`, etc.
    /// to handle being launched from `target/debug` or similar.
    ///
    /// Skinning data (joints/weights) is *ignored* here — this is the static
    /// loader. Use `SkinnedMesh::from_gltf` (added in Phase 2) for skinning.
    pub fn from_gltf<P: AsRef<std::path::Path>>(path: P) -> anyhow::Result<Self> {
        let original = path.as_ref().to_path_buf();
        let candidates = [
            original.clone(),
            std::path::PathBuf::from("..").join(&original),
            std::path::PathBuf::from("../..").join(&original),
            std::path::PathBuf::from("../../..").join(&original),
        ];
        let resolved = candidates.iter().find(|p| p.exists()).cloned()
            .ok_or_else(|| anyhow::anyhow!(
                "gltf file not found in any candidate path (cwd={:?}): {:?}",
                std::env::current_dir().ok(), original
            ))?;
        log::info!("Loading glTF from {:?}", resolved);

        // Load the document and buffers but skip images — we don't sample
        // the model's textures yet, and a single missing/misnamed image
        // would otherwise cause the whole import to fail.
        let gltf = gltf::Gltf::open(&resolved)
            .map_err(|e| anyhow::anyhow!("gltf open failed for {:?}: {}", resolved, e))?;
        let base_dir = resolved.parent().unwrap_or_else(|| std::path::Path::new("."));
        let buffers = gltf::import_buffers(&gltf.document, Some(base_dir), gltf.blob.clone())
            .map_err(|e| anyhow::anyhow!("gltf buffer load failed for {:?}: {}", resolved, e))?;
        let doc = gltf.document;

        let mut mesh = Self::empty();
        let scene = doc.default_scene().or_else(|| doc.scenes().next())
            .ok_or_else(|| anyhow::anyhow!("gltf has no scenes: {:?}", resolved))?;

        for node in scene.nodes() {
            visit_node(&node, glam::Mat4::IDENTITY, &buffers, &mut mesh);
        }

        if mesh.vertices.is_empty() {
            anyhow::bail!("gltf {:?} produced an empty mesh", resolved);
        }
        // Compute bounds so the user can see the mesh actually loaded.
        let mut mn = glam::Vec3::splat(f32::INFINITY);
        let mut mx = glam::Vec3::splat(f32::NEG_INFINITY);
        for v in &mesh.vertices {
            mn = mn.min(v.position);
            mx = mx.max(v.position);
        }
        log::info!(
            "Loaded glTF {:?}: {} verts, {} tris, bounds [{:.2},{:.2},{:.2}] -> [{:.2},{:.2},{:.2}] (size {:.2}x{:.2}x{:.2})",
            resolved.file_name().unwrap_or_default(),
            mesh.vertices.len(),
            mesh.indices.len() / 3,
            mn.x, mn.y, mn.z, mx.x, mx.y, mx.z,
            mx.x - mn.x, mx.y - mn.y, mx.z - mn.z,
        );
        Ok(mesh)
    }
}

fn visit_node(
    node: &gltf::Node,
    parent: glam::Mat4,
    buffers: &[gltf::buffer::Data],
    out: &mut Mesh,
) {
    let local = glam::Mat4::from_cols_array_2d(&node.transform().matrix());
    let world = parent * local;
    let normal_mat = glam::Mat3::from_mat4(world).inverse().transpose();

    if let Some(gmesh) = node.mesh() {
        for prim in gmesh.primitives() {
            let reader = prim.reader(|b| Some(&buffers[b.index()]));
            let positions: Vec<[f32; 3]> = match reader.read_positions() {
                Some(it) => it.collect(),
                None => continue,
            };
            let normals: Vec<[f32; 3]> = reader
                .read_normals()
                .map(|it| it.collect())
                .unwrap_or_else(|| vec![[0.0, 1.0, 0.0]; positions.len()]);
            let uvs: Vec<[f32; 2]> = reader
                .read_tex_coords(0)
                .map(|tc| tc.into_f32().collect())
                .unwrap_or_else(|| vec![[0.5, 0.5]; positions.len()]);
            let colors: Option<Vec<[f32; 3]>> = reader
                .read_colors(0)
                .map(|c| c.into_rgb_f32().collect());

            // Material base color factor (no texture sampling here — we keep the
            // engine's default brick texture for now and tint with vertex color).
            let base_color = prim.material().pbr_metallic_roughness().base_color_factor();
            let tint = glam::Vec3::new(base_color[0], base_color[1], base_color[2]);

            let base_idx = out.vertices.len() as u32;
            for i in 0..positions.len() {
                let p_local = glam::Vec3::from(positions[i]).extend(1.0);
                let p_world = (world * p_local).truncate();
                let n_local = glam::Vec3::from(normals[i]);
                let n_world = (normal_mat * n_local).normalize_or_zero();
                let color = match &colors {
                    Some(c) => glam::Vec3::from(c[i]) * tint,
                    None => tint,
                };
                out.vertices.push(Vertex {
                    position: p_world,
                    normal: if n_world == glam::Vec3::ZERO { glam::Vec3::Y } else { n_world },
                    color,
                    uv: glam::Vec2::from(uvs[i]),
                });
            }

            if let Some(idx_iter) = reader.read_indices() {
                for i in idx_iter.into_u32() {
                    out.indices.push(base_idx + i);
                }
            } else {
                // Non-indexed primitive: emit sequential indices.
                for i in 0..(positions.len() as u32) {
                    out.indices.push(base_idx + i);
                }
            }
        }
    }

    for child in node.children() {
        visit_node(&child, world, buffers, out);
    }
}

// =====================================================================
// Skinned mesh (Phase 2a — data + loader; rendering wired in Phase 2b)
// =====================================================================

/// Per-vertex skinning influence: up to 4 joints with normalized weights.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VertexSkin {
    pub joints: [u16; 4],
    pub weights: [f32; 4],
}

/// One node in the skeleton.
///
/// `parent` is an index into `SkinnedMesh::joints` (None = skeleton root).
/// `local_bind` is the joint's local transform in bind pose, in the parent's
/// space (extracted from the glTF node's TRS).
/// `inverse_bind` is the standard glTF `inverseBindMatrices` entry: it maps
/// a vertex from model space into the joint's local space at bind pose.
#[derive(Clone, Debug)]
pub struct Joint {
    pub name: String,
    pub parent: Option<u16>,
    pub local_bind: glam::Mat4,
    pub inverse_bind: glam::Mat4,
    /// glTF node index — used to match animation channels back to joints.
    pub node_index: u32,
}

/// A mesh authored in skin/bind-pose space, with per-vertex skinning data
/// and a flat skeleton array. The bind-pose vertex positions in
/// `bind_vertices` are already expressed in *model* space (i.e. the space
/// that `inverse_bind` matrices map into).
///
/// To render, compute a `bone_palette: [Mat4; joints.len()]` such that
/// `bone_palette[j] = current_joint_world(j) * inverse_bind[j]`, then call
/// `skin_to` to produce the deformed vertex buffer.
pub struct SkinnedMesh {
    pub bind_vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub vertex_skin: Vec<VertexSkin>,
    pub joints: Vec<Joint>,
    /// Joint name → index into `joints`. Convenience for animation lookup.
    pub joint_index_by_name: std::collections::HashMap<String, u16>,
    /// Joint glTF-node-index → index into `joints`. Used by animation
    /// channels which reference target nodes by index, not by name.
    pub joint_index_by_node: std::collections::HashMap<u32, u16>,
}

impl SkinnedMesh {
    /// Load a skinned mesh from a glTF / .glb file. Picks the *first* skin in
    /// the document. Vertices from every primitive of every skinned mesh that
    /// uses that skin are merged into one buffer in model space (i.e. skin
    /// space — the space referenced by inverseBindMatrices).
    pub fn from_gltf<P: AsRef<std::path::Path>>(path: P) -> anyhow::Result<Self> {
        let original = path.as_ref().to_path_buf();
        let candidates = [
            original.clone(),
            std::path::PathBuf::from("..").join(&original),
            std::path::PathBuf::from("../..").join(&original),
            std::path::PathBuf::from("../../..").join(&original),
        ];
        let resolved = candidates.iter().find(|p| p.exists()).cloned()
            .ok_or_else(|| anyhow::anyhow!(
                "skinned gltf file not found in any candidate path (cwd={:?}): {:?}",
                std::env::current_dir().ok(), original
            ))?;
        log::info!("Loading skinned glTF from {:?}", resolved);

        let gltf = gltf::Gltf::open(&resolved)
            .map_err(|e| anyhow::anyhow!("gltf open failed for {:?}: {}", resolved, e))?;
        let base_dir = resolved.parent().unwrap_or_else(|| std::path::Path::new("."));
        let buffers = gltf::import_buffers(&gltf.document, Some(base_dir), gltf.blob.clone())
            .map_err(|e| anyhow::anyhow!("gltf buffer load failed for {:?}: {}", resolved, e))?;
        let doc = gltf.document;

        let skin = doc.skins().next()
            .ok_or_else(|| anyhow::anyhow!("gltf has no skin: {:?}", resolved))?;

        // ---- Build the skeleton (flat joint array) ----
        // Map glTF node index -> our joint index, in the order glTF's skin lists them
        // (this is also the order inverseBindMatrices uses).
        let joint_node_indices: Vec<u32> =
            skin.joints().map(|n| n.index() as u32).collect();
        let joint_index_by_node: std::collections::HashMap<u32, u16> = joint_node_indices
            .iter()
            .enumerate()
            .map(|(i, &n)| (n, i as u16))
            .collect();

        // inverseBindMatrices accessor — required for skinning.
        let ibm_reader = skin.reader(|b| Some(&buffers[b.index()]));
        let inverse_binds: Vec<glam::Mat4> = ibm_reader
            .read_inverse_bind_matrices()
            .map(|it| it.map(|m| glam::Mat4::from_cols_array_2d(&m)).collect())
            .unwrap_or_else(|| vec![glam::Mat4::IDENTITY; joint_node_indices.len()]);

        // For each joint node, record local TRS (its bind-pose local) and parent.
        let mut parent_of: std::collections::HashMap<u32, u32> =
            std::collections::HashMap::new();
        for node in doc.nodes() {
            for child in node.children() {
                parent_of.insert(child.index() as u32, node.index() as u32);
            }
        }
        let mut joints: Vec<Joint> = Vec::with_capacity(joint_node_indices.len());
        let mut joint_index_by_name: std::collections::HashMap<String, u16> =
            std::collections::HashMap::new();
        for (i, &node_idx) in joint_node_indices.iter().enumerate() {
            let node = doc.nodes().nth(node_idx as usize).unwrap();
            let local = glam::Mat4::from_cols_array_2d(&node.transform().matrix());
            let parent = parent_of
                .get(&node_idx)
                .and_then(|p| joint_index_by_node.get(p).copied());
            let name = node.name().unwrap_or("").to_string();
            if !name.is_empty() {
                joint_index_by_name.insert(name.clone(), i as u16);
            }
            joints.push(Joint {
                name,
                parent,
                local_bind: local,
                inverse_bind: inverse_binds.get(i).copied().unwrap_or(glam::Mat4::IDENTITY),
                node_index: node_idx,
            });
        }

        // ---- Collect skinned mesh primitives ----
        // A node uses this skin if `node.skin() == Some(skin)`. Vertices from
        // such primitives are already authored in skin/model space (no node
        // transform should be applied — glTF spec).
        let mut bind_vertices: Vec<Vertex> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut vertex_skin: Vec<VertexSkin> = Vec::new();
        let target_skin_idx = skin.index();

        for node in doc.nodes() {
            let Some(node_skin) = node.skin() else { continue };
            if node_skin.index() != target_skin_idx { continue }
            let Some(gmesh) = node.mesh() else { continue };

            for prim in gmesh.primitives() {
                let reader = prim.reader(|b| Some(&buffers[b.index()]));
                let positions: Vec<[f32; 3]> = match reader.read_positions() {
                    Some(it) => it.collect(),
                    None => continue,
                };
                let normals: Vec<[f32; 3]> = reader
                    .read_normals()
                    .map(|it| it.collect())
                    .unwrap_or_else(|| vec![[0.0, 1.0, 0.0]; positions.len()]);
                let uvs: Vec<[f32; 2]> = reader
                    .read_tex_coords(0)
                    .map(|tc| tc.into_f32().collect())
                    .unwrap_or_else(|| vec![[0.5, 0.5]; positions.len()]);
                let joints_attr: Vec<[u16; 4]> = reader
                    .read_joints(0)
                    .map(|j| j.into_u16().collect())
                    .unwrap_or_else(|| vec![[0; 4]; positions.len()]);
                let weights_attr: Vec<[f32; 4]> = reader
                    .read_weights(0)
                    .map(|w| w.into_f32().collect())
                    .unwrap_or_else(|| vec![[1.0, 0.0, 0.0, 0.0]; positions.len()]);

                let base_color = prim.material().pbr_metallic_roughness().base_color_factor();
                let tint = glam::Vec3::new(base_color[0], base_color[1], base_color[2]);
                let colors: Option<Vec<[f32; 3]>> = reader
                    .read_colors(0)
                    .map(|c| c.into_rgb_f32().collect());

                let base_idx = bind_vertices.len() as u32;
                for i in 0..positions.len() {
                    let color = match &colors {
                        Some(c) => glam::Vec3::from(c[i]) * tint,
                        None => tint,
                    };
                    bind_vertices.push(Vertex {
                        position: glam::Vec3::from(positions[i]),
                        normal: glam::Vec3::from(normals[i]).normalize_or_zero(),
                        color,
                        uv: glam::Vec2::from(uvs[i]),
                    });
                    // Renormalize weights defensively (glTF requires them to sum to 1,
                    // but exporters sometimes drift).
                    let mut w = weights_attr[i];
                    let sum = w[0] + w[1] + w[2] + w[3];
                    if sum > 1e-5 {
                        let inv = 1.0 / sum;
                        w[0] *= inv; w[1] *= inv; w[2] *= inv; w[3] *= inv;
                    } else {
                        w = [1.0, 0.0, 0.0, 0.0];
                    }
                    vertex_skin.push(VertexSkin { joints: joints_attr[i], weights: w });
                }

                if let Some(idx_iter) = reader.read_indices() {
                    for i in idx_iter.into_u32() {
                        indices.push(base_idx + i);
                    }
                } else {
                    for i in 0..(positions.len() as u32) {
                        indices.push(base_idx + i);
                    }
                }
            }
        }

        if bind_vertices.is_empty() {
            anyhow::bail!("gltf {:?} produced an empty skinned mesh", resolved);
        }

        // Bounds for sanity log.
        let mut mn = glam::Vec3::splat(f32::INFINITY);
        let mut mx = glam::Vec3::splat(f32::NEG_INFINITY);
        for v in &bind_vertices {
            mn = mn.min(v.position);
            mx = mx.max(v.position);
        }
        log::info!(
            "Loaded skinned glTF {:?}: {} verts, {} tris, {} joints, bounds [{:.2},{:.2},{:.2}] -> [{:.2},{:.2},{:.2}]",
            resolved.file_name().unwrap_or_default(),
            bind_vertices.len(), indices.len() / 3, joints.len(),
            mn.x, mn.y, mn.z, mx.x, mx.y, mx.z,
        );

        Ok(Self {
            bind_vertices,
            indices,
            vertex_skin,
            joints,
            joint_index_by_name,
            joint_index_by_node,
        })
    }

    /// Number of joints (palette size needed for rendering).
    pub fn joint_count(&self) -> usize { self.joints.len() }

    /// Build a per-joint mask in `[0, 1]` selecting "upper-body" joints
    /// (spine, neck, head, clavicles, arms, hands, weapons). Used by the
    /// animation layer system so a spell-cast clip can override the upper
    /// body while the base locomotion clip continues to drive the legs.
    ///
    /// A joint receives weight 1 if any *ancestor* (including itself)
    /// matches an upper-body name pattern. This way fingers/weapons that
    /// don't directly contain "spine"/"arm" still inherit the mask via
    /// their parent chain.
    pub fn upper_body_mask(&self) -> Vec<f32> {
        const UPPER_TOKENS: &[&str] = &[
            "spine", "chest", "neck", "head",
            "clavicle", "shoulder",
            "upperarm", "forearm", "lowerarm", "hand", "finger", "thumb",
            "weapon", "prop", "tool",
        ];
        // First pass: direct hits.
        let mut weight: Vec<f32> = self.joints.iter().map(|j| {
            let n = j.name.to_ascii_lowercase();
            if UPPER_TOKENS.iter().any(|tok| n.contains(tok)) { 1.0 } else { 0.0 }
        }).collect();
        // Second pass: propagate from any matched ancestor down to descendants.
        // Joints in skin order have parents earlier in the array (per glTF spec).
        for i in 0..self.joints.len() {
            if weight[i] >= 1.0 { continue }
            if let Some(p) = self.joints[i].parent {
                if weight[p as usize] >= 1.0 {
                    weight[i] = 1.0;
                }
            }
        }
        weight
    }

    /// Index of the lowest joint in the spine chain — the joint where a
    /// torso-twist (e.g. "aim offset" between hips and shoulders) should
    /// be applied. Returns the first joint whose name contains "spine"
    /// and whose parent does NOT contain "spine", which in standard UE/
    /// Mixamo/UAL skeletons is `spine_01`. Falls back to the first
    /// matched spine joint, then to None if the rig has no spine.
    pub fn spine_root_joint(&self) -> Option<usize> {
        let lower = |s: &str| s.to_ascii_lowercase();
        let is_spine = |i: usize| lower(&self.joints[i].name).contains("spine");
        for (i, _) in self.joints.iter().enumerate() {
            if is_spine(i) {
                let parent_is_spine = self.joints[i].parent
                    .map(|p| is_spine(p as usize))
                    .unwrap_or(false);
                if !parent_is_spine { return Some(i); }
            }
        }
        // Fallback: any matched spine joint.
        self.joints.iter().position(|j| j.name.to_ascii_lowercase().contains("spine"))
    }

    /// Build a bone palette that produces the bind pose (i.e. an identity
    /// deformation). Useful as a starting point and for Phase 2b verification.
    pub fn bind_pose_palette(&self) -> Vec<glam::Mat4> {
        vec![glam::Mat4::IDENTITY; self.joints.len()]
    }

    /// Build a `Vec<Mat4>` of joint *world* transforms in bind pose by
    /// composing `local_bind` up the parent chain. Useful for animation work.
    pub fn bind_world_transforms(&self) -> Vec<glam::Mat4> {
        let mut out = vec![glam::Mat4::IDENTITY; self.joints.len()];
        for (i, j) in self.joints.iter().enumerate() {
            let parent = j.parent
                .map(|p| out[p as usize])
                .unwrap_or(glam::Mat4::IDENTITY);
            out[i] = parent * j.local_bind;
        }
        out
    }

    /// Apply the bone palette to `bind_vertices`, writing results into `out`.
    /// `out` is resized to match. `bone_palette[j]` should equal
    /// `current_joint_world[j] * inverse_bind[j]`.
    pub fn skin_to(&self, bone_palette: &[glam::Mat4], out: &mut Vec<Vertex>) {
        out.clear();
        out.reserve(self.bind_vertices.len());
        for (v, s) in self.bind_vertices.iter().zip(self.vertex_skin.iter()) {
            // Skinning matrix = sum_i(weight_i * palette[joint_i])
            let mut m = glam::Mat4::ZERO;
            for k in 0..4 {
                let w = s.weights[k];
                if w == 0.0 { continue }
                let idx = s.joints[k] as usize;
                if idx < bone_palette.len() {
                    m += bone_palette[idx] * w;
                }
            }
            // If a vertex somehow had zero weight (shouldn't happen post-renorm),
            // fall back to identity so it stays in bind pose.
            if m == glam::Mat4::ZERO {
                m = glam::Mat4::IDENTITY;
            }
            let p = (m * v.position.extend(1.0)).truncate();
            // Transform normal by upper-3x3 (assumes uniform-ish skinning;
            // good enough for game characters).
            let n3 = glam::Mat3::from_mat4(m);
            let n = (n3 * v.normal).normalize_or_zero();
            out.push(Vertex {
                position: p,
                normal: if n == glam::Vec3::ZERO { v.normal } else { n },
                color: v.color,
                uv: v.uv,
            });
        }
    }
}