//! Fire-themed combat presets: AoE flame waves, persistent
//! fireball trails, and detonation bursts.

use glam::Vec3;

use crate::renderer::vfx::spec::*;

/// Rain of Fire — a 2-second meteoric downpour over a 3 m
/// radius zone, anchored 5 m above the placed target. Six
/// composited layers:
///
///  1. Sky-portal flare — single Ring burst at the anchor on
///     spawn so the player gets a clear "incoming from above"
///     telegraph before the first meteor lands.
///  2. Meteor cores — fast, fat Streak sprites in HDR
///     yellow-white that punch downward through the column.
///  3. Ember-tail sparks — sharper Spark sprites trailing
///     behind the cores, slightly slower with drag so they
///     read as flaming debris.
///  4. Ground impact flashes — continuous SoftGlow bursts
///     spawned along the bottom of the column (Column with
///     `-Y` axis distributes spawn positions all the way down
///     to ground level), short-lifetime hot puffs.
///  5. Persistent ground fire ring — slow ember layer that
///     marks the zone footprint while the AoE ticks.
///  6. Rising smoke — alpha-blended grey-brown plumes that
///     drift upward as the volley winds down.
pub fn rain_of_fire() -> Effect {
    Effect {
        duration: 2.0,
        layers: vec![
            // 1. Sky-portal flare — a single growing Ring at
            //    the apex of the column so the cast reads as
            //    "a hole tore open in the sky and meteors are
            //    coming through it". Hollow Ring sprite, lives
            //    ~0.45 s.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.45, 0.45),
                forces: vec![],
                size: Curve::from_stops([(0.00, 1.0), (0.30, 3.5), (1.00, 4.0)]),
                color: Gradient::from_stops([
                    (0.00, [6.0, 3.2, 0.6, 1.0]),
                    (0.50, [3.5, 1.4, 0.2, 0.7]),
                    (1.00, [0.6, 0.10, 0.02, 0.0]),
                ]),
                sprite: SpriteShape::Ring,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 2. Meteor cores — bright stretched streaks
            //    falling vertically through the column. Streak
            //    sprites stay anisotropic at any speed so the
            //    motion line reads even on a static frame.
            //    Spawned across a 3 m disc at the top with a
            //    tiny vertical jitter; gravity does the heavy
            //    lifting on top of the seeded downward speed.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Column {
                    radius: 3.0,
                    height: 0.8,
                    axis: Vec3::Y,
                },
                emission: EmissionMode::BurstAndContinuous {
                    burst: 8,
                    rate: 30.0,
                },
                speed: (14.0, 22.0),
                lifetime: (0.45, 0.75),
                forces: vec![
                    ForceField::Gravity {
                        axis: -Vec3::Y,
                        strength: 30.0,
                    },
                    ForceField::Drag { coefficient: 0.04 },
                ],
                size: Curve::from_stops([(0.00, 0.50), (0.40, 0.42), (1.00, 0.18)]),
                color: Gradient::from_stops([
                    (0.00, [8.0, 5.5, 1.6, 1.0]),
                    (0.40, [5.0, 2.4, 0.5, 0.95]),
                    (1.00, [1.2, 0.20, 0.05, 0.0]),
                ]),
                sprite: SpriteShape::Streak,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 3. Ember-tail sparks — smaller, sharper sparks
            //    seeded slightly behind the meteor cores so
            //    each meteor visually drags a trail of
            //    flaming debris. Heavier drag so they fall
            //    off into ember showers rather than punching
            //    straight through.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Column {
                    radius: 3.0,
                    height: 1.5,
                    axis: Vec3::Y,
                },
                emission: EmissionMode::Continuous { rate: 90.0 },
                speed: (6.0, 12.0),
                lifetime: (0.35, 0.65),
                forces: vec![
                    ForceField::Gravity {
                        axis: -Vec3::Y,
                        strength: 22.0,
                    },
                    ForceField::Drag { coefficient: 0.6 },
                ],
                size: Curve::from_stops([(0.00, 0.18), (1.00, 0.0)]),
                color: Gradient::from_stops([
                    (0.00, [5.5, 2.8, 0.6, 1.0]),
                    (0.50, [2.8, 0.9, 0.15, 0.85]),
                    (1.00, [0.4, 0.05, 0.02, 0.0]),
                ]),
                sprite: SpriteShape::Spark,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 4. Ground impact flashes — bright soft puffs
            //    seeded along the bottom of the column. A
            //    `-Y` axis Column biases spawn density
            //    downward so particles appear at the floor of
            //    the zone, not at the apex. Short lifetime +
            //    zero velocity makes them read as "a meteor
            //    just hit here".
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Column {
                    radius: 3.0,
                    height: 5.0,
                    axis: -Vec3::Y,
                },
                emission: EmissionMode::Continuous { rate: 35.0 },
                speed: (0.0, 0.5),
                lifetime: (0.10, 0.20),
                forces: vec![ForceField::Drag { coefficient: 6.0 }],
                size: Curve::from_stops([(0.00, 0.30), (0.30, 0.85), (1.00, 0.20)]),
                color: Gradient::from_stops([
                    (0.00, [7.0, 3.5, 0.9, 1.0]),
                    (0.50, [3.5, 1.1, 0.2, 0.7]),
                    (1.00, [0.4, 0.05, 0.02, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 5. Persistent ground fire ring — slow ember
            //    layer pinned at the bottom of the column so
            //    a circular smouldering pool sits under the
            //    zone for its entire 2 s. Sharp Ring sprite
            //    keeps the radius unambiguous to the player.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Continuous { rate: 5.0 },
                speed: (0.0, 0.0),
                lifetime: (0.4, 0.4),
                forces: vec![ForceField::Gravity {
                    // Drop instantly to ground so the ring
                    // sits at the floor, not at the sky
                    // anchor.
                    axis: -Vec3::Y,
                    strength: 50.0,
                }],
                size: Curve::from_stops([(0.00, 5.8), (1.00, 6.4)]),
                color: Gradient::from_stops([
                    (0.00, [5.0, 1.8, 0.3, 0.9]),
                    (0.60, [2.4, 0.6, 0.10, 0.5]),
                    (1.00, [0.4, 0.05, 0.02, 0.0]),
                ]),
                sprite: SpriteShape::Ring,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 6. Rising smoke — alpha-blended grey-brown
            //    plumes seeded near the ground that drift
            //    upward as the volley winds down. Cuts the
            //    "all bright additive" overload and gives
            //    the post-cast moment a real settling beat.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Column {
                    radius: 2.6,
                    height: 4.5,
                    axis: -Vec3::Y,
                },
                emission: EmissionMode::Continuous { rate: 18.0 },
                speed: (0.5, 1.4),
                lifetime: (0.8, 1.4),
                forces: vec![
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 2.0,
                    },
                    ForceField::Drag { coefficient: 0.7 },
                ],
                size: Curve::from_stops([(0.00, 0.50), (0.50, 1.10), (1.00, 1.60)]),
                color: Gradient::from_stops([
                    (0.00, [0.20, 0.15, 0.12, 0.55]),
                    (0.50, [0.14, 0.10, 0.08, 0.32]),
                    (1.00, [0.06, 0.04, 0.04, 0.0]),
                ]),
                sprite: SpriteShape::Smoke,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            }),
        ],
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
                size: Curve::from_stops([(0.00, 0.50), (0.30, 6.00), (1.00, 15.00)]),
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
                size: Curve::from_stops([(0.00, 0.45), (0.30, 0.85), (1.00, 0.30)]),
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
                size: Curve::from_stops([(0.00, 0.13), (1.00, 0.0)]),
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
                size: Curve::from_stops([(0.00, 0.45), (0.50, 0.85), (1.00, 1.20)]),
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
/// An earlier revision also had a high-rate `Streak` ember
/// layer scattered on a sphere, intended to read as motion
/// lines. In practice the streaks oriented along each
/// particle's individual velocity — which, after the small
/// random sphere component was added to the inherited
/// projectile velocity, splayed in every direction. Players
/// (correctly) read this as "stars and lines shooting backward
/// from the fireball", an effect that fights the projectile's
/// forward read. Removed in favour of the cleaner two-layer
/// nucleus + smoke composition: the head glows, a soft trail
/// hangs behind it, the impact does the dramatic flourish.
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
                spawn: SpawnShape::Point,
                emission: EmissionMode::Continuous { rate: 320.0 },
                speed: (0.0, 0.0),
                lifetime: (0.10, 0.20),
                forces: vec![ForceField::Drag { coefficient: 5.5 }],
                size: Curve::from_stops([(0.00, 0.20), (0.30, 0.16), (1.00, 0.03)]),
                color: Gradient::from_stops([
                    (0.00, [5.5, 4.2, 1.8, 1.0]),
                    (0.40, [3.0, 1.4, 0.35, 0.9]),
                    (1.00, [0.9, 0.18, 0.05, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // Smoke wake removed entirely. Even with Point
            // spawn + zero random velocity + heavy drag, smoke
            // sprites that live up to ~1 s sit visibly behind
            // the projectile after it has moved several metres
            // — from a side-on camera that reads as "puffs
            // flying backward". A pure SoftGlow nucleus is the
            // cleanest read: the projectile is one tight ball
            // of light with no lingering particles trailing
            // behind. The dramatic plume now lives entirely in
            // `fireball_explosion`, which fires at impact.
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
                size: Curve::from_stops([(0.00, 1.50), (1.00, 2.40)]),
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
                size: Curve::from_stops([(0.00, 0.30), (0.30, 0.60), (1.00, 0.14)]),
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
                size: Curve::from_stops([(0.00, 0.32), (0.40, 0.75), (1.00, 1.05)]),
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
                size: Curve::from_stops([(0.00, 0.13), (1.00, 0.0)]),
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
                size: Curve::from_stops([(0.00, 0.40), (1.00, 3.60)]),
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
            (0.00, 1.00), // peak — all particles freshly spawned
            (0.10, 0.92), // brief sustain as the densest core fires
            (0.25, 0.70), // initial cooling
            (0.45, 0.42),
            (0.65, 0.20),
            (0.82, 0.07),
            (0.95, 0.015),
            (1.00, 0.00), // last particle dies
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

/// Fire Beam — warm-orange channeled beam used by Embercrown's
/// Fireball → Beam transform. Structurally identical to
/// `frost_ray` (hand-base swirl + length-gradient HDR ribbon +
/// anchor light + tip light + per-tick impact bursts) but
/// re-tinted to a fireball-trail palette so the channel reads
/// as a sustained jet of flame rather than a cyan ice ray.
pub fn fire_beam() -> EffectBundle {
    EffectBundle::new(Effect {
        duration: 0.0,
        layers: vec![
            // Hand-base ember swirl — continuous warm glow
            // anchored to the caster's hand joint each frame.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Continuous { rate: 60.0 },
                speed: (0.3, 1.0),
                lifetime: (0.25, 0.55),
                forces: vec![
                    ForceField::Drag { coefficient: 3.0 },
                    // Embers rise off the hand rather than fall —
                    // gravity points +Y so they drift upward.
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 1.5,
                    },
                    ForceField::Orbit {
                        axis: Vec3::Y,
                        speed: 6.0,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.10), (0.40, 0.14), (1.00, 0.0)]),
                color: Gradient::from_stops([
                    // HDR yellow-white core → warm orange → dark red.
                    (0.00, [5.0, 2.4, 0.6, 1.0]),
                    (0.50, [2.2, 0.9, 0.2, 0.7]),
                    (1.00, [0.6, 0.10, 0.02, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // Beam ribbon — HDR fire-orange core, warm-amber
            // rim, transparent edges. Same cross/length
            // gradient and noise structure as frost_ray; only
            // the colour stops swap.
            Layer::Ribbon(RibbonSpec {
                width: 0.45,
                cross_gradient: Gradient::from_stops([
                    (0.00, [0.90, 0.35, 0.10, 0.0]),
                    (0.20, [1.00, 0.55, 0.20, 0.6]),
                    (0.50, [8.00, 4.00, 1.20, 1.0]),
                    (0.80, [1.00, 0.55, 0.20, 0.6]),
                    (1.00, [0.90, 0.35, 0.10, 0.0]),
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
    })
    // Warm orange hand-light — same role as the frost_ray
    // anchor light, just retuned to a fireball palette so the
    // caster's body and the floor under them read as lit by
    // the beam rather than floating in shadow.
    .with_light(EffectLight {
        color: Vec3::new(3.6, 1.8, 0.6),
        radius: 4.0,
        intensity: 1.6,
        intensity_curve: None,
        lifetime: None,
        // Fire flickers harder than frost — bump amplitude
        // up and drop the rate a touch so it reads as flame,
        // not coil whine.
        flicker_amp: 0.10,
        flicker_hz: 9.0,
        offset: Vec3::new(0.0, 0.15, 0.0),
        follow_particles: true,
        heat_haze: false,
    })
    // Tip light pinned to the beam's far endpoint —
    // brightens whatever the beam is currently scorching so
    // the gap between per-tick impact flashes still reads as
    // a continuous heat source.
    .with_tip_light(EffectLight {
        color: Vec3::new(4.2, 2.0, 0.6),
        radius: 3.5,
        intensity: 1.7,
        intensity_curve: None,
        lifetime: None,
        flicker_amp: 0.12,
        flicker_hz: 11.0,
        offset: Vec3::ZERO,
        follow_particles: true,
        heat_haze: false,
    })
}

/// Per-tick scorch burst at every enemy a `fire_beam` is
/// piercing (plus the terminal point when the beam clips a
/// wall). Warm-tinted counterpart to `frost_impact` — same
/// two-layer structure (soft puff + sharp shards), fire palette.
pub fn fire_beam_impact() -> EffectBundle {
    EffectBundle::new(Effect {
        duration: 0.05,
        layers: vec![
            // Soft heat puff
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 14 },
                speed: (1.0, 3.0),
                lifetime: (0.25, 0.45),
                forces: vec![ForceField::Drag { coefficient: 5.0 }],
                size: Curve::from_stops([(0.0, 0.18), (1.0, 0.05)]),
                color: Gradient::from_stops([
                    (0.0, [6.0, 2.8, 0.7, 0.9]),
                    (1.0, [0.6, 0.10, 0.02, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // Ember shards
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 8 },
                speed: (3.0, 6.0),
                lifetime: (0.18, 0.28),
                forces: vec![ForceField::Drag { coefficient: 3.0 }],
                size: Curve::from_stops([(0.0, 0.10), (1.0, 0.0)]),
                color: Gradient::from_stops([
                    (0.0, [8.0, 3.4, 0.8, 1.0]),
                    (1.0, [0.8, 0.15, 0.02, 0.0]),
                ]),
                sprite: SpriteShape::Shard,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
        ],
    })
}

/// Legendary proc explosion — the visual companion for
/// `ProcAction::Explosion` server-side AoE pops (Splinterstep
/// OnDodge, Mirrorglass OnLowHealth). A compact, percussive
/// fire-orange burst designed to read clearly without
/// overlapping a full Fire Wave: ground shockwave ring,
/// outward flame plumes, radial embers, and a brief smoke
/// puff. ~0.05 s emission, particles age out by ~0.7 s so it
/// matches the single-tick AoE the proc spawns.
pub fn proc_explosion() -> EffectBundle {
    EffectBundle::new(Effect {
        duration: 0.05,
        layers: vec![
            // 1. Ground shockwave ring — hollow expanding
            //    sprite that calls out the blast radius. ~3 m
            //    final size matches the proc's authoritative
            //    AoE radius from `ITEMS.md` (Splinterstep,
            //    Mirrorglass).
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.45, 0.45),
                forces: vec![],
                size: Curve::from_stops([(0.00, 0.40), (0.40, 4.5), (1.00, 7.0)]),
                color: Gradient::from_stops([
                    (0.00, [7.0, 3.5, 0.8, 1.0]),
                    (0.50, [3.5, 1.2, 0.2, 0.8]),
                    (1.00, [0.6, 0.10, 0.02, 0.0]),
                ]),
                sprite: SpriteShape::Ring,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 2. Outward flame plumes — radial burst seeded on
            //    a tight ground ring. Reads as the body of the
            //    explosion catching enemies as it expands.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Ring {
                    radius: 0.4,
                    thickness: 0.3,
                },
                emission: EmissionMode::Burst { count: 60 },
                speed: (7.0, 11.0),
                lifetime: (0.30, 0.50),
                forces: vec![
                    ForceField::Drag { coefficient: 2.0 },
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 3.0,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.40), (0.30, 0.75), (1.00, 0.25)]),
                color: Gradient::from_stops([
                    (0.00, [5.5, 2.6, 0.6, 1.0]),
                    (0.40, [3.0, 1.0, 0.2, 0.85]),
                    (1.00, [0.4, 0.06, 0.02, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 3. Embers — bright sparks thrown radially with
            //    real gravity so they arc and rain back down.
            //    Carries the "crunch" of the impact frame.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Ring {
                    radius: 0.3,
                    thickness: 0.3,
                },
                emission: EmissionMode::Burst { count: 50 },
                speed: (9.0, 15.0),
                lifetime: (0.30, 0.50),
                forces: vec![
                    ForceField::Drag { coefficient: 0.7 },
                    ForceField::Gravity {
                        axis: -Vec3::Y,
                        strength: 16.0,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.12), (1.00, 0.0)]),
                color: Gradient::from_stops([
                    (0.00, [7.0, 4.0, 1.4, 1.0]),
                    (0.50, [2.8, 1.0, 0.25, 0.9]),
                    (1.00, [0.5, 0.10, 0.05, 0.0]),
                ]),
                sprite: SpriteShape::Spark,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 4. Smoke puff — short alpha-blended residue so
            //    the explosion settles instead of vanishing
            //    in a single bright frame.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Disc { radius: 0.8 },
                emission: EmissionMode::Burst { count: 14 },
                speed: (0.6, 1.2),
                lifetime: (0.6, 0.9),
                forces: vec![
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 1.4,
                    },
                    ForceField::Drag { coefficient: 0.6 },
                ],
                size: Curve::from_stops([(0.00, 0.40), (0.50, 0.85), (1.00, 1.15)]),
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
    })
    // Punchy orange flash light so dim-room explosions
    // actually light up the floor and nearby walls. Short
    // envelope — single-shot, no follow_particles.
    .with_light(EffectLight {
        color: Vec3::new(3.6, 1.7, 0.5),
        radius: 5.0,
        intensity: 2.4,
        intensity_curve: Some(Curve::from_stops([(0.00, 1.0), (0.20, 0.7), (1.00, 0.0)])),
        lifetime: Some(0.45),
        flicker_amp: 0.10,
        flicker_hz: 10.0,
        offset: Vec3::new(0.0, 0.4, 0.0),
        follow_particles: false,
        heat_haze: false,
    })
}
