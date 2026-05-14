//! Ambient world-prop presets — wall torches and other
//! always-on environment effects.

use glam::Vec3;

use crate::renderer::vfx::spec::*;

/// Wall-torch flame — a continuous, looping fire plume that sits
/// on a wall sconce. Three stacked layers compose the look:
///
/// 1. **Core flame**: short-lived bright HDR additive particles
///    rising fast. Keeps the flame's silhouette tight and drives
///    the bloom highlight.
/// 2. **Outer flame**: longer-lived softer particles that drift
///    upward and outward, giving the flame visible volume.
/// 3. **Smoke wisp**: dim translucent puff that lingers above
///    the flame, fading to nothing.
///
/// The effect is `duration: 0.0` (infinite) — gameplay code
/// despawns it when the floor changes. All forces are vertical
/// so the flame stays anchored to its wall position; the
/// `Wind` force adds a tiny upward bias so even slow particles
/// rise reliably.
pub fn wall_torch() -> Effect {
    Effect {
        duration: 0.0,
        layers: vec![
            // Core flame — small, very bright, short life.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Disc { radius: 0.04 },
                emission: EmissionMode::Continuous { rate: 55.0 },
                speed: (1.6, 2.4),
                lifetime: (0.18, 0.30),
                forces: vec![
                    // Upward acceleration so flames lick higher
                    // as they age (negative gravity along Y).
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 4.5,
                    },
                    ForceField::Drag { coefficient: 1.5 },
                    // Subtle curl gives the flame its dancing
                    // silhouette without expensive simulation.
                    ForceField::Curl {
                        frequency: 4.0,
                        strength: 1.6,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.10), (0.30, 0.16), (1.00, 0.02)]),
                // HDR amber → orange → dim red. Bright enough at
                // birth (~3-4×) to drive bloom; tonemap brings
                // the visible colour back to a clean orange.
                color: Gradient::from_stops([
                    (0.00, [4.5, 2.4, 0.6, 1.00]),
                    (0.40, [3.0, 1.0, 0.2, 1.00]),
                    (1.00, [0.6, 0.1, 0.0, 0.00]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // Outer flame — wider, dimmer, longer-lived. Reads
            // as the flame's volume / aura.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Disc { radius: 0.09 },
                emission: EmissionMode::Continuous { rate: 35.0 },
                speed: (0.7, 1.2),
                lifetime: (0.35, 0.55),
                forces: vec![
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 2.8,
                    },
                    ForceField::Drag { coefficient: 1.2 },
                    ForceField::Curl {
                        frequency: 2.5,
                        strength: 1.0,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.16), (0.40, 0.22), (1.00, 0.04)]),
                color: Gradient::from_stops([
                    (0.00, [2.5, 1.0, 0.20, 0.85]),
                    (0.50, [1.4, 0.45, 0.10, 0.55]),
                    (1.00, [0.30, 0.08, 0.02, 0.00]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // Smoke wisp — slow rising dim grey puff. Alpha-
            // blended so it can sit above the additive flame
            // without blowing out.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Disc { radius: 0.08 },
                emission: EmissionMode::Continuous { rate: 6.0 },
                speed: (0.25, 0.45),
                lifetime: (1.2, 1.8),
                forces: vec![
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 1.0,
                    },
                    ForceField::Drag { coefficient: 0.6 },
                    ForceField::Curl {
                        frequency: 1.2,
                        strength: 0.6,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.10), (0.50, 0.30), (1.00, 0.55)]),
                color: Gradient::from_stops([
                    (0.00, [0.10, 0.09, 0.08, 0.40]),
                    (0.40, [0.08, 0.07, 0.06, 0.20]),
                    (1.00, [0.04, 0.04, 0.04, 0.00]),
                ]),
                sprite: SpriteShape::Smoke,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            }),
        ],
    }
}

/// Drifting sand haze — a wide-area ambient layer that sells
/// "you're standing inside a sandstorm". Anchored on the
/// player (set via `set_anchor` each frame) so the field of
/// dust travels with the camera and never feels like a fixed
/// volumetric panel.
///
/// Two stacked layers compose the look:
///
/// 1. **Low-drift sand sheets**: large soft alpha smoke
///    sprites born inside a wide disc around the player at
///    ankle-to-shoulder height. A strong constant `Wind`
///    sweeps them past horizontally so the player perceives
///    motion across the entire arena, not just where they're
///    looking. Curl noise breaks up the front so the sheets
///    don't read as a marching wall. Lifetime is short so
///    the cloud refreshes constantly and never piles up
///    behind the camera.
///
/// 2. **Fast sand streaks**: small `Streak` sprites born on
///    the same disc but faster and lower (knee height). The
///    anisotropic streak shape orients along velocity so
///    these read as individual airborne grains skimming the
///    ground — cheap detail that hides the bulk sheet's
///    texture-less softness.
///
/// Tan/brown palette pulled from the sandstorm sky horizon
/// so the haze visually merges with the fog wall instead of
/// reading as a separate effect. Both layers are alpha-
/// blended (no HDR boost) — the goal is *occlusion* of the
/// background, not extra brightness, which preserves the sun
/// disc and god rays as the brightest things on screen.
///
/// Infinite duration; gameplay code despawns it on hub
/// teardown.
pub fn sandstorm_haze() -> Effect {
    Effect {
        duration: 0.0,
        layers: vec![
            // Bulk drifting dust sheets — the main occlusion
            // layer. Disc spans the visible play arena from
            // the player's anchor, so the field of haze
            // travels with the camera.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Disc { radius: 22.0 },
                // Bulk haze emission rate is the dominant
                // fillrate cost in the hub: each sheet grows
                // to ~6 m wide and stacks alpha-blended
                // overdraw across the screen. Dropping from
                // 22/s -> 8/s keeps a continuous "wall of
                // dust" feel (the long lifetime + slow drift
                // means there's always a sheet in frame) but
                // cuts steady-state count from ~110 to ~40
                // particles, which is the difference between
                // 35 and ~55 FPS in the hub.
                emission: EmissionMode::Continuous { rate: 8.0 },
                // Speed is the random outward kick from the
                // disc's spawn shape; we want it small —
                // most of the motion comes from the constant
                // `Wind` force below so every sheet drifts
                // in the *same* direction (a real sandstorm
                // has a coherent prevailing wind, not a
                // turbulent expansion).
                speed: (0.3, 0.7),
                lifetime: (3.5, 5.5),
                forces: vec![
                    // Coherent wind sweep — strong enough
                    // (~7 m/s) that a sheet crosses the
                    // 22 m disc in roughly its lifetime, so
                    // the player sees fresh sheets at the
                    // upwind side and old fading ones
                    // downwind. Matches the warm dust angle
                    // (slight rise) so distant sheets ride
                    // up off the platform.
                    ForceField::Wind {
                        velocity: Vec3::new(6.5, 0.4, 4.0),
                    },
                    // Light drag so the wind dominates and
                    // each particle quickly forgets its
                    // random spawn velocity.
                    ForceField::Drag { coefficient: 0.4 },
                    // Curl noise breaks up the marching
                    // front so the haze reads as turbulent
                    // dust, not a sliding texture sheet.
                    ForceField::Curl {
                        frequency: 0.18,
                        strength: 1.4,
                    },
                ],
                // Spawn small (sub-metre puff), grow huge
                // mid-life (~4 m wide), fade to nothing.
                // The mid-life size is the visible "looking
                // through dust" depth cue \u2014 a hair smaller
                // than the original (~6 m) saves another
                // ~30 % of the per-particle screen footprint
                // without losing the sheet feel, and combines
                // with the lower spawn rate above to bring
                // hub fillrate back into budget.
                size: Curve::from_stops([(0.00, 0.6), (0.45, 4.0), (1.00, 5.5)]),
                // Tan dust matched to `SkyConfig::sandstorm_hub`
                // horizon. RGB stays well under 1.0 (no HDR)
                // so the layer can't outshine the sun disc
                // or god rays. Alpha curve fades in at birth
                // and out at death so spawning isn't a hard
                // pop.
                color: Gradient::from_stops([
                    (0.00, [0.78, 0.55, 0.30, 0.00]),
                    (0.20, [0.82, 0.60, 0.34, 0.18]),
                    (0.65, [0.78, 0.55, 0.30, 0.14]),
                    (1.00, [0.70, 0.50, 0.28, 0.00]),
                ]),
                sprite: SpriteShape::Smoke,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            }),
            // Fast low streaks \u2014 individual grains skimming
            // the ground. Halved emission rate (80 -> 40)
            // because each streak is a small alpha quad
            // rendered against the sky/dunes; the visual
            // cadence at 40/s already reads as "flickering
            // grain motion" without the fillrate stack.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Disc { radius: 14.0 },
                emission: EmissionMode::Continuous { rate: 40.0 },
                speed: (1.5, 3.0),
                lifetime: (0.6, 1.1),
                forces: vec![
                    ForceField::Wind {
                        velocity: Vec3::new(9.0, 0.0, 5.5),
                    },
                    ForceField::Drag { coefficient: 0.6 },
                ],
                size: Curve::from_stops([(0.00, 0.04), (0.50, 0.10), (1.00, 0.02)]),
                color: Gradient::from_stops([
                    (0.00, [0.95, 0.78, 0.50, 0.00]),
                    (0.30, [0.92, 0.74, 0.46, 0.55]),
                    (1.00, [0.78, 0.55, 0.30, 0.00]),
                ]),
                sprite: SpriteShape::Streak,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            }),
        ],
    }
}

/// Rift-floor void embers — a slow, continuous field of
/// mood-tinted motes rising from below the dungeon floor.
/// Anchored on the player (set via `set_anchor` each
/// frame) so the field travels with the camera and the
/// player always sees fresh embers around them regardless of
/// where they walk on the floor.
///
/// The host should anchor this effect ~10 m *below* the
/// playable floor plane (e.g. `player_pos - Vec3::Y * 10.0`)
/// so embers spawn well beneath the floor mesh and rise
/// upward. The floor geometry then occludes embers behind /
/// under it via the regular depth test; only the embers
/// that drift past the floor's outer edges become visible.
/// Result: a heat-shimmer ring of glowing motes hugging the
/// silhouette of the dungeon, selling "there is something
/// molten directly below us".
///
/// Two layers compose the look:
///
/// 1. **Bulk motes** — wide disc of soft-glow particles at a
///    low emission rate. The mass of the effect. HDR colour is
///    derived from the active floor fog so crypt floors glow
///    blue, archive floors violet, prison floors green, etc.
///
/// 2. **Hot sparks** — sparse, brighter, faster-rising
///    streaks born in the same disc. Gives the eye
///    something to track and adds high-frequency motion on
///    top of the slow bulk drift.
///
/// Additive layers provide glow; alpha ash provides occlusion.
/// All chroma is pulled from the same fog colour that themes
/// the abyss shader so the background particles belong to the
/// floor mood instead of always reading as infernal.
pub fn rift_void_embers() -> Effect {
    rift_void_embers_tinted([0.090, 0.012, 0.008])
}

pub fn rift_void_embers_tinted(fog_color: [f32; 3]) -> Effect {
    let fog = Vec3::from(fog_color);
    let peak = fog.max_element().max(0.001);
    let chroma = fog / peak;

    let mote_core = fog * 14.0 + chroma * 1.55;
    let mote_mid = fog * 8.0 + chroma * 0.70;
    let mote_tail = fog * 2.2 + chroma * 0.10;

    let spark_core = fog * 20.0 + chroma * 2.60;
    let spark_mid = fog * 10.0 + chroma * 0.90;
    let spark_tail = fog * 2.6 + chroma * 0.12;

    let ash_core = (fog * 3.6 + Vec3::splat(0.055)).min(Vec3::splat(0.52));
    let ash_mid = (fog * 2.8 + Vec3::splat(0.040)).min(Vec3::splat(0.42));
    let ash_tail = (fog * 1.8 + Vec3::splat(0.025)).min(Vec3::splat(0.30));

    let wisp_core = fog * 15.0 + chroma * 0.90;
    let wisp_mid = fog * 9.0 + chroma * 0.42;
    let wisp_tail = fog * 2.5 + chroma * 0.08;

    Effect {
        duration: 0.0,
        layers: vec![
            // Bulk motes — wide soft glow field.
            Layer::Particles(ParticleSpec {
                // Disc radius spans the visible play arena
                // around the player. A bit beyond the
                // typical room width so embers reliably
                // emerge past the floor's outer edges
                // wherever the player walks.
                spawn: SpawnShape::Disc { radius: 16.0 },
                // Low rate — these are ambient embers, not
                // a fire. Steady-state count ~50.
                emission: EmissionMode::Continuous { rate: 9.0 },
                // Small random initial speed; most of the
                // vertical motion comes from the upward
                // gravity force below.
                speed: (0.05, 0.25),
                // Long enough for an ember spawned 10 m
                // below the floor to rise well past the
                // floor plane before fading.
                lifetime: (4.5, 7.5),
                forces: vec![
                    // Upward acceleration — heat-rise. A
                    // gentle pull so the embers float
                    // rather than rocket.
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 0.6,
                    },
                    // Strong drag so embers settle into a
                    // slow terminal rise (~1 m/s) instead
                    // of accelerating unboundedly.
                    ForceField::Drag { coefficient: 0.7 },
                    // Soft curl noise gives the field a
                    // shimmer, breaks up the uniform rise.
                    ForceField::Curl {
                        frequency: 0.6,
                        strength: 0.4,
                    },
                ],
                // Tiny → slightly larger → fade. Small base
                // size keeps individual embers reading as
                // discrete motes, not soft clouds.
                size: Curve::from_stops([(0.00, 0.04), (0.30, 0.10), (1.00, 0.02)]),
                // HDR mood colour at birth (drives bloom),
                // fading back toward the floor fog. Alpha ramps in over the first
                // ~15 % so embers don't pop into existence
                // — they "ignite" as they rise into view.
                color: Gradient::from_stops([
                    (0.00, [mote_core.x, mote_core.y, mote_core.z, 0.00]),
                    (0.15, [mote_core.x, mote_core.y, mote_core.z, 0.90]),
                    (0.55, [mote_mid.x, mote_mid.y, mote_mid.z, 0.70]),
                    (1.00, [mote_tail.x, mote_tail.y, mote_tail.z, 0.00]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // Hot sparks — sparser, brighter, faster.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Disc { radius: 14.0 },
                // Very low rate — these are punctuation,
                // not a stream.
                emission: EmissionMode::Continuous { rate: 2.0 },
                speed: (0.15, 0.45),
                lifetime: (2.5, 4.0),
                forces: vec![
                    // Stronger upward pull so sparks rise
                    // faster than the bulk haze, giving
                    // the eye trackable motion against the
                    // slower field.
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 1.4,
                    },
                    ForceField::Drag { coefficient: 0.5 },
                    ForceField::Curl {
                        frequency: 1.2,
                        strength: 0.6,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.05), (0.40, 0.09), (1.00, 0.01)]),
                // Hotter mood palette. Higher HDR boost so
                // individual sparks read as bright pinpricks
                // against the bulk glow without turning orange
                // on non-infernal floors.
                color: Gradient::from_stops([
                    (0.00, [spark_core.x, spark_core.y, spark_core.z, 0.00]),
                    (0.12, [spark_core.x, spark_core.y, spark_core.z, 1.00]),
                    (0.55, [spark_mid.x, spark_mid.y, spark_mid.z, 0.70]),
                    (1.00, [spark_tail.x, spark_tail.y, spark_tail.z, 0.00]),
                ]),
                sprite: SpriteShape::Streak,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // Drifting ash flakes — alpha-blended grey
            // motes falling slowly from above the player.
            // Counter-balances the rising embers with a
            // downward motion layer, selling "this place is
            // burning somewhere out of sight and the ash
            // settles past us". `Column` spawns are
            // distributed over the column's full Y range
            // (anchor .. anchor + height); since the anchor
            // sits ~10 m below the player, a 22 m column
            // covers from below-the-floor up to ~12 m
            // overhead. Flakes spawned below the floor sink
            // unseen — only those born above the platform
            // contribute to the visible drift.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Column {
                    radius: 14.0,
                    height: 22.0,
                    axis: Vec3::Y,
                },
                // Sparse — ash is punctuation in the field,
                // not a snowstorm. Lifetime × rate keeps the
                // steady-state count around ~50 flakes.
                emission: EmissionMode::Continuous { rate: 6.0 },
                speed: (0.02, 0.10),
                // Long lifetime: a flake spawned overhead
                // takes most of this duration to drift past
                // the platform on its way down.
                lifetime: (6.0, 9.0),
                forces: vec![
                    // Gentle downward pull. Negative
                    // strength along +Y == falling. Drag
                    // dominates so flakes settle into a
                    // slow terminal sink rather than
                    // accelerating into bullets.
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: -0.35,
                    },
                    ForceField::Drag { coefficient: 0.6 },
                    // Wide low-frequency curl so individual
                    // flakes wander laterally as they fall
                    // — reads as light air currents, not
                    // straight-line snow.
                    ForceField::Curl {
                        frequency: 0.25,
                        strength: 0.25,
                    },
                ],
                // Moderate, roughly constant size — flakes
                // shouldn't shrink to nothing because the
                // alpha curve already handles the fade.
                size: Curve::from_stops([(0.00, 0.06), (0.20, 0.14), (1.00, 0.10)]),
                // Mood-tinted dim grey, no HDR — alpha-blended so
                // the ash *occludes* the void instead of
                // glowing. The tint follows the same fog chroma
                // as the abyss instead of defaulting warm.
                // Alpha tops out around 0.35 so the flakes
                // are visible against the dark void but
                // never fight the embers for attention.
                color: Gradient::from_stops([
                    (0.00, [ash_core.x, ash_core.y, ash_core.z, 0.00]),
                    (0.15, [ash_core.x, ash_core.y, ash_core.z, 0.35]),
                    (0.70, [ash_mid.x, ash_mid.y, ash_mid.z, 0.28]),
                    (1.00, [ash_tail.x, ash_tail.y, ash_tail.z, 0.00]),
                ]),
                sprite: SpriteShape::Smoke,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            }),
            // Distant void wisps — slow vertical columns of
            // dim crimson light at the silhouette horizon,
            // far from the player. Adds parallax depth: as
            // the player walks, these stay roughly fixed
            // relative to the anchor (which follows the
            // player), so they read as distant landmarks in
            // the abyss rather than nearby motes. `Ring`
            // spawn keeps them clear of the play area; the
            // `Wisp` sprite always renders world-up so each
            // particle reads as a tall ethereal strand
            // regardless of velocity.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Ring {
                    radius: 28.0,
                    thickness: 5.0,
                },
                // Very sparse — only a handful visible at
                // any time. Long lifetime keeps the silhouette
                // populated even at this low rate.
                emission: EmissionMode::Continuous { rate: 1.2 },
                // Near-zero initial velocity — wisps barely
                // move; the curl gives them an organic
                // sway and the gentle gravity lets them
                // drift slowly upward.
                speed: (0.02, 0.08),
                lifetime: (8.0, 14.0),
                forces: vec![
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 0.15,
                    },
                    ForceField::Drag { coefficient: 0.8 },
                    ForceField::Curl {
                        frequency: 0.15,
                        strength: 0.20,
                    },
                ],
                // Tall and slim — `Wisp` is anisotropic
                // along world-up so the size value
                // controls strand height/width. Grow over
                // the first 25 % so they fade in rather
                // than pop, then taper for the long fade.
                size: Curve::from_stops([(0.00, 0.20), (0.25, 0.80), (1.00, 0.30)]),
                // Dim HDR mood colour — bright enough to glow
                // against the void backdrop, dim enough that
                // they read as distant. Alpha curve ramps in/out
                // for soft births and deaths.
                color: Gradient::from_stops([
                    (0.00, [wisp_core.x, wisp_core.y, wisp_core.z, 0.00]),
                    (0.20, [wisp_core.x, wisp_core.y, wisp_core.z, 0.55]),
                    (0.70, [wisp_mid.x, wisp_mid.y, wisp_mid.z, 0.45]),
                    (1.00, [wisp_tail.x, wisp_tail.y, wisp_tail.z, 0.00]),
                ]),
                sprite: SpriteShape::Wisp,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
        ],
    }
}
