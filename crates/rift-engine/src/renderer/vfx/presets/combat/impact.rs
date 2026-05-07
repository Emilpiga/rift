//! Generic impact / hit visuals.

use glam::Vec3;

use crate::renderer::vfx::spec::*;

/// Generic hit spark — a small cone of bright sparks fired in
/// the surface normal direction. Used for projectile impacts and
/// melee hits. Tuned to mirror the legacy
/// [`crate::renderer::particles::EmitterConfig::hit_spark`].
pub fn hit_spark(normal: Vec3) -> Effect {
    Effect {
        duration: 0.05,
        layers: vec![Layer::Particles(ParticleSpec {
            spawn: SpawnShape::Cone {
                axis: normal.normalize_or_zero(),
                half_angle: 0.6,
            },
            emission: EmissionMode::Burst { count: 18 },
            speed: (3.0, 6.5),
            lifetime: (0.18, 0.32),
            forces: vec![
                ForceField::Drag { coefficient: 4.0 },
                ForceField::Gravity {
                    axis: -Vec3::Y,
                    strength: 6.0,
                },
            ],
            size: Curve::from_stops([(0.0, 0.06), (1.0, 0.0)]),
            color: Gradient::from_stops([
                (0.00, [4.0, 3.0, 1.5, 1.0]),
                (0.40, [1.5, 0.8, 0.3, 0.8]),
                (1.00, [0.6, 0.2, 0.1, 0.0]),
            ]),
            sprite: SpriteShape::Spark,
            blend: BlendMode::Additive,
            opacity: 1.0,
        })],
    }
}

/// Big visceral blood burst on death.
///
/// Three layers, all alpha-blended (blood does **not** glow,
/// so HDR is intentionally avoided here):
///
/// 1. **Spurt** — a single large, very dark crimson "splat" puff
///    that flashes at the kill point and fades fast. Sells the
///    moment of contact.
/// 2. **Droplets** — a wide dome of small fast droplets fired
///    upward + outward. Heavy gravity (~22 m/s²) and low drag,
///    so they arc, peak, and fall in <0.6 s — reading as wet
///    matter rather than fire embers.
/// 3. **Mist** — a few soft puffs of dark red haze that linger
///    for ~0.5 s and drift slightly upward. Smoky sprite ties
///    the burst together visually.
///
/// `up` is the world-space direction the spew should aim
/// (typically `Vec3::Y` — pass another vector for directional
/// hits, e.g. opposite of the projectile velocity).
pub fn blood_splatter(up: Vec3) -> Effect {
    let axis = if up.length_squared() > 1e-4 {
        up.normalize()
    } else {
        Vec3::Y
    };
    Effect {
        duration: 0.05,
        layers: vec![
            // 1. Initial dark crimson spurt — a few large soft
            //    puffs at the kill point.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 3 },
                speed: (0.0, 0.6),
                lifetime: (0.16, 0.22),
                forces: vec![ForceField::Drag { coefficient: 6.0 }],
                size: Curve::from_stops([
                    (0.00, 0.55),
                    (0.30, 0.85),
                    (1.00, 0.50),
                ]),
                // Deep, slightly-dark red. Alpha drops gracefully
                // without going additive — blood doesn't emit.
                color: Gradient::from_stops([
                    (0.00, [0.55, 0.04, 0.04, 0.95]),
                    (0.50, [0.35, 0.02, 0.02, 0.70]),
                    (1.00, [0.18, 0.01, 0.01, 0.00]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            }),
            // 2. Droplets — wide upward+outward cone, fast, heavy
            //    gravity. Low drag so they keep their momentum
            //    until gravity pulls them back down.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Cone {
                    axis,
                    half_angle: 1.05, // ~60° spread
                },
                emission: EmissionMode::Burst { count: 36 },
                speed: (4.0, 8.5),
                lifetime: (0.45, 0.75),
                forces: vec![
                    ForceField::Drag { coefficient: 1.2 },
                    ForceField::Gravity {
                        axis: -Vec3::Y,
                        strength: 22.0,
                    },
                ],
                size: Curve::from_stops([
                    (0.00, 0.07),
                    (0.85, 0.07),
                    (1.00, 0.0),
                ]),
                color: Gradient::from_stops([
                    (0.00, [0.62, 0.05, 0.05, 1.00]),
                    (0.70, [0.40, 0.03, 0.03, 0.95]),
                    (1.00, [0.20, 0.01, 0.01, 0.00]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            }),
            // 3. Mist — a few slow, soft, dark-red smoky puffs
            //    that drift up briefly and dissolve.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 6 },
                speed: (0.4, 1.2),
                lifetime: (0.45, 0.65),
                forces: vec![
                    ForceField::Drag { coefficient: 3.0 },
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 0.8,
                    },
                ],
                size: Curve::from_stops([
                    (0.00, 0.30),
                    (1.00, 0.55),
                ]),
                color: Gradient::from_stops([
                    (0.00, [0.30, 0.02, 0.02, 0.55]),
                    (1.00, [0.10, 0.01, 0.01, 0.00]),
                ]),
                sprite: SpriteShape::Smoke,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            }),
        ],
    }
}

/// Smaller blood spurt for non-fatal hits — fewer droplets, no
/// mist, lower velocity than [`blood_splatter`]. Cheap enough
/// to fire on every damage tick without overwhelming the screen.
///
/// `up` is the spew axis (typically `Vec3::Y`).
pub fn blood_hit_spurt(up: Vec3) -> Effect {
    let axis = if up.length_squared() > 1e-4 {
        up.normalize()
    } else {
        Vec3::Y
    };
    Effect {
        duration: 0.05,
        layers: vec![Layer::Particles(ParticleSpec {
            spawn: SpawnShape::Cone {
                axis,
                half_angle: 0.85,
            },
            emission: EmissionMode::Burst { count: 12 },
            speed: (2.5, 5.5),
            lifetime: (0.25, 0.45),
            forces: vec![
                ForceField::Drag { coefficient: 1.4 },
                ForceField::Gravity {
                    axis: -Vec3::Y,
                    strength: 18.0,
                },
            ],
            size: Curve::from_stops([
                (0.00, 0.06),
                (0.85, 0.06),
                (1.00, 0.0),
            ]),
            color: Gradient::from_stops([
                (0.00, [0.62, 0.05, 0.05, 1.00]),
                (0.70, [0.40, 0.03, 0.03, 0.90]),
                (1.00, [0.20, 0.01, 0.01, 0.00]),
            ]),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Alpha,
            opacity: 1.0,
        })],
    }
}
