//! Procedural terrain heightfield generation.
//!
//! This module produces *just the math* — sampling functions
//! and grid generators — leaving mesh assembly to the renderer
//! crate. That split lets gameplay code (e.g. ground-clamp
//! queries, collision shape generation, AI line-of-sight) read
//! the same elevations the visual mesh was tessellated from
//! without depending on Vulkan.

use glam::{Vec2, Vec3};

use crate::noise::{fbm2, ridged_fbm2};

/// Parameters describing one mountain-ring terrain band.
///
/// The ring is a polar heightfield: an inner edge at
/// `inner_radius` (typically just past the visible play
/// arena), an outer edge at `outer_radius` (where distance fog
/// is fully opaque), and a per-vertex elevation sampled from
/// fBm + ridged-multifractal noise. Heights are biased toward
/// the centre of the radial band so peaks bunch up in the
/// middle and the silhouette tapers cleanly into both the
/// floor near the player and the fog wall behind.
#[derive(Clone, Copy, Debug)]
pub struct MountainRingParams {
    /// Inner edge of the terrain band, in world units. The
    /// ring's elevation tapers to `base_y` here so it joins
    /// flush with the floor at the play-area boundary.
    pub inner_radius: f32,
    /// Outer edge of the terrain band, in world units. The
    /// ring also tapers to `base_y` here so the back of the
    /// massif drops away into the abyss / fog.
    pub outer_radius: f32,
    /// World-Y of the ring's base perimeter (both inner and
    /// outer edges). Set this *below* the play-area floor so
    /// distance fog can swallow the lower flanks while peaks
    /// still cut crisp against the sky.
    pub base_y: f32,
    /// Maximum world-space peak height above `base_y`. Actual
    /// peak elevations vary from ~30% to 100% of this through
    /// the noise field.
    pub peak_height: f32,
    /// Number of angular subdivisions around the ring. Higher
    /// = smoother silhouette; cost is linear. 256 is a good
    /// default for a hub skybox; 512 if peaks read jagged.
    pub angular_segments: u32,
    /// Number of radial subdivisions across the band (between
    /// inner and outer edge). 24 gives nicely curved slopes;
    /// fewer flattens the massif into a wall, more is wasted
    /// at typical view distances.
    pub radial_segments: u32,
    /// Frequency multiplier applied to the noise lookup. Larger
    /// values produce more, narrower peaks; smaller produce
    /// fewer, broader massifs. `0.05`–`0.12` is a good range
    /// for ring radii in the 30–60 m bracket.
    pub noise_frequency: f32,
    /// Blend between fBm (`0.0`) and ridged-multifractal
    /// (`1.0`). Pure fBm gives rounded hills; pure ridged
    /// gives sharp alpine spines; ~`0.7` reads as "rocky
    /// mountains with eroded lower slopes".
    pub ridged_blend: f32,
    /// Deterministic seed. Same seed → identical terrain,
    /// across machines and Rust versions.
    pub seed: u64,
}

impl Default for MountainRingParams {
    fn default() -> Self {
        Self {
            inner_radius: 36.0,
            outer_radius: 70.0,
            base_y: -40.0,
            peak_height: 22.0,
            angular_segments: 256,
            radial_segments: 24,
            noise_frequency: 0.085,
            ridged_blend: 0.70,
            seed: 0xA855_E575_FACE_BEEF,
        }
    }
}

/// One vertex of a generated mountain-ring heightfield.
///
/// `position` is in world space (centred on the caller's pivot
/// — the generator takes a `center` argument). `radial_t` is
/// the normalised radial coordinate `0.0` (inner edge) → `1.0`
/// (outer edge), useful for caller-side shading or material
/// blending. `tile_uv` is a continuous UV running ~1 unit per
/// world metre along the ridge direction (`x`, around the
/// ring) and across slope (`y`, radial), suitable for direct
/// use with a tiling cliff-rocks texture.
#[derive(Clone, Copy, Debug)]
pub struct TerrainVertex {
    pub position: Vec3,
    pub normal: Vec3,
    pub radial_t: f32,
    pub tile_uv: Vec2,
}

/// Sample the height of a mountain-ring terrain at one
/// `(angle, radial_t)` polar coordinate. Exposed separately
/// from [`generate_mountain_ring`] so callers that just need
/// to ground-clamp a single point (a particle, a probe, a
/// collision query) can do so without rebuilding the grid.
///
/// `radial_t` is `0.0` at `inner_radius`, `1.0` at
/// `outer_radius`. Returns world-space Y.
pub fn sample_mountain_ring_height(params: &MountainRingParams, angle: f32, radial_t: f32) -> f32 {
    // Project the polar sample onto a 2D noise plane. Using
    // `(cos*r, sin*r)` (i.e. world XZ around the ring centre)
    // keeps the noise field continuous around the seam — a
    // ring sampled in `(angle, radial_t)` directly would
    // produce a visible discontinuity at angle=0=2π because
    // the noise lattice doesn't wrap.
    let r = params.inner_radius
        + (params.outer_radius - params.inner_radius) * radial_t.clamp(0.0, 1.0);
    let nx = angle.cos() * r * params.noise_frequency;
    let ny = angle.sin() * r * params.noise_frequency;

    let fbm = fbm2(nx, ny, params.seed, 5, 2.0, 0.5);
    let ridged = ridged_fbm2(nx, ny, params.seed ^ 0xD1B5_4A32_D192_ED03, 5, 2.0, 0.55);
    let blend = params.ridged_blend.clamp(0.0, 1.0);
    let h_norm = fbm * (1.0 - blend) + ridged * blend;

    // Radial taper: heights pinch to zero at both edges of the
    // band so the ring meets the floor and the fog wall
    // cleanly. `4t(1-t)` is the standard normalised parabola
    // peaking at `t = 0.5`. Squared so the rolloff is gentler
    // near the peak band and steeper at the edges.
    let t = radial_t.clamp(0.0, 1.0);
    let taper = (4.0 * t * (1.0 - t)).max(0.0);
    let taper = taper * taper;

    params.base_y + params.peak_height * h_norm * taper
}

/// Build a full polar heightfield grid for a mountain ring.
///
/// Returns a row-major buffer of size `(radial_segments + 1) *
/// angular_segments`: row `j` is the radial slice at radial
/// fraction `j / radial_segments`, column `i` is the angular
/// step `i / angular_segments` of a full circle.
///
/// Vertices in the last angular column at `i = angular_segments
/// - 1` are *not* duplicated for `i = 0`; the renderer is
/// expected to wrap indices around the seam with `i % cols`.
/// This avoids building two seam vertices that would have
/// slightly different normals from one-sided neighbour-finite-
/// difference normal estimation.
///
/// Normals are computed by central differences along the
/// angular and radial axes, in world space. The seam is
/// wrapped during normal estimation so the lighting is
/// continuous.
pub fn generate_mountain_ring(params: &MountainRingParams, center: Vec3) -> MountainRingGrid {
    let cols = params.angular_segments.max(16);
    let rows = params.radial_segments.max(2) + 1;

    // First pass: positions only. We need the full grid before
    // we can do central differences for normals.
    let mut positions: Vec<Vec3> = Vec::with_capacity((rows * cols) as usize);
    for j in 0..rows {
        let radial_t = j as f32 / (rows - 1) as f32;
        for i in 0..cols {
            let a = (i as f32 / cols as f32) * std::f32::consts::TAU;
            let r = params.inner_radius + (params.outer_radius - params.inner_radius) * radial_t;
            let y = sample_mountain_ring_height(params, a, radial_t);
            positions.push(Vec3::new(center.x + a.cos() * r, y, center.z + a.sin() * r));
        }
    }

    // Second pass: normals via central differences. Radial
    // edges (j = 0, j = rows-1) use one-sided differences
    // because neighbours don't exist past the band. Angular
    // direction wraps so the seam is seamless.
    let mut vertices: Vec<TerrainVertex> = Vec::with_capacity(positions.len());
    let idx = |j: u32, i: u32| -> usize { (j * cols + (i % cols)) as usize };
    for j in 0..rows {
        let radial_t = j as f32 / (rows - 1) as f32;
        for i in 0..cols {
            let p = positions[idx(j, i)];

            // Angular tangent (wraps).
            let ip = (i + 1) % cols;
            let im = (i + cols - 1) % cols;
            let tangent_a = positions[idx(j, ip)] - positions[idx(j, im)];

            // Radial tangent (one-sided at edges).
            let tangent_r = if rows == 1 {
                Vec3::Z
            } else if j == 0 {
                positions[idx(1, i)] - positions[idx(0, i)]
            } else if j == rows - 1 {
                positions[idx(rows - 1, i)] - positions[idx(rows - 2, i)]
            } else {
                positions[idx(j + 1, i)] - positions[idx(j - 1, i)]
            };

            let n = tangent_r.cross(tangent_a).normalize_or_zero();
            // Force outward / upward orientation. The mountain
            // is viewed from inside the ring, so any normal
            // with a strongly negative `y` is back-facing and
            // we flip it.
            let n = if n.y < 0.0 { -n } else { n };

            // Tile-UV: arclength along the ring (≈ 1 m per unit
            // at the mid-radius) horizontally, world-Y
            // vertically. Caller can scale uniformly via the
            // material `uv_scale` push-constant.
            let arclen = (i as f32 / cols as f32)
                * std::f32::consts::TAU
                * 0.5
                * (params.inner_radius + params.outer_radius);
            let tile_uv = Vec2::new(arclen, p.y);

            vertices.push(TerrainVertex {
                position: p,
                normal: n,
                radial_t,
                tile_uv,
            });
        }
    }

    MountainRingGrid {
        cols,
        rows,
        vertices,
    }
}

/// Output of [`generate_mountain_ring`]: a row-major grid of
/// terrain vertices plus its dimensions, suitable for direct
/// triangulation. The angular axis wraps (column `cols` ==
/// column `0`); the radial axis does not.
pub struct MountainRingGrid {
    /// Number of angular subdivisions (= columns in the grid).
    pub cols: u32,
    /// Number of radial subdivisions + 1 (= rows in the grid).
    pub rows: u32,
    /// Row-major vertices: `row j, col i` lives at index
    /// `j * cols + i`.
    pub vertices: Vec<TerrainVertex>,
}
