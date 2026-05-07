//! Self-cast / movement spark presets — small one-shot
//! visuals that fire on the caster's body when activating
//! abilities or dodging.

use glam::Vec3;

use crate::renderer::vfx::spec::*;

/// Generic ability cast spark — a small omni-directional burst
/// in the caster's tinted ability colour. Replaces the legacy
/// `EmitterConfig::hit_spark(rgb)` call from the ability runtime.
pub fn cast_spark(rgb: [f32; 3]) -> Effect {
    Effect {
        duration: 0.05,
        layers: vec![Layer::Particles(ParticleSpec {
            spawn: SpawnShape::Sphere,
            emission: EmissionMode::Burst { count: 12 },
            speed: (3.0, 6.0),
            lifetime: (0.15, 0.4),
            forces: vec![
                ForceField::Drag { coefficient: 2.0 },
                ForceField::Gravity {
                    axis: -Vec3::Y,
                    strength: 8.0,
                },
            ],
            size: Curve::from_stops([(0.0, 0.08), (1.0, 0.0)]),
            color: Gradient::from_stops([
                (0.0, [rgb[0] * 1.4, rgb[1] * 1.4, rgb[2] * 1.4, 1.0]),
                (1.0, [rgb[0] * 0.5, rgb[1] * 0.5, rgb[2] * 0.5, 0.0]),
            ]),
            sprite: SpriteShape::Spark,
            blend: BlendMode::Additive,
            opacity: 1.0,
        })],
    }
}

/// Evasive-roll puff — a short-lived smoky burst left behind
/// when the player dodges. Replaces `EmitterConfig::dodge_puff`.
pub fn dodge_puff() -> Effect {
    Effect {
        duration: 0.05,
        layers: vec![Layer::Particles(ParticleSpec {
            spawn: SpawnShape::Sphere,
            emission: EmissionMode::Burst { count: 8 },
            speed: (1.0, 2.5),
            lifetime: (0.2, 0.4),
            forces: vec![
                ForceField::Drag { coefficient: 3.0 },
                ForceField::Gravity {
                    axis: Vec3::Y,
                    strength: 0.5, // gentle upward drift
                },
            ],
            size: Curve::from_stops([(0.0, 0.12), (1.0, 0.30)]),
            color: Gradient::from_stops([
                (0.0, [0.6, 0.8, 1.0, 0.6]),
                (1.0, [0.4, 0.6, 0.9, 0.0]),
            ]),
            sprite: SpriteShape::Smoke,
            blend: BlendMode::Alpha,
            opacity: 1.0,
        })],
    }
}
