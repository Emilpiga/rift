//! Revive-shrine presets — pillar VFX, hand-to-shrine
//! channel beam, and the ghost-rise wisp shown when a player
//! transitions to ghost form.

use glam::Vec3;

use crate::renderer::vfx::spec::*;

/// Ghost rise — soft burst of pale cyan-white wisps that
/// drift upward over ~1s. Played at the dead body's last
/// position when a player flips into ghost mode, so remote
/// teammates see a gentle "soul departing" cue instead of
/// the avatar simply popping out of existence. Owner
/// suppresses this for themselves.
pub fn ghost_rise() -> Effect {
    Effect {
        duration: 0.10,
        layers: vec![Layer::Particles(ParticleSpec {
            spawn: SpawnShape::Sphere,
            emission: EmissionMode::Burst { count: 24 },
            speed: (0.6, 1.8),
            lifetime: (0.8, 1.4),
            forces: vec![
                ForceField::Drag { coefficient: 1.4 },
                ForceField::Gravity {
                    axis: Vec3::Y,
                    strength: 1.6, // upward float
                },
            ],
            size: Curve::from_stops([(0.0, 0.18), (0.5, 0.42), (1.0, 0.55)]),
            color: Gradient::from_stops([
                (0.0, [0.85, 0.95, 1.05, 0.75]),
                (0.6, [0.60, 0.80, 1.00, 0.40]),
                (1.0, [0.35, 0.55, 0.85, 0.0]),
            ]),
            sprite: SpriteShape::Smoke,
            blend: BlendMode::Alpha,
            opacity: 1.0,
        })],
    }
}

/// Revive-shrine pillar — a tall holy beam reaching skyward,
/// distinct from the rarity-coloured `loot_beam` so a ghost can
/// recognise it at a glance. Three layers:
///   1. Wide rising column of holy motes in a desaturated
///      cyan-white gradient (HDR > 1 in the core for bloom).
///   2. Slow upward sparks orbiting the beam at half the
///      column's radius for a denser core.
///   3. Soft halo at the base for ground anchoring.
pub fn revive_shrine_pillar() -> Effect {
    Effect {
        duration: 0.0,
        layers: vec![
            // Wide rising column.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Column {
                    radius: 0.55,
                    height: 0.0,
                    axis: Vec3::Y,
                },
                emission: EmissionMode::BurstAndContinuous {
                    burst: 50,
                    rate: 220.0,
                },
                speed: (1.8, 4.5),
                lifetime: (1.2, 2.6),
                forces: vec![
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 4.5,
                    },
                    ForceField::Drag { coefficient: 0.5 },
                ],
                size: Curve::from_stops([(0.0, 0.08), (1.0, 0.02)]),
                color: Gradient::from_stops([
                    (0.0, [3.5, 4.5, 5.5, 1.0]),
                    (0.5, [1.4, 2.0, 2.6, 0.7]),
                    (1.0, [0.4, 0.6, 0.9, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // Inner sparks orbiting upward.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Column {
                    radius: 0.22,
                    height: 0.0,
                    axis: Vec3::Y,
                },
                emission: EmissionMode::Continuous { rate: 90.0 },
                speed: (3.0, 6.0),
                lifetime: (0.8, 1.5),
                forces: vec![
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 6.0,
                    },
                    ForceField::Drag { coefficient: 0.4 },
                    ForceField::Orbit {
                        axis: Vec3::Y,
                        speed: 4.0,
                    },
                ],
                size: Curve::from_stops([(0.0, 0.06), (1.0, 0.0)]),
                color: Gradient::from_stops([
                    (0.0, [5.0, 5.5, 6.0, 1.0]),
                    (1.0, [0.6, 0.9, 1.2, 0.0]),
                ]),
                sprite: SpriteShape::Spark,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // Ground halo.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Continuous { rate: 35.0 },
                speed: (0.4, 1.2),
                lifetime: (0.5, 1.1),
                forces: vec![ForceField::Drag { coefficient: 1.8 }],
                size: Curve::from_stops([(0.0, 0.18), (1.0, 0.32)]),
                color: Gradient::from_stops([
                    (0.0, [4.0, 4.8, 5.5, 0.9]),
                    (1.0, [0.5, 0.8, 1.0, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
        ],
    }
}

/// Channel beam linking a player's hand to a revive shrine. A
/// thin holy ribbon plus a continuous trickle of orbiting motes
/// rendered in cyan-white. Endpoints are driven each frame by
/// the gameplay layer via `set_endpoints` so the beam tracks
/// the player as they move within the channel radius.
pub fn shrine_channel_beam() -> Effect {
    Effect {
        duration: 0.0,
        layers: vec![
            // Hand-base swirl: small bright motes orbiting the
            // caster's hand for a "gathering energy" read.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Continuous { rate: 80.0 },
                speed: (0.3, 1.0),
                lifetime: (0.2, 0.5),
                forces: vec![
                    ForceField::Drag { coefficient: 3.5 },
                    ForceField::Orbit {
                        axis: Vec3::Y,
                        speed: 7.0,
                    },
                ],
                size: Curve::from_stops([(0.0, 0.08), (0.4, 0.10), (1.0, 0.0)]),
                color: Gradient::from_stops([
                    (0.0, [3.0, 4.0, 5.5, 1.0]),
                    (1.0, [0.4, 0.7, 1.0, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // The actual beam ribbon. Endpoints driven by
            // gameplay; HDR core for bloom; gentle scrolling
            // noise so it shimmers along its length.
            Layer::Ribbon(RibbonSpec {
                width: 0.18,
                cross_gradient: Gradient::from_stops([
                    (0.0, [0.30, 0.55, 0.85, 0.0]),
                    (0.3, [0.6, 0.9, 1.1, 0.7]),
                    (0.5, [3.5, 4.5, 5.5, 1.0]),
                    (0.7, [0.6, 0.9, 1.1, 0.7]),
                    (1.0, [0.30, 0.55, 0.85, 0.0]),
                ]),
                length_gradient: Some(Gradient::from_stops([
                    (0.0, [0.5, 0.5, 0.5, 0.5]),
                    (0.15, [1.0, 1.0, 1.0, 1.0]),
                    (0.85, [1.0, 1.0, 1.0, 1.0]),
                    (1.0, [0.6, 0.6, 0.6, 0.4]),
                ])),
                noise: Some(RibbonNoise {
                    tile: 0.4,
                    scroll: 5.0,
                    strength: 0.45,
                    octaves: 3,
                }),
                blend: BlendMode::Additive,
            }),
            Layer::Ribbon(RibbonSpec {
                width: 0.055,
                cross_gradient: Gradient::from_stops([
                    (0.0, [0.60, 0.95, 1.30, 0.0]),
                    (0.38, [1.40, 1.90, 2.40, 0.42]),
                    (0.5, [5.80, 6.80, 7.60, 1.0]),
                    (0.62, [1.40, 1.90, 2.40, 0.42]),
                    (1.0, [0.60, 0.95, 1.30, 0.0]),
                ]),
                length_gradient: Some(Gradient::from_stops([
                    (0.0, [0.20, 0.24, 0.30, 0.0]),
                    (0.18, [1.05, 1.12, 1.20, 0.85]),
                    (0.80, [1.0, 1.0, 1.0, 1.0]),
                    (1.0, [0.35, 0.44, 0.55, 0.0]),
                ])),
                noise: Some(RibbonNoise {
                    tile: 0.18,
                    scroll: 6.5,
                    strength: 0.66,
                    octaves: 4,
                }),
                blend: BlendMode::Additive,
            }),
        ],
    }
}
