//! Arcane / violet ranged-attack presets — used for caster
//! enemy projectiles.

use glam::Vec3;

use crate::renderer::vfx::spec::*;

/// Caster bolt trail — enemy ranged-attack visual. Themed cool
/// violet / arcane to read distinctly from the player's hot
/// orange-red fireball trail. Same anchor-driven streak pattern
/// as `fireball_trail`: persistent (`duration = 0.0`), with the
/// gameplay layer re-anchoring to the projectile every frame
/// and despawning on hit.
///
/// Tuning targets:
///   * Inner core — saturated violet / magenta with a hot white
///     centre, smaller and tighter than the fireball core so the
///     bolt reads as a focused projectile rather than a roiling
///     ball of fire.
///   * Outer wake — dark indigo smoke that fades fast, so the
///     trail is shorter and more sinister than the fireball's
///     long warm tail.
pub fn arcane_bolt_trail() -> Effect {
    Effect {
        duration: 0.0,
        layers: vec![
            // Inner arcane core. Hot violet-white embers fizzing
            // off the bolt body. Slightly slower-moving than the
            // fireball's so the trail visually compresses against
            // the projectile.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Continuous { rate: 180.0 },
                speed: (0.3, 1.0),
                lifetime: (0.16, 0.28),
                forces: vec![ForceField::Drag { coefficient: 5.0 }],
                size: Curve::from_stops([(0.00, 0.18), (0.30, 0.14), (1.00, 0.03)]),
                color: Gradient::from_stops([
                    (0.00, [3.6, 1.6, 4.2, 1.0]),
                    (0.40, [1.8, 0.4, 2.6, 0.85]),
                    (1.00, [0.3, 0.05, 0.6, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // Smoke wake — short-lived indigo puffs that hang in
            // the bolt's path briefly. Lower rate than the
            // fireball wake so the trail is wispier and gives
            // away less of the bolt's path.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Continuous { rate: 40.0 },
                speed: (0.05, 0.4),
                lifetime: (0.30, 0.55),
                forces: vec![ForceField::Drag { coefficient: 2.5 }],
                size: Curve::from_stops([(0.00, 0.14), (1.00, 0.32)]),
                color: Gradient::from_stops([
                    (0.00, [0.6, 0.25, 1.0, 0.55]),
                    (0.50, [0.20, 0.10, 0.45, 0.30]),
                    (1.00, [0.05, 0.04, 0.10, 0.0]),
                ]),
                sprite: SpriteShape::Smoke,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            }),
        ],
    }
}

/// Arcane bolt impact — a smaller, cooler counterpart to
/// `fireball_explosion()` keyed to the violet/indigo palette
/// of `arcane_bolt_trail()` and `Mesh::arcane_bolt`. Built as:
///   1. Bright lavender flash (very brief).
///   2. Outward arcane cloud — soft violet puffs that drift
///      slightly upward instead of falling, so it reads as
///      magical rather than incendiary.
///   3. Hard sparks — magenta motes that arc out without the
///      heavy gravity used by fireball embers.
///   4. Indigo shockwave ring on the ground.
/// Self-terminates after ~0.55 s once every layer's particles
/// have aged out.
pub fn arcane_bolt_impact() -> Effect {
    Effect {
        duration: 0.05,
        layers: vec![
            // 1. Flash — single tight lavender puff.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.09, 0.11),
                forces: vec![],
                size: Curve::from_stops([(0.00, 1.10), (1.00, 1.80)]),
                color: Gradient::from_stops([
                    (0.00, [5.0, 3.5, 6.5, 1.0]),
                    (1.00, [1.2, 0.4, 2.2, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 2. Arcane cloud — outward sphere of violet puffs.
            //    Slight upward drift instead of fireball's
            //    downward settle.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 28 },
                speed: (2.0, 5.0),
                lifetime: (0.35, 0.60),
                forces: vec![
                    ForceField::Drag { coefficient: 4.0 },
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 1.2,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.32), (0.30, 0.55), (1.00, 0.22)]),
                color: Gradient::from_stops([
                    (0.00, [3.6, 1.4, 4.6, 1.0]),
                    (0.40, [1.6, 0.4, 2.4, 0.8]),
                    (1.00, [0.10, 0.04, 0.30, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 3. Sparks — fast magenta motes. Lighter gravity
            //    than fireball embers so they drift instead of
            //    raining down.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 22 },
                speed: (4.0, 9.0),
                lifetime: (0.28, 0.50),
                forces: vec![
                    ForceField::Drag { coefficient: 1.4 },
                    ForceField::Gravity {
                        axis: -Vec3::Y,
                        strength: 4.0,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.09), (1.00, 0.0)]),
                color: Gradient::from_stops([
                    (0.00, [4.5, 1.8, 5.0, 1.0]),
                    (0.50, [1.8, 0.4, 2.6, 0.9]),
                    (1.00, [0.20, 0.05, 0.40, 0.0]),
                ]),
                sprite: SpriteShape::Spark,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 4. Shockwave ring — indigo flat ring expanding
            //    along the ground plane.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.32, 0.32),
                forces: vec![],
                size: Curve::from_stops([(0.00, 0.35), (1.00, 2.60)]),
                color: Gradient::from_stops([
                    (0.00, [2.4, 1.0, 3.6, 0.9]),
                    (1.00, [0.30, 0.10, 0.55, 0.0]),
                ]),
                sprite: SpriteShape::Ring,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
        ],
    }
}
