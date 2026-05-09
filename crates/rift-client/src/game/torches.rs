//! Wall-mounted torch placement and lighting.
//!
//! Each torch is a candlestick-stand prop placed against a wall,
//! a looping `wall_torch` flame VFX anchored to the top of the
//! candle, and a warm [`PointLight`] to push interactive
//! illumination onto nearby geometry. Because the renderer is
//! hard-capped at 8 point lights
//! ([`crate::game::torches::TorchSystem::update_lights`]),
//! lights are re-selected each frame as the nearest 8 to the
//! local player.

use glam::Vec3;
use rift_engine::renderer::vfx::{presets, EffectId};
use rift_engine::{PointLight, Renderer};
use rift_engine::dungeon::Floor;

use super::props::placement::{collect_floor_tiles, tile_centre, SmallRng};
use super::props::{
    fantasy::CANDLESTICK_STAND, Props,
};

/// One placed torch: the warm point light it casts and the live
/// VFX effect id, kept around so we can despawn it on floor change.
#[derive(Clone, Copy)]
pub struct Torch {
    pub light: PointLight,
    pub vfx: EffectId,
    /// Random phase offset in seconds, used to decorrelate
    /// flicker between torches so a corridor lined with sconces
    /// doesn't pulse in unison.
    pub flicker_phase: f32,
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
    /// wall-adjacent tiles, and spawn a candlestick prop +
    /// flame VFX + warm point light at each one.
    ///
    /// `seed` is folded with a fixed salt so torch placement is
    /// stable for a given floor seed but doesn't collide with
    /// other prop scatterers.
    pub fn place(
        &mut self,
        floor: &Floor,
        renderer: &mut Renderer,
        props: &mut Props,
        world: &mut hecs::World,
        seed: u64,
    ) {
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
            let centre = tile_centre(tx, tz);

            // Probe the candlestick mesh height so the flame can
            // sit on top of the actual model rather than a magic
            // 1.7 m. `mesh_bounds` returns the gltf-local AABB;
            // we apply the asset's scale to get world height.
            // Falls back to a sensible default if the mesh fails
            // to load (e.g. on first frame before the asset
            // server has resolved it).
            let candle_top = props
                .assets()
                .mesh_bounds(CANDLESTICK_STAND.gltf)
                .map(|(mn, mx)| (mx.y - mn.y) * CANDLESTICK_STAND.scale)
                .unwrap_or(1.05);

            // Flame position: directly above the candle's top.
            // Push the prop slightly toward the wall (the prop
            // spawner's WallAligned hint snaps it the rest of
            // the way) and lift the flame ~5 cm above the
            // candle's wax for the wick.
            let prop_anchor = Vec3::new(
                centre.x + ox as f32 * 0.30,
                0.0,
                centre.z + oz as f32 * 0.30,
            );
            let flame_pos = Vec3::new(
                prop_anchor.x,
                candle_top + 0.05,
                prop_anchor.z,
            );

            // Spacing check against already-placed torches.
            let too_close = self.torches.iter().any(|t| {
                t.light.position.distance_squared(flame_pos) < MIN_SPACING_SQ
            });
            if too_close {
                continue;
            }

            // Spawn the candlestick model. Yaw faces the candle
            // away from the wall (toward the room) so the wick
            // and any sculpted detail reads at viewer angle.
            // `WallAligned` placement in the asset table snaps
            // the back face to the wall; we just supply the
            // wall direction.
            let yaw = (ox as f32).atan2(oz as f32);
            let _ = props.spawn(
                world,
                renderer,
                &CANDLESTICK_STAND,
                prop_anchor,
                yaw,
                (ox, oz),
                None,
            );

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
            self.torches.push(Torch {
                light,
                vfx,
                // Spread phases over a wide range so the
                // flicker sum-of-sines (with periods 0.7..3.1 s)
                // is fully decorrelated across torches.
                flicker_phase: rng.frange(0.0, 100.0),
            });
        }
    }

    /// Per-frame: replace `renderer.point_lights` with the
    /// nearest torches to `player_pos`, soft-faded so swap-overs
    /// at the renderer's hard 8-light cap are imperceptible.
    ///
    /// Three fades run together:
    ///
    /// 1. **Fog-aligned distance fade** — every selected light
    ///    is faded in over the fog band so that a torch which
    ///    is fully fog-veiled has zero contribution and the
    ///    light grows in as the fog clears, matching what the
    ///    player perceives. The fade starts at `fog_start` and
    ///    reaches zero at `fog_end`, matching the shader's own
    ///    fog falloff. This eliminates the "POOF — corridor
    ///    lights up" pop when the player walks past a hard
    ///    cutoff distance.
    /// 2. **Rank fade** — the bottom-most ranks (7th and 8th
    ///    closest of those selected) are scaled by a smoothstep
    ///    from 1.0 at the top of the active set down to 0.0
    ///    just past the cap. When the 8th is displaced by a new
    ///    closer torch, *both* are near zero intensity at the
    ///    moment of the swap, so the change is invisible.
    /// 3. **Flicker** — per-torch random phase + amplitude on
    ///    a fast/slow sine combo, applied last.
    ///
    /// `fog_start` / `fog_end` should match the renderer's
    /// active fog parameters; pass the same values the shader
    /// receives so the perceptual fade lines up.
    pub fn update_lights(
        &self,
        renderer: &mut Renderer,
        player_pos: Vec3,
        time: f32,
        fog_start: f32,
        fog_end: f32,
    ) {
        renderer.point_lights.clear();
        if self.torches.is_empty() {
            return;
        }

        // Reach matches the fog wall plus a small margin so the
        // last sliver of fade can complete before the geometry
        // gets culled. Anything past `cutoff` is invisible to
        // the player anyway, so skipping the upload is free.
        let cutoff = fog_end + 2.0;
        let max_d2 = cutoff * cutoff;

        // Number of candidates to consider; we'll rank-fade
        // anything above `RANK_FULL` and drop everything past
        // `RANK_CAP` (the renderer's hard limit).
        const RANK_CAP: usize = 8;
        const RANK_FULL: usize = 6;

        let mut scored: Vec<(f32, PointLight, f32)> = self
            .torches
            .iter()
            .map(|t| (t.light.position.distance_squared(player_pos), t.light, t.flicker_phase))
            .filter(|(d2, _, _)| *d2 <= max_d2)
            .collect();
        scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        for (rank, (d2, mut light, phase)) in scored.into_iter().take(RANK_CAP).enumerate() {
            // Fog-aligned distance fade. `dist_s` runs from 1.0
            // when the torch is at or inside `fog_start` to 0.0
            // at `fog_end`. The smoothstep curve matches the
            // forward shader's `fogFactor * fogFactor` quadratic
            // so the light's perceived intensity tracks the
            // fog's perceived opacity for the same source.
            let d = d2.sqrt();
            let fog_range = (fog_end - fog_start).max(0.001);
            let raw = ((fog_end - d) / fog_range).clamp(0.0, 1.0);
            let dist_s = rift_math::smoothstep(raw);

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

            // Flicker: a fast layer (high-freq jitter —
            // burst-style flame turbulence) summed with a
            // slow layer (1–2 Hz envelope — the lazy bob of a
            // settled flame). Each torch carries its own
            // phase offset so a row of sconces flickers
            // independently. Also pull the colour very
            // slightly toward red on dim moments so the
            // light reads as cooling embers between flares.
            let t = time + phase;
            let fast = (t * 11.3).sin() * 0.5
                + (t * 17.7 + 1.3).sin() * 0.3
                + (t * 23.1 + 2.7).sin() * 0.2;
            let slow = (t * 1.9).sin() * 0.5
                + (t * 3.1 + 0.7).sin() * 0.5;
            // Combined modulation in roughly [-1, 1]; scale
            // to ±15% intensity so the flicker is obviously
            // alive without strobing into the next slot's
            // visibility.
            let flicker = fast * 0.10 + slow * 0.05;
            light.intensity *= (1.0 + flicker).max(0.0);
            // Warm-cool dip: when the flame is dim (negative
            // flicker), nudge the colour slightly redder by
            // pulling green/blue down. When it flares
            // brighter, leave the warm amber alone.
            if flicker < 0.0 {
                let dim = (-flicker).min(0.15);
                light.color.y *= 1.0 - dim * 0.6;
                light.color.z *= 1.0 - dim * 0.9;
            }
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
