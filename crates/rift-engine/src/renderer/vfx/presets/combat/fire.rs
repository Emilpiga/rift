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
    // Softly modulated (low-amplitude, low-frequency flicker)
    // so the corridor lighting reads as a live flame rather
    // than a fixed spotlight, but never dips noticeably. The
    // light has no `lifetime` set: it tracks the trail's
    // particle pool, so when the projectile detonates and the
    // trail is `despawn`ed, the light naturally fades out as
    // the last embers age out. The bright impact light belongs
    // to `fireball_explosion`, which spawns at the same world
    // point at detonation and overlaps cleanly.
    .with_light(EffectLight {
        color: Vec3::new(3.6, 1.2, 0.30),
        radius: 5.5,
        intensity: 2.8,
        intensity_curve: None,
        lifetime: None,
        flicker_amp: 0.06,
        flicker_hz: 4.0,
        offset: Vec3::ZERO,
        // Track the live ember population. While the
        // projectile is in flight the trail spawns
        // continuously and the envelope sits at peak, so the
        // light reads as a steady comet glow. The instant
        // the trail is `despawn`ed (on impact / expiry /
        // wall collision) `inst.spawning` flips false and
        // the runtime's exponential envelope decay (~0.85 s
        // half-life) takes over — the corridor glow softly
        // fades out instead of cutting off the moment the
        // last continuously-spawned ember dies. Without
        // this, the light went from full intensity to zero
        // in a single frame and read as a hard pop.
        follow_particles: true,
        heat_haze: false,
    })
}

/// Fireball detonation — a one-shot explosion at the impact
/// point. Built to read as a *single coherent fireball* rather
/// than a spray of separate puffs:
///
///   1. **White-hot flash** — instantaneous bright nucleus,
///      drives the eye to the impact point.
///   2. **Hot core plasma** — densely-packed `SoftGlow` puffs
///      with curl-noise turbulence, so the core *swirls*
///      instead of just expanding radially.
///   3. **Outer fireball cloud** — many small `Smoke` sprites
///      with noise-eroded silhouettes and curl-driven motion,
///      packed together so individual sprites blend into one
///      organic shape rather than reading as a grid of
///      squares.
///   4. **Hot ember sparks** — `Streak`-mode sparks fired
///      radially with real gravity.
///   5. **Shockwave ring** on the ground.
///   6. **Attached point light** with its own lifetime curve:
///      ramp up to peak in 30 ms, hold briefly, decay over
///      ~600 ms. The light persists past the particle pool so
///      the explosion's afterglow reads cleanly even after
///      the visible fire has dissipated.
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
            // 2. Hot core plasma — a tight ball of SoftGlow
            //    puffs that swirl around the impact point via
            //    curl-noise. Reads as the *roiling core* of
            //    the fireball, not a static cluster.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 32 },
                speed: (1.5, 4.5),
                lifetime: (0.30, 0.55),
                forces: vec![
                    ForceField::Drag { coefficient: 4.0 },
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 3.0,
                    },
                    // Internal turbulence: smooth divergence-
                    // driven motion that makes each puff
                    // wander instead of moving on a straight
                    // line. Cheap (5 hashes/particle/tick).
                    ForceField::Curl {
                        frequency: 0.9,
                        strength: 14.0,
                    },
                ],
                size: Curve::from_stops([
                    (0.00, 0.30),
                    (0.30, 0.60),
                    (1.00, 0.14),
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
            // 3. Outer fireball cloud — bulky smoke-and-fire
            //    body. Many small puffs with curl-driven
            //    motion + noise-eroded silhouettes (the new
            //    3-octave Smoke sprite) so the cloud reads as
            //    a single organic mass instead of a grid of
            //    squares.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 56 },
                speed: (3.5, 8.5),
                lifetime: (0.45, 0.80),
                forces: vec![
                    ForceField::Drag { coefficient: 2.8 },
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 1.8,
                    },
                    ForceField::Curl {
                        frequency: 0.6,
                        strength: 8.0,
                    },
                ],
                size: Curve::from_stops([
                    (0.00, 0.32),
                    (0.40, 0.75),
                    (1.00, 1.05),
                ]),
                color: Gradient::from_stops([
                    (0.00, [4.0, 2.0, 0.55, 0.92]),
                    (0.30, [2.2, 0.85, 0.25, 0.78]),
                    (0.70, [0.55, 0.22, 0.10, 0.40]),
                    (1.00, [0.10, 0.07, 0.06, 0.0]),
                ]),
                sprite: SpriteShape::Smoke,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            }),
            // 4. Hot embers — fast radial streaks with gravity.
            //    The `Streak` sprite + screen-space stretch in
            //    the vertex shader makes each spark read as a
            //    real motion line, not a dot.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 40 },
                speed: (6.0, 13.0),
                lifetime: (0.35, 0.65),
                forces: vec![
                    ForceField::Drag { coefficient: 1.0 },
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
                    (0.00, [5.5, 3.8, 1.6, 1.0]),
                    (0.50, [2.4, 0.9, 0.20, 0.9]),
                    (1.00, [0.5, 0.10, 0.05, 0.0]),
                ]),
                sprite: SpriteShape::Streak,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 5. Shockwave ring — flat ring on the ground that
            //    expands and fades. The Ring sprite gets a rim
            //    highlight in the fragment shader so the
            //    leading edge reads as an antialiased band.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.35, 0.35),
                forces: vec![],
                size: Curve::from_stops([
                    (0.00, 0.40),
                    (1.00, 3.60),
                ]),
                color: Gradient::from_stops([
                    (0.00, [3.8, 2.2, 0.6, 0.95]),
                    (1.00, [0.6, 0.20, 0.05, 0.0]),
                ]),
                sprite: SpriteShape::Ring,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
        ],
    })
    // Impact light driven by `follow_particles`: intensity
    // tracks the effect's live-particle population, so the
    // light is at peak the instant the impact has spawned all
    // its embers, smoke and shockwave puffs, then decays in
    // exact lockstep with the impact animation as those
    // particles age out. No hand-tuned wall-clock lifetime —
    // if you tweak particle counts or layer lifetimes the
    // light automatically follows.
    //
    // The curve shapes the response: the population drain is
    // roughly linear (oldest particles spawn first / die
    // first), but the eye perceives light brightness
    // non-linearly. A curve that's nearly flat near `t = 0`
    // (the peak) and accelerates toward `t = 1` (all dead)
    // gives the "BANG! ... afterglow ... fade" pulse shape
    // that reads as a real explosion.
    .with_light(EffectLight {
        color: Vec3::new(5.2, 2.6, 0.85),
        radius: 9.0,
        intensity: 1.2,
        intensity_curve: Some(Curve::from_stops([
            (0.00, 1.00),  // peak — all particles freshly spawned
            (0.10, 0.92),  // brief sustain as the densest core fires
            (0.25, 0.70),  // initial cooling
            (0.45, 0.42),
            (0.65, 0.20),
            (0.82, 0.07),
            (0.95, 0.015),
            (1.00, 0.00),  // last particle dies
        ])),
        lifetime: None,
        flicker_amp: 0.08,
        flicker_hz: 24.0,
        offset: Vec3::new(0.0, 0.4, 0.0),
        follow_particles: true,
        // Heat-haze opt-in is wired through the engine but
        // disabled — the warp didn't read as "radiant heat",
        // it read as "the screen broke". Leave the plumbing
        // in place for future tuning.
        heat_haze: false,
    })
}
