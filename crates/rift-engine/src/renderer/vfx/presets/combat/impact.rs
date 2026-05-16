//! Generic impact / hit visuals.

use glam::Vec3;

use crate::renderer::vfx::builder::{particle, EffectBuilder, SparkBurstOpts};
use crate::renderer::vfx::spec::*;

/// Generic hit spark — a small cone of bright sparks fired in
/// the surface normal direction. Used for projectile impacts and
/// melee hits. Tuned to mirror the legacy
/// [`crate::renderer::particles::EmitterConfig::hit_spark`].
pub fn hit_spark(normal: Vec3) -> Effect {
    EffectBuilder::new(0.05)
        .spark_burst(SparkBurstOpts::hit(normal))
        .finish()
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
    EffectBuilder::oneshot()
        .layers(vec![
            // 1. Initial dark crimson spurt — a few large soft
            //    puffs at the kill point.
            particle(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 3 },
                speed: (0.0, 0.6),
                lifetime: (0.16, 0.22),
                forces: vec![ForceField::Drag { coefficient: 6.0 }],
                size: Curve::from_stops([(0.00, 0.55), (0.30, 0.85), (1.00, 0.50)]),
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
            hybrid: None,
        vfx_role: 0,
    }),
            // 2. Droplets — wide upward+outward cone, fast, heavy
            //    gravity. Low drag so they keep their momentum
            //    until gravity pulls them back down.
            particle(ParticleSpec {
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
                size: Curve::from_stops([(0.00, 0.07), (0.85, 0.07), (1.00, 0.0)]),
                color: Gradient::from_stops([
                    (0.00, [0.62, 0.05, 0.05, 1.00]),
                    (0.70, [0.40, 0.03, 0.03, 0.95]),
                    (1.00, [0.20, 0.01, 0.01, 0.00]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
            // 3. Mist — a few slow, soft, dark-red smoky puffs
            //    that drift up briefly and dissolve.
            particle(ParticleSpec {
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
                size: Curve::from_stops([(0.00, 0.30), (1.00, 0.55)]),
                color: Gradient::from_stops([
                    (0.00, [0.30, 0.02, 0.02, 0.55]),
                    (1.00, [0.10, 0.01, 0.01, 0.00]),
                ]),
                sprite: SpriteShape::Smoke,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
        ])
        .finish()
}

/// Smaller blood spurt for non-fatal hits — few droplets, no
/// mist, lower velocity than [`blood_splatter`]. Kept deliberately
/// lean because combat can emit many damage events in one frame.
///
/// `up` is the spew axis (typically `Vec3::Y`).
pub fn blood_hit_spurt(up: Vec3) -> Effect {
    let axis = if up.length_squared() > 1e-4 {
        up.normalize()
    } else {
        Vec3::Y
    };
    EffectBuilder::oneshot()
        .layers(vec![particle(ParticleSpec {
            spawn: SpawnShape::Cone {
                axis,
                half_angle: 0.85,
            },
            emission: EmissionMode::Burst { count: 6 },
            speed: (2.5, 5.5),
            lifetime: (0.20, 0.34),
            forces: vec![
                ForceField::Drag { coefficient: 1.4 },
                ForceField::Gravity {
                    axis: -Vec3::Y,
                    strength: 18.0,
                },
            ],
            size: Curve::from_stops([(0.00, 0.06), (0.85, 0.06), (1.00, 0.0)]),
            color: Gradient::from_stops([
                (0.00, [0.62, 0.05, 0.05, 1.00]),
                (0.70, [0.40, 0.03, 0.03, 0.90]),
                (1.00, [0.20, 0.01, 0.01, 0.00]),
            ]),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Alpha,
            opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    })])
        .finish()
}

/// "Return to hell" — played when an enemy corpse leaves the
/// replicated snapshot and despawns. Reads as the body dissolving
/// back into the underworld:
///
/// 1. **Brimstone flash** — a single bright orange-red HDR pop
///    at the body's centre. Sells the dissolution moment.
/// 2. **Embers** — a wide upward cone of additive sparks that
///    rise briefly before being yanked back down by strong
///    negative gravity (drawn into the ground).
/// 3. **Charcoal smoke** — a slow ground-hugging puff of dark
///    smoke that lingers ~0.8 s, hiding the vanish.
pub fn enemy_soul_return() -> Effect {
    EffectBuilder::oneshot()
        .layers(vec![
            // 1. Brimstone flash — short HDR burst. Sized to
            //    cover roughly the enemy's torso so the puff
            //    visually swallows the body on the despawn frame
            //    instead of looking like a small floating spark.
            particle(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 6 },
                speed: (0.0, 0.6),
                lifetime: (0.22, 0.34),
                forces: vec![ForceField::Drag { coefficient: 6.0 }],
                size: Curve::from_stops([(0.00, 0.90), (0.30, 1.80), (1.00, 0.55)]),
                color: Gradient::from_stops([
                    (0.00, [3.5, 1.4, 0.30, 0.95]),
                    (0.50, [2.0, 0.55, 0.10, 0.70]),
                    (1.00, [0.40, 0.06, 0.02, 0.00]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
            // 2. Embers — bright additive sparks rising then
            //    dragged back down (souls being pulled under).
            //    Wider cone + more particles + longer sparks so
            //    the upward column reads from across the room.
            particle(ParticleSpec {
                spawn: SpawnShape::Cone {
                    axis: Vec3::Y,
                    half_angle: 1.05,
                },
                emission: EmissionMode::Burst { count: 48 },
                speed: (3.2, 6.5),
                lifetime: (0.65, 1.05),
                forces: vec![
                    ForceField::Drag { coefficient: 0.9 },
                    // Strong downward pull — embers arc up then
                    // fall back through the floor.
                    ForceField::Gravity {
                        axis: -Vec3::Y,
                        strength: 14.0,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.18), (0.70, 0.13), (1.00, 0.0)]),
                color: Gradient::from_stops([
                    (0.00, [3.2, 1.10, 0.20, 1.00]),
                    (0.50, [2.0, 0.40, 0.06, 0.90]),
                    (1.00, [0.40, 0.05, 0.01, 0.00]),
                ]),
                sprite: SpriteShape::Spark,
                blend: BlendMode::Additive,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
            // 3. Charcoal smoke — slow, dark, ground-hugging
            //    puff that hides the despawn frame. Doubled in
            //    size and count so a body-sized enemy gets a
            //    body-sized cloud.
            particle(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 18 },
                speed: (0.6, 1.7),
                lifetime: (0.75, 1.05),
                forces: vec![
                    ForceField::Drag { coefficient: 2.6 },
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 0.5,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.65), (1.00, 1.55)]),
                color: Gradient::from_stops([
                    (0.00, [0.10, 0.06, 0.05, 0.70]),
                    (0.60, [0.06, 0.03, 0.03, 0.45]),
                    (1.00, [0.02, 0.01, 0.01, 0.00]),
                ]),
                sprite: SpriteShape::Smoke,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
        ])
        .finish()
}

/// Friendly summon dismissal — a compact violet/teal collapse used
/// when player-owned minions expire, are unsummoned, or disappear on
/// floor changes. Smaller and cooler than `enemy_soul_return` so it
/// reads as a controlled arcane release rather than a hostile corpse.
pub fn summon_despawn(scale: f32) -> Effect {
    let s = scale.max(0.25);
    EffectBuilder::oneshot()
        .layers(vec![
            particle(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 14 },
                speed: (0.4 * s, 1.8 * s),
                lifetime: (0.22, 0.42),
                forces: vec![
                    ForceField::Drag { coefficient: 4.4 },
                    ForceField::Curl {
                        frequency: 1.5,
                        strength: 2.8 * s,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.30 * s), (0.45, 0.72 * s), (1.00, 0.0)]),
                color: Gradient::from_stops([
                    (0.00, [1.80, 3.20, 4.60, 0.78]),
                    (0.40, [1.60, 0.70, 3.80, 0.58]),
                    (1.00, [0.06, 0.08, 0.18, 0.00]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
            particle(ParticleSpec {
                spawn: SpawnShape::Ring {
                    radius: 0.35 * s,
                    thickness: 0.18 * s,
                },
                emission: EmissionMode::Burst { count: 28 },
                speed: (1.6 * s, 3.8 * s),
                lifetime: (0.28, 0.56),
                forces: vec![
                    ForceField::Drag { coefficient: 1.2 },
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 2.4 * s,
                    },
                    ForceField::Orbit {
                        axis: Vec3::Y,
                        speed: -3.2,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.08 * s), (0.70, 0.05 * s), (1.00, 0.0)]),
                color: Gradient::from_stops([
                    (0.00, [2.20, 4.40, 5.20, 0.92]),
                    (0.45, [2.20, 0.90, 4.40, 0.76]),
                    (1.00, [0.08, 0.12, 0.26, 0.00]),
                ]),
                sprite: SpriteShape::Streak,
                blend: BlendMode::Additive,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
            particle(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.34, 0.34),
                forces: vec![],
                size: Curve::from_stops([(0.00, 1.10 * s), (0.35, 1.55 * s), (1.00, 0.65 * s)]),
                color: Gradient::from_stops([
                    (0.00, [0.08, 0.05, 0.18, 0.56]),
                    (0.60, [0.04, 0.03, 0.10, 0.34]),
                    (1.00, [0.01, 0.01, 0.03, 0.00]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
        ])
        .finish()
}

/// Enemy arrival cue — played when a replicated enemy mesh is
/// first materialised on the client. The goal is to hide the
/// snap-in frame and sell the spawn as a deliberate rift/summon:
/// a fractured ground seal flashes, smoke rises through the body,
/// and embers spiral upward as the monster resolves into the room.
///
/// `scale` is a coarse monster-size multiplier: normal enemies use
/// `1.0`, elites slightly larger, bosses much larger.
pub fn enemy_summon_arrival(scale: f32) -> Effect {
    let s = scale.max(0.35);
    EffectBuilder::timed(0.08)
        .layers(vec![
            // 1. Flat rift scar at the feet. Dark alpha base so
            //    the spawn has contact with the floor instead of
            //    reading as a mid-air particle cloud.
            particle(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.70, 0.70),
                forces: vec![],
                size: Curve::from_stops([(0.00, 1.55 * s), (0.22, 2.15 * s), (1.00, 1.80 * s)]),
                color: Gradient::from_stops([
                    (0.00, [0.025, 0.018, 0.040, 0.72]),
                    (0.46, [0.035, 0.020, 0.055, 0.46]),
                    (1.00, [0.012, 0.010, 0.018, 0.00]),
                ]),
                sprite: SpriteShape::GroundCrack,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
            // 2. Hot arcane edges inside the same fracture mask.
            //    Teal-violet keeps this distinct from fire/slam
            //    while still sitting in the game's supernatural
            //    rift language.
            particle(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 1 },
                speed: (0.0, 0.0),
                lifetime: (0.34, 0.34),
                forces: vec![],
                size: Curve::from_stops([(0.00, 1.35 * s), (0.35, 2.10 * s), (1.00, 1.95 * s)]),
                color: Gradient::from_stops([
                    (0.00, [2.20, 0.55, 4.80, 0.00]),
                    (0.12, [3.20, 0.90, 6.20, 0.72]),
                    (0.45, [1.00, 2.80, 3.60, 0.50]),
                    (1.00, [0.08, 0.20, 0.30, 0.00]),
                ]),
                sprite: SpriteShape::GroundCrack,
                blend: BlendMode::Additive,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
            // 3. Resolve flash around the torso. Brief and wide;
            //    it masks the first rendered pose without leaving
            //    a long-lived glow stuck to the enemy.
            particle(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 10 },
                speed: (0.0, 1.1 * s),
                lifetime: (0.18, 0.34),
                forces: vec![ForceField::Drag { coefficient: 5.0 }],
                size: Curve::from_stops([(0.00, 0.65 * s), (0.32, 1.28 * s), (1.00, 0.45 * s)]),
                color: Gradient::from_stops([
                    (0.00, [2.60, 4.20, 4.80, 0.88]),
                    (0.42, [1.20, 0.70, 2.20, 0.58]),
                    (1.00, [0.08, 0.06, 0.14, 0.00]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
            // 4. Smoke veil rising through the body. This is the
            //    cheap-but-effective pop hider: alpha smoke crosses
            //    the silhouette while the skinned mesh appears.
            particle(ParticleSpec {
                spawn: SpawnShape::TaperedColumn {
                    radius_base: 0.62 * s,
                    radius_top: 0.18 * s,
                    height: 1.85 * s,
                    axis: Vec3::Y,
                },
                emission: EmissionMode::Burst { count: 32 },
                speed: (0.55 * s, 1.80 * s),
                lifetime: (0.55, 0.95),
                forces: vec![
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 1.4 * s,
                    },
                    ForceField::Drag { coefficient: 1.5 },
                    ForceField::Curl {
                        frequency: 0.85,
                        strength: 3.6 * s,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.34 * s), (0.45, 0.72 * s), (1.00, 1.05 * s)]),
                color: Gradient::from_stops([
                    (0.00, [0.10, 0.08, 0.14, 0.58]),
                    (0.55, [0.055, 0.045, 0.075, 0.34]),
                    (1.00, [0.018, 0.016, 0.024, 0.00]),
                ]),
                sprite: SpriteShape::Smoke,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
            // 5. Upward sparks, biased teal and violet. These give
            //    the cue a crisp read at distance and make the spawn
            //    feel energetic rather than just smoky.
            particle(ParticleSpec {
                spawn: SpawnShape::Ring {
                    radius: 0.42 * s,
                    thickness: 0.30 * s,
                },
                emission: EmissionMode::Burst {
                    count: (42.0 * s).round().clamp(24.0, 120.0) as u32,
                },
                speed: (2.6 * s, 5.4 * s),
                lifetime: (0.42, 0.74),
                forces: vec![
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 5.5 * s,
                    },
                    ForceField::Drag { coefficient: 1.0 },
                    ForceField::Orbit {
                        axis: Vec3::Y,
                        speed: 2.4,
                    },
                ],
                size: Curve::from_stops([(0.00, 0.11 * s), (0.70, 0.07 * s), (1.00, 0.0)]),
                color: Gradient::from_stops([
                    (0.00, [2.10, 4.20, 4.80, 0.95]),
                    (0.45, [2.20, 0.90, 4.60, 0.82]),
                    (1.00, [0.10, 0.14, 0.26, 0.00]),
                ]),
                sprite: SpriteShape::Streak,
                blend: BlendMode::Additive,
                opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    }),
        ])
        .finish()
}
