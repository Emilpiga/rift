//! Loot-pillar presets — the rising rarity beam and its base
//! pulse anchor.

use glam::Vec3;

use crate::renderer::vfx::spec::*;

/// Rarity-tinted column rising from the drop point. Persistent
/// (`duration = 0.0`); the gameplay layer despawns the effect
/// when the loot is picked up.
pub fn loot_beam(color: [f32; 3]) -> Effect {
    Effect {
        duration: 0.0,
        layers: vec![Layer::Particles(ParticleSpec {
            spawn: SpawnShape::Column {
                radius: 0.08,
                height: 0.0,
                axis: Vec3::Y,
            },
            emission: EmissionMode::BurstAndContinuous {
                burst: 32,
                rate: 150.0,
            },
            speed: (2.5, 5.0),
            lifetime: (0.8, 2.0),
            forces: vec![
                ForceField::Gravity {
                    axis: Vec3::Y,
                    strength: 3.5, // upward pull
                },
                ForceField::Drag { coefficient: 0.8 },
                ForceField::Orbit {
                    axis: Vec3::Y,
                    speed: 5.0,
                },
            ],
            size: Curve::from_stops([(0.0, 0.05), (1.0, 0.02)]),
            color: Gradient::from_stops([
                (0.0, [color[0] * 1.8, color[1] * 1.8, color[2] * 1.8, 1.0]),
                (1.0, [color[0] * 0.3, color[1] * 0.3, color[2] * 0.3, 0.0]),
            ]),
            sprite: SpriteShape::Spark,
            blend: BlendMode::Additive,
            opacity: 1.0,
        })],
    }
}

/// Loot pillar base pulse — a soft glow at the drop's feet.
/// Used in tandem with [`loot_beam`] for a clearer ground anchor.
pub fn loot_beam_base(color: [f32; 3]) -> Effect {
    Effect {
        duration: 0.0,
        layers: vec![Layer::Particles(ParticleSpec {
            spawn: SpawnShape::Sphere,
            emission: EmissionMode::BurstAndContinuous {
                burst: 6,
                rate: 25.0,
            },
            speed: (0.5, 1.5),
            lifetime: (0.3, 0.8),
            forces: vec![ForceField::Drag { coefficient: 2.0 }],
            size: Curve::from_stops([(0.0, 0.07), (1.0, 0.14)]),
            color: Gradient::from_stops([
                (0.0, [color[0] * 2.5, color[1] * 2.5, color[2] * 2.5, 1.0]),
                (1.0, [color[0] * 0.5, color[1] * 0.5, color[2] * 0.5, 0.0]),
            ]),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Additive,
            opacity: 1.0,
        })],
    }
}

/// Anchored-loot halo: a slow gold-cyan ring orbiting the drop's
/// base. Spawned on top of [`loot_beam`] / [`loot_beam_base`] so
/// rarity stays readable while the unique trait is unmistakable
/// at gameplay distance. Persistent — caller despawns on pickup.
pub fn loot_anchored_halo() -> Effect {
    Effect {
        duration: 0.0,
        layers: vec![Layer::Particles(ParticleSpec {
            // Wide low ring so the halo reads as "anchor" and
            // doesn't compete with the vertical beam above.
            spawn: SpawnShape::Ring {
                radius: 0.6,
                thickness: 0.12,
            },
            emission: EmissionMode::BurstAndContinuous {
                burst: 12,
                rate: 60.0,
            },
            speed: (0.2, 0.5),
            lifetime: (1.2, 2.0),
            forces: vec![
                ForceField::Drag { coefficient: 1.5 },
                ForceField::Orbit {
                    axis: Vec3::Y,
                    speed: 1.4,
                },
            ],
            size: Curve::from_stops([(0.0, 0.10), (1.0, 0.04)]),
            // Gold → cyan gradient picks the trait out from any
            // rarity beam. Bright pre-multiply so the bloom
            // pass actually catches it.
            color: Gradient::from_stops([
                (0.0, [3.5, 2.6, 0.6, 1.0]),
                (0.5, [1.2, 2.4, 3.0, 1.0]),
                (1.0, [0.2, 0.6, 1.0, 0.0]),
            ]),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Additive,
            opacity: 1.0,
        })],
    }
}
