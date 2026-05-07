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
pub fn frost_ray() -> Effect {
    Effect {
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
                    ForceField::Gravity { axis: Vec3::Y, strength: 1.5 },
                    ForceField::Orbit { axis: Vec3::Y, speed: 6.0 },
                ],
                size: Curve::from_stops([
                    (0.00, 0.10),
                    (0.40, 0.14),
                    (1.00, 0.0),
                ]),
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
        ],
    }
}

/// Frost-impact burst at the tip of a Frost Ray (or where a
/// piercing beam crosses a target). Cold blue puff plus a few
/// sharp shards.
pub fn frost_impact() -> Effect {
    Effect {
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
    }
}
