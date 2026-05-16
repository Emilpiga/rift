//! Enemy-special VFX presets for non-boss archetypes that need
//! bespoke readability and impact.

use glam::Vec3;

use crate::renderer::vfx::builder::archetype::ImpactArchetype;
use crate::renderer::vfx::builder::{
    particle, EffectBuilder, ContinuousOpts, FlashOpts, ParticleArchetype,
    ShockwaveOpts, SmokeBillowOpts, StylePreset, VfxRole,
};
use crate::renderer::vfx::spec::*;

const WRAITH_REACH: f32 = 4.8;

/// Wind-up anchor — mouth, slightly forward so the cone reads in front of the body.
pub fn wraith_scream_telegraph_anchor(cast_origin: Vec3, aim: Vec3) -> Vec3 {
    let axis = planar_dir(aim);
    cast_origin + axis * 0.40 + Vec3::new(0.0, 0.55, 0.0)
}

/// Release anchor — mid-cone so flash / shockwave / bursts travel downrange.
pub fn wraith_scream_impact_anchor(cast_origin: Vec3, aim: Vec3) -> Vec3 {
    let axis = planar_dir(aim);
    cast_origin + axis * 1.15 + Vec3::new(0.0, 0.50, 0.0)
}

pub fn wraith_scream_telegraph(dir: Vec3, duration: f32) -> Effect {
    let axis = planar_dir(dir);
    let dur = duration.max(0.05);
    EffectBuilder::timed(dur)
        .style(StylePreset::VoidFrost)
        .layers(wraith_telegraph_layers(axis, WRAITH_REACH, dur))
        .continuous(wraith_mouth_core_telegraph())
        .finish()
}

pub fn wraith_scream_impact(dir: Vec3) -> EffectBundle {
    let axis = planar_dir(dir);
    EffectBuilder::oneshot()
        .style(StylePreset::VoidFrost)
        .flash(FlashOpts::frost())
        .layers(wraith_impact_layers(axis, WRAITH_REACH))
        .shockwave(scale_shockwave(ShockwaveOpts::frost(), 1.12))
        .particle_archetype(ParticleArchetype::void_frost_filaments())
        .with_light(EffectLight {
            color: Vec3::new(0.85, 2.8, 3.6),
            radius: 7.5,
            intensity: 2.4,
            intensity_curve: Some(Curve::from_stops([
                (0.00, 1.0),
                (0.12, 0.88),
                (0.35, 0.42),
                (0.65, 0.14),
                (1.00, 0.0),
            ])),
            lifetime: Some(0.55),
            flicker_amp: 0.12,
            flicker_hz: 26.0,
            offset: Vec3::new(0.0, 0.55, 0.0),
            follow_particles: true,
        })
        .finish_bundle()
}

pub fn void_sigil_telegraph(radius: f32, duration: f32) -> Effect {
    let r = radius.max(0.5);
    let dur = duration.max(0.05);
    EffectBuilder::timed(dur)
        .style(StylePreset::ArcLightning)
        .layers(void_sigil_ground_telegraph(r, dur))
        .smoke_billow(sigil_arcane_billow(r))
        .continuous(sigil_arcane_swirl(r))
        .finish()
}

pub fn void_sigil_impact(radius: f32) -> EffectBundle {
    let r = radius.max(0.5);
    EffectBuilder::oneshot()
        .style(StylePreset::ArcLightning)
        .layers(void_sigil_ground_impact(r))
        .layers(ImpactArchetype::Detonation.layers(StylePreset::ArcLightning))
        .shockwave(scale_shockwave(ShockwaveOpts::arcane(), r / 2.6))
        .with_light(EffectLight {
            color: Vec3::new(2.4, 0.85, 4.6),
            radius: (r * 2.6).clamp(6.0, 11.0),
            intensity: 2.6,
            intensity_curve: Some(Curve::from_stops([
                (0.00, 1.0),
                (0.10, 0.92),
                (0.28, 0.55),
                (0.55, 0.22),
                (1.00, 0.0),
            ])),
            lifetime: Some(0.65),
            flicker_amp: 0.14,
            flicker_hz: 18.0,
            offset: Vec3::new(0.0, 0.35, 0.0),
            follow_particles: true,
        })
        .finish_bundle()
}

// ─── Wraith scream ────────────────────────────────────────────────────────

fn wraith_telegraph_layers(axis: Vec3, reach: f32, dur: f32) -> Vec<Layer> {
    vec![
        particle(ParticleSpec {
            spawn: SpawnShape::Line {
                a: axis * 0.15,
                b: axis * reach,
            },
            emission: EmissionMode::Continuous { rate: 110.0 },
            speed: (0.15, 0.85),
            lifetime: (0.16, 0.32),
            forces: vec![
                ForceField::Wind {
                    velocity: axis * 2.4 + Vec3::Y * 0.04,
                },
                ForceField::Drag { coefficient: 4.2 },
                ForceField::Curl {
                    frequency: 1.05,
                    strength: 2.0,
                },
            ],
            size: Curve::from_stops([(0.00, 0.08), (0.48, 0.22), (1.00, 0.02)]),
            color: Gradient::from_stops([
                (0.00, [1.8, 5.2, 6.4, 0.52]),
                (0.50, [0.55, 1.85, 2.8, 0.36]),
                (1.00, [0.06, 0.18, 0.38, 0.00]),
            ]),
            sprite: SpriteShape::Streak,
            blend: BlendMode::Additive,
            opacity: 1.0,
            hybrid: None,
            vfx_role: VfxRole::Filament.gpu_role_id_u8(),
        }),
        particle(ParticleSpec {
            spawn: SpawnShape::Cone {
                axis,
                half_angle: 0.34,
            },
            emission: EmissionMode::Continuous { rate: 48.0 },
            speed: (2.6, 5.2),
            lifetime: (0.30, 0.55),
            forces: vec![
                ForceField::Drag { coefficient: 2.5 },
                ForceField::Curl {
                    frequency: 0.70,
                    strength: 3.2,
                },
            ],
            size: Curve::from_stops([(0.00, 0.18), (0.48, 0.52), (1.00, 0.10)]),
            color: Gradient::from_stops([
                (0.00, [0.88, 2.2, 2.7, 0.28]),
                (0.55, [0.20, 0.65, 1.10, 0.20]),
                (1.00, [0.05, 0.16, 0.30, 0.00]),
            ]),
            sprite: SpriteShape::Smoke,
            blend: BlendMode::Alpha,
            opacity: 1.0,
            hybrid: Some(HybridMaterial::smoke_billow()),
            vfx_role: VfxRole::Vapor.gpu_role_id_u8(),
        }),
        particle(ParticleSpec {
            spawn: SpawnShape::Point,
            emission: EmissionMode::Burst { count: 1 },
            speed: (0.0, 0.0),
            lifetime: (dur, dur),
            forces: vec![],
            size: Curve::from_stops([(0.00, 0.42), (0.72, 0.88), (1.00, 1.12)]),
            color: Gradient::from_stops([
                (0.00, [0.50, 1.90, 2.45, 0.12]),
                (0.70, [0.90, 3.2, 4.0, 0.28]),
                (1.00, [1.65, 5.0, 5.8, 0.48]),
            ]),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Additive,
            opacity: 0.88,
            hybrid: None,
            vfx_role: VfxRole::Core.gpu_role_id_u8(),
        }),
    ]
}

fn wraith_mouth_core_telegraph() -> ContinuousOpts {
    ContinuousOpts {
        spawn: SpawnShape::Point,
        rate: 70.0,
        speed: (0.0, 0.25),
        lifetime: (0.09, 0.16),
        size: Curve::from_stops([(0.00, 0.11), (0.40, 0.16), (1.00, 0.04)]),
        color: Gradient::from_stops([
            (0.00, [3.6, 7.0, 7.8, 1.0]),
            (0.45, [1.1, 2.8, 3.6, 0.72]),
            (1.00, [0.14, 0.42, 0.65, 0.0]),
        ]),
        sprite: SpriteShape::SoftGlow,
        blend: BlendMode::Additive,
        opacity: 0.95,
        drag: 5.5,
        forces_extra: vec![],
    }
}

fn wraith_impact_layers(axis: Vec3, reach: f32) -> Vec<Layer> {
    vec![
        particle(ParticleSpec {
            spawn: SpawnShape::Line {
                a: axis * 0.20,
                b: axis * reach,
            },
            emission: EmissionMode::Burst { count: 110 },
            speed: (0.5, 1.8),
            lifetime: (0.16, 0.32),
            forces: vec![
                ForceField::Wind {
                    velocity: axis * 6.0 + Vec3::Y * 0.06,
                },
                ForceField::Drag { coefficient: 3.6 },
            ],
            size: Curve::from_stops([(0.00, 0.10), (0.42, 0.30), (1.00, 0.0)]),
            color: Gradient::from_stops([
                (0.00, [5.2, 9.0, 9.4, 1.0]),
                (0.44, [1.35, 4.0, 5.4, 0.78]),
                (1.00, [0.10, 0.28, 0.50, 0.00]),
            ]),
            sprite: SpriteShape::Streak,
            blend: BlendMode::Additive,
            opacity: 1.0,
            hybrid: None,
            vfx_role: VfxRole::Impact.gpu_role_id_u8(),
        }),
        particle(ParticleSpec {
            spawn: SpawnShape::Cone {
                axis,
                half_angle: 0.40,
            },
            emission: EmissionMode::Burst { count: 120 },
            speed: (7.5, 14.5),
            lifetime: (0.18, 0.38),
            forces: vec![ForceField::Drag { coefficient: 2.0 }],
            size: Curve::from_stops([(0.00, 0.18), (0.40, 0.38), (1.00, 0.0)]),
            color: Gradient::from_stops([
                (0.00, [3.6, 7.0, 7.6, 0.96]),
                (0.42, [1.0, 2.5, 3.4, 0.70]),
                (1.00, [0.08, 0.22, 0.40, 0.00]),
            ]),
            sprite: SpriteShape::Streak,
            blend: BlendMode::Additive,
            opacity: 1.0,
            hybrid: None,
            vfx_role: VfxRole::Filament.gpu_role_id_u8(),
        }),
        particle(ParticleSpec {
            spawn: SpawnShape::Cone {
                axis,
                half_angle: 0.38,
            },
            emission: EmissionMode::Burst { count: 52 },
            speed: (2.0, 5.0),
            lifetime: (0.38, 0.72),
            forces: vec![
                ForceField::Wind {
                    velocity: axis * 1.6 + Vec3::Y * 0.10,
                },
                ForceField::Drag { coefficient: 2.6 },
                ForceField::Curl {
                    frequency: 0.90,
                    strength: 3.8,
                },
            ],
            size: Curve::from_stops([(0.00, 0.30), (0.38, 0.78), (1.00, 0.20)]),
            color: Gradient::from_stops([
                (0.00, [0.48, 1.35, 1.70, 0.40]),
                (0.55, [0.16, 0.46, 0.72, 0.28]),
                (1.00, [0.02, 0.06, 0.12, 0.00]),
            ]),
            sprite: SpriteShape::Smoke,
            blend: BlendMode::Alpha,
            opacity: 1.0,
            hybrid: Some(HybridMaterial::smoke_billow()),
            vfx_role: VfxRole::Residue.gpu_role_id_u8(),
        }),
    ]
}

// ─── Mindbinder void sigil ──────────────────────────────────────────────────

fn void_sigil_ground_telegraph(r: f32, dur: f32) -> Vec<Layer> {
    vec![
        particle(ParticleSpec {
            spawn: SpawnShape::Point,
            emission: EmissionMode::Burst { count: 1 },
            speed: (0.0, 0.0),
            lifetime: (dur, dur),
            forces: vec![],
            size: Curve::from_stops([(0.00, r * 1.18), (0.68, r * 1.68), (1.00, r * 2.05)]),
            color: Gradient::from_stops([
                (0.00, [0.14, 0.04, 0.32, 0.18]),
                (0.62, [0.32, 0.08, 0.62, 0.38]),
                (1.00, [0.62, 0.14, 1.35, 0.62]),
            ]),
            sprite: SpriteShape::GroundCrack,
            blend: BlendMode::Alpha,
            opacity: 1.0,
            hybrid: None,
            vfx_role: VfxRole::Rupture.gpu_role_id_u8(),
        }),
        particle(ParticleSpec {
            spawn: SpawnShape::Point,
            emission: EmissionMode::Burst { count: 1 },
            speed: (0.0, 0.0),
            lifetime: (dur, dur),
            forces: vec![],
            size: Curve::from_stops([(0.00, r * 0.52), (0.70, r * 1.55), (1.00, r * 2.12)]),
            color: Gradient::from_stops([
                (0.00, [0.55, 0.14, 1.65, 0.10]),
                (0.70, [1.65, 0.42, 4.20, 0.32]),
                (1.00, [3.80, 1.15, 7.40, 0.72]),
            ]),
            sprite: SpriteShape::Ring,
            blend: BlendMode::Additive,
            opacity: 0.98,
            hybrid: None,
            vfx_role: VfxRole::Rupture.gpu_role_id_u8(),
        }),
        particle(ParticleSpec {
            spawn: SpawnShape::Disc { radius: r * 0.78 },
            emission: EmissionMode::Continuous { rate: 88.0 },
            speed: (0.15, 0.82),
            lifetime: (0.32, 0.68),
            forces: vec![
                ForceField::Wind {
                    velocity: Vec3::new(0.0, 0.45, 0.0),
                },
                ForceField::Curl {
                    frequency: 0.62,
                    strength: 5.0,
                },
                ForceField::Drag { coefficient: 3.0 },
            ],
            size: Curve::from_stops([(0.00, 0.16), (0.46, 0.44), (1.00, 0.06)]),
            color: Gradient::from_stops([
                (0.00, [1.85, 0.65, 4.20, 0.38]),
                (0.55, [0.72, 0.20, 2.10, 0.28]),
                (1.00, [0.08, 0.02, 0.28, 0.00]),
            ]),
            sprite: SpriteShape::Wisp,
            blend: BlendMode::Additive,
            opacity: 0.94,
            hybrid: None,
            vfx_role: VfxRole::Vapor.gpu_role_id_u8(),
        }),
    ]
}

fn void_sigil_ground_impact(r: f32) -> Vec<Layer> {
    vec![
        particle(ParticleSpec {
            spawn: SpawnShape::Point,
            emission: EmissionMode::Burst { count: 1 },
            speed: (0.0, 0.0),
            lifetime: (0.44, 0.44),
            forces: vec![],
            size: Curve::from_stops([(0.00, r * 1.65), (0.36, r * 2.28), (1.00, r * 2.62)]),
            color: Gradient::from_stops([
                (0.00, [6.2, 2.6, 8.6, 0.98]),
                (0.30, [2.8, 0.72, 5.8, 0.68]),
                (1.00, [0.14, 0.04, 0.48, 0.00]),
            ]),
            sprite: SpriteShape::GroundCrack,
            blend: BlendMode::Additive,
            opacity: 1.0,
            hybrid: None,
            vfx_role: VfxRole::Rupture.gpu_role_id_u8(),
        }),
        particle(ParticleSpec {
            spawn: SpawnShape::Point,
            emission: EmissionMode::Burst { count: 1 },
            speed: (0.0, 0.0),
            lifetime: (0.48, 0.48),
            forces: vec![],
            size: Curve::from_stops([(0.00, r * 0.38), (0.32, r * 2.15), (1.00, r * 3.15)]),
            color: Gradient::from_stops([
                (0.00, [5.2, 2.2, 8.4, 1.0]),
                (0.38, [2.4, 0.58, 5.6, 0.76]),
                (1.00, [0.18, 0.04, 0.62, 0.00]),
            ]),
            sprite: SpriteShape::Ring,
            blend: BlendMode::Additive,
            opacity: 1.0,
            hybrid: None,
            vfx_role: VfxRole::Rupture.gpu_role_id_u8(),
        }),
        particle(ParticleSpec {
            spawn: SpawnShape::Ring {
                radius: r * 0.48,
                thickness: r * 0.38,
            },
            emission: EmissionMode::Burst { count: 64 },
            speed: (5.5, 12.5),
            lifetime: (0.26, 0.52),
            forces: vec![ForceField::Drag { coefficient: 1.3 }],
            size: Curve::from_stops([(0.00, 0.14), (0.42, 0.10), (1.00, 0.0)]),
            color: Gradient::from_stops([
                (0.00, [5.6, 2.2, 8.4, 1.0]),
                (0.52, [2.2, 0.58, 5.2, 0.78]),
                (1.00, [0.16, 0.04, 0.42, 0.00]),
            ]),
            sprite: SpriteShape::Streak,
            blend: BlendMode::Additive,
            opacity: 1.0,
            hybrid: None,
            vfx_role: VfxRole::Filament.gpu_role_id_u8(),
        }),
    ]
}

fn sigil_arcane_billow(r: f32) -> SmokeBillowOpts {
    SmokeBillowOpts {
        spawn: SpawnShape::Disc { radius: r * 0.85 },
        count: 72,
        speed: (1.8, 4.8),
        lifetime: (0.48, 0.82),
        size: Curve::from_stops([(0.00, 0.24), (0.35, 0.50), (1.00, 0.62)]),
        color: Gradient::from_stops([
            (0.00, [2.4, 0.85, 4.8, 0.42]),
            (0.30, [1.2, 0.38, 2.6, 0.32]),
            (0.72, [0.35, 0.12, 0.75, 0.18]),
            (1.00, [0.06, 0.04, 0.14, 0.0]),
        ]),
        opacity: 0.58,
        forces: vec![
            ForceField::Drag { coefficient: 3.0 },
            ForceField::Gravity {
                axis: Vec3::Y,
                strength: 1.2,
            },
            ForceField::Curl {
                frequency: 0.85,
                strength: 9.5,
            },
            ForceField::Turbulence {
                frequency: 2.0,
                strength: 2.4,
            },
        ],
    }
}

fn sigil_arcane_swirl(r: f32) -> ContinuousOpts {
    ContinuousOpts {
        spawn: SpawnShape::Disc { radius: r * 0.62 },
        rate: 105.0,
        speed: (0.25, 1.05),
        lifetime: (0.32, 0.58),
        size: Curve::from_stops([(0.00, 0.14), (0.40, 0.20), (1.00, 0.04)]),
        color: Gradient::from_stops([
            (0.00, [4.2, 1.5, 5.8, 1.0]),
            (0.45, [1.6, 0.45, 2.8, 0.78]),
            (1.00, [0.25, 0.06, 0.55, 0.0]),
        ]),
        sprite: SpriteShape::Wisp,
        blend: BlendMode::Additive,
        opacity: 1.0,
        drag: 4.5,
        forces_extra: vec![
            ForceField::Curl {
                frequency: 0.55,
                strength: 3.8,
            },
            ForceField::Wind {
                velocity: Vec3::new(0.0, 0.55, 0.0),
            },
        ],
    }
}

fn scale_shockwave(mut opts: ShockwaveOpts, scale: f32) -> ShockwaveOpts {
    let s = scale.max(0.25);
    opts.start_size *= s;
    if opts.mid_size > 0.0 {
        opts.mid_size *= s;
    }
    opts.end_size *= s;
    opts
}

fn planar_dir(dir: Vec3) -> Vec3 {
    let flat = Vec3::new(dir.x, 0.0, dir.z);
    if flat.length_squared() > 1.0e-4 {
        flat.normalize()
    } else {
        Vec3::Z
    }
}
