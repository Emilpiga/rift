//! Ground-slam VFX: a sustained wind-up telegraph ring + a
//! one-shot impact burst. Used by the boss Slam attack today,
//! reusable for any "telegraphed circle on the ground" attack
//! in either direction (player or enemy).
//!
//! Both presets take the slam radius (m) so the visual width
//! matches the gameplay radius the server resolves damage on.

use glam::Vec3;

use crate::renderer::vfx::builder::{particle, EffectBuilder};
use crate::renderer::vfx::spec::*;

/// Sustained wind-up telegraph: a bright orange-red ground ring
/// at the slam's danger radius, a faint danger fill, and rising
/// embers from the inside of the circle so the player can read
/// both the edge and the fill.
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
    EffectBuilder::timed(duration.max(0.05))
        .layers(vec![
            // 0. Ground fracture silhouette. This is the visual
            //    anchor for the cast: dark cracks and chipped
            //    scorch marks lying flat on the floor, so the
            //    slam reads as world damage instead of UI arcs.
            particle(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (duration.max(0.05), duration.max(0.05)),
                forces: vec![],
                size: Curve::from_stops([(0.00, r * 1.35), (0.45, r * 1.85), (1.00, r * 2.08)]),
                color: Gradient::from_stops([
                    (0.00, [0.08, 0.035, 0.018, 0.12]),
                    (0.62, [0.10, 0.045, 0.022, 0.30]),
                    (1.00, [0.14, 0.055, 0.026, 0.46]),
                ]),
                sprite: SpriteShape::GroundCrack,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
            // 0b. Heat in the fissures. Same flat mask, additive
            //     and much thinner in time: it brightens as the
            //     stomp nears instead of painting a solid orange
            //     disc for the whole wind-up.
            particle(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (duration.max(0.05), duration.max(0.05)),
                forces: vec![],
                size: Curve::from_stops([(0.00, r * 1.20), (0.72, r * 1.84), (1.00, r * 2.04)]),
                color: Gradient::from_stops([
                    (0.00, [0.80, 0.12, 0.025, 0.00]),
                    (0.55, [1.40, 0.30, 0.055, 0.12]),
                    (0.86, [3.80, 1.15, 0.18, 0.34]),
                    (1.00, [6.20, 2.25, 0.42, 0.68]),
                ]),
                sprite: SpriteShape::GroundCrack,
                blend: BlendMode::Additive,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
            // 1. Faint danger fill. This makes the slam read as
            //    an occupied zone instead of only a thin decal at
            //    the edge.
            particle(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (duration.max(0.05), duration.max(0.05)),
                forces: vec![],
                size: Curve::from_stops([(0.00, r * 1.45), (0.70, r * 1.75), (1.00, r * 1.95)]),
                color: Gradient::from_stops([
                    (0.00, [0.95, 0.12, 0.035, 0.08]),
                    (0.70, [1.20, 0.22, 0.055, 0.12]),
                    (1.00, [1.65, 0.40, 0.09, 0.18]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 0.70,
            hybrid: None,
        vfx_role: 0,
    }),
            // 2. Static ring at the danger edge. Single particle
            //    that lives the whole wind-up; sprite scale is
            //    set so the ring renders at full radius.
            particle(ParticleSpec {
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
                    (0.00, [1.80, 0.38, 0.08, 0.34]),
                    (0.70, [2.70, 0.78, 0.14, 0.46]),
                    (1.00, [3.80, 1.30, 0.24, 0.62]),
                ]),
                sprite: SpriteShape::Ring,
                blend: BlendMode::Additive,
                opacity: 0.78,
            hybrid: None,
        vfx_role: 0,
    }),
            // 3. Closing warning ring. Starts inside the danger
            //    circle and expands toward the edge as impact
            //    approaches, giving the player a readable timer.
            particle(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (duration.max(0.05), duration.max(0.05)),
                forces: vec![],
                size: Curve::from_stops([(0.00, r * 0.45), (0.70, r * 1.45), (1.00, r * 2.0)]),
                color: Gradient::from_stops([
                    (0.00, [1.50, 0.25, 0.055, 0.08]),
                    (0.75, [3.00, 0.72, 0.14, 0.24]),
                    (1.00, [5.80, 2.00, 0.34, 0.66]),
                ]),
                sprite: SpriteShape::Ring,
                blend: BlendMode::Additive,
                opacity: 0.72,
            hybrid: None,
        vfx_role: 0,
    }),
            // 4. Inside-the-circle embers — slow rising sparks
            //    seeded across the danger disc so the fill
            //    reads as "active" not just "outlined".
            particle(ParticleSpec {
                spawn: SpawnShape::Disc { radius: r * 0.95 },
                emission: EmissionMode::Continuous { rate: 45.0 * r },
                speed: (0.5, 1.5),
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
            hybrid: None,
        vfx_role: 0,
    }),
            // 5. Edge sparks — a hot crackle right on the radius
            //    line so the actual unsafe boundary is unmistakable.
            particle(ParticleSpec {
                spawn: SpawnShape::Ring {
                    radius: r,
                    thickness: 0.18,
                },
                emission: EmissionMode::Continuous { rate: 24.0 * r },
                speed: (0.2, 0.8),
                lifetime: (0.20, 0.38),
                forces: vec![ForceField::Drag { coefficient: 2.0 }],
                size: Curve::from_stops([(0.00, 0.13), (1.00, 0.0)]),
                color: Gradient::from_stops([
                    (0.00, [5.0, 2.0, 0.35, 0.95]),
                    (1.00, [0.8, 0.10, 0.02, 0.0]),
                ]),
                sprite: SpriteShape::Spark,
                blend: BlendMode::Additive,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
        ])
        .finish()
}

/// One-shot impact burst paired with [`ground_slam_telegraph`].
/// Built like the fireball detonation: a coherent stack of
/// pressure rings, central compression flash, dirty dust wall,
/// rock streaks, settling smoke, and a short-lived point light.
///
/// `radius` should match the radius the matching telegraph used
/// so the shockwave reads as "the danger ring just resolved".
pub fn ground_slam_impact(radius: f32) -> EffectBundle {
    let r = radius.max(0.5);
    EffectBuilder::oneshot()
        .layers(vec![
            // 0. Impact scorch decal — the ground keeps a brief,
            //    readable fracture silhouette after the flash.
            //    Alpha-blended, dark, and flat on XZ so it feels
            //    like damage to the room rather than a particle hoop.
            particle(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.72, 0.72),
                forces: vec![],
                size: Curve::from_stops([(0.00, r * 2.18), (0.18, r * 2.34), (1.00, r * 2.46)]),
                color: Gradient::from_stops([
                    (0.00, [0.16, 0.070, 0.032, 0.70]),
                    (0.42, [0.09, 0.050, 0.030, 0.46]),
                    (1.00, [0.035, 0.025, 0.020, 0.00]),
                ]),
                sprite: SpriteShape::GroundCrack,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
            // 0b. White-hot fissure flash. It shares the scorch
            //     mask but dies quickly, giving the stomp a crisp
            //     high-quality pop before smoke takes over.
            particle(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.24, 0.24),
                forces: vec![],
                size: Curve::from_stops([(0.00, r * 1.92), (0.38, r * 2.28), (1.00, r * 2.42)]),
                color: Gradient::from_stops([
                    (0.00, [7.20, 4.60, 1.65, 0.95]),
                    (0.25, [5.20, 1.85, 0.35, 0.68]),
                    (0.70, [1.45, 0.34, 0.08, 0.22]),
                    (1.00, [0.20, 0.05, 0.02, 0.00]),
                ]),
                sprite: SpriteShape::GroundCrack,
                blend: BlendMode::Additive,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
            // 1. Compression flash — a hot, low central burst.
            //    This is the stomp's equivalent of fireball's
            //    white-hot nucleus: the eye snaps to the origin,
            //    then the shock rings carry the motion outward.
            particle(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.10, 0.12),
                forces: vec![],
                size: Curve::from_stops([(0.00, r * 0.85), (0.45, r * 1.35), (1.00, r * 1.75)]),
                color: Gradient::from_stops([
                    (0.00, [5.8, 4.4, 2.2, 1.00]),
                    (0.45, [3.2, 1.45, 0.42, 0.70]),
                    (1.00, [0.65, 0.18, 0.06, 0.00]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
            // 2. Primary shock front — the gameplay radius made
            //    visible as a thick leading edge, then pushed just
            //    beyond the danger circle so the impact feels like
            //    mass leaving the ground.
            particle(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.42, 0.42),
                forces: vec![],
                size: Curve::from_stops([
                    (0.00, r * 0.35),
                    (0.24, r * 1.95),
                    (0.58, r * 2.35),
                    (1.00, r * 2.80),
                ]),
                color: Gradient::from_stops([
                    (0.00, [6.8, 4.2, 1.25, 1.00]),
                    (0.28, [5.4, 2.0, 0.42, 0.95]),
                    (0.70, [1.6, 0.42, 0.10, 0.38]),
                    (1.00, [0.22, 0.05, 0.02, 0.00]),
                ]),
                sprite: SpriteShape::Ring,
                blend: BlendMode::Additive,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
            // 3. Ground compression disc — an alpha-smoke layer
            //    that darkens and dirties the centre so the stomp
            //    has weight instead of only glowing orange.
            particle(ParticleSpec {
                spawn: SpawnShape::Disc { radius: r * 0.28 },
                emission: EmissionMode::Burst { count: 28 },
                speed: (0.4, 1.6),
                lifetime: (0.34, 0.58),
                forces: vec![
                    ForceField::Drag { coefficient: 3.4 },
                    ForceField::Curl {
                        frequency: 0.75,
                        strength: 4.0,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.42), (0.35, 0.95), (1.00, 1.55)]),
                color: Gradient::from_stops([
                    (0.00, [0.58, 0.30, 0.13, 0.52]),
                    (0.60, [0.20, 0.11, 0.07, 0.36]),
                    (1.00, [0.05, 0.03, 0.02, 0.00]),
                ]),
                sprite: SpriteShape::Smoke,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
            // 4. Secondary pressure ripple — a lower, wider ring
            //    with delayed brightness. This gives the stomp a
            //    bass-note aftershock instead of one flat sprite.
            particle(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.62, 0.62),
                forces: vec![],
                size: Curve::from_stops([
                    (0.00, r * 0.90),
                    (0.32, r * 1.75),
                    (0.70, r * 2.95),
                    (1.00, r * 3.45),
                ]),
                color: Gradient::from_stops([
                    (0.00, [3.0, 0.75, 0.12, 0.0]),
                    (0.22, [4.2, 1.25, 0.22, 0.48]),
                    (0.62, [1.5, 0.40, 0.10, 0.26]),
                    (1.00, [0.25, 0.06, 0.02, 0.0]),
                ]),
                sprite: SpriteShape::Ring,
                blend: BlendMode::Additive,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
            // 5. Dirty pressure wall — a ring of smoke born near
            //    the danger boundary, expanding and curling as if
            //    the floor shoved dust outward.
            particle(ParticleSpec {
                spawn: SpawnShape::Ring {
                    radius: r * 0.82,
                    thickness: r * 0.18,
                },
                emission: EmissionMode::Burst { count: 64 },
                speed: (2.4, 5.6),
                lifetime: (0.55, 0.95),
                forces: vec![
                    ForceField::Drag { coefficient: 2.0 },
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 0.35,
                    },
                    ForceField::Curl {
                        frequency: 0.45,
                        strength: 5.5,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.34), (0.45, 0.82), (1.00, 1.35)]),
                color: Gradient::from_stops([
                    (0.00, [0.55, 0.32, 0.16, 0.46]),
                    (0.55, [0.28, 0.16, 0.09, 0.34]),
                    (1.00, [0.06, 0.04, 0.03, 0.00]),
                ]),
                sprite: SpriteShape::Smoke,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
            // 6. Rock/ember streaks — fast radial debris. These
            //    are the stomp's equivalent of fireball embers:
            //    crisp motion lines that sell force and direction.
            particle(ParticleSpec {
                spawn: SpawnShape::Ring {
                    radius: r * 0.32,
                    thickness: r * 0.34,
                },
                emission: EmissionMode::Burst { count: 72 },
                speed: (8.0, 15.0),
                lifetime: (0.30, 0.58),
                forces: vec![
                    ForceField::Drag { coefficient: 1.15 },
                    ForceField::Gravity {
                        axis: -Vec3::Y,
                        strength: 12.0,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.16), (0.55, 0.11), (1.00, 0.0)]),
                color: Gradient::from_stops([
                    (0.00, [5.0, 3.2, 1.35, 1.0]),
                    (0.42, [2.3, 0.85, 0.18, 0.88]),
                    (1.00, [0.35, 0.08, 0.03, 0.0]),
                ]),
                sprite: SpriteShape::Streak,
                blend: BlendMode::Additive,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
            // 7. Ground grit — dull non-emissive particles thrown
            //    lower and slower than the hot streaks. This makes
            //    the hit feel physical, not magical-only.
            particle(ParticleSpec {
                spawn: SpawnShape::Ring {
                    radius: r * 0.42,
                    thickness: r * 0.55,
                },
                emission: EmissionMode::Burst { count: 52 },
                speed: (3.5, 8.0),
                lifetime: (0.42, 0.82),
                forces: vec![
                    ForceField::Drag { coefficient: 2.4 },
                    ForceField::Gravity {
                        axis: -Vec3::Y,
                        strength: 9.0,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.16), (0.55, 0.24), (1.00, 0.0)]),
                color: Gradient::from_stops([
                    (0.00, [0.55, 0.36, 0.20, 0.62]),
                    (0.65, [0.28, 0.18, 0.11, 0.38]),
                    (1.00, [0.06, 0.04, 0.03, 0.00]),
                ]),
                sprite: SpriteShape::Spark,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
            // 8. Low rolling smoke — the lingering body of the
            //    effect after the rings have gone, like dust being
            //    dragged along the floor by the shockwave.
            particle(ParticleSpec {
                spawn: SpawnShape::Disc { radius: r * 0.9 },
                emission: EmissionMode::Burst { count: 46 },
                speed: (0.6, 2.0),
                lifetime: (0.85, 1.35),
                forces: vec![
                    ForceField::Drag { coefficient: 2.8 },
                    ForceField::Wind {
                        velocity: Vec3::new(0.0, 0.18, 0.0),
                    },
                    ForceField::Curl {
                        frequency: 0.36,
                        strength: 3.2,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.45), (0.45, 0.92), (1.00, 1.35)]),
                color: Gradient::from_stops([
                    (0.00, [0.30, 0.18, 0.11, 0.36]),
                    (0.58, [0.16, 0.10, 0.07, 0.28]),
                    (1.00, [0.04, 0.03, 0.025, 0.00]),
                ]),
                sprite: SpriteShape::Smoke,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
        ])
    .with_light(EffectLight {
        color: Vec3::new(4.6, 2.35, 0.78),
        radius: (r * 2.15).clamp(5.0, 12.0),
        intensity: 0.95,
        intensity_curve: Some(Curve::from_stops([
            (0.00, 1.00),
            (0.08, 0.88),
            (0.24, 0.55),
            (0.45, 0.28),
            (0.72, 0.08),
            (1.00, 0.00),
        ])),
        lifetime: None,
        flicker_amp: 0.05,
        flicker_hz: 18.0,
        offset: Vec3::new(0.0, 0.35, 0.0),
        follow_particles: true,
    })
    .finish_bundle()
}
