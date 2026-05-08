//! Hub / rift portal presets.

use glam::Vec3;

use crate::renderer::vfx::spec::*;

/// Doctor-Strange-style portal halo. The mesh
/// ([`crate::renderer::mesh::Mesh::portal`]) provides a thin
/// gold ring with a near-black inner; this preset provides the
/// burning fire-circle that defines the look.
///
/// The portal mesh stands in the XY plane (axis = +Z), so the
/// halo layers spawn on a vertical [`SpawnShape::RingAxis`] with
/// `axis = +Z` and orbit around the same axis. Sparks therefore
/// circle the *visible* mesh ring rather than orbiting on the
/// floor at its base.
///
/// **Anchor:** The emitter is expected to be spawned at the
/// portal mesh's *centre* (i.e. `pos + Vec3::Y * PORTAL_CENTRE_Y`),
/// not at floor level. See [`PORTAL_CENTRE_Y`] and the spawn
/// helpers in `portal_system`.
///
/// Three layers, all additive:
///
///   1. **Outer halo (CCW)** — dense bright gold sparks orbiting
///      the rim along the ring's tangent, lifetime tuned so each
///      spark traces a bright arc before fading. Reads as a
///      fast continuous fire-ring.
///   2. **Counter-orbit halo (CW)** — sparser, slightly inside
///      the main halo, spinning the opposite way. Adds the
///      mandala/woven feel without making the silhouette messy.
///   3. **Outward flame licks** — sparks born on the rim with
///      enough outward speed to flick a few decimetres off the
///      ring before drag pulls them back. Curl noise smears
///      them into flame tongues.

/// Y-offset (in metres) from the portal entity's anchor to the
/// centre of the mesh ring. Mirrors the `cy_offset = height / 2`
/// constant inside [`crate::renderer::mesh::Mesh::portal`]. Used
/// by the spawn helpers in `portal_system` to anchor the VFX at
/// mesh centre instead of floor level so halo particles orbit
/// the *visible* ring.
pub const PORTAL_CENTRE_Y: f32 = 1.05;

pub fn portal_vortex() -> Effect {
    // Ring axis: portal mesh lies in XY, normal +Z.
    let ring_axis = Vec3::Z;
    Effect {
        duration: 0.0,
        layers: vec![
            // 1. Outer halo — bright gold, orbiting CCW around
            //    +Z. Heavy drag + small lifetime keeps each
            //    spark tightly tracking the rim instead of
            //    spiraling in or out.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::RingAxis {
                    radius: 1.15,
                    thickness: 0.04,
                    axis: ring_axis,
                },
                emission: EmissionMode::BurstAndContinuous {
                    burst: 64,
                    rate: 220.0,
                },
                speed: (0.2, 0.5),
                lifetime: (0.45, 0.8),
                forces: vec![
                    // Fast tangential pull around the ring axis —
                    // this is what turns the spawn ring into a
                    // moving halo of light.
                    ForceField::Orbit {
                        axis: ring_axis,
                        speed: 11.0,
                    },
                    // Strong drag prevents radial drift, so
                    // sparks stay glued to the rim.
                    ForceField::Drag { coefficient: 3.5 },
                    // Tiny curl jitter for liveliness.
                    ForceField::Curl {
                        frequency: 2.4,
                        strength: 0.4,
                    },
                ],
                // Born hot, taper to a fine point.
                size: Curve::from_stops([(0.0, 0.10), (0.4, 0.07), (1.0, 0.015)]),
                color: Gradient::from_stops([
                    (0.0, [3.6, 2.6, 0.9, 1.0]),   // white-hot gold
                    (0.4, [3.0, 1.6, 0.3, 0.95]),  // molten copper
                    (1.0, [1.4, 0.4, 0.05, 0.0]),  // dying ember
                ]),
                sprite: SpriteShape::Spark,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 2. Counter-orbit halo — sparser, slightly inside,
            //    spinning the opposite way. Gives the mandala
            //    weave without doubling spark count.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::RingAxis {
                    radius: 1.05,
                    thickness: 0.03,
                    axis: ring_axis,
                },
                emission: EmissionMode::BurstAndContinuous {
                    burst: 24,
                    rate: 90.0,
                },
                speed: (0.15, 0.35),
                lifetime: (0.5, 0.9),
                forces: vec![
                    // Negative speed = CW.
                    ForceField::Orbit {
                        axis: ring_axis,
                        speed: -8.0,
                    },
                    ForceField::Drag { coefficient: 3.0 },
                ],
                size: Curve::from_stops([(0.0, 0.08), (1.0, 0.012)]),
                color: Gradient::from_stops([
                    (0.0, [3.0, 2.0, 0.6, 0.9]),
                    (1.0, [1.0, 0.25, 0.03, 0.0]),
                ]),
                sprite: SpriteShape::Spark,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 3. Outward flame licks — sparks born on the rim
            //    with a small radial-out kick, smeared by curl
            //    noise into licking flame tongues. Drag pulls
            //    them back so the silhouette stays a tight ring.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::RingAxis {
                    radius: 1.18,
                    thickness: 0.02,
                    axis: ring_axis,
                },
                emission: EmissionMode::BurstAndContinuous {
                    burst: 0,
                    rate: 70.0,
                },
                // Speed multiplies the spawn shape's outward
                // normal; for `RingAxis` that's the radial
                // vector in the ring's plane, so this is the
                // flame-lick reach.
                speed: (0.6, 1.4),
                lifetime: (0.35, 0.7),
                forces: vec![
                    ForceField::Drag { coefficient: 2.5 },
                    // Curl makes each lick wobble instead of
                    // shooting straight out, reading as fire
                    // rather than radial sparks.
                    ForceField::Curl {
                        frequency: 1.8,
                        strength: 1.6,
                    },
                    // Slight upward bias so flame tongues
                    // curl up before fading.
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 1.2,
                    },
                ],
                size: Curve::from_stops([(0.0, 0.16), (1.0, 0.04)]),
                color: Gradient::from_stops([
                    (0.0, [3.4, 1.8, 0.4, 0.85]),
                    (0.6, [2.0, 0.6, 0.05, 0.5]),
                    (1.0, [0.4, 0.05, 0.01, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
        ],
    }
}
