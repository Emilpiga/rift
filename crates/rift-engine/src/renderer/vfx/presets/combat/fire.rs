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
/// projectile around. Designed to read as a single cohesive
/// plume rather than "circles fading at a point":
///
///   * **Velocity inheritance** (0.78). Every spawned particle
///     inherits ~78% of the projectile's per-frame velocity,
///     so embers don't sit at the spawn point — they actually
///     trail behind the fireball, smearing the emission across
///     the flight path.
///   * **High-rate ember stream** using the new `Streak`
///     sprite. Streaks are anisotropic motion lines, so even a
///     single ember reads as motion. Combined with velocity
///     inheritance and the vertex-shader's screen-space
///     stretch, the trail becomes a continuous ribbon of fire.
///   * **Inner core ember halo** using `SoftGlow`, dense and
///     hot, hugging the projectile body so the head of the
///     trail looks like a nucleus rather than the start of a
///     line.
///   * **Soft smoke wake** with rotating `Smoke` puffs and a
///     gentle upward drift — the fade-out tail.
///   * **Attached crimson light** so the corridor walls,
///     enemies, and the player actually get illuminated as the
///     fireball flies past. The light follows the trail's
///     anchor (which is reset to the projectile position every
///     frame by `world_sync.rs`).
///
/// Persistent (`duration = 0.0`); despawned when the
/// projectile detonates.
pub fn fireball_trail() -> EffectBundle {
    EffectBundle::new(Effect {
        duration: 0.0,
        layers: vec![
            // 1. Inner ember nucleus — small, dense, hot
            //    SoftGlow particles spawned on a tight sphere.
            //    Short lifetime so they hug the projectile body
            //    and read as the *head* of the comet.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Continuous { rate: 280.0 },
                speed: (0.30, 0.90),
                lifetime: (0.10, 0.18),
                forces: vec![
                    ForceField::Drag { coefficient: 5.5 },
                ],
                size: Curve::from_stops([
                    (0.00, 0.18),
                    (0.30, 0.14),
                    (1.00, 0.02),
                ]),
                color: Gradient::from_stops([
                    (0.00, [5.5, 4.2, 1.8, 1.0]),
                    (0.40, [3.0, 1.4, 0.35, 0.9]),
                    (1.00, [0.9, 0.18, 0.05, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 2. Streaking ember tail — the body of the trail.
            //    Anisotropic Streak sprites, every one of them
            //    inheriting most of the projectile's velocity.
            //    Result: a dense, oriented stream of fire
            //    elongated along the flight path. This is the
            //    layer that sells the cohesive-plume read.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Continuous { rate: 220.0 },
                speed: (0.20, 0.80),
                lifetime: (0.22, 0.40),
                forces: vec![
                    ForceField::Drag { coefficient: 3.2 },
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 1.5,
                    },
                ],
                size: Curve::from_stops([
                    (0.00, 0.13),
                    (0.40, 0.16),
                    (1.00, 0.04),
                ]),
                color: Gradient::from_stops([
                    (0.00, [4.5, 2.8, 0.9, 1.0]),
                    (0.45, [2.2, 0.9, 0.20, 0.95]),
                    (1.00, [0.6, 0.10, 0.04, 0.0]),
                ]),
                sprite: SpriteShape::Streak,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 3. Smoke wake — slow alpha-blended puffs spawned
            //    behind the fireball as it moves. Inherits less
            //    velocity than the embers so it visibly *lags*
            //    the projectile, drifts up and outward, and
            //    erodes via the new noise-modulated Smoke
            //    sprite for a non-circular silhouette.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Continuous { rate: 70.0 },
                speed: (0.08, 0.45),
                lifetime: (0.55, 0.95),
                forces: vec![
                    ForceField::Drag { coefficient: 1.8 },
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 0.9,
                    },
                ],
                size: Curve::from_stops([
                    (0.00, 0.18),
                    (0.50, 0.32),
                    (1.00, 0.55),
                ]),
                color: Gradient::from_stops([
                    (0.00, [1.4, 0.55, 0.18, 0.55]),
                    (0.50, [0.45, 0.18, 0.08, 0.28]),
                    (1.00, [0.08, 0.06, 0.06, 0.0]),
                ]),
                sprite: SpriteShape::Smoke,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            }),
        ],
    })
    // Embers inherit most of the projectile's velocity so they
    // *trail* behind it instead of clustering at the spawn point.
    // The smoke layer also inherits this — that's fine, the
    // smoke's high drag pulls the inherited velocity off
    // quickly so it still reads as a stationary cloud lagging
    // behind the projectile.
    .with_inherit_velocity(0.78)
    // Crimson-orange point light following the projectile.
    // The intensity is gently flickered so the corridor
    // lighting reads as a live flame rather than a fixed
    // spotlight. Radius 5 m means the player sees walls and
    // enemies catch the warm spill before the fireball arrives,
    // a tell that an attack is incoming.
    .with_light(EffectLight {
        color: Vec3::new(3.6, 1.2, 0.30),
        radius: 5.0,
        intensity: 2.6,
        intensity_curve: None,
        flicker_amp: 0.18,
        flicker_hz: 11.0,
        offset: Vec3::ZERO,
    })
}

/// Fireball detonation — a one-shot explosion at the impact
/// point. Built to read as a *single coherent fireball* rather
/// than a spray of separate puffs:
///
///   1. **White-hot flash** — instantaneous bright nucleus,
///      drives the eye to the impact point.
///   2. **Outward fireball cloud** — many small `Smoke` sprites
///      with noise-eroded silhouettes, packed together so
///      individual sprites blend into one organic shape rather
///      than reading as a grid of squares. Per-particle
///      rotation (handled in the vertex shader) prevents
///      visual repetition between adjacent puffs.
///   3. **Hot ember sparks** — `Streak`-mode sparks fired
///      radially with real gravity, the new motion-streak
///      sprite producing oriented motion lines instead of
///      static crosses.
///   4. **Shockwave ring** on the ground.
///   5. **Attached point light** — a brief intense flash that
///      decays over the 0.5 s explosion lifetime, lighting
///      walls and enemies for the duration of the burst.
///
/// The whole effect self-terminates after ~0.65 s once every
/// layer's particles have aged out.
pub fn fireball_explosion() -> EffectBundle {
    EffectBundle::new(Effect {
        duration: 0.05,
        layers: vec![
            // 1. Flash — single tight white-hot puff. The
            //    intensity_curve on the attached light handles
            //    the brightness peak; this layer is just the
            //    visible nucleus.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.10, 0.12),
                forces: vec![],
                size: Curve::from_stops([
                    (0.00, 1.50),
                    (1.00, 2.40),
                ]),
                color: Gradient::from_stops([
                    (0.00, [6.5, 5.8, 3.8, 1.0]),
                    (1.00, [2.0, 1.0, 0.4, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 2. Bright fireball plasma — a tight ball of
            //    SoftGlow puffs that cluster around the impact
            //    point. Reads as the *core* of the fireball.
            //    Smaller than before so individual sprites
            //    don't read as squares; more numerous so they
            //    blend into a single mass.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 28 },
                speed: (1.5, 4.5),
                lifetime: (0.25, 0.45),
                forces: vec![
                    ForceField::Drag { coefficient: 4.5 },
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 3.0,
                    },
                ],
                size: Curve::from_stops([
                    (0.00, 0.30),
                    (0.30, 0.55),
                    (1.00, 0.12),
                ]),
                color: Gradient::from_stops([
                    (0.00, [5.5, 3.4, 1.0, 1.0]),
                    (0.40, [3.0, 1.0, 0.25, 0.85]),
                    (1.00, [0.30, 0.05, 0.02, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 3. Outer fireball cloud — the bulky smoke-and-
            //    fire body. Uses the noise-eroded `Smoke`
            //    sprite so silhouettes never repeat, with a
            //    very warm gradient so it reads as the burning
            //    body of the explosion rather than residue
            //    smoke. Many small puffs > a few big ones —
            //    fixes the "spawning squares" tell the user
            //    flagged.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 48 },
                speed: (3.5, 8.5),
                lifetime: (0.35, 0.65),
                forces: vec![
                    ForceField::Drag { coefficient: 3.2 },
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 2.0,
                    },
                ],
                size: Curve::from_stops([
                    (0.00, 0.35),
                    (0.40, 0.70),
                    (1.00, 0.95),
                ]),
                color: Gradient::from_stops([
                    (0.00, [4.0, 2.0, 0.55, 0.90]),
                    (0.40, [1.6, 0.55, 0.18, 0.65]),
                    (0.80, [0.45, 0.20, 0.10, 0.35]),
                    (1.00, [0.10, 0.07, 0.06, 0.0]),
                ]),
                sprite: SpriteShape::Smoke,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            }),
            // 4. Hot embers — fast radial streaks with gravity.
            //    The new `Streak` sprite + screen-space stretch
            //    in the vertex shader makes each spark read as
            //    a real motion line, not a dot.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 36 },
                speed: (6.0, 12.0),
                lifetime: (0.30, 0.55),
                forces: vec![
                    ForceField::Drag { coefficient: 1.0 },
                    ForceField::Gravity {
                        axis: -Vec3::Y,
                        strength: 14.0,
                    },
                ],
                size: Curve::from_stops([
                    (0.00, 0.12),
                    (1.00, 0.0),
                ]),
                color: Gradient::from_stops([
                    (0.00, [5.5, 3.8, 1.6, 1.0]),
                    (0.50, [2.4, 0.9, 0.20, 0.9]),
                    (1.00, [0.5, 0.10, 0.05, 0.0]),
                ]),
                sprite: SpriteShape::Streak,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 5. Shockwave ring — flat ring on the ground that
            //    expands and fades. The single Ring sprite gets
            //    a rim highlight in the new fragment shader so
            //    the leading edge reads as an antialiased band.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.30, 0.30),
                forces: vec![],
                size: Curve::from_stops([
                    (0.00, 0.40),
                    (1.00, 3.40),
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
    })
    // Bright initial flash decaying over the explosion's life.
    // The intensity curve is sampled over `elapsed / duration`
    // — but `duration` here is the *spawning* duration (0.05s),
    // so the curve effectively pegs at t=1 immediately. We
    // instead lean on a flat intensity + the natural lifetime
    // of the longest-lived particles to hold the flash for
    // ~0.5 s, then the slot self-frees. To get a proper decay
    // we would need a separate light-only duration, but for
    // now: bright base + 25 Hz nervous flicker for liveliness.
    .with_light(EffectLight {
        color: Vec3::new(5.0, 2.4, 0.7),
        radius: 8.0,
        intensity: 5.0,
        intensity_curve: None,
        flicker_amp: 0.20,
        flicker_hz: 22.0,
        offset: Vec3::new(0.0, 0.4, 0.0),
    })
}
