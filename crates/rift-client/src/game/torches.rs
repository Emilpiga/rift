//! Wall-mounted torch placement and lighting.
//!
//! Torches are pure VFX (no model assets exist). Each torch is a
//! looping `wall_torch` flame effect anchored to a sconce-height
//! point a short way out from a wall, paired with a warm
//! [`PointLight`] to push interactive illumination onto nearby
//! geometry. Because the renderer is hard-capped at 8 point
//! lights ([`crate::game::torches::TorchSystem::update_lights`]),
//! lights are re-selected each frame as the nearest 8 to the
//! local player.

use glam::Vec3;
use rift_engine::renderer::vfx::{presets, EffectId};
use rift_engine::{PointLight, Renderer};
use rift_engine::dungeon::Floor;

use super::props::placement::{collect_floor_tiles, tile_centre, SmallRng};

/// One placed torch: the warm point light it casts and the live
/// VFX effect id, kept around so we can despawn it on floor change.
#[derive(Clone, Copy)]
pub struct Torch {
    pub light: PointLight,
    pub vfx: EffectId,
}

/// Owns every torch on the active floor. Stored on `FloorManager`.
#[derive(Default)]
pub struct TorchSystem {
    pub torches: Vec<Torch>,
}

impl TorchSystem {
    pub fn new() -> Self {
        Self { torches: Vec::new() }
    }

    /// Walk the floor, place torches on a sparse subset of
    /// wall-adjacent tiles, and spawn a flame VFX + warm point
    /// light at each one.
    ///
    /// `seed` is folded with a fixed salt so torch placement is
    /// stable for a given floor seed but doesn't collide with
    /// other prop scatterers.
    pub fn place(&mut self, floor: &Floor, renderer: &mut Renderer, seed: u64) {
        // Clear any prior placement (caller should also have
        // despawned the VFX — see `clear`).
        self.torches.clear();

        let (border, _interior) = collect_floor_tiles(floor);
        if border.is_empty() {
            return;
        }

        // Min spacing between torches, in metres squared. The
        // forward shader caps active point lights at 8, so we
        // intentionally place torches sparsely enough that a
        // typical room has ~2–4 of them. Otherwise the
        // per-frame nearest-8 selection has to swap lights in
        // and out as the player walks, which reads as a halo
        // tracking the player rather than static fixtures
        // anchored to the walls. ~11 m feels right: each
        // torch's lit area (radius 11) just kisses its
        // neighbour's, leaving no obvious dark gaps but also
        // no overlap that would force a swap.
        const MIN_SPACING_SQ: f32 = 11.0 * 11.0;

        let mut rng = SmallRng::new(seed ^ 0xA1B2_C3D4_E5F6_0789_u64);
        // Shuffle indices via Fisher-Yates so spacing pruning
        // doesn't bias to one corner.
        let mut order: Vec<usize> = (0..border.len()).collect();
        for i in (1..order.len()).rev() {
            let j = rng.range(0, (i as u32) + 1) as usize;
            order.swap(i, j);
        }

        for idx in order {
            let (tx, tz, (ox, oz)) = border[idx];
            // Sconce position: at the wall face, at a typical
            // torch-bracket height of ~1.7 m. Push the flame
            // ~0.42 m off the floor-tile centre toward the
            // wall (wall tile is at +1 in the `(ox,oz)`
            // direction, so 0.42 puts the flame near the wall
            // surface but still inside the floor tile).
            let centre = tile_centre(tx, tz);
            let flame_pos = Vec3::new(
                centre.x + ox as f32 * 0.42,
                1.70,
                centre.z + oz as f32 * 0.42,
            );

            // Spacing check against already-placed torches.
            let too_close = self.torches.iter().any(|t| {
                t.light.position.distance_squared(flame_pos) < MIN_SPACING_SQ
            });
            if too_close {
                continue;
            }

            let vfx = renderer.vfx_system.spawn(presets::wall_torch(), flame_pos);

            // Warm amber light. Generous radius so a single
            // torch lights its host wall + a chunk of floor;
            // the per-frame nearest-8 selection in
            // `update_lights` then fades the outermost torch
            // smoothly so swaps don't pop as the player walks.
            let light = PointLight {
                position: flame_pos + Vec3::new(0.0, 0.05, 0.0),
                color: Vec3::new(1.60, 0.85, 0.40),
                radius: 11.0,
                intensity: 1.55,
            };
            self.torches.push(Torch { light, vfx });
        }
    }

    /// Per-frame: replace `renderer.point_lights` with the
    /// nearest torches to `player_pos`, soft-faded so swap-overs
    /// at the renderer's hard 8-light cap are imperceptible.
    ///
    /// Two fades run together:
    ///
    /// 1. **Distance fade** — every selected light is faded
    ///    smoothly to zero between `MAX_DIST - FADE_BAND` and
    ///    `MAX_DIST`. Means a torch that's about to leave the
    ///    visible set has already mostly dimmed.
    /// 2. **Rank fade** — the bottom-most ranks (7th and 8th
    ///    closest of those selected) are scaled by a smoothstep
    ///    from 1.0 at the top of the active set down to 0.0
    ///    just past the cap. When the 8th is displaced by a new
    ///    closer torch, *both* are near zero intensity at the
    ///    moment of the swap, so the change is invisible.
    ///
    /// Without the rank fade you'd see pops in dense rooms
    /// where 9+ torches sit inside the distance cutoff: the
    /// 8th and 9th would swap based purely on distance ordering
    /// and the swapping light would jump straight to its full
    /// distance-faded value.
    pub fn update_lights(&self, renderer: &mut Renderer, player_pos: Vec3) {
        renderer.point_lights.clear();
        if self.torches.is_empty() {
            return;
        }

        // Generous reach: torches keep contributing well past
        // their own `radius` in distance terms — the shader's
        // own attenuation handles per-fragment falloff, this
        // value just controls when we *stop uploading* the
        // light.
        const MAX_DIST: f32 = 26.0;
        const FADE_BAND: f32 = 10.0;
        let max_d2 = MAX_DIST * MAX_DIST;

        // Number of candidates to consider; we'll rank-fade
        // anything above `RANK_FULL` and drop everything past
        // `RANK_CAP` (the renderer's hard limit).
        const RANK_CAP: usize = 8;
        const RANK_FULL: usize = 6;

        let mut scored: Vec<(f32, PointLight)> = self
            .torches
            .iter()
            .map(|t| (t.light.position.distance_squared(player_pos), t.light))
            .filter(|(d2, _)| *d2 <= max_d2)
            .collect();
        scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        for (rank, (d2, mut light)) in scored.into_iter().take(RANK_CAP).enumerate() {
            // Distance fade.
            let d = d2.sqrt();
            let fade_start = MAX_DIST - FADE_BAND;
            let dt = ((MAX_DIST - d) / FADE_BAND).clamp(0.0, 1.0);
            let dist_s = if d <= fade_start { 1.0 } else { rift_math::smoothstep(dt) };

            // Rank fade — full intensity for the closest
            // RANK_FULL lights, smoothly fading the remaining
            // slots toward zero so an inbound torch displacing
            // an outbound one swaps near-silently.
            let rank_s = if rank < RANK_FULL {
                1.0
            } else {
                // Map [RANK_FULL .. RANK_CAP] -> [1.0 .. 0.0].
                let span = (RANK_CAP - RANK_FULL) as f32;
                let r = (rank - RANK_FULL) as f32 + 1.0;
                let t = (1.0 - (r / span)).clamp(0.0, 1.0);
                rift_math::smoothstep(t)
            };

            light.intensity *= dist_s * rank_s;
            // Skip uploading lights that round to invisible —
            // saves a slot for any genuinely-bright torch.
            if light.intensity > 0.005 {
                renderer.point_lights.push(light);
            }
        }
    }

    /// Despawn every flame VFX and forget all torches. Call
    /// from the floor-teardown path before regenerating.
    pub fn clear(&mut self, renderer: &mut Renderer) {
        for t in self.torches.drain(..) {
            renderer.vfx_system.despawn(t.vfx);
        }
        renderer.point_lights.clear();
    }
}
