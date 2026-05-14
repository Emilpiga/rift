//! Enemy-special VFX presets for non-boss archetypes that need
//! bespoke readability and impact.

use glam::Vec3;

use crate::renderer::vfx::spec::*;

pub fn wraith_scream_telegraph(dir: Vec3, duration: f32) -> Effect {
    let axis = planar_dir(dir);
    let dur = duration.max(0.05);
    let reach = 4.8;
    Effect {
        duration: dur,
        layers: vec![
            // Directional breath column. Born along the scream
            // path instead of on the Wraith's body, so the wind-up
            // reads as a forward cone taking shape.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Line {
                    a: axis * 0.15,
                    b: axis * reach,
                },
                emission: EmissionMode::Continuous { rate: 120.0 },
                speed: (0.20, 0.85),
                lifetime: (0.18, 0.36),
                forces: vec![
                    ForceField::Wind {
                        velocity: axis * 2.6 + Vec3::Y * 0.10,
                    },
                    ForceField::Drag { coefficient: 4.0 },
                    ForceField::Curl {
                        frequency: 1.05,
                        strength: 2.0,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.10), (0.48, 0.26), (1.00, 0.02)]),
                color: Gradient::from_stops([
                    (0.00, [1.2, 3.8, 4.6, 0.46]),
                    (0.55, [0.42, 1.45, 2.4, 0.34]),
                    (1.00, [0.05, 0.16, 0.35, 0.00]),
                ]),
                sprite: SpriteShape::Streak,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // Wide mist envelope. It starts thin at the mouth and
            // expands downrange, giving the player a readable cone
            // volume before the damage lands.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Cone {
                    axis,
                    half_angle: 0.36,
                },
                emission: EmissionMode::Continuous { rate: 72.0 },
                speed: (3.2, 6.0),
                lifetime: (0.32, 0.58),
                forces: vec![
                    ForceField::Drag { coefficient: 2.6 },
                    ForceField::Curl {
                        frequency: 0.70,
                        strength: 3.6,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.20), (0.48, 0.62), (1.00, 0.10)]),
                color: Gradient::from_stops([
                    (0.00, [0.80, 2.10, 2.65, 0.28]),
                    (0.55, [0.18, 0.62, 1.10, 0.22]),
                    (1.00, [0.05, 0.20, 0.35, 0.00]),
                ]),
                sprite: SpriteShape::Smoke,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            }),
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Line {
                    a: axis * 0.30,
                    b: axis * (reach * 0.95),
                },
                emission: EmissionMode::Continuous { rate: 32.0 },
                speed: (0.0, 0.35),
                lifetime: (0.30, 0.54),
                forces: vec![
                    ForceField::Wind {
                        velocity: axis * 1.2 + Vec3::Y * 0.32,
                    },
                    ForceField::Drag { coefficient: 2.0 },
                ],
                size: Curve::from_stops([(0.00, 0.22), (0.55, 0.48), (1.00, 0.06)]),
                color: Gradient::from_stops([
                    (0.00, [2.6, 6.2, 6.8, 0.42]),
                    (0.50, [0.75, 2.4, 3.4, 0.30]),
                    (1.00, [0.08, 0.26, 0.46, 0.00]),
                ]),
                sprite: SpriteShape::Wisp,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (dur, dur),
                forces: vec![],
                size: Curve::from_stops([(0.00, 0.55), (0.78, 1.05), (1.00, 1.34)]),
                color: Gradient::from_stops([
                    (0.00, [0.45, 1.80, 2.35, 0.10]),
                    (0.74, [0.80, 3.30, 4.20, 0.26]),
                    (1.00, [1.80, 5.20, 6.00, 0.48]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 0.85,
            }),
        ],
    }
}

pub fn wraith_scream_impact(dir: Vec3) -> EffectBundle {
    let axis = planar_dir(dir);
    let reach = 4.8;
    EffectBundle::new(Effect {
        duration: 0.05,
        layers: vec![
            // Bright damage sheet along the actual cone path.
            // This is the frame that should make the player say
            // "that scream hit me from there".
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Line {
                    a: axis * 0.25,
                    b: axis * reach,
                },
                emission: EmissionMode::Burst { count: 120 },
                speed: (0.4, 1.6),
                lifetime: (0.18, 0.34),
                forces: vec![
                    ForceField::Wind {
                        velocity: axis * 5.2 + Vec3::Y * 0.12,
                    },
                    ForceField::Drag { coefficient: 3.8 },
                ],
                size: Curve::from_stops([(0.00, 0.12), (0.42, 0.32), (1.00, 0.0)]),
                color: Gradient::from_stops([
                    (0.00, [4.8, 8.5, 8.8, 1.0]),
                    (0.46, [1.2, 3.8, 5.2, 0.70]),
                    (1.00, [0.08, 0.24, 0.48, 0.00]),
                ]),
                sprite: SpriteShape::Streak,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Cone {
                    axis,
                    half_angle: 0.42,
                },
                emission: EmissionMode::Burst { count: 130 },
                speed: (7.5, 15.5),
                lifetime: (0.20, 0.42),
                forces: vec![ForceField::Drag { coefficient: 2.1 }],
                size: Curve::from_stops([(0.00, 0.22), (0.40, 0.42), (1.00, 0.0)]),
                color: Gradient::from_stops([
                    (0.00, [3.2, 6.5, 7.0, 0.95]),
                    (0.45, [0.90, 2.25, 3.20, 0.66]),
                    (1.00, [0.08, 0.22, 0.42, 0.00]),
                ]),
                sprite: SpriteShape::Streak,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Line {
                    a: axis * 0.45,
                    b: axis * (reach * 0.92),
                },
                emission: EmissionMode::Burst { count: 68 },
                speed: (0.8, 2.8),
                lifetime: (0.42, 0.78),
                forces: vec![
                    ForceField::Wind {
                        velocity: axis * 1.8 + Vec3::Y * 0.22,
                    },
                    ForceField::Drag { coefficient: 2.8 },
                    ForceField::Curl {
                        frequency: 0.95,
                        strength: 4.2,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.36), (0.38, 0.92), (1.00, 0.24)]),
                color: Gradient::from_stops([
                    (0.00, [0.45, 1.30, 1.65, 0.42]),
                    (0.55, [0.16, 0.45, 0.72, 0.30]),
                    (1.00, [0.02, 0.06, 0.12, 0.00]),
                ]),
                sprite: SpriteShape::Smoke,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            }),
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.20, 0.20),
                forces: vec![],
                size: Curve::from_stops([(0.00, 0.75), (0.38, 1.55), (1.00, 2.20)]),
                color: Gradient::from_stops([
                    (0.00, [4.8, 8.0, 8.4, 0.82]),
                    (0.42, [1.5, 4.4, 5.4, 0.42]),
                    (1.00, [0.12, 0.42, 0.65, 0.00]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
        ],
    })
    .with_light(EffectLight {
        color: Vec3::new(0.75, 2.7, 3.3),
        radius: 6.5,
        intensity: 0.62,
        intensity_curve: Some(Curve::from_stops([
            (0.00, 1.0),
            (0.18, 0.75),
            (0.55, 0.22),
            (1.00, 0.0),
        ])),
        lifetime: None,
        flicker_amp: 0.08,
        flicker_hz: 22.0,
        offset: Vec3::new(0.0, 0.65, 0.0),
        follow_particles: true,
    })
}

pub fn void_sigil_telegraph(radius: f32, duration: f32) -> Effect {
    let r = radius.max(0.5);
    let dur = duration.max(0.05);
    Effect {
        duration: dur,
        layers: vec![
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (dur, dur),
                forces: vec![],
                size: Curve::from_stops([(0.00, r * 1.25), (0.70, r * 1.72), (1.00, r * 2.08)]),
                color: Gradient::from_stops([
                    (0.00, [0.12, 0.03, 0.28, 0.16]),
                    (0.62, [0.24, 0.06, 0.54, 0.34]),
                    (1.00, [0.48, 0.10, 1.15, 0.58]),
                ]),
                sprite: SpriteShape::GroundCrack,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            }),
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (dur, dur),
                forces: vec![],
                size: Curve::from_stops([(0.00, r * 0.55), (0.70, r * 1.50), (1.00, r * 2.0)]),
                color: Gradient::from_stops([
                    (0.00, [0.45, 0.10, 1.45, 0.08]),
                    (0.72, [1.45, 0.36, 3.80, 0.28]),
                    (1.00, [3.30, 1.05, 6.80, 0.68]),
                ]),
                sprite: SpriteShape::Ring,
                blend: BlendMode::Additive,
                opacity: 0.95,
            }),
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Disc { radius: r * 0.72 },
                emission: EmissionMode::Continuous { rate: 72.0 },
                speed: (0.18, 0.75),
                lifetime: (0.34, 0.68),
                forces: vec![
                    ForceField::Wind {
                        velocity: Vec3::new(0.0, 0.38, 0.0),
                    },
                    ForceField::Curl {
                        frequency: 0.55,
                        strength: 4.2,
                    },
                    ForceField::Drag { coefficient: 3.2 },
                ],
                size: Curve::from_stops([(0.00, 0.18), (0.46, 0.42), (1.00, 0.06)]),
                color: Gradient::from_stops([
                    (0.00, [1.65, 0.58, 3.80, 0.34]),
                    (0.55, [0.62, 0.17, 1.95, 0.26]),
                    (1.00, [0.08, 0.02, 0.28, 0.00]),
                ]),
                sprite: SpriteShape::Smoke,
                blend: BlendMode::Alpha,
                opacity: 0.86,
            }),
        ],
    }
}

pub fn void_sigil_impact(radius: f32) -> EffectBundle {
    let r = radius.max(0.5);
    EffectBundle::new(Effect {
        duration: 0.05,
        layers: vec![
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.42, 0.42),
                forces: vec![],
                size: Curve::from_stops([(0.00, r * 1.60), (0.38, r * 2.22), (1.00, r * 2.55)]),
                color: Gradient::from_stops([
                    (0.00, [5.8, 2.5, 8.0, 0.95]),
                    (0.32, [2.5, 0.68, 5.5, 0.62]),
                    (1.00, [0.12, 0.04, 0.45, 0.00]),
                ]),
                sprite: SpriteShape::GroundCrack,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.45, 0.45),
                forces: vec![],
                size: Curve::from_stops([(0.00, r * 0.35), (0.34, r * 2.0), (1.00, r * 3.05)]),
                color: Gradient::from_stops([
                    (0.00, [4.8, 2.2, 8.0, 1.0]),
                    (0.42, [2.3, 0.55, 5.5, 0.72]),
                    (1.00, [0.18, 0.04, 0.60, 0.00]),
                ]),
                sprite: SpriteShape::Ring,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Disc { radius: r * 0.9 },
                emission: EmissionMode::Burst { count: 72 },
                speed: (2.2, 6.8),
                lifetime: (0.42, 0.86),
                forces: vec![
                    ForceField::Wind {
                        velocity: Vec3::new(0.0, 1.0, 0.0),
                    },
                    ForceField::Curl {
                        frequency: 0.6,
                        strength: 6.0,
                    },
                    ForceField::Drag { coefficient: 2.2 },
                ],
                size: Curve::from_stops([(0.00, 0.22), (0.42, 0.60), (1.00, 0.08)]),
                color: Gradient::from_stops([
                    (0.00, [2.7, 1.05, 5.4, 0.66]),
                    (0.55, [0.75, 0.20, 2.2, 0.40]),
                    (1.00, [0.05, 0.02, 0.18, 0.00]),
                ]),
                sprite: SpriteShape::Smoke,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            }),
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Ring {
                    radius: r * 0.45,
                    thickness: r * 0.35,
                },
                emission: EmissionMode::Burst { count: 58 },
                speed: (5.0, 11.0),
                lifetime: (0.28, 0.55),
                forces: vec![ForceField::Drag { coefficient: 1.4 }],
                size: Curve::from_stops([(0.00, 0.13), (0.45, 0.09), (1.00, 0.0)]),
                color: Gradient::from_stops([
                    (0.00, [5.2, 2.4, 8.0, 1.0]),
                    (0.55, [2.1, 0.55, 5.0, 0.75]),
                    (1.00, [0.16, 0.04, 0.42, 0.00]),
                ]),
                sprite: SpriteShape::Streak,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
        ],
    })
    .with_light(EffectLight {
        color: Vec3::new(2.2, 0.75, 4.8),
        radius: (r * 2.4).clamp(5.5, 10.5),
        intensity: 0.82,
        intensity_curve: Some(Curve::from_stops([
            (0.00, 1.0),
            (0.12, 0.82),
            (0.38, 0.38),
            (0.72, 0.12),
            (1.00, 0.0),
        ])),
        lifetime: None,
        flicker_amp: 0.10,
        flicker_hz: 16.0,
        offset: Vec3::new(0.0, 0.30, 0.0),
        follow_particles: true,
    })
}

fn planar_dir(dir: Vec3) -> Vec3 {
    let flat = Vec3::new(dir.x, 0.0, dir.z);
    if flat.length_squared() > 1.0e-4 {
        flat.normalize()
    } else {
        Vec3::Z
    }
}
