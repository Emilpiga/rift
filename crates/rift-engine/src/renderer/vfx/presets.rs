//! Named effect builders.
//!
//! Each `pub fn` returns a fresh [`Effect`] ready to feed
//! `VfxSystem::spawn`. Authoring a new ability visual lives here
//! — the gameplay code just spawns a preset and updates its
//! endpoints / anchor. The presets themselves are pure data with
//! no mutable state, so they're cheap to build at the call site.

use glam::Vec3;

use super::spec::*;

/// Frost Ray — a piercing cyan beam channeled from the caster's
/// hand. Built as a single ribbon layer with:
///
/// - HDR cyan core fading to transparent at the edges (cross
///   gradient with HDR > 1 in the centre to drive bloom)
/// - Slight fade-in at the hand and a soft fade right at the
///   tip so the impact point doesn't have a hard cap (length
///   gradient)
/// - Scrolling fbm noise along the beam for flow / shimmer
///
/// Width is set on the spec; length is implicit in the
/// (origin, tip) endpoints supplied by the gameplay layer.
/// Duration is `0.0` (infinite); the gameplay layer despawns on
/// channel end.
pub fn frost_ray() -> Effect {
    Effect {
        duration: 0.0,
        layers: vec![
            // Hand-base swirl: continuous cyan glow + a few sharper
            // sparks that orbit the caster's hand. Spawned at the
            // effect's anchor, which gameplay code refreshes every
            // frame via `set_anchor` so the swirl tracks the moving
            // hand joint.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Continuous { rate: 60.0 },
                speed: (0.3, 1.0),
                lifetime: (0.25, 0.55),
                forces: vec![
                    ForceField::Drag { coefficient: 3.0 },
                    ForceField::Gravity { axis: Vec3::Y, strength: 1.5 },
                    ForceField::Orbit { axis: Vec3::Y, speed: 6.0 },
                ],
                size: Curve::from_stops([
                    (0.00, 0.10),
                    (0.40, 0.14),
                    (1.00, 0.0),
                ]),
                color: Gradient::from_stops([
                    (0.00, [1.5, 3.0, 4.5, 1.0]),
                    (0.50, [0.6, 1.4, 2.2, 0.7]),
                    (1.00, [0.2, 0.4, 0.6, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            Layer::Ribbon(RibbonSpec {
                width: 0.45,
                cross_gradient: Gradient::from_stops([
                    (0.00, [0.30, 0.60, 0.90, 0.0]),
                    (0.20, [0.45, 0.85, 1.00, 0.6]),
                    (0.50, [4.00, 6.00, 8.00, 1.0]),
                    (0.80, [0.45, 0.85, 1.00, 0.6]),
                    (1.00, [0.30, 0.60, 0.90, 0.0]),
                ]),
                length_gradient: Some(Gradient::from_stops([
                    (0.00, [0.4, 0.4, 0.4, 0.6]),
                    (0.10, [1.0, 1.0, 1.0, 1.0]),
                    (0.85, [1.0, 1.0, 1.0, 1.0]),
                    (1.00, [0.6, 0.6, 0.6, 0.4]),
                ])),
                noise: Some(RibbonNoise {
                    tile: 0.5,
                    scroll: 4.0,
                    strength: 0.55,
                    octaves: 3,
                }),
                blend: BlendMode::Additive,
            }),
        ],
    }
}

/// Generic hit spark — a small cone of bright sparks fired in
/// the surface normal direction. Used for projectile impacts and
/// melee hits. Tuned to mirror the legacy
/// [`crate::renderer::particles::EmitterConfig::hit_spark`].
pub fn hit_spark(normal: Vec3) -> Effect {
    Effect {
        duration: 0.05,
        layers: vec![Layer::Particles(ParticleSpec {
            spawn: SpawnShape::Cone {
                axis: normal.normalize_or_zero(),
                half_angle: 0.6,
            },
            emission: EmissionMode::Burst { count: 18 },
            speed: (3.0, 6.5),
            lifetime: (0.18, 0.32),
            forces: vec![
                ForceField::Drag { coefficient: 4.0 },
                ForceField::Gravity {
                    axis: -Vec3::Y,
                    strength: 6.0,
                },
            ],
            size: Curve::from_stops([(0.0, 0.06), (1.0, 0.0)]),
            color: Gradient::from_stops([
                (0.00, [4.0, 3.0, 1.5, 1.0]),
                (0.40, [1.5, 0.8, 0.3, 0.8]),
                (1.00, [0.6, 0.2, 0.1, 0.0]),
            ]),
            sprite: SpriteShape::Spark,
            blend: BlendMode::Additive,
            opacity: 1.0,
        })],
    }
}

/// Frost-impact burst at the tip of a Frost Ray (or where a
/// piercing beam crosses a target). Cold blue puff plus a few
/// sharp shards.
pub fn frost_impact() -> Effect {
    Effect {
        duration: 0.05,
        layers: vec![
            // Soft cold puff
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 14 },
                speed: (1.0, 3.0),
                lifetime: (0.25, 0.45),
                forces: vec![ForceField::Drag { coefficient: 5.0 }],
                size: Curve::from_stops([(0.0, 0.18), (1.0, 0.05)]),
                color: Gradient::from_stops([
                    (0.0, [3.0, 5.5, 7.0, 0.9]),
                    (1.0, [0.2, 0.4, 0.6, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // Crystal shards
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 8 },
                speed: (3.0, 6.0),
                lifetime: (0.18, 0.28),
                forces: vec![ForceField::Drag { coefficient: 3.0 }],
                size: Curve::from_stops([(0.0, 0.10), (1.0, 0.0)]),
                color: Gradient::from_stops([
                    (0.0, [5.0, 7.0, 9.0, 1.0]),
                    (1.0, [0.3, 0.5, 0.7, 0.0]),
                ]),
                sprite: SpriteShape::Shard,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
        ],
    }
}

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

/// Caster bolt trail — enemy ranged-attack visual. Themed cool
/// violet / arcane to read distinctly from the player's hot
/// orange-red fireball trail. Same anchor-driven streak pattern
/// as [`fireball_trail`]: persistent (`duration = 0.0`), with the
/// gameplay layer re-anchoring to the projectile every frame
/// and despawning on hit.
///
/// Tuning targets:
///   * Inner core — saturated violet / magenta with a hot white
///     centre, smaller and tighter than the fireball core so the
///     bolt reads as a focused projectile rather than a roiling
///     ball of fire.
///   * Outer wake — dark indigo smoke that fades fast, so the
///     trail is shorter and more sinister than the fireball's
///     long warm tail.
pub fn caster_bolt_trail() -> Effect {
    Effect {
        duration: 0.0,
        layers: vec![
            // Inner arcane core. Hot violet-white embers fizzing
            // off the bolt body. Slightly slower-moving than the
            // fireball's so the trail visually compresses against
            // the projectile.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Continuous { rate: 180.0 },
                speed: (0.3, 1.0),
                lifetime: (0.16, 0.28),
                forces: vec![
                    ForceField::Drag { coefficient: 5.0 },
                ],
                size: Curve::from_stops([
                    (0.00, 0.18),
                    (0.30, 0.14),
                    (1.00, 0.03),
                ]),
                color: Gradient::from_stops([
                    (0.00, [3.6, 1.6, 4.2, 1.0]),
                    (0.40, [1.8, 0.4, 2.6, 0.85]),
                    (1.00, [0.3, 0.05, 0.6, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // Smoke wake — short-lived indigo puffs that hang in
            // the bolt's path briefly. Lower rate than the
            // fireball wake so the trail is wispier and gives
            // away less of the bolt's path.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Continuous { rate: 40.0 },
                speed: (0.05, 0.4),
                lifetime: (0.30, 0.55),
                forces: vec![
                    ForceField::Drag { coefficient: 2.5 },
                ],
                size: Curve::from_stops([
                    (0.00, 0.14),
                    (1.00, 0.32),
                ]),
                color: Gradient::from_stops([
                    (0.00, [0.6, 0.25, 1.0, 0.55]),
                    (0.50, [0.20, 0.10, 0.45, 0.30]),
                    (1.00, [0.05, 0.04, 0.10, 0.0]),
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

/// Caster bolt impact — a smaller, cooler counterpart to
/// `fireball_explosion()` keyed to the violet/indigo palette
/// of `caster_bolt_trail()` and `Mesh::caster_bolt`. Built as:
///   1. Bright lavender flash (very brief).
///   2. Outward arcane cloud — soft violet puffs that drift
///      slightly upward instead of falling, so it reads as
///      magical rather than incendiary.
///   3. Hard sparks — magenta motes that arc out without the
///      heavy gravity used by fireball embers.
///   4. Indigo shockwave ring on the ground.
/// Self-terminates after ~0.55 s once every layer's particles
/// have aged out.
pub fn caster_bolt_impact() -> Effect {
    Effect {
        duration: 0.05,
        layers: vec![
            // 1. Flash — single tight lavender puff.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.09, 0.11),
                forces: vec![],
                size: Curve::from_stops([
                    (0.00, 1.10),
                    (1.00, 1.80),
                ]),
                color: Gradient::from_stops([
                    (0.00, [5.0, 3.5, 6.5, 1.0]),
                    (1.00, [1.2, 0.4, 2.2, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 2. Arcane cloud — outward sphere of violet puffs.
            //    Slight upward drift instead of fireball's
            //    downward settle.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 28 },
                speed: (2.0, 5.0),
                lifetime: (0.35, 0.60),
                forces: vec![
                    ForceField::Drag { coefficient: 4.0 },
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 1.2,
                    },
                ],
                size: Curve::from_stops([
                    (0.00, 0.32),
                    (0.30, 0.55),
                    (1.00, 0.22),
                ]),
                color: Gradient::from_stops([
                    (0.00, [3.6, 1.4, 4.6, 1.0]),
                    (0.40, [1.6, 0.4, 2.4, 0.8]),
                    (1.00, [0.10, 0.04, 0.30, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 3. Sparks — fast magenta motes. Lighter gravity
            //    than fireball embers so they drift instead of
            //    raining down.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 22 },
                speed: (4.0, 9.0),
                lifetime: (0.28, 0.50),
                forces: vec![
                    ForceField::Drag { coefficient: 1.4 },
                    ForceField::Gravity {
                        axis: -Vec3::Y,
                        strength: 4.0,
                    },
                ],
                size: Curve::from_stops([
                    (0.00, 0.09),
                    (1.00, 0.0),
                ]),
                color: Gradient::from_stops([
                    (0.00, [4.5, 1.8, 5.0, 1.0]),
                    (0.50, [1.8, 0.4, 2.6, 0.9]),
                    (1.00, [0.20, 0.05, 0.40, 0.0]),
                ]),
                sprite: SpriteShape::Spark,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 4. Shockwave ring — indigo flat ring expanding
            //    along the ground plane.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.32, 0.32),
                forces: vec![],
                size: Curve::from_stops([
                    (0.00, 0.35),
                    (1.00, 2.60),
                ]),
                color: Gradient::from_stops([
                    (0.00, [2.4, 1.0, 3.6, 0.9]),
                    (1.00, [0.30, 0.10, 0.55, 0.0]),
                ]),
                sprite: SpriteShape::Ring,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
        ],
    }
}


/// rarity tint rising from the drop point. Persistent
/// (`duration = 0.0`); the gameplay layer despawns the effect
/// when the loot is picked up.
pub fn loot_beam(color: [f32; 3]) -> Effect {
    Effect {
        duration: 0.0,
        layers: vec![Layer::Particles(ParticleSpec {
            spawn: SpawnShape::Column {
                radius: 0.08,
                height: 0.0,
                axis: Vec3::Y,
            },
            emission: EmissionMode::BurstAndContinuous {
                burst: 32,
                rate: 150.0,
            },
            speed: (2.5, 5.0),
            lifetime: (0.8, 2.0),
            forces: vec![
                ForceField::Gravity {
                    axis: Vec3::Y,
                    strength: 3.5, // upward pull
                },
                ForceField::Drag { coefficient: 0.8 },
                ForceField::Orbit {
                    axis: Vec3::Y,
                    speed: 5.0,
                },
            ],
            size: Curve::from_stops([(0.0, 0.05), (1.0, 0.02)]),
            color: Gradient::from_stops([
                (0.0, [color[0] * 1.8, color[1] * 1.8, color[2] * 1.8, 1.0]),
                (1.0, [color[0] * 0.3, color[1] * 0.3, color[2] * 0.3, 0.0]),
            ]),
            sprite: SpriteShape::Spark,
            blend: BlendMode::Additive,
            opacity: 1.0,
        })],
    }
}

/// Loot pillar base pulse — a soft glow at the drop's feet.
/// Used in tandem with [`loot_beam`] for a clearer ground anchor.
pub fn loot_beam_base(color: [f32; 3]) -> Effect {
    Effect {
        duration: 0.0,
        layers: vec![Layer::Particles(ParticleSpec {
            spawn: SpawnShape::Sphere,
            emission: EmissionMode::BurstAndContinuous {
                burst: 6,
                rate: 25.0,
            },
            speed: (0.5, 1.5),
            lifetime: (0.3, 0.8),
            forces: vec![ForceField::Drag { coefficient: 2.0 }],
            size: Curve::from_stops([(0.0, 0.07), (1.0, 0.14)]),
            color: Gradient::from_stops([
                (0.0, [color[0] * 2.5, color[1] * 2.5, color[2] * 2.5, 1.0]),
                (1.0, [color[0] * 0.5, color[1] * 0.5, color[2] * 0.5, 0.0]),
            ]),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Additive,
            opacity: 1.0,
        })],
    }
}

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

/// Big visceral blood burst on death.
///
/// Three layers, all alpha-blended (blood does **not** glow,
/// so HDR is intentionally avoided here):
///
/// 1. **Spurt** — a single large, very dark crimson "splat" puff
///    that flashes at the kill point and fades fast. Sells the
///    moment of contact.
/// 2. **Droplets** — a wide dome of small fast droplets fired
///    upward + outward. Heavy gravity (~22 m/s²) and low drag,
///    so they arc, peak, and fall in <0.6 s — reading as wet
///    matter rather than fire embers.
/// 3. **Mist** — a few soft puffs of dark red haze that linger
///    for ~0.5 s and drift slightly upward. Smoky sprite ties
///    the burst together visually.
///
/// `up` is the world-space direction the spew should aim
/// (typically `Vec3::Y` — pass another vector for directional
/// hits, e.g. opposite of the projectile velocity).
pub fn blood_splatter(up: Vec3) -> Effect {
    let axis = if up.length_squared() > 1e-4 {
        up.normalize()
    } else {
        Vec3::Y
    };
    Effect {
        duration: 0.05,
        layers: vec![
            // 1. Initial dark crimson spurt — a few large soft
            //    puffs at the kill point.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 3 },
                speed: (0.0, 0.6),
                lifetime: (0.16, 0.22),
                forces: vec![ForceField::Drag { coefficient: 6.0 }],
                size: Curve::from_stops([
                    (0.00, 0.55),
                    (0.30, 0.85),
                    (1.00, 0.50),
                ]),
                // Deep, slightly-dark red. Alpha drops gracefully
                // without going additive — blood doesn't emit.
                color: Gradient::from_stops([
                    (0.00, [0.55, 0.04, 0.04, 0.95]),
                    (0.50, [0.35, 0.02, 0.02, 0.70]),
                    (1.00, [0.18, 0.01, 0.01, 0.00]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            }),
            // 2. Droplets — wide upward+outward cone, fast, heavy
            //    gravity. Low drag so they keep their momentum
            //    until gravity pulls them back down.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Cone {
                    axis,
                    half_angle: 1.05, // ~60° spread
                },
                emission: EmissionMode::Burst { count: 36 },
                speed: (4.0, 8.5),
                lifetime: (0.45, 0.75),
                forces: vec![
                    ForceField::Drag { coefficient: 1.2 },
                    ForceField::Gravity {
                        axis: -Vec3::Y,
                        strength: 22.0,
                    },
                ],
                size: Curve::from_stops([
                    (0.00, 0.07),
                    (0.85, 0.07),
                    (1.00, 0.0),
                ]),
                color: Gradient::from_stops([
                    (0.00, [0.62, 0.05, 0.05, 1.00]),
                    (0.70, [0.40, 0.03, 0.03, 0.95]),
                    (1.00, [0.20, 0.01, 0.01, 0.00]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            }),
            // 3. Mist — a few slow, soft, dark-red smoky puffs
            //    that drift up briefly and dissolve.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 6 },
                speed: (0.4, 1.2),
                lifetime: (0.45, 0.65),
                forces: vec![
                    ForceField::Drag { coefficient: 3.0 },
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 0.8,
                    },
                ],
                size: Curve::from_stops([
                    (0.00, 0.30),
                    (1.00, 0.55),
                ]),
                color: Gradient::from_stops([
                    (0.00, [0.30, 0.02, 0.02, 0.55]),
                    (1.00, [0.10, 0.01, 0.01, 0.00]),
                ]),
                sprite: SpriteShape::Smoke,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            }),
        ],
    }
}

/// Smaller blood spurt for non-fatal hits — fewer droplets, no
/// mist, lower velocity than [`blood_splatter`]. Cheap enough
/// to fire on every damage tick without overwhelming the screen.
///
/// `up` is the spew axis (typically `Vec3::Y`).
pub fn blood_hit_spurt(up: Vec3) -> Effect {
    let axis = if up.length_squared() > 1e-4 {
        up.normalize()
    } else {
        Vec3::Y
    };
    Effect {
        duration: 0.05,
        layers: vec![Layer::Particles(ParticleSpec {
            spawn: SpawnShape::Cone {
                axis,
                half_angle: 0.85,
            },
            emission: EmissionMode::Burst { count: 12 },
            speed: (2.5, 5.5),
            lifetime: (0.25, 0.45),
            forces: vec![
                ForceField::Drag { coefficient: 1.4 },
                ForceField::Gravity {
                    axis: -Vec3::Y,
                    strength: 18.0,
                },
            ],
            size: Curve::from_stops([
                (0.00, 0.06),
                (0.85, 0.06),
                (1.00, 0.0),
            ]),
            color: Gradient::from_stops([
                (0.00, [0.62, 0.05, 0.05, 1.00]),
                (0.70, [0.40, 0.03, 0.03, 0.90]),
                (1.00, [0.20, 0.01, 0.01, 0.00]),
            ]),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Alpha,
            opacity: 1.0,
        })],
    }
}

/// Wall-torch flame — a continuous, looping fire plume that sits
/// on a wall sconce. Three stacked layers compose the look:
///
/// 1. **Core flame**: short-lived bright HDR additive particles
///    rising fast. Keeps the flame's silhouette tight and drives
///    the bloom highlight.
/// 2. **Outer flame**: longer-lived softer particles that drift
///    upward and outward, giving the flame visible volume.
/// 3. **Smoke wisp**: dim translucent puff that lingers above
///    the flame, fading to nothing.
///
/// The effect is `duration: 0.0` (infinite) — gameplay code
/// despawns it when the floor changes. All forces are vertical
/// so the flame stays anchored to its wall position; the
/// `Wind` force adds a tiny upward bias so even slow particles
/// rise reliably.
pub fn wall_torch() -> Effect {
    Effect {
        duration: 0.0,
        layers: vec![
            // Core flame — small, very bright, short life.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Disc { radius: 0.04 },
                emission: EmissionMode::Continuous { rate: 55.0 },
                speed: (1.6, 2.4),
                lifetime: (0.18, 0.30),
                forces: vec![
                    // Upward acceleration so flames lick higher
                    // as they age (negative gravity along Y).
                    ForceField::Gravity { axis: Vec3::Y, strength: 4.5 },
                    ForceField::Drag { coefficient: 1.5 },
                    // Subtle curl gives the flame its dancing
                    // silhouette without expensive simulation.
                    ForceField::Curl { frequency: 4.0, strength: 1.6 },
                ],
                size: Curve::from_stops([
                    (0.00, 0.10),
                    (0.30, 0.16),
                    (1.00, 0.02),
                ]),
                // HDR amber → orange → dim red. Bright enough at
                // birth (~3-4×) to drive bloom; tonemap brings
                // the visible colour back to a clean orange.
                color: Gradient::from_stops([
                    (0.00, [4.5, 2.4, 0.6, 1.00]),
                    (0.40, [3.0, 1.0, 0.2, 1.00]),
                    (1.00, [0.6, 0.1, 0.0, 0.00]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // Outer flame — wider, dimmer, longer-lived. Reads
            // as the flame's volume / aura.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Disc { radius: 0.09 },
                emission: EmissionMode::Continuous { rate: 35.0 },
                speed: (0.7, 1.2),
                lifetime: (0.35, 0.55),
                forces: vec![
                    ForceField::Gravity { axis: Vec3::Y, strength: 2.8 },
                    ForceField::Drag { coefficient: 1.2 },
                    ForceField::Curl { frequency: 2.5, strength: 1.0 },
                ],
                size: Curve::from_stops([
                    (0.00, 0.16),
                    (0.40, 0.22),
                    (1.00, 0.04),
                ]),
                color: Gradient::from_stops([
                    (0.00, [2.5, 1.0, 0.20, 0.85]),
                    (0.50, [1.4, 0.45, 0.10, 0.55]),
                    (1.00, [0.30, 0.08, 0.02, 0.00]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // Smoke wisp — slow rising dim grey puff. Alpha-
            // blended so it can sit above the additive flame
            // without blowing out.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Disc { radius: 0.08 },
                emission: EmissionMode::Continuous { rate: 6.0 },
                speed: (0.25, 0.45),
                lifetime: (1.2, 1.8),
                forces: vec![
                    ForceField::Gravity { axis: Vec3::Y, strength: 1.0 },
                    ForceField::Drag { coefficient: 0.6 },
                    ForceField::Curl { frequency: 1.2, strength: 0.6 },
                ],
                size: Curve::from_stops([
                    (0.00, 0.10),
                    (0.50, 0.30),
                    (1.00, 0.55),
                ]),
                color: Gradient::from_stops([
                    (0.00, [0.10, 0.09, 0.08, 0.40]),
                    (0.40, [0.08, 0.07, 0.06, 0.20]),
                    (1.00, [0.04, 0.04, 0.04, 0.00]),
                ]),
                sprite: SpriteShape::Smoke,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            }),
        ],
    }

}
