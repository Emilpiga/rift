use glam::Vec3;

use super::smoke::smoke_wake;
use super::smoke::SmokeWakeOpts;
use crate::renderer::vfx::spec::*;

#[derive(Clone, Debug)]
pub struct ContinuousOpts {
    pub spawn: SpawnShape,
    pub rate: f32,
    pub speed: (f32, f32),
    pub lifetime: (f32, f32),
    pub size: Curve,
    pub color: Gradient,
    pub sprite: SpriteShape,
    pub blend: BlendMode,
    pub opacity: f32,
    pub drag: f32,
    pub forces_extra: Vec<ForceField>,
}

impl ContinuousOpts {
    /// Comet head — same layout as [`Self::arcane_core`] (sphere
    /// soft-glow), tuned for ember / fireball trails.
    pub fn fireball_core() -> Self {
        Self {
            spawn: SpawnShape::Sphere,
            rate: 180.0,
            speed: (0.3, 1.0),
            lifetime: (0.16, 0.28),
            size: Curve::from_stops([(0.00, 0.18), (0.30, 0.14), (1.00, 0.03)]),
            color: Gradient::from_stops([
                (0.00, [5.4, 3.0, 0.95, 1.0]),
                (0.40, [2.6, 0.95, 0.22, 0.85]),
                (1.00, [0.55, 0.12, 0.04, 0.0]),
            ]),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Additive,
            opacity: 1.0,
            drag: 5.0,
            forces_extra: vec![],
        }
    }

    pub fn arcane_core() -> Self {
        Self {
            spawn: SpawnShape::Sphere,
            rate: 180.0,
            speed: (0.3, 1.0),
            lifetime: (0.16, 0.28),
            size: Curve::from_stops([(0.00, 0.18), (0.30, 0.14), (1.00, 0.03)]),
            color: Gradient::from_stops([
                (0.00, [3.6, 1.6, 4.2, 1.0]),
                (0.40, [1.8, 0.4, 2.6, 0.85]),
                (1.00, [0.3, 0.05, 0.6, 0.0]),
            ]),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Additive,
            opacity: 1.0,
            drag: 5.0,
            forces_extra: vec![],
        }
    }

    pub fn frost_core() -> Self {
        Self {
            spawn: SpawnShape::Sphere,
            rate: 160.0,
            speed: (0.3, 1.0),
            lifetime: (0.14, 0.26),
            size: Curve::from_stops([(0.00, 0.16), (0.30, 0.12), (1.00, 0.02)]),
            color: Gradient::from_stops([
                (0.00, [2.4, 4.6, 6.0, 1.0]),
                (0.40, [0.7, 1.8, 2.8, 0.85]),
                (1.00, [0.10, 0.25, 0.45, 0.0]),
            ]),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Additive,
            opacity: 1.0,
            drag: 5.0,
            forces_extra: vec![],
        }
    }

    pub fn hand_swirl_fire() -> Self {
        Self {
            spawn: SpawnShape::Sphere,
            rate: 60.0,
            speed: (0.3, 1.0),
            lifetime: (0.25, 0.55),
            size: Curve::from_stops([(0.00, 0.10), (0.40, 0.14), (1.00, 0.0)]),
            color: Gradient::from_stops([
                (0.00, [5.0, 2.4, 0.6, 1.0]),
                (0.50, [2.2, 0.9, 0.2, 0.7]),
                (1.00, [0.6, 0.10, 0.02, 0.0]),
            ]),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Additive,
            opacity: 1.0,
            drag: 3.0,
            forces_extra: vec![
                ForceField::Gravity {
                    axis: Vec3::Y,
                    strength: 1.5,
                },
                ForceField::Orbit {
                    axis: Vec3::Y,
                    speed: 6.0,
                },
            ],
        }
    }

    pub fn hand_swirl_frost() -> Self {
        Self {
            spawn: SpawnShape::Sphere,
            rate: 60.0,
            speed: (0.3, 1.0),
            lifetime: (0.25, 0.55),
            size: Curve::from_stops([(0.00, 0.10), (0.40, 0.14), (1.00, 0.0)]),
            color: Gradient::from_stops([
                (0.00, [1.5, 3.0, 4.5, 1.0]),
                (0.50, [0.6, 1.4, 2.2, 0.7]),
                (1.00, [0.2, 0.4, 0.6, 0.0]),
            ]),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Additive,
            opacity: 1.0,
            drag: 3.0,
            forces_extra: vec![
                ForceField::Gravity {
                    axis: Vec3::Y,
                    strength: 1.5,
                },
                ForceField::Orbit {
                    axis: Vec3::Y,
                    speed: 6.0,
                },
            ],
        }
    }

    pub fn heal_aura() -> Self {
        Self {
            spawn: SpawnShape::Sphere,
            rate: 14.0,
            speed: (0.4, 1.2),
            lifetime: (0.6, 1.0),
            size: Curve::from_stops([(0.0, 0.06), (0.4, 0.10), (1.0, 0.0)]),
            color: Gradient::from_stops([
                (0.0, [0.8, 1.6, 0.9, 0.9]),
                (0.6, [0.5, 1.2, 0.7, 0.7]),
                (1.0, [0.3, 0.8, 0.4, 0.0]),
            ]),
            sprite: SpriteShape::Spark,
            blend: BlendMode::Additive,
            opacity: 1.0,
            drag: 1.2,
            forces_extra: vec![ForceField::Gravity {
                axis: Vec3::Y,
                strength: 1.4,
            }],
        }
    }
}

pub fn continuous(opts: ContinuousOpts) -> Layer {
    let mut forces = vec![ForceField::Drag {
        coefficient: opts.drag,
    }];
    forces.extend(opts.forces_extra);
    Layer::Particles(ParticleSpec {
        spawn: opts.spawn,
        emission: EmissionMode::Continuous { rate: opts.rate },
        speed: opts.speed,
        lifetime: opts.lifetime,
        forces,
        size: opts.size,
        color: opts.color,
        sprite: opts.sprite,
        blend: opts.blend,
        opacity: opts.opacity,
        hybrid: None,
        vfx_role: 0,
    })
}

/// Fire projectile trail — mirrors [`projectile_trail_arcane`] (core +
/// smoke wake) with ember palette via [`StylePreset::EmberVoid`].
pub fn projectile_trail_fire() -> Vec<Layer> {
    vec![
        continuous(ContinuousOpts::fireball_core()),
        smoke_wake(SmokeWakeOpts::fireball_trail()),
    ]
}

pub fn projectile_trail_arcane() -> Vec<Layer> {
    vec![
        continuous(ContinuousOpts::arcane_core()),
        smoke_wake(SmokeWakeOpts::arcane()),
    ]
}

pub fn projectile_trail_frost() -> Vec<Layer> {
    crate::renderer::vfx::builder::archetype::projectile_trail_layers(
        crate::renderer::vfx::style::StylePreset::VoidFrost,
    )
}
