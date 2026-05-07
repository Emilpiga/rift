//! Fire-themed combat presets: AoE flame waves, persistent
//! fireball trails, and detonation bursts.

use glam::Vec3;

use crate::renderer::vfx::spec::*;

/// Rain of Fire — a 2-second downpour of falling embers over
/// a 3 m radius column. Replaces `EmitterConfig::rain_of_fire`
/// driving the AoE-zone visual.
pub fn rain_of_fire() -> Effect {
    Effect {
        duration: 2.0,
        layers: vec![Layer::Particles(ParticleSpec {
            spawn: SpawnShape::Column {
                radius: 3.0,
                height: 0.5,
                axis: Vec3::Y,
            },
            emission: EmissionMode::BurstAndContinuous {
                burst: 14,
                rate: 90.0,
            },
            speed: (6.0, 10.0),
            lifetime: (1.4, 1.9),
            forces: vec![
                ForceField::Gravity {
                    axis: -Vec3::Y,
                    strength: 22.0,
                },
                ForceField::Drag { coefficient: 0.05 },
            ],
            size: Curve::from_stops([(0.0, 0.25), (1.0, 0.07)]),
            color: Gradient::from_stops([
                (0.0, [3.5, 1.6, 0.3, 1.0]),
                (0.5, [1.8, 0.4, 0.0, 0.9]),
                (1.0, [0.8, 0.05, 0.0, 0.0]),
            ]),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Additive,
            opacity: 1.0,
        })],
    }
}

/// Fire Wave — a one-shot, fast-expanding ring of fire centred
/// on the caster. Built to feel impactful: a flat ground
/// shockwave ring, an outward-rushing wall of flame, hot
/// embers thrown radially, and a brief column of smoke left
/// behind. All four layers self-terminate by ~0.7 s so the
/// preset can be used as a fire-and-forget client emitter.
pub fn fire_wave() -> Effect {
    Effect {
        // Stops emitting almost immediately; particles age out
        // through the rest of the visual.
        duration: 0.05,
        layers: vec![
            // 1. Ground shockwave — single hollow ring sprite
            //    that grows from a tight nucleus to the full
            //    wave radius. The Ring sprite is hollow so it
            //    reads as the leading edge of the wave rather
            //    than a fill disc.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.55, 0.55),
                forces: vec![],
                size: Curve::from_stops([
                    (0.00, 0.50),
                    (0.30, 6.00),
                    (1.00, 15.00),
                ]),
                color: Gradient::from_stops([
                    (0.00, [6.0, 3.2, 0.6, 1.0]),
                    (0.40, [3.5, 1.2, 0.2, 0.85]),
                    (1.00, [0.8, 0.10, 0.02, 0.0]),
                ]),
                sprite: SpriteShape::Ring,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 2. Flame wall — particles burst from a small ring
            //    on the ground, flying outward + slightly up.
            //    Reads as the body of the wave catching enemies
            //    as it sweeps out. Spawning on a `Ring` rather
            //    than a `Point` gives every particle a unique
            //    radial direction without us having to stamp
            //    one in code.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Ring {
                    radius: 0.6,
                    thickness: 0.4,
                },
                emission: EmissionMode::Burst { count: 80 },
                speed: (8.0, 13.0),
                lifetime: (0.35, 0.55),
                forces: vec![
                    ForceField::Drag { coefficient: 1.8 },
                    // Subtle upward bias so the flame plumes
                    // rise as they expand instead of skidding
                    // flat on the floor.
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 3.5,
                    },
                ],
                size: Curve::from_stops([
                    (0.00, 0.45),
                    (0.30, 0.85),
                    (1.00, 0.30),
                ]),
                color: Gradient::from_stops([
                    (0.00, [5.5, 2.6, 0.6, 1.0]),
                    (0.40, [3.0, 1.0, 0.2, 0.80]),
                    (1.00, [0.4, 0.06, 0.02, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 3. Embers — bright sparks thrown out radially with
            //    real gravity so they arc and land. Heavy
            //    particle count for a satisfying crunch on the
            //    impact frame.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Ring {
                    radius: 0.4,
                    thickness: 0.3,
                },
                emission: EmissionMode::Burst { count: 60 },
                speed: (10.0, 16.0),
                lifetime: (0.30, 0.55),
                forces: vec![
                    ForceField::Drag { coefficient: 0.8 },
                    ForceField::Gravity {
                        axis: -Vec3::Y,
                        strength: 14.0,
                    },
                ],
                size: Curve::from_stops([
                    (0.00, 0.13),
                    (1.00, 0.0),
                ]),
                color: Gradient::from_stops([
                    (0.00, [6.0, 4.0, 1.6, 1.0]),
                    (0.50, [2.5, 1.0, 0.25, 0.9]),
                    (1.00, [0.5, 0.10, 0.05, 0.0]),
                ]),
                sprite: SpriteShape::Spark,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 4. Smoke residue — slow alpha-blended puffs left
            //    behind once the flame fades. Sells the "this
            //    just happened" beat after the wave has passed.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Disc { radius: 1.2 },
                emission: EmissionMode::Burst { count: 18 },
                speed: (0.6, 1.2),
                lifetime: (0.7, 1.1),
                forces: vec![
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 1.2,
                    },
                    ForceField::Drag { coefficient: 0.5 },
                ],
                size: Curve::from_stops([
                    (0.00, 0.45),
                    (0.50, 0.85),
                    (1.00, 1.20),
                ]),
                color: Gradient::from_stops([
                    (0.00, [0.18, 0.13, 0.10, 0.55]),
                    (0.50, [0.12, 0.09, 0.07, 0.30]),
                    (1.00, [0.05, 0.04, 0.04, 0.0]),
                ]),
                sprite: SpriteShape::Smoke,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            }),
        ],
    }
}

/// Fireball trail — persistent emitter that follows the
/// projectile around. Two layers: an inner hot core of dense
/// embers that drift slightly outward + a cooler smoke wake
/// that lingers briefly to read as a tail. The gameplay layer
/// re-anchors this every frame to the projectile's position
/// via `set_anchor`, so the trail naturally streaks along the
/// flight path. Persistent (`duration = 0.0`); despawned when
/// the projectile detonates.
pub fn fireball_trail() -> Effect {
    Effect {
        duration: 0.0,
        layers: vec![
            // Inner ember core — hot white-yellow embers fizzing
            // outward off the projectile body. Short lifetime so
            // they hug the fireball.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Continuous { rate: 220.0 },
                speed: (0.4, 1.4),
                lifetime: (0.18, 0.32),
                forces: vec![
                    ForceField::Drag { coefficient: 4.5 },
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 2.0,
                    },
                ],
                size: Curve::from_stops([
                    (0.00, 0.22),
                    (0.30, 0.18),
                    (1.00, 0.04),
                ]),
                color: Gradient::from_stops([
                    (0.00, [4.5, 3.4, 1.4, 1.0]),
                    (0.40, [2.4, 1.2, 0.3, 0.9]),
                    (1.00, [0.8, 0.15, 0.05, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // Smoke wake — cooler, longer-lived puffs that get
            // left behind as the projectile races forward, giving
            // the impression of a streaking tail.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Continuous { rate: 60.0 },
                speed: (0.1, 0.6),
                lifetime: (0.45, 0.80),
                forces: vec![
                    ForceField::Drag { coefficient: 2.0 },
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 1.0,
                    },
                ],
                size: Curve::from_stops([
                    (0.00, 0.16),
                    (1.00, 0.45),
                ]),
                color: Gradient::from_stops([
                    (0.00, [1.6, 0.7, 0.2, 0.55]),
                    (0.50, [0.6, 0.25, 0.10, 0.30]),
                    (1.00, [0.10, 0.08, 0.08, 0.0]),
                ]),
                sprite: SpriteShape::Smoke,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            }),
        ],
    }
}

/// Fireball detonation — a one-shot explosion at the impact
/// point. Built from four layered bursts:
///   1. Bright white-hot flash (very brief).
///   2. Outward fireball cloud — large soft glow puffs.
///   3. Embers — fast sparks scattered radially.
///   4. Shockwave ring on the ground.
/// The whole effect self-terminates after ~0.5 s once every
/// layer's particles have aged out.
pub fn fireball_explosion() -> Effect {
    Effect {
        // Short non-zero duration: the system stops emitting and
        // self-removes once the longest-lived particles age out.
        duration: 0.05,
        layers: vec![
            // 1. Flash — single tight white-hot puff.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.10, 0.12),
                forces: vec![],
                size: Curve::from_stops([
                    (0.00, 1.40),
                    (1.00, 2.20),
                ]),
                color: Gradient::from_stops([
                    (0.00, [6.0, 5.5, 3.5, 1.0]),
                    (1.00, [2.0, 1.0, 0.4, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 2. Fireball cloud — outward sphere of large glowing
            //    puffs. Mostly horizontal so it reads as a
            //    ground-hugging fireball, with a touch of upward
            //    bias for the rising plume.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 36 },
                speed: (3.0, 7.0),
                lifetime: (0.35, 0.65),
                forces: vec![
                    ForceField::Drag { coefficient: 3.5 },
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 2.5,
                    },
                ],
                size: Curve::from_stops([
                    (0.00, 0.45),
                    (0.30, 0.70),
                    (1.00, 0.30),
                ]),
                color: Gradient::from_stops([
                    (0.00, [5.0, 3.0, 0.8, 1.0]),
                    (0.40, [2.5, 0.8, 0.2, 0.8]),
                    (1.00, [0.3, 0.05, 0.02, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 3. Embers — fast hard sparks that arc out and fall.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 28 },
                speed: (5.0, 11.0),
                lifetime: (0.30, 0.55),
                forces: vec![
                    ForceField::Drag { coefficient: 1.0 },
                    ForceField::Gravity {
                        axis: -Vec3::Y,
                        strength: 12.0,
                    },
                ],
                size: Curve::from_stops([
                    (0.00, 0.10),
                    (1.00, 0.0),
                ]),
                color: Gradient::from_stops([
                    (0.00, [5.0, 3.5, 1.5, 1.0]),
                    (0.50, [2.0, 0.8, 0.2, 0.9]),
                    (1.00, [0.5, 0.10, 0.05, 0.0]),
                ]),
                sprite: SpriteShape::Spark,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 4. Shockwave ring — a flat ring that expands and
            //    fades on the ground plane.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.30, 0.30),
                forces: vec![],
                size: Curve::from_stops([
                    (0.00, 0.40),
                    (1.00, 3.20),
                ]),
                color: Gradient::from_stops([
                    (0.00, [3.5, 2.0, 0.5, 0.9]),
                    (1.00, [0.6, 0.20, 0.05, 0.0]),
                ]),
                sprite: SpriteShape::Ring,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
        ],
    }
}
