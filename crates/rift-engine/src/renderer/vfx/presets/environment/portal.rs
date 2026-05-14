//! Hub / rift portal presets.

use glam::Vec3;

use crate::renderer::vfx::spec::*;

/// Particle accompaniment for the dimensional rift mesh.
///
/// The mesh ([`crate::renderer::mesh::Mesh::portal_with_palette`])
/// + the `shadeRift` shader branch already do the heavy lifting
/// (wobbling silhouette, layered swirl, chromatic edge, dark-red
/// veins). This preset's job is to extend the rift's *physical
/// presence* outward into the world: ash and dust caught in its
/// gravitational pull. Fire-coded outer-edge layers are
/// intentionally absent — the read is **a tear in space-time**,
/// not a furnace.
///
/// One additive layer:
///
///   1. **Inward-falling ash** — pale-grey/charcoal motes drift
///      *toward* the rift centre and vanish near the rim.
///      Slower and finer than the suction layer; adds ambient
///      haze without monopolising attention.
///
/// **Anchor:** Spawned at `pos + Vec3::Y * PORTAL_CENTRE_Y`
/// (mesh centre), same as before — see [`PORTAL_CENTRE_Y`].

/// Y-offset (in metres) from the portal entity's anchor to the
/// centre of the mesh ring. Mirrors the `cy_offset = height / 2`
/// constant inside [`crate::renderer::mesh::Mesh::portal_with_palette`].
pub const PORTAL_CENTRE_Y: f32 = 1.05;

pub fn portal_vortex() -> Effect {
    // Ring axis: portal mesh lies in XY, normal +Z.
    let ring_axis = Vec3::Z;
    Effect {
        duration: 0.0,
        layers: vec![
            // 1. Inward-falling ash. Born on a slightly larger
            //    ring than the rift's silhouette and pulled
            //    tangentially+inward; lifetime long enough to
            //    streak across the disc before fading. Curl
            //    gives each particle a wobbly, drifting path
            //    rather than a clean radial line.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::RingAxis {
                    radius: 1.55,
                    thickness: 0.50,
                    axis: ring_axis,
                },
                emission: EmissionMode::BurstAndContinuous {
                    burst: 32,
                    rate: 90.0,
                },
                speed: (0.05, 0.20),
                lifetime: (1.6, 2.6),
                forces: vec![
                    // Slow CCW orbit — gives the ash a lazy
                    // gravitational swirl rather than a clean
                    // radial fall.
                    ForceField::Orbit {
                        axis: ring_axis,
                        speed: 1.4,
                    },
                    // Light drag so the curl noise dominates
                    // the trajectory shape.
                    ForceField::Drag { coefficient: 0.6 },
                    // Curl jitter — the dust looks like it's
                    // caught in turbulence.
                    ForceField::Curl {
                        frequency: 1.2,
                        strength: 0.8,
                    },
                ],
                // Fade in (born small, peak in mid-life, dim
                // out) — feels less like emitter-vomited
                // particles and more like ambient haze.
                size: Curve::from_stops([(0.0, 0.04), (0.5, 0.09), (1.0, 0.02)]),
                // Cool charcoal/ash palette; very low alpha so
                // the layer reads as drifting haze, not bright
                // pixels. No warm end-of-life tint, keeping the
                // rim from reading as flame.
                color: Gradient::from_stops([
                    (0.0, [0.24, 0.27, 0.34, 0.0]),
                    (0.3, [0.40, 0.44, 0.56, 0.26]),
                    (0.7, [0.46, 0.38, 0.62, 0.20]),
                    (1.0, [0.14, 0.10, 0.24, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
        ],
    }
}
