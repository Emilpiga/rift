//! Hub / rift portal presets.

use glam::Vec3;

use crate::renderer::vfx::spec::*;

/// Particle accompaniment for the dimensional rift mesh.
///
/// The mesh ([`crate::renderer::mesh::Mesh::portal_with_palette`])
/// + the `shadeRift` shader branch already do the heavy lifting
/// (wobbling silhouette, layered swirl, chromatic edge, dark-red
/// veins, ember tendrils). This preset's job is to extend the
/// rift's *physical presence* outward into the world: ash and
/// dust caught in its gravitational pull, embers torn off the
/// edge, dark debris tumbling along the rim, and a ring of
/// inward-falling motes that visibly accelerate as they near
/// the silhouette and vanish at the rim. **No bright gold
/// sparkle.** The rift wants to read as a wound in reality.
///
/// Four layers, all additive:
///
///   1. **Inward suction** — particles spawned on a wide outer
///      ring with *negative* radial speed (inward kick) plus a
///      tangential orbit, drag, and curl. Reads as the
///      surrounding air being consumed.
///   2. **Inward-falling ash** — pale-grey/charcoal motes drift
///      *toward* the rift centre and vanish near the rim.
///      Slower and finer than the suction layer; adds ambient
///      haze without monopolising attention.
///   3. **Ember spit-off** — sparse hot ember sparks born on the
///      rim with a slight outward kick, immediately yanked back
///      tangentially.
///   4. **Edge tendrils** — short-lived violent tendril flecks
///      with high curl-noise displacement; the silhouette
///      "tears" outward in flicker-bursts.
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
            // 1. Inward suction. Particles spawn on a wide
            //    *outer* ring (radius 2.2 m, well past the
            //    rift's silhouette) with a NEGATIVE radial
            //    speed — the spawn shape's outward direction
            //    is multiplied by a negative scalar, so each
            //    particle launches *inward* toward the rim.
            //    A weak tangential orbit curves the path into
            //    a spiral, drag tapers velocity as the
            //    particle approaches the rim, and an opacity
            //    curve fades the particle to nothing right
            //    when it would reach the silhouette. Reads as
            //    the surrounding air being inhaled.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::RingAxis {
                    radius: 2.20,
                    thickness: 0.80,
                    axis: ring_axis,
                },
                emission: EmissionMode::BurstAndContinuous {
                    burst: 24,
                    rate: 70.0,
                },
                // Negative range = inward initial velocity.
                speed: (-0.55, -0.25),
                lifetime: (1.4, 2.2),
                forces: vec![
                    // Tangential pull around the rift axis —
                    // bends straight inward paths into spirals,
                    // which reads more clearly as gravitational
                    // capture than radial fall.
                    ForceField::Orbit {
                        axis: ring_axis,
                        speed: 2.6,
                    },
                    // Light drag — the particle should *speed
                    // up* (visually) as the orbit tightens, so
                    // we keep drag low enough not to kill the
                    // motion before the particle reaches the
                    // rim.
                    ForceField::Drag { coefficient: 0.4 },
                    // Curl jitter so suction streams aren't
                    // perfectly clean spirals.
                    ForceField::Curl {
                        frequency: 1.4,
                        strength: 0.7,
                    },
                ],
                // Born small (mote-sized), grow slightly as
                // they accelerate, then collapse to a point
                // right when they're consumed.
                size: Curve::from_stops([
                    (0.0, 0.03),
                    (0.6, 0.06),
                    (0.95, 0.05),
                    (1.0, 0.005),
                ]),
                // Cool ash → warm crimson as the particle
                // approaches the rim's heat. Alpha ramps up
                // mid-life and snaps to 0 at end-of-life so
                // particles visibly "vanish into blackness"
                // rather than fading politely.
                color: Gradient::from_stops([
                    (0.0,  [0.45, 0.40, 0.36, 0.0]),
                    (0.25, [0.55, 0.42, 0.32, 0.40]),
                    (0.70, [1.10, 0.45, 0.18, 0.55]),
                    (0.95, [1.80, 0.35, 0.10, 0.35]),
                    (1.0,  [0.0,  0.0,  0.0,  0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 2. Inward-falling ash. Born on a slightly larger
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
                size: Curve::from_stops([
                    (0.0, 0.04),
                    (0.5, 0.09),
                    (1.0, 0.02),
                ]),
                // Cool charcoal/ash palette; very low alpha so
                // the layer reads as drifting haze, not bright
                // pixels. Slight warm tint at end-of-life as
                // the ash falls into the rift's heat.
                color: Gradient::from_stops([
                    (0.0, [0.35, 0.32, 0.30, 0.0]),
                    (0.3, [0.45, 0.38, 0.35, 0.30]),
                    (0.7, [0.55, 0.30, 0.20, 0.22]),
                    (1.0, [0.60, 0.10, 0.05, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 2. Ember spit-off. Born on the rift's rim with a
            //    small outward kick and yanked tangentially by
            //    a fast orbit. Drag is heavy so each ember
            //    only travels a few centimetres before fading
            //    — the visual is "the rift is shedding sparks
            //    every direction", not "fire-ring".
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::RingAxis {
                    radius: 1.18,
                    thickness: 0.05,
                    axis: ring_axis,
                },
                emission: EmissionMode::BurstAndContinuous {
                    burst: 12,
                    rate: 60.0,
                },
                speed: (0.10, 0.35),
                lifetime: (0.35, 0.75),
                forces: vec![
                    // Random direction per ember (some CW,
                    // some CCW) — the orbit force always points
                    // the same way around the axis, so we use a
                    // moderate value and let curl/drag decide
                    // the rest.
                    ForceField::Orbit {
                        axis: ring_axis,
                        speed: 4.0,
                    },
                    ForceField::Drag { coefficient: 4.0 },
                    ForceField::Curl {
                        frequency: 2.0,
                        strength: 0.6,
                    },
                ],
                size: Curve::from_stops([(0.0, 0.07), (1.0, 0.012)]),
                // Hot crimson-orange embers; not gold. HDR-
                // boosted so bloom catches them.
                color: Gradient::from_stops([
                    (0.0, [3.20, 1.10, 0.30, 0.95]),
                    (0.4, [2.00, 0.45, 0.10, 0.70]),
                    (1.0, [0.50, 0.04, 0.02, 0.0]),
                ]),
                sprite: SpriteShape::Spark,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 3. Edge tendrils. Sparse, large, fast curl-noise
            //    fragments born just outside the rim with an
            //    outward radial kick — they reach a few
            //    decimetres past the silhouette before drag
            //    and gravity pull them down. Reads as the
            //    rift "tearing" the air around it.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::RingAxis {
                    radius: 1.22,
                    thickness: 0.04,
                    axis: ring_axis,
                },
                emission: EmissionMode::BurstAndContinuous {
                    burst: 0,
                    rate: 22.0,
                },
                // Outward radial kick (RingAxis spawn dir =
                // radial), strong enough that tendrils flick
                // 0.3–0.5 m past the rim before curl smears
                // them.
                speed: (0.8, 1.6),
                lifetime: (0.40, 0.85),
                forces: vec![
                    ForceField::Drag { coefficient: 2.8 },
                    // Strong curl makes tendrils whip and
                    // wobble instead of flying straight,
                    // selling the "wound under tension" feel.
                    ForceField::Curl {
                        frequency: 1.6,
                        strength: 2.2,
                    },
                    // Slight downward bias — tendrils drift
                    // and fall instead of floating up like
                    // flame, reinforcing the gravitational /
                    // wound feel rather than fire.
                    ForceField::Gravity {
                        axis: -Vec3::Y,
                        strength: 0.6,
                    },
                ],
                size: Curve::from_stops([(0.0, 0.18), (1.0, 0.05)]),
                // Dark blood-red into deep crimson — wound
                // matter, not bright flame. The final stop
                // fades to near-black so the tendrils read as
                // dissipating, not extinguishing.
                color: Gradient::from_stops([
                    (0.0, [2.20, 0.30, 0.10, 0.85]),
                    (0.5, [1.40, 0.10, 0.05, 0.55]),
                    (1.0, [0.20, 0.02, 0.01, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
        ],
    }
}
