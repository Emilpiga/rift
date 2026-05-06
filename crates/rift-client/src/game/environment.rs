//! Procedurally-generated tiling textures for the dungeon floor and walls.
//!
//! The asset pack we ship doesn't include ground/wall textures, so we
//! synthesize them at runtime: the floor is a moss-streaked cobblestone,
//! and the walls are a weathered ashlar (large rectangular masonry).
//! Both textures tile seamlessly at 1 unit = 1 wall stride, matching the
//! UV layout of `Mesh::dungeon_floor` and `Mesh::wall_colored`.

use std::sync::{Arc, Mutex};

use rift_engine::ash::vk;
use rift_engine::ash::Device;
use rift_engine::gpu_allocator::vulkan::Allocator;
use rift_engine::renderer::texture::Texture;
use rift_engine::Renderer;

const FLOOR_SIZE: u32 = 256;
const WALL_SIZE: u32 = 256;

pub struct EnvTextures {
    pub floor_set: Option<vk::DescriptorSet>,
    pub wall_set: Option<vk::DescriptorSet>,
    /// Grass tile bound on the hub floor for the outdoor / nature
    /// theme. Optional — only uploaded when first requested via
    /// [`Self::ensure_grass`].
    pub grass_floor_set: Option<vk::DescriptorSet>,
    textures: Vec<Texture>,
}

impl Default for EnvTextures {
    fn default() -> Self {
        Self {
            floor_set: None,
            wall_set: None,
            grass_floor_set: None,
            textures: Vec::new(),
        }
    }
}

impl EnvTextures {
    /// Upload (or re-upload) procedural floor and wall textures sized to
    /// the current floor's theme.  Old textures are dropped *only* when
    /// `cleanup_gpu` is called — repeated calls allocate fresh descriptor
    /// sets and grow `self.textures`.  In practice we call `ensure` once
    /// at startup and reuse the same sets for every floor.
    pub fn ensure(&mut self, renderer: &mut Renderer) {
        if self.floor_set.is_none() {
            let pixels = generate_floor(FLOOR_SIZE);
            match renderer.upload_shared_texture_from_rgba(FLOOR_SIZE, FLOOR_SIZE, &pixels) {
                Ok((tex, set)) => {
                    self.textures.push(tex);
                    self.floor_set = Some(set);
                }
                Err(e) => log::warn!("env floor texture upload failed: {}", e),
            }
        }
        if self.wall_set.is_none() {
            let pixels = generate_wall(WALL_SIZE);
            match renderer.upload_shared_texture_from_rgba(WALL_SIZE, WALL_SIZE, &pixels) {
                Ok((tex, set)) => {
                    self.textures.push(tex);
                    self.wall_set = Some(set);
                }
                Err(e) => log::warn!("env wall texture upload failed: {}", e),
            }
        }
    }

    pub fn cleanup_gpu(&mut self, device: &Device, allocator: &Arc<Mutex<Allocator>>) {
        for mut tex in self.textures.drain(..) {
            tex.cleanup(device, allocator);
        }
        self.floor_set = None;
        self.wall_set = None;
        self.grass_floor_set = None;
    }

    /// Lazy-initialise the grass tile used by the outdoor hub.
    /// Idempotent: subsequent calls are a no-op once the descriptor
    /// set has been allocated.
    pub fn ensure_grass(&mut self, renderer: &mut Renderer) {
        if self.grass_floor_set.is_some() {
            return;
        }
        let pixels = generate_grass(FLOOR_SIZE);
        match renderer.upload_shared_texture_from_rgba(FLOOR_SIZE, FLOOR_SIZE, &pixels) {
            Ok((tex, set)) => {
                self.textures.push(tex);
                self.grass_floor_set = Some(set);
            }
            Err(e) => log::warn!("env grass texture upload failed: {}", e),
        }
    }
}

// ---------------------------------------------------------------------
// Generators
// ---------------------------------------------------------------------

fn hash2(x: i32, y: i32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x27D4_EB2D)
        ^ (y as u32).wrapping_mul(0x9E37_79B1);
    h ^= h >> 15;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    h
}

fn rand01(seed: u32) -> f32 {
    (seed & 0x00FF_FFFF) as f32 / 0x0100_0000 as f32
}

/// Smooth value-noise sample at (u, v) in [0,1]^2 using `cells` cells
/// per axis. Wraps so the result tiles seamlessly.
fn vnoise(u: f32, v: f32, cells: u32, seed: u32) -> f32 {
    let su = u * cells as f32;
    let sv = v * cells as f32;
    let x0 = su.floor() as i32;
    let y0 = sv.floor() as i32;
    let fx = su - x0 as f32;
    let fy = sv - y0 as f32;
    let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
    let sx = smooth(fx);
    let sy = smooth(fy);
    let h = |ix: i32, iy: i32| -> f32 {
        let wx = ix.rem_euclid(cells as i32);
        let wy = iy.rem_euclid(cells as i32);
        rand01(hash2(wx, wy).wrapping_add(seed))
    };
    let a = h(x0, y0);
    let b = h(x0 + 1, y0);
    let c = h(x0, y0 + 1);
    let d = h(x0 + 1, y0 + 1);
    let ab = a + (b - a) * sx;
    let cd = c + (d - c) * sx;
    ab + (cd - ab) * sy
}

fn fbm(u: f32, v: f32, base_cells: u32, octaves: u32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut total = 0.0;
    let mut cells = base_cells;
    for o in 0..octaves {
        sum += vnoise(u, v, cells, seed.wrapping_add(o * 131)) * amp;
        total += amp;
        amp *= 0.5;
        cells = (cells * 2).max(1);
    }
    sum / total
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn pack(c: [f32; 3]) -> [u8; 4] {
    [
        (c[0].clamp(0.0, 1.0) * 255.0) as u8,
        (c[1].clamp(0.0, 1.0) * 255.0) as u8,
        (c[2].clamp(0.0, 1.0) * 255.0) as u8,
        255,
    ]
}

/// Cobblestone floor: irregular polygonal cells, dark grout between them,
/// faint moss in the cracks. Tiles seamlessly.
fn generate_floor(size: u32) -> Vec<u8> {
    let mut out = vec![0u8; (size * size * 4) as usize];

    // Worley-style cells: pick N jittered points, color each pixel by
    // distance to the nearest point and to the second-nearest (for grout).
    let cell_count: i32 = 6; // cells per axis
    let inv = 1.0 / size as f32;

    // Pre-compute jittered cell points (in [0,1] tiling space).
    let cells = cell_count;
    let seed_pts = 0xA1B2_C3D4u32;

    for y in 0..size {
        for x in 0..size {
            let u = x as f32 * inv;
            let v = y as f32 * inv;
            let cu = (u * cells as f32).floor() as i32;
            let cv = (v * cells as f32).floor() as i32;
            let mut d1 = f32::INFINITY;
            let mut d2 = f32::INFINITY;
            let mut nearest: (i32, i32) = (0, 0);
            for oy in -1..=1 {
                for ox in -1..=1 {
                    let cx = cu + ox;
                    let cy = cv + oy;
                    // Hash uses wrapped cell (so the same point appears on
                    // both sides of the seam), but the world-space position
                    // uses the un-wrapped cell so distance is correct.
                    let hx = cx.rem_euclid(cells);
                    let hy = cy.rem_euclid(cells);
                    let h = hash2(hx, hy).wrapping_add(seed_pts);
                    let jx = rand01(h);
                    let jy = rand01(h.wrapping_mul(0x9E37));
                    let px = (cx as f32 + 0.15 + 0.7 * jx) / cells as f32;
                    let py = (cy as f32 + 0.15 + 0.7 * jy) / cells as f32;
                    let ddx = px - u;
                    let ddy = py - v;
                    let d = ddx * ddx + ddy * ddy;
                    if d < d1 {
                        d2 = d1;
                        d1 = d;
                        nearest = (hx, hy);
                    } else if d < d2 {
                        d2 = d;
                    }
                }
            }
            let edge = (d2.sqrt() - d1.sqrt()).max(0.0);

            // Per-cell stone tint variation.
            let cell_h = hash2(nearest.0, nearest.1);
            let stone_tone = rand01(cell_h);
            let warm = lerp3([0.34, 0.30, 0.27], [0.46, 0.42, 0.38], stone_tone);

            // Inner stone fbm noise for surface detail.
            let detail = fbm(u, v, 8, 4, 0x55AA_3322);
            let stone = lerp3(
                [warm[0] * 0.85, warm[1] * 0.85, warm[2] * 0.85],
                [warm[0] * 1.10, warm[1] * 1.08, warm[2] * 1.06],
                detail,
            );

            // Grout: dark when edge distance is small.
            let grout_t = (1.0 - (edge * 18.0).min(1.0)).powf(2.0);
            let grout_color = [0.10, 0.09, 0.08];
            let mut color = lerp3(stone, grout_color, grout_t);

            // Subtle moss in the grout cracks.
            let moss_n = fbm(u * 1.3, v * 1.3, 5, 3, 0x77BB_99CC);
            let moss_t = grout_t * (moss_n - 0.45).max(0.0) * 1.6;
            color = lerp3(color, [0.10, 0.18, 0.08], moss_t.clamp(0.0, 0.7));

            // Dust speckles.
            let speck = fbm(u, v, 64, 2, 0x4242_4242);
            if speck > 0.86 {
                color = lerp3(color, [0.55, 0.50, 0.42], 0.4);
            }

            let i = ((y * size + x) * 4) as usize;
            let p = pack(color);
            out[i] = p[0];
            out[i + 1] = p[1];
            out[i + 2] = p[2];
            out[i + 3] = p[3];
        }
    }
    out
}

/// Ashlar (rectangular block) wall: courses of large stones with mortar
/// between them, alternating offsets per row, plus surface noise.  Tiles
/// vertically every `BRICK_ROWS` courses and horizontally every block
/// width, so the wall mesh's UV mapping (u in [0,1], v in [0, h]) repeats
/// cleanly.
fn generate_wall(size: u32) -> Vec<u8> {
    let mut out = vec![0u8; (size * size * 4) as usize];
    const ROWS: u32 = 4;          // courses per UV repeat
    const COLS_EVEN: u32 = 2;     // blocks across at v=0
    const COLS_ODD: u32 = 2;      // staggered rows have same density, half-offset

    let inv = 1.0 / size as f32;
    let mortar = [0.07, 0.06, 0.055];

    for y in 0..size {
        for x in 0..size {
            let u = x as f32 * inv;
            let v = y as f32 * inv;
            let row_f = v * ROWS as f32;
            let row = row_f.floor() as i32;
            let row_frac = row_f - row as f32;
            let cols = if row % 2 == 0 { COLS_EVEN } else { COLS_ODD };
            let offset = if row % 2 == 0 { 0.0 } else { 0.5 };
            let col_f = u * cols as f32 + offset;
            let col = col_f.floor() as i32;
            let col_frac = col_f - col as f32;

            // Distance to nearest mortar line in normalized brick space.
            let dx = (col_frac - 0.5).abs() * 2.0;     // 0 center, 1 edge
            let dy = (row_frac - 0.5).abs() * 2.0;
            let edge = 1.0 - dx.max(dy);              // 0 = on edge, ~1 = center
            let mortar_t = (1.0 - (edge * 14.0).min(1.0)).powf(2.0);

            // Per-block stone tint.
            let cell_h = hash2(col.rem_euclid(cols as i32), row.rem_euclid(ROWS as i32));
            let stone_tone = rand01(cell_h);
            let warm = lerp3([0.30, 0.27, 0.24], [0.45, 0.40, 0.36], stone_tone);

            // Surface detail inside the block.
            let detail = fbm(u * 4.0, v * 4.0, 6, 4, 0x33AA_55BB);
            let stone = lerp3(
                [warm[0] * 0.82, warm[1] * 0.82, warm[2] * 0.82],
                [warm[0] * 1.10, warm[1] * 1.08, warm[2] * 1.06],
                detail,
            );

            // Subtle horizontal streaking inside each block (water staining).
            let streak = fbm(u * 8.0, v * 1.5, 3, 3, 0x9911_2233);
            let stone = lerp3(stone, [stone[0] * 0.85, stone[1] * 0.84, stone[2] * 0.82], (streak - 0.5).max(0.0) * 0.6);

            let mut color = lerp3(stone, mortar, mortar_t);

            // Cracks: thin dark streaks scattered over the surface.
            let crack = fbm(u * 2.0, v * 2.0, 5, 4, 0xBEEF_F00D);
            if (crack - 0.55).abs() < 0.025 {
                color = lerp3(color, [0.05, 0.04, 0.04], 0.65);
            }

            let i = ((y * size + x) * 4) as usize;
            let p = pack(color);
            out[i] = p[0];
            out[i + 1] = p[1];
            out[i + 2] = p[2];
            out[i + 3] = p[3];
        }
    }
    out
}

/// Lush meadow grass: warm green base modulated by clumpy fbm with
/// occasional dirt patches and tiny yellow flower specks. Tiles
/// seamlessly at the same scale as `generate_floor`.
fn generate_grass(size: u32) -> Vec<u8> {
    let mut out = vec![0u8; (size * size * 4) as usize];
    let inv = 1.0 / size as f32;

    for y in 0..size {
        for x in 0..size {
            let u = x as f32 * inv;
            let v = y as f32 * inv;

            let clump = fbm(u, v, 4, 4, 0x6A11_CE03);
            let blade = fbm(u, v, 32, 3, 0x12AB_CD34);

            let cool = [0.20, 0.46, 0.18];
            let warm = [0.42, 0.62, 0.22];
            let mut color = lerp3(cool, warm, clump);

            color = lerp3(
                [color[0] * 0.78, color[1] * 0.82, color[2] * 0.74],
                [color[0] * 1.18, color[1] * 1.14, color[2] * 1.10],
                blade,
            );

            let dirt_t = (0.18 - clump).max(0.0) * 4.0;
            color = lerp3(color, [0.34, 0.26, 0.18], dirt_t.clamp(0.0, 0.55));

            let speck = fbm(u, v, 96, 2, 0xF10A_77E5);
            if speck > 0.88 && clump > 0.45 {
                color = lerp3(color, [0.95, 0.86, 0.30], 0.55);
            }
            let speck2 = fbm(u, v, 96, 2, 0x21B5_DDAA);
            if speck2 > 0.90 && clump > 0.40 {
                color = lerp3(color, [0.92, 0.92, 0.86], 0.55);
            }

            let i = ((y * size + x) * 4) as usize;
            let p = pack(color);
            out[i] = p[0];
            out[i + 1] = p[1];
            out[i + 2] = p[2];
            out[i + 3] = p[3];
        }
    }
    out
}
