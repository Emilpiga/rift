//! Frost-themed combat presets.

use glam::Vec3;

use crate::renderer::vfx::spec::*;

/// Frost Ray — a piercing cyan beam channeled from the caster's
/// hand. Built as a single ribbon layer with:
///
/// - HDR cyan core fading to transparent at the edges (cross
///   gradient with HDR > 1 in the centre to drive bloom)
/// - Slight fade-in at the hand and a soft fade right at the
///   tip so the impact point doesn't have a hard cap (length
///   gradient)
/// - Scrolling fbm noise along the beam for flow / shimmer
///
/// Width is set on the spec; length is implicit in the
/// (origin, tip) endpoints supplied by the gameplay layer.
/// Duration is `0.0` (infinite); the gameplay layer despawns on
/// channel end.
pub fn frost_ray() -> EffectBundle {
    EffectBundle::new(Effect {
        duration: 0.0,
        layers: vec![
            // Hand-base swirl: continuous cyan glow + a few sharper
            // sparks that orbit the caster's hand. Spawned at the
            // effect's anchor, which gameplay code refreshes every
            // frame via `set_anchor` so the swirl tracks the moving
            // hand joint.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Continuous { rate: 60.0 },
                speed: (0.3, 1.0),
                lifetime: (0.25, 0.55),
                forces: vec![
                    ForceField::Drag { coefficient: 3.0 },
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 1.5,
                    },
                    ForceField::Orbit {
                        axis: Vec3::Y,
                        speed: 6.0,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.10), (0.40, 0.14), (1.00, 0.0)]),
                color: Gradient::from_stops([
                    (0.00, [1.5, 3.0, 4.5, 1.0]),
                    (0.50, [0.6, 1.4, 2.2, 0.7]),
                    (1.00, [0.2, 0.4, 0.6, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            Layer::Ribbon(RibbonSpec {
                width: 0.45,
                cross_gradient: Gradient::from_stops([
                    (0.00, [0.30, 0.60, 0.90, 0.0]),
                    (0.20, [0.45, 0.85, 1.00, 0.6]),
                    (0.50, [4.00, 6.00, 8.00, 1.0]),
                    (0.80, [0.45, 0.85, 1.00, 0.6]),
                    (1.00, [0.30, 0.60, 0.90, 0.0]),
                ]),
                length_gradient: Some(Gradient::from_stops([
                    (0.00, [0.4, 0.4, 0.4, 0.6]),
                    (0.10, [1.0, 1.0, 1.0, 1.0]),
                    (0.85, [1.0, 1.0, 1.0, 1.0]),
                    (1.00, [0.6, 0.6, 0.6, 0.4]),
                ])),
                noise: Some(RibbonNoise {
                    tile: 0.5,
                    scroll: 4.0,
                    strength: 0.55,
                    octaves: 3,
                }),
                blend: BlendMode::Additive,
            }),
            Layer::Ribbon(RibbonSpec {
                width: 0.14,
                cross_gradient: Gradient::from_stops([
                    (0.00, [0.28, 0.70, 1.20, 0.0]),
                    (0.34, [0.90, 2.20, 3.60, 0.46]),
                    (0.50, [5.60, 8.20, 10.5, 1.00]),
                    (0.66, [0.90, 2.20, 3.60, 0.46]),
                    (1.00, [0.28, 0.70, 1.20, 0.0]),
                ]),
                length_gradient: Some(Gradient::from_stops([
                    (0.00, [0.20, 0.26, 0.34, 0.0]),
                    (0.12, [0.90, 1.05, 1.20, 0.90]),
                    (0.74, [1.05, 1.18, 1.28, 1.00]),
                    (1.00, [0.35, 0.50, 0.70, 0.0]),
                ])),
                noise: Some(RibbonNoise {
                    tile: 0.20,
                    scroll: 7.5,
                    strength: 0.72,
                    octaves: 4,
                }),
                blend: BlendMode::Additive,
            }),
        ],
    })
    // Cold cyan hand-glow. The beam ribbon is additive and
    // doesn't actually illuminate world geometry, so without
    // this light the caster's body and the floor at their
    // feet stay completely dark while channeling — the
    // shimmering beam reads as a sticker floating in mid-air.
    //
    // Driven by `follow_particles`: the hand-base swirl
    // emits at 60/s while the channel is active, so the
    // light envelope quickly reaches peak and stays there.
    // The instant gameplay calls `despawn` on channel end,
    // emission stops, the envelope's exponential decay
    // takes over (~0.85 s half-life) and the corridor glow
    // softly fades — same mechanism as the fireball trail.
    //
    // No `intensity_curve` set: the runtime maps the
    // envelope directly to `curve_mul`, so brightness
    // tracks the swirl population 1:1.
    .with_light(EffectLight {
        // Cool cyan-white. Slightly biased toward white at
        // the centre so up-close walls read as "frozen mist
        // condensing" rather than a flat blue spotlight.
        color: Vec3::new(1.6, 2.6, 3.6),
        // 4 m reach: enough that the floor under the caster
        // and the closest wall pick it up, short enough that
        // it doesn't fight nearby torches at corridor scale.
        radius: 4.0,
        intensity: 1.4,
        intensity_curve: None,
        lifetime: None,
        // Subtle, fast modulation. Frost Ray is a *focused*
        // beam, not a flickering torch — keep amplitude
        // small (3%) and the rate high enough (12 Hz) that
        // it reads as energy crackle, not a dying flame.
        flicker_amp: 0.03,
        flicker_hz: 12.0,
        // Lift slightly off the hand so the light's centre
        // sits roughly inside the wrist/forearm region
        // rather than at the palm, giving a more even rim
        // across the caster's mesh.
        offset: Vec3::new(0.0, 0.15, 0.0),
        follow_particles: true,
    })
    // Persistent tip light pinned to the beam's *tip*
    // endpoint by the engine. Without this the only
    // illumination at the impact end is the per-burst
    // `frost_impact` flashes, which fire at 10 Hz and have
    // ~0.45 s pool lives — perceptually they read as a
    // strobing light, not a continuous source. The tip
    // light bridges the gaps so the floor/wall/pierced
    // enemies stay lit while the channel is active.
    //
    // Color/radius mirror the per-burst flash so the steady
    // glow and the pulses sum cleanly when both fire on
    // the same frame.
    .with_tip_light(EffectLight {
        color: Vec3::new(1.8, 3.0, 4.2),
        radius: 3.5,
        intensity: 1.5,
        intensity_curve: None,
        lifetime: None,
        flicker_amp: 0.04,
        flicker_hz: 14.0,
        offset: Vec3::ZERO,
        // Driven by the same hand-base swirl population as
        // the anchor light — channel active means swirl
        // emitting means envelope at peak. On `despawn` the
        // exponential decay fades both lights together.
        follow_particles: true,
    })
}

/// Frost-impact burst at the tip of a Frost Ray (or where a
/// piercing beam crosses a target). Cold blue puff plus a few
/// sharp shards.
pub fn frost_impact() -> EffectBundle {
    EffectBundle::new(Effect {
        duration: 0.05,
        layers: vec![
            // Soft cold puff
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 14 },
                speed: (1.0, 3.0),
                lifetime: (0.25, 0.45),
                forces: vec![ForceField::Drag { coefficient: 5.0 }],
                size: Curve::from_stops([(0.0, 0.18), (1.0, 0.05)]),
                color: Gradient::from_stops([
                    (0.0, [3.0, 5.5, 7.0, 0.9]),
                    (1.0, [0.2, 0.4, 0.6, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // Crystal shards
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 8 },
                speed: (3.0, 6.0),
                lifetime: (0.18, 0.28),
                forces: vec![ForceField::Drag { coefficient: 3.0 }],
                size: Curve::from_stops([(0.0, 0.10), (1.0, 0.0)]),
                color: Gradient::from_stops([
                    (0.0, [5.0, 7.0, 9.0, 1.0]),
                    (1.0, [0.3, 0.5, 0.7, 0.0]),
                ]),
                sprite: SpriteShape::Shard,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
        ],
    })
}

/// Trailing wake for a Frost Shatter shard projectile. Cyan
/// counterpart to `arcane_bolt_trail()` — same persistent
/// (`duration = 0.0`) anchor-driven trail pattern, recoloured
/// to match Frost Ray's palette so the shards visually descend
/// from the same beam they spawned out of.
pub fn frost_shard_trail() -> Effect {
    Effect {
        duration: 0.0,
        layers: vec![
            // Cold core — sharp cyan-white embers fizzing off
            // the shard body.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Continuous { rate: 160.0 },
                speed: (0.3, 1.0),
                lifetime: (0.14, 0.26),
                forces: vec![ForceField::Drag { coefficient: 5.0 }],
                size: Curve::from_stops([(0.00, 0.16), (0.30, 0.12), (1.00, 0.02)]),
                color: Gradient::from_stops([
                    (0.00, [2.4, 4.6, 6.0, 1.0]),
                    (0.40, [0.7, 1.8, 2.8, 0.85]),
                    (1.00, [0.10, 0.25, 0.45, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // Frost mist wake — pale blue puffs hanging briefly
            // along the shard's path.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Continuous { rate: 35.0 },
                speed: (0.05, 0.4),
                lifetime: (0.30, 0.55),
                forces: vec![ForceField::Drag { coefficient: 2.5 }],
                size: Curve::from_stops([(0.00, 0.12), (1.00, 0.28)]),
                color: Gradient::from_stops([
                    (0.00, [0.7, 1.0, 1.4, 0.5]),
                    (0.50, [0.20, 0.35, 0.55, 0.28]),
                    (1.00, [0.04, 0.06, 0.12, 0.0]),
                ]),
                sprite: SpriteShape::Smoke,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            }),
        ],
    }
}

/// Impact burst for a Frost Shatter shard projectile. Larger
/// and sharper than the per-tick `frost_impact()` ping —
/// shaped like `arcane_bolt_impact()` (flash + cloud + sparks
/// + ground ring) but in cold blue so it reads as part of the
/// Frost Ray family.
pub fn frost_shard_impact() -> Effect {
    Effect {
        duration: 0.05,
        layers: vec![
            // 1. Flash — single tight cyan-white puff.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.09, 0.11),
                forces: vec![],
                size: Curve::from_stops([(0.00, 1.05), (1.00, 1.70)]),
                color: Gradient::from_stops([
                    (0.00, [4.0, 6.0, 8.0, 1.0]),
                    (1.00, [0.5, 1.2, 2.0, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 2. Cold mist cloud — outward sphere of pale blue
            //    puffs with slight upward drift.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 26 },
                speed: (2.0, 5.0),
                lifetime: (0.32, 0.55),
                forces: vec![
                    ForceField::Drag { coefficient: 4.0 },
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 1.0,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.30), (0.30, 0.52), (1.00, 0.20)]),
                color: Gradient::from_stops([
                    (0.00, [2.4, 4.0, 5.8, 1.0]),
                    (0.40, [0.7, 1.6, 2.6, 0.8]),
                    (1.00, [0.06, 0.14, 0.30, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 3. Crystal sparks — fast cyan motes flung outward.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 20 },
                speed: (4.0, 9.0),
                lifetime: (0.26, 0.48),
                forces: vec![
                    ForceField::Drag { coefficient: 1.4 },
                    ForceField::Gravity {
                        axis: -Vec3::Y,
                        strength: 4.0,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.09), (1.00, 0.0)]),
                color: Gradient::from_stops([
                    (0.00, [3.5, 5.5, 7.0, 1.0]),
                    (0.50, [1.0, 2.0, 3.0, 0.9]),
                    (1.00, [0.10, 0.20, 0.40, 0.0]),
                ]),
                sprite: SpriteShape::Shard,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 4. Frost ring — flat cyan ring expanding along
            //    the ground plane.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.32, 0.32),
                forces: vec![],
                size: Curve::from_stops([(0.00, 0.35), (1.00, 2.50)]),
                color: Gradient::from_stops([
                    (0.00, [1.6, 3.0, 4.4, 0.9]),
                    (1.00, [0.10, 0.25, 0.55, 0.0]),
                ]),
                sprite: SpriteShape::Ring,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
        ],
    }
}
