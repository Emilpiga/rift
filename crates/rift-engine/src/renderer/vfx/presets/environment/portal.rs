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
/// gravitational pull, and a cold chromatic shimmer flickering
/// along the seam where reality is split. Fire-coded layers
/// (warm-orange embers, hot tendril sparks) have been removed —
/// the new read is **a tear in space-time**, not a furnace.
///
/// Three layers, all additive:
///
///   1. **Inward suction** — particles spawned on a wide outer
///      ring with *negative* radial speed (inward kick) plus a
///      tangential orbit, drag, and curl. Reads as the
///      surrounding air being consumed.
///   2. **Inward-falling ash** — pale-grey/charcoal motes drift
///      *toward* the rift centre and vanish near the rim.
///      Slower and finer than the suction layer; adds ambient
///      haze without monopolising attention.
///   3. **Rim fracture shimmer** — short, cold, anisotropic
///      streaks spawned right on the rim that whip tangentially
///      along the seam in both directions. Cool white-violet
///      with HDR-boosted highlights so bloom catches them as
///      flickering chromatic cracks rather than warm sparks.
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
            // 3. Rim fracture shimmer. Thin anisotropic streaks
            //    spawned right on the silhouette and *flung
            //    tangentially* along the rim by a strong orbit
            //    force, then yanked back by heavy drag so each
            //    streak only lives a fraction of a second and
            //    travels a few centimetres. The motion direction
            //    is along the seam (not radial), and the colour
            //    is cool white with a violet edge — the cue is
            //    "reality is fracturing along this line", not
            //    "fire is leaking out of a hole".
            //
            //    Two passes back-to-back at half rate apiece
            //    with opposite orbit signs would be ideal but
            //    `Orbit` only takes a single signed scalar; the
            //    high curl frequency randomises the per-particle
            //    direction enough that the shimmer reads
            //    bidirectional anyway.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::RingAxis {
                    radius: 1.18,
                    // Razor-thin spawn band so streaks land
                    // precisely on the silhouette, not floating
                    // inside or outside it. The rim is the
                    // visual *seam* the fracture lives on.
                    thickness: 0.015,
                    axis: ring_axis,
                },
                emission: EmissionMode::BurstAndContinuous {
                    burst: 6,
                    rate: 80.0,
                },
                // Outward radial kick is small — the orbit
                // force does almost all the work. Without
                // any radial speed the streak would have
                // zero motion at t=0 and the `Streak` sprite
                // would render as a degenerate dot for the
                // first frame. A few cm/s gets it moving
                // before the orbit dominates.
                speed: (0.04, 0.10),
                // Very short life — the shimmer is a *flicker*
                // along the seam, not a streamer. Multiple
                // overlapping flickers at 80/s read as a
                // continuous restless edge.
                lifetime: (0.18, 0.40),
                forces: vec![
                    // Strong tangential orbit. This is what
                    // gives each streak its primary motion
                    // vector; the `Streak` sprite then
                    // orients along that vector, so the
                    // visible shape is a thin line *along
                    // the rim*.
                    ForceField::Orbit {
                        axis: ring_axis,
                        speed: 6.5,
                    },
                    // Heavy drag so the streak collapses
                    // before it can travel far enough to
                    // visibly orbit the disc — we want
                    // a flicker, not a planet.
                    ForceField::Drag { coefficient: 7.0 },
                    // High-frequency curl noise — without it
                    // every streak orbits in the same
                    // direction (the orbit force is signed)
                    // and the rim reads as a clockwise
                    // conveyor belt. The curl scrambles
                    // velocity per-particle so half end up
                    // going CW, half CCW, and a few flick
                    // outward briefly before drag eats them.
                    ForceField::Curl {
                        frequency: 3.0,
                        strength: 4.0,
                    },
                ],
                // Born thin, briefly stretches as the orbit
                // force accelerates it, then collapses to
                // pixel-width as drag wins. The `Streak`
                // sprite renders this as a tapered line.
                size: Curve::from_stops([
                    (0.0, 0.06),
                    (0.35, 0.14),
                    (1.0, 0.02),
                ]),
                // Cool chromatic palette. Cold white core with
                // a violet/cyan tint — the colour signature of
                // *reality fracturing*, distinct from any warm
                // light source in the scene. HDR-boosted on
                // all three channels so bloom paints the
                // shimmer as a hot rim highlight rather than a
                // muted blue-grey. Final stop fades to deep
                // violet (not black) so the streak's tail
                // dissipates with the same chromatic signature
                // as its head.
                color: Gradient::from_stops([
                    (0.0,  [1.40, 1.60, 2.40, 0.0]),
                    (0.20, [2.40, 2.20, 3.20, 0.95]),
                    (0.65, [1.10, 0.55, 1.80, 0.65]),
                    (1.0,  [0.30, 0.10, 0.45, 0.0]),
                ]),
                // `Streak` orients along velocity → reads as a
                // tangent-aligned shard of light along the
                // rim, exactly the "tear in fabric" cue.
                sprite: SpriteShape::Streak,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
        ],
    }
}
