use glam::Vec3;

use crate::renderer::vfx::spec::*;

#[derive(Clone, Debug)]
pub struct SmokeResidueOpts {
    pub spawn: SpawnShape,
    pub count: u32,
    pub speed: (f32, f32),
    pub lifetime: (f32, f32),
    pub size: Curve,
    pub color: Gradient,
    pub opacity: f32,
    pub hybrid: Option<HybridMaterial>,
    pub lift: f32,
    pub forces_extra: Vec<ForceField>,
}

impl SmokeResidueOpts {
    pub fn fire_wave_residue() -> Self {
        Self {
            spawn: SpawnShape::Disc { radius: 1.2 },
            count: 18,
            speed: (0.6, 1.2),
            lifetime: (0.7, 1.1),
            size: Curve::from_stops([(0.00, 0.45), (0.50, 0.85), (1.00, 1.20)]),
            color: Gradient::from_stops([
                (0.00, [0.18, 0.13, 0.10, 0.55]),
                (0.50, [0.12, 0.09, 0.07, 0.30]),
                (1.00, [0.05, 0.04, 0.04, 0.0]),
            ]),
            opacity: 1.0,
            hybrid: None,
            lift: 1.2,
            forces_extra: vec![],
        }
    }

    pub fn proc_puff() -> Self {
        Self {
            spawn: SpawnShape::Disc { radius: 0.8 },
            count: 10,
            speed: (0.4, 0.9),
            lifetime: (0.5, 0.8),
            size: Curve::from_stops([(0.00, 0.35), (1.00, 0.70)]),
            color: Gradient::from_stops([
                (0.00, [0.16, 0.12, 0.10, 0.45]),
                (1.00, [0.05, 0.04, 0.04, 0.0]),
            ]),
            opacity: 1.0,
            hybrid: None,
            lift: 1.2,
            forces_extra: vec![],
        }
    }

    pub fn proc_explosion_puff() -> Self {
        Self {
            spawn: SpawnShape::Sphere,
            count: 42,
            speed: (0.8, 2.2),
            lifetime: (0.55, 0.85),
            size: Curve::from_stops([(0.00, 0.22), (0.45, 0.40), (1.00, 0.52)]),
            color: Gradient::from_stops([
                (0.00, [0.18, 0.13, 0.10, 0.48]),
                (0.50, [0.12, 0.09, 0.07, 0.26]),
                (1.00, [0.05, 0.04, 0.04, 0.0]),
            ]),
            opacity: 0.82,
            hybrid: None,
            lift: 1.2,
            forces_extra: vec![
                ForceField::Curl {
                    frequency: 0.9,
                    strength: 5.5,
                },
                ForceField::Turbulence {
                    frequency: 2.2,
                    strength: 1.6,
                },
            ],
        }
    }
}

pub fn smoke_residue(opts: SmokeResidueOpts) -> Layer {
    let mut forces = vec![ForceField::Drag { coefficient: 0.5 }];
    if opts.lift.abs() > 1e-4 {
        forces.push(ForceField::Gravity {
            axis: if opts.lift > 0.0 {
                Vec3::Y
            } else {
                -Vec3::Y
            },
            strength: opts.lift.abs(),
        });
    }
    forces.extend(opts.forces_extra);

    Layer::Particles(ParticleSpec {
        spawn: opts.spawn,
        emission: EmissionMode::Burst { count: opts.count },
        speed: opts.speed,
        lifetime: opts.lifetime,
        forces,
        size: opts.size,
        color: opts.color,
        sprite: SpriteShape::Smoke,
        blend: BlendMode::Alpha,
        opacity: opts.opacity,
        hybrid: opts.hybrid,
        vfx_role: 0,
    })
}

#[derive(Clone, Debug)]
pub struct SmokeBillowOpts {
    pub spawn: SpawnShape,
    pub count: u32,
    pub speed: (f32, f32),
    pub lifetime: (f32, f32),
    pub size: Curve,
    pub color: Gradient,
    pub opacity: f32,
    pub forces: Vec<ForceField>,
}

impl Default for SmokeBillowOpts {
    fn default() -> Self {
        Self::fireball_cloud()
    }
}

impl SmokeBillowOpts {
    pub fn fireball_cloud() -> Self {
        Self {
            spawn: SpawnShape::Sphere,
            count: 84,
            speed: (2.2, 6.0),
            lifetime: (0.52, 0.88),
            size: Curve::from_stops([(0.00, 0.28), (0.35, 0.52), (1.00, 0.68)]),
            color: Gradient::from_stops([
                (0.00, [2.2, 1.20, 0.55, 0.44]),
                (0.28, [1.10, 0.58, 0.32, 0.34]),
                (0.70, [0.32, 0.22, 0.18, 0.18]),
                (1.00, [0.06, 0.055, 0.05, 0.0]),
            ]),
            opacity: 0.52,
            forces: vec![
                ForceField::Drag { coefficient: 3.2 },
                ForceField::Gravity {
                    axis: Vec3::Y,
                    strength: 1.6,
                },
                ForceField::Curl {
                    frequency: 0.75,
                    strength: 10.0,
                },
                ForceField::Turbulence {
                    frequency: 1.8,
                    strength: 2.2,
                },
            ],
        }
    }
}

pub fn smoke_billow(opts: SmokeBillowOpts) -> Layer {
    Layer::Particles(ParticleSpec {
        spawn: opts.spawn,
        emission: EmissionMode::Burst { count: opts.count },
        speed: opts.speed,
        lifetime: opts.lifetime,
        forces: opts.forces,
        size: opts.size,
        color: opts.color,
        sprite: SpriteShape::Smoke,
        blend: BlendMode::Alpha,
        opacity: opts.opacity,
        hybrid: Some(HybridMaterial::smoke_billow()),
        vfx_role: 0,
    })
}

#[derive(Clone, Debug)]
pub struct SmokeWakeOpts {
    pub rate: f32,
    pub speed: (f32, f32),
    pub lifetime: (f32, f32),
    pub size: Curve,
    pub color: Gradient,
    pub opacity: f32,
    pub drag: f32,
}

impl SmokeWakeOpts {
    pub fn arcane() -> Self {
        Self {
            rate: 40.0,
            speed: (0.05, 0.4),
            lifetime: (0.30, 0.55),
            size: Curve::from_stops([(0.00, 0.14), (1.00, 0.32)]),
            color: Gradient::from_stops([
                (0.00, [0.6, 0.25, 1.0, 0.55]),
                (0.50, [0.20, 0.10, 0.45, 0.30]),
                (1.00, [0.05, 0.04, 0.10, 0.0]),
            ]),
            opacity: 1.0,
            drag: 2.5,
        }
    }

    pub fn frost() -> Self {
        Self {
            rate: 35.0,
            speed: (0.05, 0.4),
            lifetime: (0.30, 0.55),
            size: Curve::from_stops([(0.00, 0.12), (1.00, 0.28)]),
            color: Gradient::from_stops([
                (0.00, [0.7, 1.0, 1.4, 0.5]),
                (0.50, [0.20, 0.35, 0.55, 0.28]),
                (1.00, [0.04, 0.06, 0.12, 0.0]),
            ]),
            opacity: 1.0,
            drag: 2.5,
        }
    }

    /// Tail wisps — same cadence as [`Self::arcane`], warm tint.
    pub fn fireball_trail() -> Self {
        Self {
            rate: 40.0,
            speed: (0.05, 0.4),
            lifetime: (0.30, 0.55),
            size: Curve::from_stops([(0.00, 0.14), (1.00, 0.32)]),
            color: Gradient::from_stops([
                (0.00, [1.0, 0.42, 0.14, 0.52]),
                (0.50, [0.42, 0.16, 0.06, 0.30]),
                (1.00, [0.10, 0.05, 0.04, 0.0]),
            ]),
            opacity: 1.0,
            drag: 2.5,
        }
    }
}

pub fn smoke_wake(opts: SmokeWakeOpts) -> Layer {
    Layer::Particles(ParticleSpec {
        spawn: SpawnShape::Point,
        emission: EmissionMode::Continuous { rate: opts.rate },
        speed: opts.speed,
        lifetime: opts.lifetime,
        forces: vec![ForceField::Drag {
            coefficient: opts.drag,
        }],
        size: opts.size,
        color: opts.color,
        sprite: SpriteShape::Smoke,
        blend: BlendMode::Alpha,
        opacity: opts.opacity,
        hybrid: None,
        vfx_role: 0,
    })
}
