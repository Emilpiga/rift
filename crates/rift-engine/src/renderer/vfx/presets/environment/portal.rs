//! Hub / rift portal presets.

use glam::Vec3;

use crate::renderer::vfx::spec::*;

/// Hub-portal vortex — a layered swirling spectacle marking the
/// rift entrance. Three stacked particle layers:
///   1. **Rising column** — dense cyan-white motes spiralling
///      upward through the portal ring.
///   2. **Inward sparks** — bright spark sprites born on a wide
///      ring at ground level, dragged inward by orbit + drag,
///      reading as energy being sucked into the gate.
///   3. **Ground halo** — a slow cyan ring on the floor that
///      pulses outward, anchoring the portal to the world and
///      catching the eye from across the room.
pub fn portal_vortex() -> Effect {
    Effect {
        duration: 0.0,
        layers: vec![
            // 1. Rising column.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Column {
                    radius: 0.95,
                    height: 2.2,
                    axis: Vec3::Y,
                },
                emission: EmissionMode::BurstAndContinuous {
                    burst: 32,
                    rate: 90.0,
                },
                speed: (0.6, 1.8),
                lifetime: (0.9, 2.0),
                forces: vec![
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 0.4,
                    },
                    ForceField::Drag { coefficient: 0.5 },
                    ForceField::Orbit {
                        axis: Vec3::Y,
                        speed: 7.0,
                    },
                    ForceField::Curl {
                        frequency: 1.4,
                        strength: 0.6,
                    },
                ],
                size: Curve::from_stops([(0.0, 0.14), (0.4, 0.10), (1.0, 0.02)]),
                color: Gradient::from_stops([
                    (0.0, [1.6, 2.4, 3.2, 0.95]),
                    (0.5, [0.6, 1.4, 2.6, 0.7]),
                    (1.0, [0.2, 0.6, 1.8, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 2. Inward-orbiting sparks at the ring perimeter.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Ring {
                    radius: 1.25,
                    thickness: 0.05,
                },
                emission: EmissionMode::BurstAndContinuous {
                    burst: 0,
                    rate: 60.0,
                },
                speed: (0.2, 0.6),
                lifetime: (0.6, 1.2),
                forces: vec![
                    // Strong inward pull via heavy drag + orbit:
                    // the spawn ring's tangential motion combined
                    // with drag spirals particles toward the centre.
                    ForceField::Drag { coefficient: 1.6 },
                    ForceField::Orbit {
                        axis: Vec3::Y,
                        speed: 9.0,
                    },
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 0.8,
                    },
                ],
                size: Curve::from_stops([(0.0, 0.06), (1.0, 0.01)]),
                color: Gradient::from_stops([
                    (0.0, [2.4, 2.8, 3.2, 1.0]),
                    (1.0, [0.4, 1.0, 2.4, 0.0]),
                ]),
                sprite: SpriteShape::Spark,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 3. Ground halo pulse — slow soft ring that fades in
            //    place, painting a glowing footprint on the floor.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Ring {
                    radius: 1.4,
                    thickness: 0.1,
                },
                emission: EmissionMode::BurstAndContinuous {
                    burst: 0,
                    rate: 6.0,
                },
                speed: (0.0, 0.05),
                lifetime: (1.6, 2.4),
                forces: vec![ForceField::Drag { coefficient: 2.5 }],
                size: Curve::from_stops([(0.0, 0.6), (1.0, 1.4)]),
                color: Gradient::from_stops([
                    (0.0, [0.4, 1.0, 2.0, 0.6]),
                    (1.0, [0.1, 0.4, 1.4, 0.0]),
                ]),
                sprite: SpriteShape::Ring,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
        ],
    }
}
