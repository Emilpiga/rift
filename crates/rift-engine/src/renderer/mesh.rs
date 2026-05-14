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
        let body = Vec3::new(0.35, 0.65, 1.30); // bright spectral blue HDR
        let eye = Vec3::new(1.60, 1.40, 0.50); // gold (HDR)
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
        let skin = Vec3::new(0.55, 0.42, 0.32); // muted leather/skin tone
        let cuff = Vec3::new(0.30, 0.22, 0.16); // darker at the hand
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
            v([0.5, -0.5, 1.0], [0.0, 0.0, 1.0], cuff),
            v([0.5, 0.5, 1.0], [0.0, 0.0, 1.0], cuff),
            v([-0.5, 0.5, 1.0], [0.0, 0.0, 1.0], cuff),
            // Back face (z = 0, shoulder)
            v([0.5, -0.5, 0.0], [0.0, 0.0, -1.0], skin),
            v([-0.5, -0.5, 0.0], [0.0, 0.0, -1.0], skin),
            v([-0.5, 0.5, 0.0], [0.0, 0.0, -1.0], skin),
            v([0.5, 0.5, 0.0], [0.0, 0.0, -1.0], skin),
            // Top face (y = 0.5)
            v([-0.5, 0.5, 0.0], [0.0, 1.0, 0.0], skin),
            v([0.5, 0.5, 0.0], [0.0, 1.0, 0.0], skin),
            v([0.5, 0.5, 1.0], [0.0, 1.0, 0.0], cuff),
            v([-0.5, 0.5, 1.0], [0.0, 1.0, 0.0], cuff),
            // Bottom face (y = -0.5)
            v([-0.5, -0.5, 1.0], [0.0, -1.0, 0.0], cuff),
            v([0.5, -0.5, 1.0], [0.0, -1.0, 0.0], cuff),
            v([0.5, -0.5, 0.0], [0.0, -1.0, 0.0], skin),
            v([-0.5, -0.5, 0.0], [0.0, -1.0, 0.0], skin),
            // Right face (x = 0.5)
            v([0.5, -0.5, 0.0], [1.0, 0.0, 0.0], skin),
            v([0.5, -0.5, 1.0], [1.0, 0.0, 0.0], cuff),
            v([0.5, 0.5, 1.0], [1.0, 0.0, 0.0], cuff),
            v([0.5, 0.5, 0.0], [1.0, 0.0, 0.0], skin),
            // Left face (x = -0.5)
            v([-0.5, -0.5, 1.0], [-1.0, 0.0, 0.0], cuff),
            v([-0.5, -0.5, 0.0], [-1.0, 0.0, 0.0], skin),
            v([-0.5, 0.5, 0.0], [-1.0, 0.0, 0.0], skin),
            v([-0.5, 0.5, 1.0], [-1.0, 0.0, 0.0], cuff),
        ];

        let indices = vec![
            0, 1, 2, 2, 3, 0, 4, 5, 6, 6, 7, 4, 8, 9, 10, 10, 11, 8, 12, 13, 14, 14, 15, 12, 16,
            17, 18, 18, 19, 16, 20, 21, 22, 22, 23, 20,
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
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        let mut emit_face = |p00: Vec3,
                             p10: Vec3,
                             p11: Vec3,
                             p01: Vec3,
                             normal: Vec3,
                             uv00: Vec2,
                             uv10: Vec2,
                             uv11: Vec2,
                             uv01: Vec2,
                             u_steps: u32,
                             v_steps: u32| {
            let base = vertices.len() as u32;
            for v in 0..=v_steps {
                let tv = v as f32 / v_steps as f32;
                let left = p00.lerp(p01, tv);
                let right = p10.lerp(p11, tv);
                let uv_left = uv00.lerp(uv01, tv);
                let uv_right = uv10.lerp(uv11, tv);
                for u in 0..=u_steps {
                    let tu = u as f32 / u_steps as f32;
                    vertices.push(Vertex {
                        position: left.lerp(right, tu),
                        normal,
                        color,
                        uv: uv_left.lerp(uv_right, tu),
                    });
                }
            }
            let row = u_steps + 1;
            for v in 0..v_steps {
                for u in 0..u_steps {
                    let a = base + v * row + u;
                    let b = a + 1;
                    let c = a + row;
                    let d = c + 1;
                    indices.extend_from_slice(&[a, b, d, d, c, a]);
                }
            }
        };

        let side_u = 4;
        let side_v = 10;
        let top_steps = 4;
        emit_face(
            Vec3::new(-0.5, 0.0, 0.5),
            Vec3::new(0.5, 0.0, 0.5),
            Vec3::new(0.5, h, 0.5),
            Vec3::new(-0.5, h, 0.5),
            Vec3::Z,
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, h),
            Vec2::new(0.0, h),
            side_u,
            side_v,
        );
        emit_face(
            Vec3::new(0.5, 0.0, -0.5),
            Vec3::new(-0.5, 0.0, -0.5),
            Vec3::new(-0.5, h, -0.5),
            Vec3::new(0.5, h, -0.5),
            Vec3::NEG_Z,
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, h),
            Vec2::new(0.0, h),
            side_u,
            side_v,
        );
        emit_face(
            Vec3::new(-0.5, h, 0.5),
            Vec3::new(0.5, h, 0.5),
            Vec3::new(0.5, h, -0.5),
            Vec3::new(-0.5, h, -0.5),
            Vec3::Y,
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 1.0),
            top_steps,
            top_steps,
        );
        emit_face(
            Vec3::new(0.5, 0.0, 0.5),
            Vec3::new(0.5, 0.0, -0.5),
            Vec3::new(0.5, h, -0.5),
            Vec3::new(0.5, h, 0.5),
            Vec3::X,
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, h),
            Vec2::new(0.0, h),
            side_u,
            side_v,
        );
        emit_face(
            Vec3::new(-0.5, 0.0, -0.5),
            Vec3::new(-0.5, 0.0, 0.5),
            Vec3::new(-0.5, h, 0.5),
            Vec3::new(-0.5, h, -0.5),
            Vec3::NEG_X,
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, h),
            Vec2::new(0.0, h),
            side_u,
            side_v,
        );

        Self { vertices, indices }
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
    pub fn wraith(
        body: Vec3,
        hood: Vec3,
        eye: Vec3,
        radius: f32,
        height: f32,
        float_h: f32,
    ) -> Self {
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
        m.append_lathe(
            profile,
            body,
            hood,
            radius,
            height,
            float_h,
            azimuth_segments,
        );

        // Eyes — placed on the front of the head where the profile says
        // r ≈ 0.78. Use the head's t-range center.
        let eye_t = 0.83;
        let eye_y = float_h + height * eye_t;
        let eye_r_at = radius * 0.78;
        let eye_r = (radius * 0.10).clamp(0.025, 0.07);
        let eye_x = eye_r_at * 0.32;
        let eye_z = eye_r_at * 0.95;
        m.append_ellipsoid(
            Vec3::new(eye_r, eye_r, eye_r * 0.7),
            Vec3::new(-eye_x, eye_y, eye_z),
            eye,
            6,
            4,
        );
        m.append_ellipsoid(
            Vec3::new(eye_r, eye_r, eye_r * 0.7),
            Vec3::new(eye_x, eye_y, eye_z),
            eye,
            6,
            4,
        );
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
        if profile.len() < 2 || segments < 3 {
            return;
        }

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
            let (t_next, rf_next) = if i + 1 == profile.len() {
                profile[profile.len() - 1]
            } else {
                profile[i + 1]
            };
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
                } else {
                    0.0
                };
                let r = (rf + wobble).max(0.0) * radius;

                let pos = Vec3::new(s * r, y, c * r);
                let normal = Vec3::new(s * n_r, n_y, c * n_r).normalize_or_zero();
                self.vertices.push(Vertex {
                    position: pos,
                    normal: if normal == Vec3::ZERO {
                        Vec3::Y
                    } else {
                        normal
                    },
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
        Self {
            vertices: Vec::new(),
            indices: Vec::new(),
        }
    }

    /// Append an ellipsoid to this mesh. The ellipsoid is a unit UV sphere
    /// non-uniformly scaled by `scale` and translated to `offset`. Normals are
    /// rescaled by `1/scale` to remain (approximately) correct under the
    /// non-uniform deformation.
    pub fn append_ellipsoid(
        &mut self,
        scale: Vec3,
        offset: Vec3,
        color: Vec3,
        slices: u32,
        stacks: u32,
    ) {
        let base = self.vertices.len() as u32;
        let inv_scale = Vec3::new(
            1.0 / scale.x.max(1e-4),
            1.0 / scale.y.max(1e-4),
            1.0 / scale.z.max(1e-4),
        );

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
                    normal: if normal == Vec3::ZERO {
                        Vec3::Y
                    } else {
                        normal
                    },
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

    /// Generate a batched dungeon floor from tile positions.
    /// Uses checker pattern with slight color variation for visual interest.
    pub fn dungeon_floor(positions: &[Vec3], floor_num: u32) -> Self {
        const SUBDIV: u32 = 4;
        let verts_per_tile = ((SUBDIV + 1) * (SUBDIV + 1)) as usize;
        let indices_per_tile = (SUBDIV * SUBDIV * 6) as usize;
        let mut vertices = Vec::with_capacity(positions.len() * verts_per_tile);
        let mut indices = Vec::with_capacity(positions.len() * indices_per_tile);

        // Color palette changes per floor for visual variety
        // These tint the stone texture, so keep them brighter (texture darkens them)
        let base_color = match floor_num % 4 {
            0 => Vec3::new(0.55, 0.50, 0.45), // dark stone
            1 => Vec3::new(0.45, 0.55, 0.38), // mossy dungeon
            2 => Vec3::new(0.60, 0.35, 0.30), // infernal
            _ => Vec3::new(0.38, 0.48, 0.60), // ice cavern
        };

        for pos in positions.iter() {
            let base_idx = vertices.len() as u32;
            let ix = pos.x as u32;
            let iz = pos.z as u32;
            // Subtle variation using position hash
            let hash = ((ix.wrapping_mul(7) ^ iz.wrapping_mul(13)) % 100) as f32 / 800.0;
            let color = base_color + Vec3::splat(hash);

            // pos.y carries the tile's elevation (set by
            // dungeon::Floor::floor_positions); we lay the
            // quad at that height so raised daises and sunken
            // pits land at the right Y without per-tile
            // transforms.
            for z in 0..=SUBDIV {
                let tz = z as f32 / SUBDIV as f32;
                let local_z = -0.5 + tz;
                for x in 0..=SUBDIV {
                    let tx = x as f32 / SUBDIV as f32;
                    let local_x = -0.5 + tx;
                    let world = *pos + Vec3::new(local_x, 0.0, local_z);
                    vertices.push(Vertex {
                        position: world,
                        normal: Vec3::Y,
                        color,
                        uv: Vec2::new(world.x, world.z),
                    });
                }
            }

            let row = SUBDIV + 1;
            for z in 0..SUBDIV {
                for x in 0..SUBDIV {
                    let a = base_idx + z * row + x;
                    let b = a + 1;
                    let c = a + row;
                    let d = c + 1;
                    indices.extend_from_slice(&[a, d, b, a, c, d]);
                }
            }
        }

        Self { vertices, indices }
    }

    /// Vertical "skirt" geometry along elevation discontinuities
    /// between adjacent walkable tiles. Without these, a
    /// raised dais or sunken pit shows a thin gap right through
    /// the world wherever its lip meets a floor at a different
    /// elevation — the upper tile's quad sits at one Y and the
    /// neighbouring lower tile's quad sits at another, with
    /// nothing connecting them.
    ///
    /// Stair tiles **are** included on their slope-perpendicular
    /// sides: a ramp going up between two flat floors has a
    /// triangular wedge of empty space on each lateral side
    /// (the side you'd brush past walking up the ramp), and
    /// without a trapezoidal skirt the player can see right
    /// through the world there. Stair sides along the slope
    /// axis (the leading low / high edges) normally meet a
    /// floor at matching elevation and emit nothing.
    ///
    /// Each emitted quad is double-sided (two opposed
    /// triangles) so it reads correctly whether the player
    /// stands above or below the lip — the engine renders
    /// with backface culling and we may see the skirt from
    /// either direction depending on camera angle.
    pub fn dungeon_floor_skirts(floor: &rift_dungeon::Floor, floor_num: u32) -> Self {
        Self::dungeon_floor_skirts_filtered(floor, floor_num, |_, _| true)
    }

    /// Like [`Self::dungeon_floor_skirts`] but only emits a
    /// skirt quad for adjacent tile pairs `(a, b)` where the
    /// caller's `accept((ax, az), (bx, bz))` predicate returns
    /// `true`. Used by the per-room texture-pack split: the
    /// client builds one skirt mesh per material pack and
    /// asks for only the elevation seams whose two endpoints
    /// both belong to that pack, so each skirt strip carries
    /// the same authored material as the floor it bridges
    /// instead of always falling back to the default stone
    /// pack.
    pub fn dungeon_floor_skirts_filtered(
        floor: &rift_dungeon::Floor,
        floor_num: u32,
        accept: impl Fn((usize, usize), (usize, usize)) -> bool,
    ) -> Self {
        use rift_dungeon::{StairDir, Tile, ELEVATION_STEP};

        // Same palette as `dungeon_floor` but slightly darker
        // so the vertical face reads as receding into shadow,
        // even before the lighting pass kicks in. Skirts sit
        // in cavities where the key light rarely hits at
        // grazing angles, so darkening them here prevents
        // them from reading as a disconnected shelf in the
        // ambient term.
        let base_color = match floor_num % 4 {
            0 => Vec3::new(0.45, 0.40, 0.35),
            1 => Vec3::new(0.35, 0.45, 0.28),
            2 => Vec3::new(0.50, 0.25, 0.20),
            _ => Vec3::new(0.28, 0.38, 0.50),
        };

        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        // Top-surface world Y at a tile-local corner. `local`
        // is in `[-0.5, 0.5]^2` (tile centre at (0,0)). For
        // [`Tile::Floor`] this is constant; for
        // [`Tile::Stair`] it varies linearly along the slope
        // axis from `elevation*STEP` at the low edge to
        // `(elevation+1)*STEP` at the high edge. Walls return
        // `None` — they don't need skirts because the wall
        // mesh itself extends from y=0 upward.
        let corner_y = |i: usize, local: (f32, f32)| -> Option<f32> {
            match floor.tiles[i] {
                Tile::Floor => Some(floor.elevation[i] as f32 * ELEVATION_STEP),
                Tile::Stair { dir } => {
                    let base = floor.elevation[i] as f32 * ELEVATION_STEP;
                    // `t` ramps from 0 at the low edge to 1 at
                    // the high edge along the slope axis.
                    let t = match dir {
                        StairDir::PosX => local.0 + 0.5,
                        StairDir::NegX => 0.5 - local.0,
                        StairDir::PosZ => local.1 + 0.5,
                        StairDir::NegZ => 0.5 - local.1,
                    };
                    Some(base + t.clamp(0.0, 1.0) * ELEVATION_STEP)
                }
                Tile::Wall => None,
            }
        };

        // Emit a (potentially trapezoidal) skirt quad whose
        // shared edge runs from `a` to `b` in the XZ plane.
        // `a_low..a_high` is the vertical span at endpoint
        // `a`; same for `b`. `normal` faces the side that's
        // visible to a player standing on the *lower* tile.
        // Degenerate (both endpoints have zero span) inputs
        // are filtered by the caller.
        let mut emit_quad =
            |a: Vec3, b: Vec3, a_low: f32, a_high: f32, b_low: f32, b_high: f32, normal: Vec3| {
                let base = vertices.len() as u32;
                // World-space UVs along the strip — keep the
                // shipped ground-tile texture wrapping naturally.
                let u0 = a.x + a.z;
                let u1 = b.x + b.z;
                let p_al = Vec3::new(a.x, a_low, a.z);
                let p_bl = Vec3::new(b.x, b_low, b.z);
                let p_bh = Vec3::new(b.x, b_high, b.z);
                let p_ah = Vec3::new(a.x, a_high, a.z);
                let push = |v: &mut Vec<Vertex>, p: Vec3, n: Vec3, uv: Vec2| {
                    v.push(Vertex {
                        position: p,
                        normal: n,
                        color: base_color,
                        uv,
                    });
                };
                // Front face (normal pointing toward the lower
                // tile — the side a player on the lower floor
                // sees).
                push(&mut vertices, p_al, normal, Vec2::new(u0, a_low));
                push(&mut vertices, p_bl, normal, Vec2::new(u1, b_low));
                push(&mut vertices, p_bh, normal, Vec2::new(u1, b_high));
                push(&mut vertices, p_ah, normal, Vec2::new(u0, a_high));
                indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
                // Back face (opposite normal). Same vertex
                // positions, opposite winding + flipped normal.
                let back = vertices.len() as u32;
                push(&mut vertices, p_al, -normal, Vec2::new(u0, a_low));
                push(&mut vertices, p_bl, -normal, Vec2::new(u1, b_low));
                push(&mut vertices, p_bh, -normal, Vec2::new(u1, b_high));
                push(&mut vertices, p_ah, -normal, Vec2::new(u0, a_high));
                indices.extend_from_slice(&[back, back + 2, back + 1, back, back + 3, back + 2]);
            };

        // Helper: process one shared edge between tiles `i_a`
        // and `i_b`. `a_corners` / `b_corners` give the
        // tile-local positions of the edge's two endpoints
        // from each tile's frame (a tile sees the shared
        // edge through its own +/- 0.5 corners). `world_*`
        // are the corresponding world-space XZ positions.
        // `normal_from_a_to_b` is the unit vector from a's
        // centre toward b's centre.
        let mut process_edge = |i_a: usize,
                                i_b: usize,
                                a_corners: [(f32, f32); 2],
                                b_corners: [(f32, f32); 2],
                                world_c0: Vec3,
                                world_c1: Vec3,
                                normal_from_a_to_b: Vec3,
                                a_tile: (usize, usize),
                                b_tile: (usize, usize)| {
            // Both tiles must be walkable (have a top
            // surface). Wall vs anything is bridged by
            // the wall mesh.
            let ya0 = corner_y(i_a, a_corners[0]);
            let yb0 = corner_y(i_b, b_corners[0]);
            let ya1 = corner_y(i_a, a_corners[1]);
            let yb1 = corner_y(i_b, b_corners[1]);
            let (ya0, yb0, ya1, yb1) = match (ya0, yb0, ya1, yb1) {
                (Some(a0), Some(b0), Some(a1), Some(b1)) => (a0, b0, a1, b1),
                _ => return,
            };
            // No gap anywhere along the edge → nothing
            // to bridge. Strict equality is fine because
            // both sides compute Y from the same integer
            // elevation × step constant.
            let diff0 = (ya0 - yb0).abs();
            let diff1 = (ya1 - yb1).abs();
            if diff0 < 1.0e-4 && diff1 < 1.0e-4 {
                return;
            }
            if !accept(a_tile, b_tile) {
                return;
            }
            // Per-endpoint low/high so the quad is a
            // trapezoid when one side is sloped (stair).
            let a_low = ya0.min(yb0);
            let a_high = ya0.max(yb0);
            let b_low = ya1.min(yb1);
            let b_high = ya1.max(yb1);
            // Front-facing normal points from the higher
            // tile toward the lower one. Use the average
            // of the two endpoints to pick a side — for
            // a stair-vs-floor side the higher one is
            // unambiguous along most of the edge anyway.
            let avg_a = 0.5 * (ya0 + ya1);
            let avg_b = 0.5 * (yb0 + yb1);
            let n = if avg_a > avg_b {
                normal_from_a_to_b
            } else {
                -normal_from_a_to_b
            };
            emit_quad(world_c0, world_c1, a_low, a_high, b_low, b_high, n);
        };

        // East-west adjacencies (compare tile (x, z) with (x+1, z)).
        // Shared edge runs along Z at the meeting X. From
        // tile a's frame the two endpoints are at local
        // (+0.5, -0.5) and (+0.5, +0.5); from tile b's frame
        // (-0.5, -0.5) and (-0.5, +0.5).
        for z in 0..floor.depth {
            for x in 0..floor.width.saturating_sub(1) {
                let i_a = z * floor.width + x;
                let i_b = z * floor.width + (x + 1);
                let edge_x = (x as f32) + 0.5;
                let c0 = Vec3::new(edge_x, 0.0, (z as f32) - 0.5);
                let c1 = Vec3::new(edge_x, 0.0, (z as f32) + 0.5);
                process_edge(
                    i_a,
                    i_b,
                    [(0.5, -0.5), (0.5, 0.5)],
                    [(-0.5, -0.5), (-0.5, 0.5)],
                    c0,
                    c1,
                    Vec3::new(1.0, 0.0, 0.0),
                    (x, z),
                    (x + 1, z),
                );
            }
        }

        // North-south adjacencies (compare tile (x, z) with (x, z+1)).
        // Shared edge runs along X at the meeting Z. From
        // tile a's frame the two endpoints are at local
        // (-0.5, +0.5) and (+0.5, +0.5); from tile b's frame
        // (-0.5, -0.5) and (+0.5, -0.5).
        for z in 0..floor.depth.saturating_sub(1) {
            for x in 0..floor.width {
                let i_a = z * floor.width + x;
                let i_b = (z + 1) * floor.width + x;
                let edge_z = (z as f32) + 0.5;
                let c0 = Vec3::new((x as f32) - 0.5, 0.0, edge_z);
                let c1 = Vec3::new((x as f32) + 0.5, 0.0, edge_z);
                process_edge(
                    i_a,
                    i_b,
                    [(-0.5, 0.5), (0.5, 0.5)],
                    [(-0.5, -0.5), (0.5, -0.5)],
                    c0,
                    c1,
                    Vec3::new(0.0, 0.0, 1.0),
                    (x, z),
                    (x, z + 1),
                );
            }
        }

        Self { vertices, indices }
    }

    /// Slanted ramp quads for [`rift_dungeon::Tile::Stair`]
    /// tiles. Each input is a `(base_pos, dir)` pair where
    /// `base_pos.y` is the low end's world Y. The high end
    /// rises by `step_y`. Geometry is a single quad per stair
    /// tile, normal recomputed from the slope so lighting
    /// reads as a ramp rather than a flat tile.
    pub fn dungeon_stairs(
        positions: &[(Vec3, rift_dungeon::StairDir)],
        floor_num: u32,
        step_y: f32,
    ) -> Self {
        use rift_dungeon::StairDir;
        let mut vertices = Vec::with_capacity(positions.len() * 4);
        let mut indices = Vec::with_capacity(positions.len() * 6);

        let base_color = match floor_num % 4 {
            0 => Vec3::new(0.55, 0.50, 0.45),
            1 => Vec3::new(0.45, 0.55, 0.38),
            2 => Vec3::new(0.60, 0.35, 0.30),
            _ => Vec3::new(0.38, 0.48, 0.60),
        };

        for (i, (pos, dir)) in positions.iter().enumerate() {
            let base_idx = (i * 4) as u32;
            let ix = pos.x as u32;
            let iz = pos.z as u32;
            let hash = ((ix.wrapping_mul(7) ^ iz.wrapping_mul(13)) % 100) as f32 / 800.0;
            let color = base_color + Vec3::splat(hash);

            // Offsets per corner before lift. We label the
            // four corners by the cardinal side they sit on.
            // posX corner = (+0.5, _, ±0.5); negX = (-0.5,
            // _, ±0.5); etc. The lift is +step_y on the two
            // corners that sit on the rising side, 0 on the
            // others.
            let lift = |sx: f32, sz: f32| -> f32 {
                let on_rise = match dir {
                    StairDir::PosX => sx > 0.0,
                    StairDir::NegX => sx < 0.0,
                    StairDir::PosZ => sz > 0.0,
                    StairDir::NegZ => sz < 0.0,
                };
                if on_rise {
                    step_y
                } else {
                    0.0
                }
            };

            // Slope normal: cross product of along-slope and
            // across-slope tangents. For a +X-rising ramp:
            // along = (1, step_y, 0), across = (0, 0, 1)
            // → normal = (-step_y, 1, 0).normalised.
            let normal = match dir {
                StairDir::PosX => Vec3::new(-step_y, 1.0, 0.0).normalize(),
                StairDir::NegX => Vec3::new(step_y, 1.0, 0.0).normalize(),
                StairDir::PosZ => Vec3::new(0.0, 1.0, -step_y).normalize(),
                StairDir::NegZ => Vec3::new(0.0, 1.0, step_y).normalize(),
            };

            let corners = [(-0.5_f32, -0.5_f32), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)];
            for (sx, sz) in corners {
                let p = *pos + Vec3::new(sx, lift(sx, sz), sz);
                vertices.push(Vertex {
                    position: p,
                    normal,
                    color,
                    uv: Vec2::new(p.x - 0.5, p.z - 0.5),
                });
            }

            indices.extend_from_slice(&[
                base_idx,
                base_idx + 2,
                base_idx + 1,
                base_idx,
                base_idx + 3,
                base_idx + 2,
            ]);
        }

        Self { vertices, indices }
    }

    /// Flat horizontal disc centred at `center`. Used by the hub to
    /// extend the ground far beyond the playable area so the floor's
    /// hard edge fades into the fog instead of cutting off in mid-air.
    /// UVs are world-space so a tiling grass / stone material maps
    /// continuously with the main dungeon floor.
    /// Flat horizontal disc fan in the XZ plane centred at `center`.
    /// `uv_scale` is multiplied into the world-space (x, z) UVs so
    /// callers can choose how many world metres one texture tile
    /// spans (e.g. `uv_scale = 1.0 / 12.0` means one tile covers
    /// 12 m before the sampler wraps). Lower values reduce visible
    /// repetition for large discs.
    pub fn ground_disc(
        center: Vec3,
        radius: f32,
        segments: u32,
        color: Vec3,
        uv_scale: f32,
    ) -> Self {
        let segments = segments.max(8);
        let mut vertices = Vec::with_capacity((segments + 1) as usize);
        let mut indices = Vec::with_capacity((segments * 3) as usize);

        // Centre vertex.
        vertices.push(Vertex {
            position: center,
            normal: Vec3::Y,
            color,
            uv: Vec2::new(center.x * uv_scale, center.z * uv_scale),
        });
        for i in 0..segments {
            let a = (i as f32 / segments as f32) * std::f32::consts::TAU;
            let p = Vec3::new(
                center.x + a.cos() * radius,
                center.y,
                center.z + a.sin() * radius,
            );
            vertices.push(Vertex {
                position: p,
                normal: Vec3::Y,
                color,
                uv: Vec2::new(p.x * uv_scale, p.z * uv_scale),
            });
        }
        for i in 0..segments {
            let next = (i + 1) % segments;
            // World-CCW from +Y, i.e. normal = +Y, so the
            // disc's upward face is the front face under
            // this engine's Y-flipped projection +
            // `FrontFace::CCW` pipeline state.
            indices.extend_from_slice(&[0, 1 + next, 1 + i]);
        }

        Self { vertices, indices }
    }

    /// Flat horizontal annulus (ring) in the XZ plane centred at
    /// `center`. Used by the hub to draw a thin glowing rim along
    /// the floating-platform edge so the silhouette of the island
    /// reads against the dark sky / abyss. Vertex colors carry the
    /// glow tint directly so callers don't need a material.
    pub fn ring(
        center: Vec3,
        inner_radius: f32,
        outer_radius: f32,
        segments: u32,
        color: Vec3,
    ) -> Self {
        let segments = segments.max(8);
        let mut vertices = Vec::with_capacity((segments * 2) as usize);
        let mut indices = Vec::with_capacity((segments * 6) as usize);
        for i in 0..segments {
            let a = (i as f32 / segments as f32) * std::f32::consts::TAU;
            let (cos, sin) = (a.cos(), a.sin());
            let inner = Vec3::new(
                center.x + cos * inner_radius,
                center.y,
                center.z + sin * inner_radius,
            );
            let outer = Vec3::new(
                center.x + cos * outer_radius,
                center.y,
                center.z + sin * outer_radius,
            );
            vertices.push(Vertex {
                position: inner,
                normal: Vec3::Y,
                color,
                uv: Vec2::new(inner.x, inner.z),
            });
            vertices.push(Vertex {
                position: outer,
                normal: Vec3::Y,
                color,
                uv: Vec2::new(outer.x, outer.z),
            });
        }
        for i in 0..segments {
            let next = (i + 1) % segments;
            let i_in = i * 2;
            let i_out = i * 2 + 1;
            let n_in = next * 2;
            let n_out = next * 2 + 1;
            indices.extend_from_slice(&[i_in, i_out, n_out, i_in, n_out, n_in]);
        }
        Self { vertices, indices }
    }

    /// Procedural distant-mountain silhouette ring. Builds a
    /// vertical strip wrapped into a circle of radius `radius`
    /// around `center`, with each angular segment rising to a
    /// pseudo-random height sampled from `[min_height, max_height]`
    /// using a per-segment hash of `seed`. The base of every
    /// segment is sunk to `base_y` so distance fog can swallow
    /// the lower portion and the silhouette reads as bare peaks
    /// rising out of the abyss.
    ///
    /// All faces point inward (toward `center`); the player is
    /// always inside the ring so backfaces are culled invisibly.
    /// Vertex colors carry `color` directly — no material needed.
    pub fn mountain_ring(
        center: Vec3,
        radius: f32,
        base_y: f32,
        min_height: f32,
        max_height: f32,
        segments: u32,
        seed: u64,
        color: Vec3,
    ) -> Self {
        let segments = segments.max(16);
        let mut vertices = Vec::with_capacity((segments * 2) as usize);
        let mut indices = Vec::with_capacity((segments * 6) as usize);
        // Cheap fbm-ish height: blend two hash octaves so the
        // silhouette has both narrow spikes and wider massifs.
        let hash = |i: u32, salt: u64| -> f32 {
            let mut x = (i as u64)
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add(seed)
                .wrapping_add(salt);
            x ^= x >> 33;
            x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
            x ^= x >> 33;
            (x & 0xFFFF) as f32 / 65536.0
        };
        for i in 0..segments {
            let a = (i as f32 / segments as f32) * std::f32::consts::TAU;
            let (cos, sin) = (a.cos(), a.sin());
            // Two-octave height: a slow ridge (low-freq hash on
            // i/4) + a fine spike (per-segment hash). Keeps the
            // silhouette varied without looking like white noise.
            let coarse = hash(i / 4, 0xA);
            let fine = hash(i, 0xB);
            let h_norm = (coarse * 0.65 + fine * 0.35).clamp(0.0, 1.0);
            let top_y = min_height + (max_height - min_height) * h_norm;
            let bx = center.x + cos * radius;
            let bz = center.z + sin * radius;
            // Inward-facing normal so the inner side of the ring
            // catches the key/fog correctly.
            let n = Vec3::new(-cos, 0.0, -sin);
            vertices.push(Vertex {
                position: Vec3::new(bx, base_y, bz),
                normal: n,
                color,
                uv: Vec2::new(i as f32, 0.0),
            });
            vertices.push(Vertex {
                position: Vec3::new(bx, top_y, bz),
                normal: n,
                color,
                uv: Vec2::new(i as f32, 1.0),
            });
        }
        for i in 0..segments {
            let next = (i + 1) % segments;
            let bi = i * 2;
            let ti = i * 2 + 1;
            let bn = next * 2;
            let tn = next * 2 + 1;
            // Wind so the inward face is front-facing.
            indices.extend_from_slice(&[bi, tn, ti, bi, bn, tn]);
        }
        Self { vertices, indices }
    }

    /// Triangulated mountain-ring terrain mesh with computed
    /// normals + tiling UVs, ready for a PBR cliff/rock
    /// material. The shape is described by
    /// [`rift_math::terrain::MountainRingParams`]; this method
    /// is a thin adapter that:
    ///
    /// 1. Calls [`rift_math::terrain::generate_mountain_ring`]
    ///    to get the polar heightfield grid.
    /// 2. Triangulates the grid as a wrapping strip (the
    ///    angular axis wraps; the radial axis does not).
    /// 3. Copies positions / normals / UVs into [`Vertex`].
    ///
    /// `color` is written into every vertex as a constant tint
    /// so callers without a material set still get a sensible
    /// silhouette colour. Callers binding a PBR material
    /// usually pass `Vec3::ONE` and let the basecolor texture
    /// do the work.
    ///
    /// `uv_world_scale` divides the world-space tile UVs from
    /// the generator (1 unit per metre) down to a value that
    /// makes a single tile of the bound texture span an
    /// appropriate world distance. For our 2 k cliff_rocks
    /// pack a value around `4.0` (= one tile per 4 m)
    /// produces detail that reads at climbing distance
    /// without obvious repeats from the play arena.
    pub fn mountain_terrain(
        params: &rift_math::terrain::MountainRingParams,
        center: Vec3,
        color: Vec3,
        uv_world_scale: f32,
    ) -> Self {
        let grid = rift_math::terrain::generate_mountain_ring(params, center);
        let cols = grid.cols;
        let rows = grid.rows;
        let inv_uv = 1.0 / uv_world_scale.max(1e-3);

        let mut vertices = Vec::with_capacity(grid.vertices.len());
        for tv in &grid.vertices {
            vertices.push(Vertex {
                position: tv.position,
                normal: tv.normal,
                color,
                uv: tv.tile_uv * inv_uv,
            });
        }

        // The angular axis wraps, so each ring of quads links
        // column `i` to column `(i + 1) % cols`. The radial
        // axis is open, so we stop at `j = rows - 1`.
        let mut indices = Vec::with_capacity(((rows - 1) * cols * 6) as usize);
        for j in 0..rows.saturating_sub(1) {
            for i in 0..cols {
                let next_i = (i + 1) % cols;
                let a = j * cols + i;
                let b = j * cols + next_i;
                let c = (j + 1) * cols + i;
                let d = (j + 1) * cols + next_i;
                // For a heightfield with the radial direction
                // pointing outward (`j` increasing) and the
                // angular direction wrapping CCW from above
                // (`i` increasing), the cross product of
                // (c-a) × (d-a) on a flat slice points
                // *downward* (−Y). With the engine's
                // Y-flipped projection + `FrontFace::CCW`,
                // world-CCW around an outward / upward
                // normal is the front face — so we need to
                // wind the triangles the *other* way around
                // to make the inner-facing slope (the side
                // visible to a player on the platform
                // looking outward) the front face. The
                // previous `[a, c, d, a, d, b]` winding put
                // the front face on the *outside* of the
                // ring, leaving the inner slopes back-culled
                // and reading as see-through.
                indices.extend_from_slice(&[a, d, c, a, b, d]);
            }
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
            v([0.0, -h, 0.0], [0.0, -1.0, 0.0]), // 1: bottom
            v([s, 0.0, 0.0], [1.0, 0.0, 0.0]),   // 2: +x
            v([-s, 0.0, 0.0], [-1.0, 0.0, 0.0]), // 3: -x
            v([0.0, 0.0, s], [0.0, 0.0, 1.0]),   // 4: +z
            v([0.0, 0.0, -s], [0.0, 0.0, -1.0]), // 5: -z
        ];

        let indices = vec![
            // Top pyramid
            0, 2, 4, 0, 4, 3, 0, 3, 5, 0, 5, 2, // Bottom pyramid
            1, 4, 2, 1, 3, 4, 1, 5, 3, 1, 2, 5,
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
            vertices.push(Vertex {
                position: Vec3::new(a0.cos() * inner_r, y, a0.sin() * inner_r),
                normal: Vec3::Y,
                color: dim,
                uv,
            });
            vertices.push(Vertex {
                position: Vec3::new(a0.cos() * outer_r, y, a0.sin() * outer_r),
                normal: Vec3::Y,
                color: bright,
                uv,
            });
            vertices.push(Vertex {
                position: Vec3::new(a1.cos() * outer_r, y, a1.sin() * outer_r),
                normal: Vec3::Y,
                color: bright,
                uv,
            });
            vertices.push(Vertex {
                position: Vec3::new(a1.cos() * inner_r, y, a1.sin() * inner_r),
                normal: Vec3::Y,
                color: dim,
                uv,
            });

            // Render double-sided so it's visible regardless of camera angle / winding.
            // Front faces (normal up): 0,3,2 and 0,2,1
            indices.extend_from_slice(&[
                base_idx,
                base_idx + 3,
                base_idx + 2,
                base_idx,
                base_idx + 2,
                base_idx + 1,
            ]);
            // Back faces (normal down): 0,1,2 and 0,2,3
            indices.extend_from_slice(&[
                base_idx,
                base_idx + 1,
                base_idx + 2,
                base_idx,
                base_idx + 2,
                base_idx + 3,
            ]);
        }

        Self { vertices, indices }
    }

    /// Terrain-conforming variant of [`Self::targeting_circle`].
    /// Same hollow-ring topology and shading, but the
    /// vertices are baked into world space with each one's
    /// Y sampled from `height_at(world_x, world_z)` plus a
    /// small lift so the ring sits just above the surface
    /// without z-fighting. The mesh is intended to be
    /// uploaded as a *dynamic* mesh and re-baked every frame
    /// (or whenever the cursor moves) — the vertex count and
    /// winding match across rebuilds because both paths run
    /// through this same constructor.
    ///
    /// `center` provides the world XZ centre; its Y is
    /// ignored (the ring follows the floor, not the cursor's
    /// projection plane). Caller's `model_matrix` should be
    /// `Mat4::IDENTITY` since positions are already world-
    /// space; collapse the ring by writing `Mat4::ZERO`
    /// rather than zeroing the vertex buffer.
    pub fn targeting_circle_conformed(
        color: [f32; 3],
        center: Vec3,
        radius: f32,
        height_at: impl Fn(f32, f32) -> f32,
    ) -> Self {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        // Must match `targeting_circle` so callers can switch
        // between the flat and conformed variants without
        // re-allocating the renderer slot.
        let segments = 64u32;
        // Constant lift above the sampled floor height. Same
        // value as the flat variant's `y = 0.08` — chosen so
        // the ring clears both the dungeon floor mesh and any
        // skirt/stair geometry the renderer might rasterise
        // at the same Y. Independent of `radius` so the ring
        // doesn't "float" higher for larger AoEs.
        let lift = 0.08_f32;
        let bright = Vec3::new(color[0] * 8.0, color[1] * 8.0, color[2] * 8.0);
        let dim = Vec3::new(color[0] * 4.0, color[1] * 4.0, color[2] * 4.0);

        let inner_r = radius * 0.75;
        let outer_r = radius;
        let uv = Vec2::new(0.5, 0.5);

        // Closure: world XZ → world position with Y locked
        // to the sampled floor + lift. Inlined twice (inner
        // and outer rim) to avoid a per-vertex branch.
        let push = |angle: f32, r: f32, tint: Vec3, vs: &mut Vec<Vertex>| {
            let wx = center.x + angle.cos() * r;
            let wz = center.z + angle.sin() * r;
            let wy = height_at(wx, wz) + lift;
            vs.push(Vertex {
                position: Vec3::new(wx, wy, wz),
                normal: Vec3::Y,
                color: tint,
                uv,
            });
        };

        for i in 0..segments {
            let a0 = (i as f32 / segments as f32) * std::f32::consts::TAU;
            let a1 = ((i + 1) as f32 / segments as f32) * std::f32::consts::TAU;
            let base_idx = vertices.len() as u32;

            // 0 = inner_a0, 1 = outer_a0, 2 = outer_a1, 3 = inner_a1
            push(a0, inner_r, dim, &mut vertices);
            push(a0, outer_r, bright, &mut vertices);
            push(a1, outer_r, bright, &mut vertices);
            push(a1, inner_r, dim, &mut vertices);

            // Front-facing triangles (normal up).
            indices.extend_from_slice(&[
                base_idx,
                base_idx + 3,
                base_idx + 2,
                base_idx,
                base_idx + 2,
                base_idx + 1,
            ]);
            // Back-facing triangles (so the ring is visible
            // from below too, matching the flat variant).
            indices.extend_from_slice(&[
                base_idx,
                base_idx + 1,
                base_idx + 2,
                base_idx,
                base_idx + 2,
                base_idx + 3,
            ]);
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
        let top_width = 0.04_f32; // tapers at top

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
                vertices.push(Vertex {
                    position: Vec3::new(-sin_a * w0, y0, -cos_a * w0),
                    normal,
                    color: c0,
                    uv: Vec2::ZERO,
                });
                vertices.push(Vertex {
                    position: Vec3::new(sin_a * w0, y0, cos_a * w0),
                    normal,
                    color: c0,
                    uv: Vec2::ZERO,
                });
                vertices.push(Vertex {
                    position: Vec3::new(sin_a * w1, y1, cos_a * w1),
                    normal,
                    color: c1,
                    uv: Vec2::ZERO,
                });
                vertices.push(Vertex {
                    position: Vec3::new(-sin_a * w1, y1, -cos_a * w1),
                    normal,
                    color: c1,
                    uv: Vec2::ZERO,
                });
                indices.extend_from_slice(&[
                    base_idx,
                    base_idx + 1,
                    base_idx + 2,
                    base_idx + 2,
                    base_idx + 3,
                    base_idx,
                ]);

                // Back side (so visible from both directions)
                let base_idx2 = vertices.len() as u32;
                let normal2 = -normal;
                vertices.push(Vertex {
                    position: Vec3::new(sin_a * w0, y0, cos_a * w0),
                    normal: normal2,
                    color: c0,
                    uv: Vec2::ZERO,
                });
                vertices.push(Vertex {
                    position: Vec3::new(-sin_a * w0, y0, -cos_a * w0),
                    normal: normal2,
                    color: c0,
                    uv: Vec2::ZERO,
                });
                vertices.push(Vertex {
                    position: Vec3::new(-sin_a * w1, y1, -cos_a * w1),
                    normal: normal2,
                    color: c1,
                    uv: Vec2::ZERO,
                });
                vertices.push(Vertex {
                    position: Vec3::new(sin_a * w1, y1, cos_a * w1),
                    normal: normal2,
                    color: c1,
                    uv: Vec2::ZERO,
                });
                indices.extend_from_slice(&[
                    base_idx2,
                    base_idx2 + 1,
                    base_idx2 + 2,
                    base_idx2 + 2,
                    base_idx2 + 3,
                    base_idx2,
                ]);
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

            vertices.push(Vertex {
                position: Vec3::new(a0.cos() * ring_inner, ring_y, a0.sin() * ring_inner),
                normal: Vec3::Y,
                color: base_color,
                uv: Vec2::ZERO,
            });
            vertices.push(Vertex {
                position: Vec3::new(a1.cos() * ring_inner, ring_y, a1.sin() * ring_inner),
                normal: Vec3::Y,
                color: base_color,
                uv: Vec2::ZERO,
            });
            vertices.push(Vertex {
                position: Vec3::new(a1.cos() * ring_outer, ring_y, a1.sin() * ring_outer),
                normal: Vec3::Y,
                color: top_color,
                uv: Vec2::ZERO,
            });
            vertices.push(Vertex {
                position: Vec3::new(a0.cos() * ring_outer, ring_y, a0.sin() * ring_outer),
                normal: Vec3::Y,
                color: top_color,
                uv: Vec2::ZERO,
            });

            indices.extend_from_slice(&[
                base_idx,
                base_idx + 1,
                base_idx + 2,
                base_idx + 2,
                base_idx + 3,
                base_idx,
            ]);
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
        let tip = v([0.0, 0.0, -len], [0.0, 0.0, -1.0]);
        let tail = v([0.0, 0.0, len], [0.0, 0.0, 1.0]);
        let east = v([r, 0.0, 0.0], [1.0, 0.0, 0.0]);
        let west = v([-r, 0.0, 0.0], [-1.0, 0.0, 0.0]);
        let up = v([0.0, r, 0.0], [0.0, 1.0, 0.0]);
        let down = v([0.0, -r, 0.0], [0.0, -1.0, 0.0]);

        let vertices = vec![tip, tail, east, west, up, down];
        // Index layout:
        // 0=tip, 1=tail, 2=east, 3=west, 4=up, 5=down
        let indices = vec![
            // Front cone (toward tip)
            0, 4, 2, 0, 2, 5, 0, 5, 3, 0, 3, 4, // Back cone (toward tail)
            1, 2, 4, 1, 5, 2, 1, 3, 5, 1, 4, 3,
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
        let tip_color = [1.0, 1.0, 1.0]; // white-hot tip
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
            0, 1, 2, 2, 3, 0, // Shaft bottom
            4, 5, 6, 6, 7, 4, // Shaft left
            8, 9, 10, 10, 11, 8, // Shaft right
            12, 13, 14, 14, 15, 12, // Tip (4 triangular faces)
            16, 17, 18, // top
            16, 18, 19, // right
            16, 19, 20, // bottom
            16, 20, 17, // left
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
            base_idx,
            base_idx + 1,
            base_idx + 2,
            base_idx + 2,
            base_idx + 3,
            base_idx,
            base_idx + 4,
            base_idx + 5,
            base_idx + 6,
            base_idx + 6,
            base_idx + 7,
            base_idx + 4,
        ]);

        Self { vertices, indices }
    }

    /// Fireball: a glowing emissive sphere built as a low-poly UV sphere.
    /// Vertex colors interpolate from a hot white core to orange edges so it
    /// reads as a fireball even without point lights. The colours are pushed
    /// well above 1.0 so the bloom pass picks the body up as a bright
    /// glowing core, and the edge tint is a deeper orange-red so the sphere
    /// reads against the fireball trail's embers instead of disappearing
    /// into them. Diameter ≈ 0.55 units.
    pub fn fireball() -> Self {
        let radius = 0.27_f32;
        let stacks = 10usize;
        let sectors = 16usize;

        // HDR core / edge — bloom picks these up so the projectile
        // looks like a self-lit ball of fire even before the trail
        // particles draw on top of it.
        let core = glam::Vec3::new(4.5, 3.8, 1.6); // white-hot HDR core
        let edge = glam::Vec3::new(2.4, 0.6, 0.05); // saturated orange flame

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

    /// Arcane bolt — smaller violet-cored sphere used to render
    /// enemy arcane projectiles. Same UV-sphere construction as
    /// [`Self::fireball`] but with a tighter radius and a cool
    /// arcane palette so the bolt reads distinctly from the
    /// player's fireball even at a glance.
    pub fn arcane_bolt() -> Self {
        let radius = 0.18_f32;
        let stacks = 8usize;
        let sectors = 12usize;

        // HDR core / edge in violet-arcane palette. Bloom picks
        // these up so the bolt glows in dim dungeon lighting.
        let core = glam::Vec3::new(3.4, 1.2, 4.6); // hot violet-white core
        let edge = glam::Vec3::new(1.4, 0.2, 2.8); // saturated indigo edge

        let mut vertices: Vec<Vertex> = Vec::with_capacity((stacks + 1) * (sectors + 1));
        for i in 0..=stacks {
            let v = i as f32 / stacks as f32;
            let phi = v * std::f32::consts::PI;
            let y = phi.cos();
            let r = phi.sin();
            for j in 0..=sectors {
                let u = j as f32 / sectors as f32;
                let theta = u * std::f32::consts::TAU;
                let x = r * theta.cos();
                let z = r * theta.sin();
                let n = glam::Vec3::new(x, y, z).normalize_or_zero();
                let pos = n * radius;
                let fade = (1.0 - (y.abs() * 0.6)).max(0.4);
                let color = edge.lerp(core, fade * 0.55);
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
    /// Doctor-Strange-style portal frame.
    ///
    /// Geometry is intentionally minimal — the iconic look comes
    /// from the [`portal_vortex`](crate::renderer::vfx::presets::portal_vortex)
    /// VFX preset, which orbits a dense halo of golden sparks
    /// around the rim. The mesh just provides:
    ///
    ///   * a thin outer torus frame in molten copper,
    ///   * a brighter inner rim in white-hot gold,
    ///   * a dimmed inner disc with a near-black core fading to
    ///     a hint of orange at the rim — reads as "burning hole
    ///     through space" rather than a glowing plate. Mirrored
    ///     on the back face so the portal reads from any camera
    ///     angle.
    pub fn portal() -> Self {
        // Default = the "destination unknown" gold-on-black look.
        // Spawn sites that know where the portal leads should
        // call [`Self::portal_with_palette`] instead.
        Self::portal_with_palette(
            Vec3::new(0.42, 0.66, 0.92), // generic cyan zenith
            Vec3::new(0.85, 0.88, 0.92), // pale horizon
            Vec3::new(0.05, 0.04, 0.04), // dark rim
        )
    }

    /// Dimensional rift portal — a torn hole in reality.
    ///
    /// Geometry intentionally provides only **polar UVs** and a
    /// **wobbly silhouette**; all the visual content (swirling
    /// depth, chromatic veins, edge tendrils) is generated in
    /// `assets/shaders/forward/rift_surface.glsl::shadeRift` at
    /// fragment time. The mesh's job is to:
    ///
    ///   * present an unstable, *non-circular* contour (per-
    ///     vertex angular noise on the outermost ring) so the
    ///     shader's procedural silhouette wobble has an
    ///     irregular base to work against,
    ///   * bake polar coordinates into UVs (`uv.x = radial
    ///     0..1`, `uv.y = angle / TAU`) so the fragment shader
    ///     can layer rotating noise fields without needing a
    ///     model-inverse matrix,
    ///   * fill the disc with enough subdivisions that the
    ///     procedural detail (veins, tendrils, parallax swirls)
    ///     reads smoothly across the surface.
    ///
    /// **No frame torus.** The original gold-ring frame read as
    /// "fantasy MMO loot portal"; the rift look pushes the
    /// boundary into the disc's own shader-driven tendrils and
    /// chromatic edge-bleed. The thin bright aperture is
    /// reconstituted at fragment time by the rim ember band in
    /// `shadeRift`.
    ///
    /// **Vertex colors are intentionally zero.** The portal
    /// shading branch ignores `fragColor` and computes its own
    /// HDR emissive output from the polar UVs + time uniform.
    /// The `_zenith` / `_horizon` / `_ground` palette inputs
    /// from the legacy signature are kept for callsite
    /// compatibility but no longer used.
    pub fn portal_with_palette(_zenith: Vec3, _horizon: Vec3, _ground: Vec3) -> Self {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        let height = 2.1_f32;
        let cy_offset = height * 0.5;
        let disc_segments: u32 = 96;
        let disc_r = 1.20_f32;

        // Per-angle silhouette wobble. Sum of three low-freq
        // sines with prime-ish multipliers and irrational phase
        // offsets gives a non-repeating, organic "torn paper"
        // contour. Magnitude tuned so the outermost ring breaks
        // its perfect-circle shape by ~6–10% radius without
        // becoming spiky. The shader layers an additional
        // *animated* wobble on top of this static baseline; the
        // combination reads as a contour under tension.
        let wobble = |a: f32| -> f32 {
            0.045 * (a * 3.0 + 1.7).sin()
                + 0.030 * (a * 5.0 - 0.9).sin()
                + 0.020 * (a * 11.0 + 2.4).sin()
        };

        // Ring radii (as a fraction of `disc_r`). Inner rings
        // are *not* wobbled — only the outermost contour
        // distorts, otherwise the inner rings would clip
        // through the silhouette and produce visible seams.
        let ring_ts: [f32; 6] = [0.0, 0.18, 0.40, 0.62, 0.82, 1.0];

        // Centre vertex — uv = (0, 0) so the shader maps it to
        // the deepest "core" of the rift.
        let center_idx = vertices.len() as u32;
        vertices.push(Vertex {
            position: Vec3::new(0.0, cy_offset, 0.0),
            normal: Vec3::Z,
            color: Vec3::ZERO,
            uv: Vec2::new(0.0, 0.0),
        });

        // Push a ring of vertices at radial fraction `t`. UVs
        // carry polar coords for the fragment shader: `uv.x` =
        // radial fraction (0 at core, 1 at silhouette), `uv.y`
        // = angle / TAU (0..1 around the ring).
        let push_ring = |vertices: &mut Vec<Vertex>, t: f32, normal: Vec3| -> u32 {
            let r_base = disc_r * t;
            let r_y_base = height * 0.45 * t;
            let start = vertices.len() as u32;
            for i in 0..disc_segments {
                let a = (i as f32 / disc_segments as f32) * std::f32::consts::TAU;
                // Wobble only on the outermost ring so inner
                // rings don't clip the contour.
                let w = if (t - 1.0).abs() < 1e-3 {
                    wobble(a)
                } else {
                    0.0
                };
                let scale = 1.0 + w;
                let r_xz = r_base * scale;
                let r_y = r_y_base * scale;
                vertices.push(Vertex {
                    position: Vec3::new(a.cos() * r_xz, a.sin() * r_y + cy_offset, 0.0),
                    normal,
                    color: Vec3::ZERO,
                    uv: Vec2::new(t, i as f32 / disc_segments as f32),
                });
            }
            start
        };

        // Front face rings (skip ring_ts[0] — that's the centre).
        let mut front_starts: [u32; 5] = [0; 5];
        for (i, &t) in ring_ts[1..].iter().enumerate() {
            front_starts[i] = push_ring(&mut vertices, t, Vec3::Z);
        }

        // Core fan: centre → first ring.
        for i in 0..disc_segments {
            let next = (i + 1) % disc_segments;
            indices.extend_from_slice(&[center_idx, front_starts[0] + i, front_starts[0] + next]);
        }
        // Outer bands: ring[k] → ring[k+1] quads.
        for k in 0..(front_starts.len() - 1) {
            let inner = front_starts[k];
            let outer = front_starts[k + 1];
            for i in 0..disc_segments {
                let next = (i + 1) % disc_segments;
                indices.extend_from_slice(&[
                    inner + i,
                    outer + i,
                    outer + next,
                    inner + i,
                    outer + next,
                    inner + next,
                ]);
            }
        }

        // Back face mirror so the rift reads from behind too.
        let back_center = vertices.len() as u32;
        vertices.push(Vertex {
            position: Vec3::new(0.0, cy_offset, 0.0),
            normal: -Vec3::Z,
            color: Vec3::ZERO,
            uv: Vec2::new(0.0, 0.0),
        });
        let mut back_starts: [u32; 5] = [0; 5];
        for (i, &t) in ring_ts[1..].iter().enumerate() {
            back_starts[i] = push_ring(&mut vertices, t, -Vec3::Z);
        }
        for i in 0..disc_segments {
            let next = (i + 1) % disc_segments;
            indices.extend_from_slice(&[back_center, back_starts[0] + next, back_starts[0] + i]);
        }
        for k in 0..(back_starts.len() - 1) {
            let inner = back_starts[k];
            let outer = back_starts[k + 1];
            for i in 0..disc_segments {
                let next = (i + 1) % disc_segments;
                indices.extend_from_slice(&[
                    inner + i,
                    outer + next,
                    outer + i,
                    inner + i,
                    inner + next,
                    outer + next,
                ]);
            }
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
    ///
    /// Uses [`AssetServer::global`] for image-decode dedup.
    /// Prefer [`Self::from_gltf_with_assets`] when you have an
    /// `AssetServer` handy; new code should accept one explicitly.
    pub fn from_gltf<P: AsRef<std::path::Path>>(path: P) -> anyhow::Result<Self> {
        Self::from_gltf_with_assets(path, crate::assets::AssetServer::global())
    }

    /// Variant that takes an explicit [`AssetServer`] for the
    /// base-colour texture cache.
    pub fn from_gltf_with_assets<P: AsRef<std::path::Path>>(
        path: P,
        assets: &crate::assets::AssetServer,
    ) -> anyhow::Result<Self> {
        Self::from_gltf_with_assets_filtered(path, assets, |_, _| true)
    }

    /// Same as [`Self::from_gltf_with_assets`], but only includes
    /// primitives whose owning glTF node + mesh names pass
    /// `mesh_name_filter`. Useful for rigid multi-part props
    /// authored as one glTF where a caller needs to animate a
    /// single node independently.
    pub fn from_gltf_with_assets_filtered<P, F>(
        path: P,
        assets: &crate::assets::AssetServer,
        mut mesh_name_filter: F,
    ) -> anyhow::Result<Self>
    where
        P: AsRef<std::path::Path>,
        F: FnMut(&str, &str) -> bool,
    {
        let original = path.as_ref().to_path_buf();
        let candidates = [
            original.clone(),
            std::path::PathBuf::from("..").join(&original),
            std::path::PathBuf::from("../..").join(&original),
            std::path::PathBuf::from("../../..").join(&original),
        ];
        let resolved = candidates
            .iter()
            .find(|p| p.exists())
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "gltf file not found in any candidate path (cwd={:?}): {:?}",
                    std::env::current_dir().ok(),
                    original
                )
            })?;
        log::info!("Loading glTF from {:?}", resolved);

        // Load the document and buffers but skip images — we don't sample
        // the model's textures yet, and a single missing/misnamed image
        // would otherwise cause the whole import to fail.
        let gltf = gltf::Gltf::open(&resolved)
            .map_err(|e| anyhow::anyhow!("gltf open failed for {:?}: {}", resolved, e))?;
        let base_dir = resolved
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        let buffers = gltf::import_buffers(&gltf.document, Some(base_dir), gltf.blob.clone())
            .map_err(|e| anyhow::anyhow!("gltf buffer load failed for {:?}: {}", resolved, e))?;
        let doc = gltf.document;

        let mut mesh = Self::empty();
        let scene = doc
            .default_scene()
            .or_else(|| doc.scenes().next())
            .ok_or_else(|| anyhow::anyhow!("gltf has no scenes: {:?}", resolved))?;

        for node in scene.nodes() {
            visit_node_inner(
                &node,
                glam::Mat4::IDENTITY,
                &buffers,
                base_dir,
                assets,
                &mut mesh_name_filter,
                &mut mesh,
            );
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
            mn.x,
            mn.y,
            mn.z,
            mx.x,
            mx.y,
            mx.z,
            mx.x - mn.x,
            mx.y - mn.y,
            mx.z - mn.z,
        );
        Ok(mesh)
    }
}

/// Resolve a glTF image source to a decoded [`ImageData`], handling
/// both external file URIs and images embedded in the binary chunk
/// of a `.glb`. External URIs are routed through [`AssetServer`]
/// (cached, deduped); embedded images are decoded fresh each time
/// — there's no path to key them on, and meshes themselves are
/// already cached one level up.
fn load_gltf_image(
    image: gltf::Image,
    buffers: &[gltf::buffer::Data],
    base_dir: &std::path::Path,
    assets: &crate::assets::AssetServer,
) -> Option<std::sync::Arc<crate::assets::ImageData>> {
    match image.source() {
        gltf::image::Source::Uri { uri, .. } => assets.load_image(base_dir, uri),
        gltf::image::Source::View { view, .. } => {
            let buf = &buffers[view.buffer().index()];
            let start = view.offset();
            let end = start + view.length();
            let bytes = buf.0.get(start..end)?;
            let img = image::load_from_memory(bytes)
                .map_err(|e| log::warn!("gltf embedded image decode failed: {}", e))
                .ok()?
                .to_rgba8();
            Some(std::sync::Arc::new(crate::assets::ImageData {
                width: img.width(),
                height: img.height(),
                pixels: img.into_raw(),
            }))
        }
    }
}

fn visit_node_inner(
    node: &gltf::Node,
    parent: glam::Mat4,
    buffers: &[gltf::buffer::Data],
    base_dir: &std::path::Path,
    assets: &crate::assets::AssetServer,
    mesh_name_filter: &mut dyn FnMut(&str, &str) -> bool,
    out: &mut Mesh,
) {
    let local = glam::Mat4::from_cols_array_2d(&node.transform().matrix());
    let world = parent * local;
    let normal_mat = glam::Mat3::from_mat4(world).inverse().transpose();

    if let Some(gmesh) = node.mesh() {
        let node_name = node.name().unwrap_or("");
        let mesh_name = gmesh.name().unwrap_or("");
        if mesh_name_filter(node_name, mesh_name) {
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
                let colors: Option<Vec<[f32; 3]>> =
                    reader.read_colors(0).map(|c| c.into_rgb_f32().collect());

                // Material base colour: factor + (optional) texture sampled
                // at each vertex's UV and baked into vertex colour. This is
                // a cheap stand-in for binding per-primitive material sets
                // and is what makes the nature-prop pack actually look
                // like trees / leaves / mushrooms instead of pure white.
                let material = prim.material();
                let pbr = material.pbr_metallic_roughness();
                let base_color = pbr.base_color_factor();
                let tint = glam::Vec3::new(base_color[0], base_color[1], base_color[2]);
                let base_tex = pbr.base_color_texture().and_then(|info| {
                    load_gltf_image(info.texture().source(), buffers, base_dir, assets)
                });
                log::info!(
                "  primitive material={:?} base_color_factor=[{:.2},{:.2},{:.2},{:.2}] base_tex={} emissive=[{:.2},{:.2},{:.2}] emissive_strength={:.2} emissive_tex={}",
                material.name(),
                base_color[0],
                base_color[1],
                base_color[2],
                base_color[3],
                if base_tex.is_some() { "yes" } else { "no" },
                material.emissive_factor()[0],
                material.emissive_factor()[1],
                material.emissive_factor()[2],
                material.emissive_strength().unwrap_or(1.0),
                if material.emissive_texture().is_some() {
                    "yes"
                } else {
                    "no"
                },
            );

                // Emissive contribution from the Principled BSDF's
                // Emission socket (Blender) → glTF `emissiveFactor`,
                // optionally amplified by `KHR_materials_emissive_strength`
                // when the Emission Strength in Blender is > 1. We add
                // this on top of the base colour so emissive primitives
                // (e.g. a wand's crystal tip) push past linear 1.0 and
                // get picked up by the bloom bright-pass downstream.
                // Areas without emission contribute zero, so a base-only
                // primitive is unchanged.
                let emissive_factor = material.emissive_factor();
                let emissive_strength = material.emissive_strength().unwrap_or(1.0);
                let emissive =
                    glam::Vec3::new(emissive_factor[0], emissive_factor[1], emissive_factor[2])
                        * emissive_strength;
                let emissive_tex = material.emissive_texture().and_then(|info| {
                    load_gltf_image(info.texture().source(), buffers, base_dir, assets)
                });

                let base_idx = out.vertices.len() as u32;
                for i in 0..positions.len() {
                    let p_local = glam::Vec3::from(positions[i]).extend(1.0);
                    let p_world = (world * p_local).truncate();
                    let n_local = glam::Vec3::from(normals[i]);
                    let n_world = (normal_mat * n_local).normalize_or_zero();
                    let mut color = match &colors {
                        Some(c) => glam::Vec3::from(c[i]) * tint,
                        None => tint,
                    };
                    if let Some(img) = &base_tex {
                        color *= img.sample(uvs[i]);
                    }
                    let mut em = emissive;
                    if let Some(img) = &emissive_tex {
                        em *= img.sample(uvs[i]);
                    }
                    color += em;
                    out.vertices.push(Vertex {
                        position: p_world,
                        normal: if n_world == glam::Vec3::ZERO {
                            glam::Vec3::Y
                        } else {
                            n_world
                        },
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
    }

    for child in node.children() {
        visit_node_inner(
            &child,
            world,
            buffers,
            base_dir,
            assets,
            mesh_name_filter,
            out,
        );
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

/// Extract the bytes of the first base-color texture image referenced by
/// any material in the glTF at `path`. Returns `None` if the file has
/// no embedded image or no material binds a base color texture. The
/// returned bytes are the original PNG/JPG payload (suitable for
/// `image::load_from_memory`).
pub fn extract_base_color_image_bytes<P: AsRef<std::path::Path>>(
    path: P,
) -> anyhow::Result<Option<Vec<u8>>> {
    let original = path.as_ref().to_path_buf();
    let candidates = [
        original.clone(),
        std::path::PathBuf::from("..").join(&original),
        std::path::PathBuf::from("../..").join(&original),
        std::path::PathBuf::from("../../..").join(&original),
    ];
    let resolved = match candidates.iter().find(|p| p.exists()).cloned() {
        Some(p) => p,
        None => return Ok(None),
    };
    let gltf = gltf::Gltf::open(&resolved)?;
    let base_dir = resolved
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let buffers = gltf::import_buffers(&gltf.document, Some(base_dir), gltf.blob.clone())?;
    let doc = gltf.document;

    // Find the first material that has a pbrMetallicRoughness baseColorTexture.
    for mat in doc.materials() {
        if let Some(info) = mat.pbr_metallic_roughness().base_color_texture() {
            let tex = info.texture();
            let img = tex.source();
            // Two possibilities: bufferView (embedded) or URI (external file).
            let source = img.source();
            match source {
                gltf::image::Source::View { view, .. } => {
                    let buf = &buffers[view.buffer().index()];
                    let start = view.offset();
                    let end = start + view.length();
                    if end <= buf.len() {
                        return Ok(Some(buf[start..end].to_vec()));
                    }
                }
                gltf::image::Source::Uri { uri, .. } => {
                    // Resolve relative to gltf's directory.
                    let full = base_dir.join(uri);
                    if let Ok(bytes) = std::fs::read(&full) {
                        return Ok(Some(bytes));
                    }
                }
            }
        }
    }
    Ok(None)
}

impl SkinnedMesh {
    /// Load a skinned mesh from a glTF / .glb file. Picks the *first* skin in
    /// the document. Vertices from every primitive of every skinned mesh that
    /// uses that skin are merged into one buffer in model space (i.e. skin
    /// space — the space referenced by inverseBindMatrices).
    pub fn from_gltf<P: AsRef<std::path::Path>>(path: P) -> anyhow::Result<Self> {
        Self::from_gltf_filtered(path, |_, _| true)
    }

    /// Same as [`from_gltf`], but only includes primitives whose owning
    /// glTF node + mesh names pass `mesh_name_filter(node_name, mesh_name)`.
    /// Lets callers split a multi-mesh authored asset (e.g. base-character
    /// `Eyes` / `Eyebrows` / body siblings under one skin) into separate
    /// `SkinnedMesh`es so each can be rendered as its own attachment with
    /// its own texture. Both names are passed because exporters disagree
    /// about which one carries the meaningful label: the female base-
    /// character gltf names its meshes `Eyes`/`Eyebrows`; the male variant
    /// names the *nodes* that, with the meshes themselves named
    /// `Face`/`Face.001`.
    pub fn from_gltf_filtered<P, F>(path: P, mut mesh_name_filter: F) -> anyhow::Result<Self>
    where
        P: AsRef<std::path::Path>,
        F: FnMut(&str, &str) -> bool,
    {
        let original = path.as_ref().to_path_buf();
        let candidates = [
            original.clone(),
            std::path::PathBuf::from("..").join(&original),
            std::path::PathBuf::from("../..").join(&original),
            std::path::PathBuf::from("../../..").join(&original),
        ];
        let resolved = candidates
            .iter()
            .find(|p| p.exists())
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "skinned gltf file not found in any candidate path (cwd={:?}): {:?}",
                    std::env::current_dir().ok(),
                    original
                )
            })?;
        log::info!("Loading skinned glTF from {:?}", resolved);

        let gltf = gltf::Gltf::open(&resolved)
            .map_err(|e| anyhow::anyhow!("gltf open failed for {:?}: {}", resolved, e))?;
        let base_dir = resolved
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        let buffers = gltf::import_buffers(&gltf.document, Some(base_dir), gltf.blob.clone())
            .map_err(|e| anyhow::anyhow!("gltf buffer load failed for {:?}: {}", resolved, e))?;
        let doc = gltf.document;

        let skin = doc
            .skins()
            .next()
            .ok_or_else(|| anyhow::anyhow!("gltf has no skin: {:?}", resolved))?;

        // ---- Build the skeleton (flat joint array) ----
        // Map glTF node index -> our joint index, in the order glTF's skin lists them
        // (this is also the order inverseBindMatrices uses).
        let joint_node_indices: Vec<u32> = skin.joints().map(|n| n.index() as u32).collect();
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
        let mut parent_of: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
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
                inverse_bind: inverse_binds
                    .get(i)
                    .copied()
                    .unwrap_or(glam::Mat4::IDENTITY),
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
            let Some(node_skin) = node.skin() else {
                continue;
            };
            if node_skin.index() != target_skin_idx {
                continue;
            }
            let Some(gmesh) = node.mesh() else { continue };
            // Pass both names to the filter — exporters disagree about
            // which one carries the meaningful label. The female base-
            // character gltf labels its meshes `Eyes`/`Eyebrows`; the
            // male variant labels the *nodes* that and the meshes
            // `Face`/`Face.001`.
            let node_name = node.name().unwrap_or("");
            let mesh_name = gmesh.name().unwrap_or("");
            if !mesh_name_filter(node_name, mesh_name) {
                continue;
            }

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
                let colors: Option<Vec<[f32; 3]>> =
                    reader.read_colors(0).map(|c| c.into_rgb_f32().collect());

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
                        w[0] *= inv;
                        w[1] *= inv;
                        w[2] *= inv;
                        w[3] *= inv;
                    } else {
                        w = [1.0, 0.0, 0.0, 0.0];
                    }
                    vertex_skin.push(VertexSkin {
                        joints: joints_attr[i],
                        weights: w,
                    });
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
            bind_vertices.len(),
            indices.len() / 3,
            joints.len(),
            mn.x,
            mn.y,
            mn.z,
            mx.x,
            mx.y,
            mx.z,
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
    pub fn joint_count(&self) -> usize {
        self.joints.len()
    }

    /// Override every bind-vertex's `color` with `rgb`. Used by the
    /// avatar-cosmetic pass to force the eye-ball mesh to render as
    /// pure white regardless of whatever vertex-color/material tint
    /// the base-character glTF baked in (the source asset bakes the
    /// MI_Eyes baseColor into COLOR_0, which can read off-white).
    pub fn override_vertex_colors(&mut self, rgb: glam::Vec3) {
        for v in &mut self.bind_vertices {
            v.color = rgb;
        }
    }

    /// Remap this mesh's `vertex_skin.joint_indices` to refer to the joint
    /// ordering of `target_names` (joint name → palette index in the
    /// target skeleton). Used when an attachment outfit (modular body /
    /// legs / etc.) was authored against the same logical skeleton as
    /// the player's base mesh and should be skinned with the player's
    /// bone palette directly. Returns false (and leaves the mesh
    /// untouched) if any of this mesh's joints is missing from the
    /// target skeleton, in which case the caller should not re-skin the
    /// attachment with that palette.
    pub fn remap_joint_indices_to(
        &mut self,
        target_names: &std::collections::HashMap<String, u16>,
    ) -> bool {
        let mut remap: Vec<u16> = Vec::with_capacity(self.joints.len());
        for j in &self.joints {
            match target_names.get(&j.name) {
                Some(&idx) => remap.push(idx),
                None => {
                    log::warn!(
                        "attachment joint {:?} not found in target skeleton, skipping remap",
                        j.name
                    );
                    return false;
                }
            }
        }
        for vs in &mut self.vertex_skin {
            for slot in 0..4 {
                let local = vs.joints[slot] as usize;
                if local < remap.len() {
                    vs.joints[slot] = remap[local];
                }
            }
        }
        // Rebuild name table to reflect the new index space.
        self.joint_index_by_name = self
            .joints
            .iter()
            .filter_map(|j| target_names.get(&j.name).map(|&i| (j.name.clone(), i)))
            .collect();
        true
    }

    /// Build a per-joint mask in `[0, 1]` selecting "upper-body" joints
    /// (spine, neck, head, clavicles, arms, hands, weapons). Used by the
    /// animation layer system so a spell-cast clip can override the upper
    /// body while the base locomotion clip continues to drive the legs.
    ///
    /// A joint receives weight 1 if any *ancestor* (including itself)
    /// matches an upper-body name pattern. This way fingers/weapons that
    /// don't directly contain "spine"/"arm" still inherit the mask via
    /// their parent chain.
    ///
    /// **Spine / chest get a reduced weight** (~0.35) rather than the
    /// full 1.0. The arms and shoulders need the full overlay to play
    /// a recognisable swing / cast pose, but the spine sits at the
    /// root of the upper-body chain — letting a Punch / Fireball clip
    /// fully override its rotation makes the whole torso tip down to
    /// match the cast pose's idle stance, which on top of a running
    /// gait reads as "punching into the floor". Softening the spine
    /// weight preserves the base locomotion's upright posture while
    /// still letting the cast clip add some upper-body lean.
    pub fn upper_body_mask(&self) -> Vec<f32> {
        self.upper_body_mask_with_axis().0
    }

    /// Return `(weight, yaw_only)` per joint:
    ///
    /// * `weight[i]` is the layered-blend mix weight in `[0, 1]`
    ///   (same as [`Self::upper_body_mask`]).
    /// * `yaw_only[i]` is `1.0` for joints whose rotation should
    ///   be **yaw-projected** before being mixed with the base
    ///   pose — i.e. spine / chest. Their pitch and roll from
    ///   the cast clip are dropped so a forward-pitched punch
    ///   pose doesn't tip the running torso into the floor;
    ///   only the lateral twist (yaw) needed to aim the arm
    ///   transfers onto the locomotion pose.
    ///
    /// All other joints (shoulders, arms, hands, head) get the
    /// full rotation overlay (`yaw_only = 0.0`).
    pub fn upper_body_mask_with_axis(&self) -> (Vec<f32>, Vec<f32>) {
        // Full-weight tokens: limbs, hands, weapons, head. These joints
        // are leaves of the rig and need 100 % override for the cast
        // pose to read.
        const FULL_TOKENS: &[&str] = &[
            "neck", "head", "clavicle", "shoulder", "upperarm", "forearm", "lowerarm", "hand",
            "finger", "thumb", "weapon", "prop", "tool",
        ];
        // Chest / upper-spine tokens: full weight. The arm hangs
        // off the chest, so for the punch arm to extend in the
        // direction the clip *authored* it (straight forward),
        // the chest has to be in the clip's frame too. Anything
        // less leaves the chest partly twisted by the run
        // cycle's counter-swing (run animations rotate the
        // chest opposite to the swinging arm), and the punch
        // ends up extending along that residual twist — up to
        // ~90° outward from the body's forward axis. The
        // layered-blend code mixes ROTATION ONLY (translations
        // / scales come from the base clip), so full chest
        // weight no longer drags the torso downward — the
        // "punching into the ground" failure mode is gone.
        const CHEST_TOKENS: &[&str] = &["chest", "upperchest"];
        const CHEST_WEIGHT: f32 = 1.0;
        // Lower-spine tokens: full weight too. Same reasoning —
        // residual run-cycle twist on the lower spine would
        // pull the chest off-axis. The post-blend cursor
        // twist (`build_bone_palette_layered` `twist` arg)
        // still rotates the spine toward the cursor on top of
        // this, so the punch's authored forward direction
        // ends up pointing at the cursor.
        const SPINE_TOKENS: &[&str] = &["spine"];
        const SPINE_WEIGHT: f32 = 1.0;

        // First pass: direct hits. Priority order: full tokens beat
        // chest tokens beat spine tokens — so a "chest" joint with
        // a "spine" parent ends up at CHEST_WEIGHT, and an
        // "upperchest" containing both "chest" and "spine" still
        // gets the chest weight via FULL_TOKENS / CHEST_TOKENS
        // priority. (`upperchest` doesn't actually appear in our
        // current rigs, but listing it keeps the rule robust.)
        //
        // Track which joints are "yaw-only" (spine chain) so the
        // blend layer can strip pitch / roll from the layer pose
        // before mixing — keeping the running torso upright while
        // still letting the punch's lateral twist aim the arm.
        let mut yaw_only: Vec<f32> = vec![0.0; self.joints.len()];
        let mut weight: Vec<f32> = self
            .joints
            .iter()
            .enumerate()
            .map(|(i, j)| {
                let n = j.name.to_ascii_lowercase();
                if FULL_TOKENS.iter().any(|tok| n.contains(tok)) {
                    1.0
                } else if CHEST_TOKENS.iter().any(|tok| n.contains(tok)) {
                    yaw_only[i] = 1.0;
                    CHEST_WEIGHT
                } else if SPINE_TOKENS.iter().any(|tok| n.contains(tok)) {
                    yaw_only[i] = 1.0;
                    SPINE_WEIGHT
                } else {
                    0.0
                }
            })
            .collect();
        // Second pass: propagate from any matched ancestor down to
        // descendants. Joints in skin order have parents earlier in
        // the array (per glTF spec). A descendant inherits the
        // *maximum* of its parent's and its own weight, so e.g. a
        // hand under a partial-weighted spine still gets the full
        // arm override (the hand itself matched a FULL_TOKEN), but
        // a stray bone child of the spine with no direct match
        // inherits the spine's weight.
        //
        // Yaw-only propagates the *same way* but only when the
        // child doesn't override with a stronger weight: a hand
        // under a yaw-only chest still wants full-rotation
        // blending (it matched FULL_TOKENS directly).
        for i in 0..self.joints.len() {
            if let Some(p) = self.joints[i].parent {
                let parent_w = weight[p as usize];
                if parent_w > weight[i] {
                    weight[i] = parent_w;
                    yaw_only[i] = yaw_only[p as usize];
                }
            }
        }
        (weight, yaw_only)
    }

    /// Find a joint whose name matches one of the left-hand naming
    /// conventions (`hand_l`, `left_hand`, `mixamorig:LeftHand`,
    /// etc.). Used as the spawn anchor for hand-held VFX like the
    /// Frost Ray beam — most casting animations in the UAL pack
    /// raise the left hand for the spell pose.
    pub fn left_hand_joint(&self) -> Option<usize> {
        let lc = |s: &str| s.to_ascii_lowercase();
        self.joints.iter().enumerate().find_map(|(i, j)| {
            let n = lc(&j.name);
            let is_hand = n.contains("hand")
                && !n.contains("forearm")
                && !n.contains("lower")
                && !n.contains("upper")
                && !n.contains("finger")
                && !n.contains("thumb");
            let is_left = n.contains("left")
                || n.ends_with("_l")
                || n.contains(".l")
                || n.contains("_l_")
                || n.contains("lhand");
            if is_hand && is_left {
                Some(i)
            } else {
                None
            }
        })
    }

    /// Find a joint whose name matches one of the right-hand naming
    /// conventions (`hand_r`, `right_hand`, `mixamorig:RightHand`,
    /// etc.). Returns the deepest such joint (i.e. the actual hand,
    /// not the wrist/forearm). Used as the spawn anchor for hand-held
    /// VFX like the Frost Ray beam.
    pub fn right_hand_joint(&self) -> Option<usize> {
        let lc = |s: &str| s.to_ascii_lowercase();
        // Score candidates so we pick the deepest hand joint (so that
        // a child like `righthand_end` doesn't beat `righthand`,
        // but `righthand` always beats `rightforearm`).
        self.joints.iter().enumerate().find_map(|(i, j)| {
            let n = lc(&j.name);
            let is_hand = n.contains("hand")
                && !n.contains("forearm")
                && !n.contains("lower")
                && !n.contains("upper")
                && !n.contains("finger")
                && !n.contains("thumb");
            let is_right = n.contains("right")
                || n.ends_with("_r")
                || n.contains(".r")
                || n.contains("_r_")
                || n.contains("rhand");
            if is_hand && is_right {
                Some(i)
            } else {
                None
            }
        })
    }

    /// Find left and right foot joints by name. Returns
    /// `(left_idx, right_idx)`, each `None` if the rig has no
    /// detectable foot joint on that side. Used for terrain-
    /// aware foot IK so each foot plants on the dungeon floor's
    /// per-tile elevation when walking on stair / dais / pit
    /// tiles instead of the baked clip's flat-ground assumption.
    pub fn foot_joints(&self) -> (Option<usize>, Option<usize>) {
        let lc = |s: &str| s.to_ascii_lowercase();
        let mut left = None;
        let mut right = None;
        for (i, j) in self.joints.iter().enumerate() {
            let n = lc(&j.name);
            let is_foot = n.contains("foot") && !n.contains("toe") && !n.contains("ball");
            if !is_foot {
                continue;
            }
            let is_left = n.contains("left")
                || n.ends_with("_l")
                || n.contains(".l")
                || n.contains("_l_")
                || n.contains("lfoot");
            let is_right = n.contains("right")
                || n.ends_with("_r")
                || n.contains(".r")
                || n.contains("_r_")
                || n.contains("rfoot");
            if is_left && left.is_none() {
                left = Some(i);
            }
            if is_right && right.is_none() {
                right = Some(i);
            }
        }
        (left, right)
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
                let parent_is_spine = self.joints[i]
                    .parent
                    .map(|p| is_spine(p as usize))
                    .unwrap_or(false);
                if !parent_is_spine {
                    return Some(i);
                }
            }
        }
        // Fallback: any matched spine joint.
        self.joints
            .iter()
            .position(|j| j.name.to_ascii_lowercase().contains("spine"))
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
            let parent = j
                .parent
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
        self.skin_to_inflated(bone_palette, 0.0, out);
    }

    /// Same as `skin_to` but pushes every output vertex `inflate` units
    /// along its (post-skinning) normal. Used to render outfit shells
    /// just outside the base body skin so the two don't z-fight.
    pub fn skin_to_inflated(
        &self,
        bone_palette: &[glam::Mat4],
        inflate: f32,
        out: &mut Vec<Vertex>,
    ) {
        out.clear();
        out.reserve(self.bind_vertices.len());
        for (v, s) in self.bind_vertices.iter().zip(self.vertex_skin.iter()) {
            // Skinning matrix = sum_i(weight_i * palette[joint_i])
            let mut m = glam::Mat4::ZERO;
            for k in 0..4 {
                let w = s.weights[k];
                if w == 0.0 {
                    continue;
                }
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
            let final_n = if n == glam::Vec3::ZERO { v.normal } else { n };
            out.push(Vertex {
                position: p + final_n * inflate,
                normal: final_n,
                color: v.color,
                uv: v.uv,
            });
        }
    }
}
