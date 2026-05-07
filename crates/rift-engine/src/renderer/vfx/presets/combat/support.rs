//! Friendly support presets — heals, buffs, regen auras.
//!
//! These VFX live in their own module rather than mixed in
//! with combat impacts because they're paced very differently:
//! soft, sustained, and golden-green. Sharing space with
//! fireballs would invite tonal drift.

use glam::Vec3;

use crate::renderer::vfx::spec::*;

/// Heal-burst — single golden-green pulse played on a target
/// that just received an instant heal. Reads as a quick
/// upward shimmer rather than an impact.
pub fn heal_burst() -> Effect {
    Effect {
        // One-shot: stop emitting almost immediately, let the
        // particles age out.
        duration: 0.05,
        layers: vec![
            // Rising sparkles around the target's torso.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 28 },
                speed: (1.0, 2.5),
                lifetime: (0.45, 0.75),
                forces: vec![
                    ForceField::Drag { coefficient: 1.5 },
                    // Gentle upward drift — heal feels like it
                    // floats up rather than falling.
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 2.5,
                    },
                ],
                size: Curve::from_stops([(0.0, 0.10), (0.4, 0.18), (1.0, 0.0)]),
                color: Gradient::from_stops([
                    (0.0, [1.4, 2.6, 1.6, 1.0]),
                    (0.5, [0.9, 1.8, 1.1, 0.85]),
                    (1.0, [0.4, 1.0, 0.5, 0.0]),
                ]),
                sprite: SpriteShape::Spark,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // Soft halo ring at the feet — a single outward
            // pulse so the cast lands grounded.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.40, 0.40),
                forces: vec![],
                size: Curve::from_stops([
                    (0.00, 0.40),
                    (0.50, 1.60),
                    (1.00, 2.20),
                ]),
                color: Gradient::from_stops([
                    (0.00, [1.4, 2.4, 1.4, 0.85]),
                    (1.00, [0.4, 1.0, 0.5, 0.0]),
                ]),
                sprite: SpriteShape::Ring,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
        ],
    }
}

/// Heal-over-time aura — sustained gentle green sparkle that
/// stays on a target while a `Rejuvenation` buff is ticking.
/// Designed to be spawned on buff apply and visually persist
/// for the buff duration.
pub fn heal_over_time_aura() -> Effect {
    Effect {
        // 10-second buff — the engine doesn't yet stop emitters
        // on buff expiry, so we cap the emitter at the buff's
        // nominal length and let particles age out.
        duration: 10.0,
        layers: vec![Layer::Particles(ParticleSpec {
            // Spawning on a small sphere centred on the target
            // gives the sparkle a soft volume rather than a
            // single point.
            spawn: SpawnShape::Sphere,
            emission: EmissionMode::Continuous { rate: 14.0 },
            speed: (0.4, 1.2),
            lifetime: (0.6, 1.0),
            forces: vec![
                ForceField::Drag { coefficient: 1.2 },
                ForceField::Gravity {
                    axis: Vec3::Y,
                    strength: 1.4,
                },
            ],
            size: Curve::from_stops([(0.0, 0.06), (0.4, 0.10), (1.0, 0.0)]),
            color: Gradient::from_stops([
                (0.0, [0.8, 1.6, 0.9, 0.9]),
                (0.6, [0.5, 1.2, 0.7, 0.7]),
                (1.0, [0.3, 0.8, 0.4, 0.0]),
            ]),
            sprite: SpriteShape::Spark,
            blend: BlendMode::Additive,
            opacity: 1.0,
        })],
    }
}
