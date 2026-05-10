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
use rift_engine::dungeon::{Floor, RoomTheme};
use rift_engine::renderer::vfx::{presets, EffectId};
use rift_engine::{PointLight, Renderer};

use super::props::placement::{collect_floor_tiles, tile_centre, SmallRng};
use super::props::{fantasy::CANDLESTICK_STAND, Props};

/// Per-theme torch colour + intensity. Returned as
/// `(rgb, intensity_mul)` so the base radius stays per-floor
/// uniform (lighting density and game pacing depend on it)
/// while the visual character of each room reads cleanly.
///
/// Colours are HDR and not normalised — channels above 1.0
/// drive the bloom pass for a more saturated read on the
/// brighter themes.
fn theme_torch_lighting(theme: RoomTheme) -> (Vec3, f32) {
    match theme {
        // Cold blue-grey ghostlight. Subtle, dim — the crypt
        // shouldn't feel hospitable.
        RoomTheme::Crypt => (Vec3::new(0.55, 0.80, 1.20), 0.85),
        // Default warm amber. Soldiers' quarters read as
        // "lived in by humans".
        RoomTheme::Barracks => (Vec3::new(1.60, 0.85, 0.40), 1.00),
        // Slightly cooler reading-lamp yellow. Dimmer so
        // shadows from book stacks read against the warm
        // amber elsewhere.
        RoomTheme::Library => (Vec3::new(1.50, 1.05, 0.60), 0.95),
        // Bright golden — the climactic chamber. Boss rooms
        // are always Shrine, so this is what the player walks
        // *toward* down a corridor.
        RoomTheme::Shrine => (Vec3::new(1.80, 1.15, 0.60), 1.35),
        // Standard amber, slightly dim. Storage cellars are
        // utility spaces, not destinations.
        RoomTheme::Storage => (Vec3::new(1.45, 0.80, 0.40), 0.85),
        // Sickly green-yellow. Prison cells should feel off.
        RoomTheme::Prison => (Vec3::new(1.00, 0.95, 0.50), 0.70),
        // Default warm — used for portal room + corridors.
        RoomTheme::Generic => (Vec3::new(1.60, 0.85, 0.40), 1.00),
    }
}

/// Find which room (if any) contains the given world XZ
/// position. Returns `None` for tiles in corridors.
fn room_at(floor: &Floor, x: f32, z: f32) -> Option<&rift_engine::dungeon::Room> {
    let gx = (x + 0.5).floor() as isize;
    let gz = (z + 0.5).floor() as isize;
    if gx < 0 || gz < 0 {
        return None;
    }
    let (gx, gz) = (gx as usize, gz as usize);
    floor
        .rooms
        .iter()
        .find(|r| gx >= r.x && gx < r.x + r.width && gz >= r.z && gz < r.z + r.depth)
}

/// One placed torch: the warm point light it casts and the live
/// VFX effect id, kept around so we can despawn it on floor change.
#[derive(Clone, Copy)]
pub struct Torch {
    pub light: PointLight,
    pub vfx: EffectId,
    /// Per-torch audio emitter for the looping flame crackle.
    /// `None` when the audio system is unavailable or the
    /// asset failed to load (the torch otherwise behaves
    /// exactly the same — silent). Despawned in [`clear`]
    /// alongside the VFX. Volume is driven each frame by
    /// [`update_lights`] from the same flicker + distance +
    /// rank fade as the light itself, so the torch sounds
    /// audibly louder when it visually brightens and fades
    /// out cleanly when it slips past the rank cap.
    pub audio: Option<rift_audio::EmitterId>,
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
        Self {
            torches: Vec::new(),
        }
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

            // Probe the candlestick mesh AABB so we can both
            // size the flame to the model's height and
            // replicate the WallAligned snap calculation
            // exactly (so the flame sits on top of where
            // `props.spawn` actually places the candle).
            let bounds = props.assets().mesh_bounds(CANDLESTICK_STAND.gltf);
            let s = CANDLESTICK_STAND.scale;
            let candle_top = bounds.map(|(mn, mx)| (mx.y - mn.y) * s).unwrap_or(1.05);
            // Half-extent of the candle along the wall normal,
            // matching the prop spawner's WallAligned logic.
            // Falls back to a conservative 0.10 m if bounds
            // aren't available yet.
            let half_along = bounds
                .map(|(mn, mx)| {
                    if ox != 0 {
                        (mx.x - mn.x) * 0.5 * s
                    } else {
                        (mx.z - mn.z) * 0.5 * s
                    }
                })
                .unwrap_or(0.10);
            // Distance to push from tile centre toward the
            // wall. The spawner uses `0.5 - half - 0.04` (4 cm
            // air gap from the wall face); we add a few extra
            // cm here so authored candle bases that have a
            // wider footprint than their bounding box —
            // sculpted scrollwork, splayed feet — clear the
            // wall surface cleanly. Previously this was
            // 0.30 m *plus* the spawner's own snap, which
            // double-pushed the candle into the wall.
            let push = (0.5 - half_along - 0.08).max(0.0);
            // Lift to the tile's authored elevation so a
            // torch sconce on a raised dais or sunken-pit
            // wall sits on the room's floor surface, not at
            // y=0. Without this the candle hovers above
            // (or sinks below) every non-base-elevation
            // floor tile and its flame VFX renders detached
            // from the model.
            let tile_y = floor.tile_floor_y_at(centre.x, centre.z);
            let prop_anchor = Vec3::new(
                centre.x + ox as f32 * push,
                tile_y,
                centre.z + oz as f32 * push,
            );
            let flame_pos = Vec3::new(prop_anchor.x, tile_y + candle_top + 0.05, prop_anchor.z);

            // Spacing check against already-placed torches.
            let too_close = self
                .torches
                .iter()
                .any(|t| t.light.position.distance_squared(flame_pos) < MIN_SPACING_SQ);
            if too_close {
                continue;
            }

            // Spawn the candlestick model. Yaw faces the candle
            // away from the wall (toward the room) so the wick
            // and any sculpted detail reads at viewer angle.
            // We pass `wall_dir = (0, 0)` to *suppress* the
            // spawner's WallAligned snap because we already
            // computed the correct push above (using the same
            // formula). Otherwise the snap stacks on top of
            // our offset and the prop ends up clipped into
            // the wall.
            let yaw = (ox as f32).atan2(oz as f32);
            let _ = props.spawn(
                world,
                renderer,
                &CANDLESTICK_STAND,
                prop_anchor,
                yaw,
                (0, 0),
                None,
            );

            let vfx = renderer.vfx_system.spawn(presets::wall_torch(), flame_pos);

            // Themed light. Look up the torch tile's parent
            // room (if any) and tint the point light by its
            // theme: cool blue in crypts, warm gold in
            // shrines, dim sickly green in prisons, etc. The
            // VFX flame stays the default warm sprite — only
            // the cast-light colour shifts, which keeps the
            // visible flame consistent across the floor while
            // the room itself reads with its own palette.
            let theme = room_at(floor, flame_pos.x, flame_pos.z)
                .map(|r| r.theme)
                .unwrap_or(RoomTheme::Generic);
            let (color, intensity_mul) = theme_torch_lighting(theme);
            let light = PointLight {
                position: flame_pos + Vec3::new(0.0, 0.05, 0.0),
                color,
                radius: 11.0,
                intensity: 1.55 * intensity_mul,
            };
            self.torches.push(Torch {
                light,
                vfx,
                // Audio emitter spawned below in `place_audio`
                // (caller threads the optional `AudioSystem`
                // separately so this method's signature
                // stays narrow).
                audio: None,
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
        // `RANK_CAP`. Stays under the renderer's
        // `MAX_POINT_LIGHTS = 16` to leave headroom for the
        // portal system's per-frame light pushes (descend +
        // extract portal can each grab a slot in the boss
        // room corridor) and for the lightning-storm flash
        // light in the hub. VFX-driven projectile lights live
        // in a separate `vfx_lights` pool that's packed first
        // in the renderer merge, so torches don't need to
        // leave room for those.
        const RANK_CAP: usize = 14;
        const RANK_FULL: usize = 12;

        let mut scored: Vec<(f32, PointLight, f32)> = self
            .torches
            .iter()
            .map(|t| {
                (
                    t.light.position.distance_squared(player_pos),
                    t.light,
                    t.flicker_phase,
                )
            })
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
            let slow = (t * 1.9).sin() * 0.5 + (t * 3.1 + 0.7).sin() * 0.5;
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
            // Audio emitters belong to a separate system; the
            // caller (FloorManager) owns the bridge call.
            // We just drop the id by clearing the vector.
        }
        renderer.point_lights.clear();
    }

    /// Despawn every torch's audio emitter via `audio`. Split
    /// from [`clear`] so callers that don't have audio access
    /// (tools, tests) can still tear down lights + VFX.
    pub fn clear_audio(&mut self, audio: &mut rift_audio::AudioSystem) {
        for t in self.torches.iter_mut() {
            if let Some(id) = t.audio.take() {
                audio.despawn_emitter(id);
            }
        }
    }

    /// Spawn one looping flame-crackle emitter per torch. Call
    /// once after [`Self::place`]. Idempotent: torches that
    /// already have an emitter are skipped, so re-running
    /// after a partial failure is safe.
    ///
    /// Audio is intentionally not spawned inside `place` so
    /// the placement code stays decoupled from the audio
    /// crate — callers that don't have audio (tests, tools)
    /// can still build a torch system with lights + VFX.
    pub fn place_audio(&mut self, audio: &mut rift_audio::AudioSystem) {
        // Authored once per call — every torch shares the same
        // source spec, so the underlying `StaticSoundData` is
        // loaded once and the cache hands out cheap clones.
        // Falloff is short (3 m full → 14 m silent): the player
        // should hear the nearest sconce clearly and have the
        // farther ones fade into ambience. Going wider would
        // turn a corridor of torches into a wash.
        let spec = rift_audio::SoundSpec {
            path: "ambient/torch_crackle.wav".into(),
            volume: 0.35,
            min_distance: 3.0,
            max_distance: 14.0,
            looping: true,
        };
        for t in self.torches.iter_mut() {
            if t.audio.is_some() {
                continue;
            }
            t.audio = audio.spawn_emitter(&spec, t.light.position);
        }
    }

    /// Per-frame: drive each torch's audio-emitter volume from
    /// the same distance + rank + flicker curves the visual
    /// light uses. The volume scaling intentionally mirrors
    /// [`Self::update_lights`] so the player hears what they
    /// see — a torch that just brightened is also louder, and
    /// one that's fading past the rank cap is also quieter.
    /// Call after `update_lights` each frame.
    pub fn tick_audio(
        &self,
        audio: &mut rift_audio::AudioSystem,
        player_pos: Vec3,
        time: f32,
        fog_start: f32,
        fog_end: f32,
    ) {
        if self.torches.is_empty() {
            return;
        }
        let cutoff = fog_end + 2.0;
        let max_d2 = cutoff * cutoff;

        // Same RANK_CAP / RANK_FULL as `update_lights` so the
        // audible set tracks the visible set.
        const RANK_CAP: usize = 14;
        const RANK_FULL: usize = 12;

        let mut scored: Vec<(f32, usize)> = self
            .torches
            .iter()
            .enumerate()
            .map(|(i, t)| (t.light.position.distance_squared(player_pos), i))
            .filter(|(d2, _)| *d2 <= max_d2)
            .collect();
        scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        // Track which torches were in the active set so the
        // rest can be muted (kept playing but at zero volume —
        // cheaper than stop/restart, and preserves the loop's
        // phase so a torch returning to audible range fades
        // back in mid-crackle rather than restarting).
        let mut audible = vec![false; self.torches.len()];

        for (rank, (d2, idx)) in scored.into_iter().take(RANK_CAP).enumerate() {
            let t = &self.torches[idx];
            let Some(em) = t.audio else { continue };
            let d = d2.sqrt();
            let fog_range = (fog_end - fog_start).max(0.001);
            let raw = ((fog_end - d) / fog_range).clamp(0.0, 1.0);
            let dist_s = rift_math::smoothstep(raw);
            let rank_s = if rank < RANK_FULL {
                1.0
            } else {
                let span = (RANK_CAP - RANK_FULL) as f32;
                let r = (rank - RANK_FULL) as f32 + 1.0;
                let ts = (1.0 - (r / span)).clamp(0.0, 1.0);
                rift_math::smoothstep(ts)
            };
            let phase = t.flicker_phase;
            let tt = time + phase;
            let fast = (tt * 11.3).sin() * 0.5
                + (tt * 17.7 + 1.3).sin() * 0.3
                + (tt * 23.1 + 2.7).sin() * 0.2;
            let slow = (tt * 1.9).sin() * 0.5 + (tt * 3.1 + 0.7).sin() * 0.5;
            let flicker = fast * 0.10 + slow * 0.05;
            let volume = (dist_s * rank_s * (1.0 + flicker)).max(0.0);
            audio.set_emitter_position(em, t.light.position);
            audio.set_emitter_volume(em, volume);
            audible[idx] = true;
        }

        // Mute everything outside the active set so a torch
        // that just fell off the rank cap doesn't keep
        // bleeding through at full volume.
        for (i, t) in self.torches.iter().enumerate() {
            if audible[i] {
                continue;
            }
            if let Some(em) = t.audio {
                audio.set_emitter_volume(em, 0.0);
            }
        }
    }
}
