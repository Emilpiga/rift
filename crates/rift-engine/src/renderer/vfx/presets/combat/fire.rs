//! Fire-themed combat presets: AoE flame waves, persistent
//! fireball trails, and detonation bursts.

use glam::Vec3;

use crate::renderer::vfx::builder::{
    particle, projectile_trail_fire, sky_portal_ring,
    EffectBuilder, ImpactTheme, ParticleOpts, RadialBurstOpts, RibbonOpts, ShockwaveOpts,
    SmokeResidueOpts, StylePreset,
};
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
    EffectBuilder::timed(2.0)
        .style(StylePreset::EmberVoid)
        .layer(sky_portal_ring())
        .layer(particle(ParticleOpts {
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
            hybrid: None,
        }))
        .layer(particle(ParticleOpts {
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
            hybrid: None,
        }))
        .layer(particle(ParticleOpts {
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
            hybrid: None,
        }))
        .layer(particle(ParticleOpts {
            spawn: SpawnShape::Point,
            emission: EmissionMode::Continuous { rate: 5.0 },
            speed: (0.0, 0.0),
            lifetime: (0.4, 0.4),
            forces: vec![ForceField::Gravity {
                axis: -Vec3::Y,
                strength: 50.0,
            }],
            size: Curve::from_stops([(0.00, 5.8), (1.00, 6.4)]),
            color: Gradient::from_stops([
                (0.00, [2.6, 0.95, 0.22, 0.42]),
                (0.60, [1.2, 0.34, 0.08, 0.24]),
                (1.00, [0.4, 0.05, 0.02, 0.0]),
            ]),
            sprite: SpriteShape::Ring,
            blend: BlendMode::Additive,
            opacity: 0.52,
            hybrid: None,
        }))
        .layer(particle(ParticleOpts {
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
            hybrid: None,
        }))
        .finish()
}

/// Fire Wave — a one-shot, fast-expanding ring of fire centred
/// on the caster. Built to feel impactful: a flat ground
/// shockwave ring, an outward-rushing wall of flame, hot
/// embers thrown radially, and a brief column of smoke left
/// behind. All four layers self-terminate by ~0.7 s so the
/// preset can be used as a fire-and-forget client emitter.
pub fn fire_wave() -> Effect {
    EffectBuilder::new(0.05)
        .style(StylePreset::EmberVoid)
        .shockwave(ShockwaveOpts::fire_wave())
        .radial_burst(RadialBurstOpts::fire_flame_wall())
        .radial_burst(RadialBurstOpts::fire_embers())
        .smoke_residue(SmokeResidueOpts::fire_wave_residue())
        .finish()
}

/// Fireball trail — same two-layer recipe as [`arcane_bolt_trail`]
/// (sphere soft-glow core + smoke wake), with [`StylePreset::EmberVoid`],
/// velocity inheritance, and a warm point light. Persistent until the
/// projectile detonates.
pub fn fireball_trail() -> EffectBundle {
    EffectBuilder::persistent()
        .style(StylePreset::EmberVoid)
        .layers(projectile_trail_fire())
        .finish_bundle()
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
    EffectBuilder::oneshot()
        .style(StylePreset::EmberVoid)
        .impact_burst(ImpactTheme::Fire)
        .finish_bundle()
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
    })
}

/// Fire Beam — warm-orange channeled beam used by Embercrown's
/// Fireball → Beam transform. Structurally identical to
/// `frost_ray` (hand-base swirl + length-gradient HDR ribbon +
/// anchor light + tip light + per-tick impact bursts) but
/// re-tinted to a fireball-trail palette so the channel reads
/// as a sustained jet of flame rather than a cyan ice ray.
pub fn fire_beam() -> EffectBundle {
    EffectBuilder::persistent()
        .style(StylePreset::EmberVoid)
        .channel_hand_swirl()
        .ribbon(RibbonOpts::fire_outer())
        .ribbon(RibbonOpts::fire_inner())
        .finish_bundle()
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
    })
    // Tip light pinned to the beam's far endpoint —
    // brightens whatever the beam is currently scorching so
    // the gap between per-tick impact flashes still reads as
    // continuous radiant energy.
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
    })
}

/// Per-tick scorch burst at every enemy a `fire_beam` is
/// piercing (plus the terminal point when the beam clips a
/// wall). Warm-tinted counterpart to `frost_impact` — same
/// two-layer structure (soft puff + sharp shards), fire palette.
pub fn fire_beam_impact() -> EffectBundle {
    EffectBuilder::oneshot()
        .style(StylePreset::EmberVoid)
        .beam_tick_impact(ImpactTheme::Fire)
        .finish_bundle()
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
    EffectBuilder::oneshot()
        .style(StylePreset::EmberVoid)
        .proc_explosion()
        .finish_bundle()
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
    })
}
