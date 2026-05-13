//! Ground-slam VFX: a sustained wind-up telegraph ring + a
//! one-shot impact burst. Used by the boss Slam attack today,
//! reusable for any "telegraphed circle on the ground" attack
//! in either direction (player or enemy).
//!
//! Both presets take the slam radius (m) so the visual width
//! matches the gameplay radius the server resolves damage on.

use glam::Vec3;

use crate::renderer::vfx::spec::*;

/// Sustained wind-up telegraph: a bright orange-red ground ring
/// at the slam's danger radius, plus rising embers from the
/// inside of the circle so the player can read both the edge
/// and the fill.
///
/// `radius` is the slam radius in metres. The ring sprite is
/// scaled directly to that diameter; embers spawn on a disc of
/// the same radius.
///
/// `duration` is the wind-up length in seconds — the emitter
/// stops emitting at that point and the remaining particles
/// age out. Pass the same value the server uses for the slam
/// wind-up so the telegraph fades exactly at impact.
pub fn ground_slam_telegraph(radius: f32, duration: f32) -> Effect {
    let r = radius.max(0.5);
    Effect {
        duration: duration.max(0.05),
        layers: vec![
            // 1. Static ring at the danger edge. Single particle
            //    that lives the whole wind-up; sprite scale is
            //    set so the ring renders at full radius.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (duration.max(0.05), duration.max(0.05)),
                forces: vec![],
                // Ring sprite size is the *diameter*, so we feed
                // 2 * radius. Mild pulse via the size curve so
                // the ring isn't visually static.
                size: Curve::from_stops([(0.00, r * 2.0), (0.50, r * 2.10), (1.00, r * 2.0)]),
                // Hot orange — reads as "danger". Brightens
                // toward the impact moment so the eye is drawn
                // back to the circle just before resolution.
                color: Gradient::from_stops([
                    (0.00, [3.5, 0.8, 0.15, 0.85]),
                    (0.70, [4.5, 1.4, 0.25, 0.95]),
                    (1.00, [5.5, 2.0, 0.35, 1.00]),
                ]),
                sprite: SpriteShape::Ring,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 2. Inside-the-circle embers — slow rising sparks
            //    seeded across the danger disc so the fill
            //    reads as "active" not just "outlined".
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Disc { radius: r * 0.95 },
                emission: EmissionMode::Continuous { rate: 35.0 * r },
                speed: (0.4, 1.2),
                lifetime: (0.35, 0.65),
                forces: vec![
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 1.5,
                    },
                    ForceField::Drag { coefficient: 1.5 },
                ],
                size: Curve::from_stops([(0.00, 0.10), (1.00, 0.0)]),
                color: Gradient::from_stops([
                    (0.00, [3.5, 1.4, 0.30, 0.9]),
                    (1.00, [0.6, 0.10, 0.05, 0.0]),
                ]),
                sprite: SpriteShape::Spark,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
        ],
    }
}

/// One-shot impact burst paired with [`ground_slam_telegraph`].
/// Three layers: a ground shockwave ring at the slam radius, a
/// dusty outward ring of embers, and a brief central flash.
///
/// `radius` should match the radius the matching telegraph used
/// so the shockwave reads as "the danger ring just resolved".
pub fn ground_slam_impact(radius: f32) -> Effect {
    let r = radius.max(0.5);
    Effect {
        duration: 0.05,
        layers: vec![
            // 1. Shockwave ring — grows from a tight nucleus
            //    out past the slam radius and fades. Sells the
            //    "thump" at the moment of resolution.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.45, 0.45),
                forces: vec![],
                size: Curve::from_stops([(0.00, r * 0.4), (0.40, r * 2.0), (1.00, r * 2.6)]),
                color: Gradient::from_stops([
                    (0.00, [6.0, 3.0, 0.6, 1.0]),
                    (0.40, [3.5, 1.2, 0.2, 0.85]),
                    (1.00, [0.8, 0.10, 0.02, 0.0]),
                ]),
                sprite: SpriteShape::Ring,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 2. Dust + ember kickup — radial cone of particles
            //    flying outward from the slam centre.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Ring {
                    radius: r * 0.5,
                    thickness: r * 0.4,
                },
                emission: EmissionMode::Burst { count: 60 },
                speed: (5.0, 9.0),
                lifetime: (0.30, 0.55),
                forces: vec![
                    ForceField::Drag { coefficient: 1.6 },
                    ForceField::Gravity {
                        axis: -Vec3::Y,
                        strength: 8.0,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.12), (1.00, 0.0)]),
                color: Gradient::from_stops([
                    (0.00, [4.5, 2.0, 0.5, 1.0]),
                    (0.50, [2.0, 0.6, 0.10, 0.85]),
                    (1.00, [0.4, 0.05, 0.02, 0.0]),
                ]),
                sprite: SpriteShape::Spark,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 3. Central flash — short bright puff at the
            //    impact origin so the eye snaps back to the
            //    centre as the ring fans out.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.12, 0.16),
                forces: vec![],
                size: Curve::from_stops([(0.00, r * 0.6), (1.00, r * 1.0)]),
                color: Gradient::from_stops([
                    (0.00, [6.0, 4.5, 2.0, 1.0]),
                    (1.00, [2.0, 0.8, 0.2, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
        ],
    }
}
