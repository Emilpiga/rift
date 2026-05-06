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

/// Loot pillar — a continuous spiral of motes in the item's
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
