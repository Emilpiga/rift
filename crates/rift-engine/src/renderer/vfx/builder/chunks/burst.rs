use glam::Vec3;

use crate::renderer::vfx::spec::*;

// ─── Flash ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct FlashOpts {
    pub lifetime: (f32, f32),
    pub size: Curve,
    pub color: Gradient,
    pub opacity: f32,
}

impl FlashOpts {
    pub fn fireball() -> Self {
        Self {
            lifetime: (0.10, 0.12),
            size: Curve::from_stops([(0.00, 1.50), (1.00, 2.40)]),
            color: Gradient::from_stops([
                (0.00, [6.5, 5.8, 3.8, 1.0]),
                (1.00, [2.0, 1.0, 0.4, 0.0]),
            ]),
            opacity: 1.0,
        }
    }

    pub fn arcane() -> Self {
        Self {
            lifetime: (0.09, 0.11),
            size: Curve::from_stops([(0.00, 1.10), (1.00, 1.80)]),
            color: Gradient::from_stops([
                (0.00, [5.0, 3.5, 6.5, 1.0]),
                (1.00, [1.2, 0.4, 2.2, 0.0]),
            ]),
            opacity: 1.0,
        }
    }

    pub fn frost() -> Self {
        Self {
            lifetime: (0.09, 0.11),
            size: Curve::from_stops([(0.00, 1.05), (1.00, 1.70)]),
            color: Gradient::from_stops([
                (0.00, [4.0, 6.0, 8.0, 1.0]),
                (1.00, [0.5, 1.2, 2.0, 0.0]),
            ]),
            opacity: 1.0,
        }
    }
}

pub fn flash(opts: FlashOpts) -> Layer {
    Layer::Particles(ParticleSpec {
        spawn: SpawnShape::Point,
        emission: EmissionMode::Burst { count: 1 },
        speed: (0.0, 0.0),
        lifetime: opts.lifetime,
        forces: vec![],
        size: opts.size,
        color: opts.color,
        sprite: SpriteShape::SoftGlow,
        blend: BlendMode::Additive,
        opacity: opts.opacity,
        hybrid: None,
        vfx_role: 0,
    })
}

// ─── Plasma core (curl sphere) ────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct PlasmaCoreOpts {
    pub count: u32,
    pub speed: (f32, f32),
    pub lifetime: (f32, f32),
    pub size: Curve,
    pub color: Gradient,
    pub opacity: f32,
}

impl PlasmaCoreOpts {
    pub fn fireball() -> Self {
        Self {
            count: 32,
            speed: (1.5, 4.5),
            lifetime: (0.30, 0.55),
            size: Curve::from_stops([(0.00, 0.30), (0.30, 0.60), (1.00, 0.14)]),
            color: Gradient::from_stops([
                (0.00, [5.5, 3.4, 1.0, 1.0]),
                (0.40, [3.0, 1.0, 0.25, 0.85]),
                (1.00, [0.30, 0.05, 0.02, 0.0]),
            ]),
            opacity: 1.0,
        }
    }
}

pub fn plasma_core(opts: PlasmaCoreOpts) -> Layer {
    Layer::Particles(ParticleSpec {
        spawn: SpawnShape::Sphere,
        emission: EmissionMode::Burst { count: opts.count },
        speed: opts.speed,
        lifetime: opts.lifetime,
        forces: vec![
            ForceField::Drag { coefficient: 4.0 },
            ForceField::Gravity {
                axis: Vec3::Y,
                strength: 3.0,
            },
            ForceField::Curl {
                frequency: 0.9,
                strength: 14.0,
            },
        ],
        size: opts.size,
        color: opts.color,
        sprite: SpriteShape::SoftGlow,
        blend: BlendMode::Additive,
        opacity: opts.opacity,
        hybrid: None,
        vfx_role: 0,
    })
}

// ─── Radial ring burst ─────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct RadialBurstOpts {
    pub ring_radius: f32,
    pub ring_thickness: f32,
    pub count: u32,
    pub speed: (f32, f32),
    pub lifetime: (f32, f32),
    pub size: Curve,
    pub color: Gradient,
    pub sprite: SpriteShape,
    pub blend: BlendMode,
    pub opacity: f32,
    pub lift: f32,
    pub drag: f32,
    pub hybrid: Option<HybridMaterial>,
}

impl RadialBurstOpts {
    pub fn fire_flame_wall() -> Self {
        Self {
            ring_radius: 0.6,
            ring_thickness: 0.4,
            count: 80,
            speed: (8.0, 13.0),
            lifetime: (0.35, 0.55),
            size: Curve::from_stops([(0.00, 0.45), (0.30, 0.85), (1.00, 0.30)]),
            color: Gradient::from_stops([
                (0.00, [5.5, 2.6, 0.6, 1.0]),
                (0.40, [3.0, 1.0, 0.2, 0.80]),
                (1.00, [0.4, 0.06, 0.02, 0.0]),
            ]),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Additive,
            opacity: 1.0,
            lift: 3.5,
            drag: 1.8,
            hybrid: None,
        }
    }

    pub fn proc_flames() -> Self {
        Self {
            ring_radius: 0.4,
            ring_thickness: 0.3,
            count: 60,
            speed: (7.0, 11.0),
            lifetime: (0.30, 0.50),
            size: Curve::from_stops([(0.00, 0.40), (0.30, 0.75), (1.00, 0.25)]),
            color: Gradient::from_stops([
                (0.00, [5.5, 2.6, 0.6, 1.0]),
                (0.40, [3.0, 1.0, 0.2, 0.85]),
                (1.00, [0.4, 0.06, 0.02, 0.0]),
            ]),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Additive,
            opacity: 1.0,
            lift: 3.0,
            drag: 2.0,
            hybrid: None,
        }
    }

    pub fn proc_embers() -> Self {
        Self {
            ring_radius: 0.3,
            ring_thickness: 0.3,
            count: 50,
            speed: (9.0, 15.0),
            lifetime: (0.30, 0.50),
            size: Curve::from_stops([(0.00, 0.12), (1.00, 0.0)]),
            color: Gradient::from_stops([
                (0.00, [7.0, 4.0, 1.4, 1.0]),
                (0.50, [2.8, 1.0, 0.25, 0.9]),
                (1.00, [0.5, 0.10, 0.05, 0.0]),
            ]),
            sprite: SpriteShape::Spark,
            blend: BlendMode::Additive,
            opacity: 1.0,
            lift: -16.0,
            drag: 0.7,
            hybrid: None,
        }
    }

    pub fn fire_embers() -> Self {
        Self {
            ring_radius: 0.4,
            ring_thickness: 0.3,
            count: 60,
            speed: (10.0, 16.0),
            lifetime: (0.30, 0.55),
            size: Curve::from_stops([(0.00, 0.13), (1.00, 0.0)]),
            color: Gradient::from_stops([
                (0.00, [6.0, 4.0, 1.6, 1.0]),
                (0.50, [2.5, 1.0, 0.25, 0.9]),
                (1.00, [0.5, 0.10, 0.05, 0.0]),
            ]),
            sprite: SpriteShape::Spark,
            blend: BlendMode::Additive,
            opacity: 1.0,
            lift: -14.0,
            drag: 0.8,
            hybrid: None,
        }
    }
}

pub fn radial_burst(opts: RadialBurstOpts) -> Layer {
    let mut forces = vec![ForceField::Drag {
        coefficient: opts.drag,
    }];
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

    Layer::Particles(ParticleSpec {
        spawn: SpawnShape::Ring {
            radius: opts.ring_radius,
            thickness: opts.ring_thickness,
        },
        emission: EmissionMode::Burst { count: opts.count },
        speed: opts.speed,
        lifetime: opts.lifetime,
        forces,
        size: opts.size,
        color: opts.color,
        sprite: opts.sprite,
        blend: opts.blend,
        opacity: opts.opacity,
        hybrid: opts.hybrid,
        vfx_role: 0,
    })
}

// ─── Sphere burst ─────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct SphereBurstOpts {
    pub count: u32,
    pub speed: (f32, f32),
    pub lifetime: (f32, f32),
    pub size: Curve,
    pub color: Gradient,
    pub sprite: SpriteShape,
    pub blend: BlendMode,
    pub opacity: f32,
    pub lift: f32,
    pub drag: f32,
    pub forces_extra: Vec<ForceField>,
}

impl SphereBurstOpts {
    pub fn arcane_cloud() -> Self {
        Self {
            count: 28,
            speed: (2.0, 5.0),
            lifetime: (0.35, 0.60),
            size: Curve::from_stops([(0.00, 0.32), (0.30, 0.55), (1.00, 0.22)]),
            color: Gradient::from_stops([
                (0.00, [3.6, 1.4, 4.6, 1.0]),
                (0.40, [1.6, 0.4, 2.4, 0.8]),
                (1.00, [0.10, 0.04, 0.30, 0.0]),
            ]),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Additive,
            opacity: 1.0,
            lift: 1.2,
            drag: 4.0,
            forces_extra: vec![],
        }
    }

    pub fn frost_cloud() -> Self {
        Self {
            count: 26,
            speed: (2.0, 5.0),
            lifetime: (0.32, 0.55),
            size: Curve::from_stops([(0.00, 0.30), (0.30, 0.52), (1.00, 0.20)]),
            color: Gradient::from_stops([
                (0.00, [2.4, 4.0, 5.8, 1.0]),
                (0.40, [0.7, 1.6, 2.6, 0.8]),
                (1.00, [0.06, 0.14, 0.30, 0.0]),
            ]),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Additive,
            opacity: 1.0,
            lift: 1.0,
            drag: 4.0,
            forces_extra: vec![],
        }
    }

    pub fn heal_sparkles() -> Self {
        Self {
            count: 28,
            speed: (1.0, 2.5),
            lifetime: (0.45, 0.75),
            size: Curve::from_stops([(0.0, 0.10), (0.4, 0.18), (1.0, 0.0)]),
            color: Gradient::from_stops([
                (0.0, [1.4, 2.6, 1.6, 1.0]),
                (0.5, [0.9, 1.8, 1.1, 0.85]),
                (1.0, [0.4, 1.0, 0.5, 0.0]),
            ]),
            sprite: SpriteShape::Spark,
            blend: BlendMode::Additive,
            opacity: 1.0,
            lift: 2.5,
            drag: 1.5,
            forces_extra: vec![],
        }
    }

    pub fn cast_spark(rgb: [f32; 3]) -> Self {
        Self {
            count: 12,
            speed: (3.0, 6.0),
            lifetime: (0.15, 0.4),
            size: Curve::from_stops([(0.0, 0.08), (1.0, 0.0)]),
            color: Gradient::from_stops([
                (0.0, [rgb[0] * 1.4, rgb[1] * 1.4, rgb[2] * 1.4, 1.0]),
                (1.0, [rgb[0] * 0.5, rgb[1] * 0.5, rgb[2] * 0.5, 0.0]),
            ]),
            sprite: SpriteShape::Spark,
            blend: BlendMode::Additive,
            opacity: 1.0,
            lift: -8.0,
            drag: 2.0,
            forces_extra: vec![],
        }
    }

    pub fn dodge_puff() -> Self {
        Self {
            count: 8,
            speed: (1.0, 2.5),
            lifetime: (0.2, 0.4),
            size: Curve::from_stops([(0.0, 0.12), (1.0, 0.30)]),
            color: Gradient::from_stops([
                (0.0, [0.6, 0.8, 1.0, 0.6]),
                (1.0, [0.4, 0.6, 0.9, 0.0]),
            ]),
            sprite: SpriteShape::Smoke,
            blend: BlendMode::Alpha,
            opacity: 1.0,
            lift: 0.5,
            drag: 3.0,
            forces_extra: vec![],
        }
    }
}

pub fn sphere_burst(opts: SphereBurstOpts) -> Layer {
    let mut forces = vec![ForceField::Drag {
        coefficient: opts.drag,
    }];
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
        spawn: SpawnShape::Sphere,
        emission: EmissionMode::Burst { count: opts.count },
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

// ─── Spark burst (cone) ───────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct SparkBurstOpts {
    pub spawn: SpawnShape,
    pub count: u32,
    pub speed: (f32, f32),
    pub lifetime: (f32, f32),
    pub size: Curve,
    pub color: Gradient,
    pub opacity: f32,
}

impl SparkBurstOpts {
    pub fn hit(normal: Vec3) -> Self {
        Self {
            spawn: SpawnShape::Cone {
                axis: normal.normalize_or_zero(),
                half_angle: 0.6,
            },
            count: 18,
            speed: (3.0, 6.5),
            lifetime: (0.18, 0.32),
            size: Curve::from_stops([(0.0, 0.06), (1.0, 0.0)]),
            color: Gradient::from_stops([
                (0.00, [4.0, 3.0, 1.5, 1.0]),
                (0.40, [1.5, 0.8, 0.3, 0.8]),
                (1.00, [0.6, 0.2, 0.1, 0.0]),
            ]),
            opacity: 1.0,
        }
    }

    pub fn arcane() -> Self {
        Self {
            spawn: SpawnShape::Sphere,
            count: 22,
            speed: (4.0, 9.0),
            lifetime: (0.28, 0.50),
            size: Curve::from_stops([(0.00, 0.09), (1.00, 0.0)]),
            color: Gradient::from_stops([
                (0.00, [4.5, 1.8, 5.0, 1.0]),
                (0.50, [1.8, 0.4, 2.6, 0.9]),
                (1.00, [0.20, 0.05, 0.40, 0.0]),
            ]),
            opacity: 1.0,
        }
    }
}

pub fn spark_burst(opts: SparkBurstOpts) -> Layer {
    let gravity = if matches!(opts.spawn, SpawnShape::Cone { .. }) {
        6.0
    } else {
        4.0
    };
    Layer::Particles(ParticleSpec {
        spawn: opts.spawn,
        emission: EmissionMode::Burst { count: opts.count },
        speed: opts.speed,
        lifetime: opts.lifetime,
        forces: vec![
            ForceField::Drag {
                coefficient: if gravity > 5.0 { 4.0 } else { 1.4 },
            },
            ForceField::Gravity {
                axis: -Vec3::Y,
                strength: gravity,
            },
        ],
        size: opts.size,
        color: opts.color,
        sprite: SpriteShape::Spark,
        blend: BlendMode::Additive,
        opacity: opts.opacity,
        hybrid: None,
        vfx_role: 0,
    })
}

// ─── Streak burst ─────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct StreakBurstOpts {
    pub spawn: SpawnShape,
    pub count: u32,
    pub speed: (f32, f32),
    pub lifetime: (f32, f32),
    pub size: Curve,
    pub color: Gradient,
    pub opacity: f32,
    pub gravity: f32,
    pub drag: f32,
}

impl StreakBurstOpts {
    pub fn fireball_embers() -> Self {
        Self {
            spawn: SpawnShape::Sphere,
            count: 40,
            speed: (6.0, 13.0),
            lifetime: (0.35, 0.65),
            size: Curve::from_stops([(0.00, 0.13), (1.00, 0.0)]),
            color: Gradient::from_stops([
                (0.00, [5.5, 3.8, 1.6, 1.0]),
                (0.50, [2.4, 0.9, 0.20, 0.9]),
                (1.00, [0.5, 0.10, 0.05, 0.0]),
            ]),
            opacity: 1.0,
            gravity: 14.0,
            drag: 1.0,
        }
    }
}

pub fn streak_burst(opts: StreakBurstOpts) -> Layer {
    Layer::Particles(ParticleSpec {
        spawn: opts.spawn,
        emission: EmissionMode::Burst { count: opts.count },
        speed: opts.speed,
        lifetime: opts.lifetime,
        forces: vec![
            ForceField::Drag {
                coefficient: opts.drag,
            },
            ForceField::Gravity {
                axis: -Vec3::Y,
                strength: opts.gravity,
            },
        ],
        size: opts.size,
        color: opts.color,
        sprite: SpriteShape::Streak,
        blend: BlendMode::Additive,
        opacity: opts.opacity,
        hybrid: None,
        vfx_role: 0,
    })
}

// ─── Shard burst ──────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct ShardBurstOpts {
    pub count: u32,
    pub speed: (f32, f32),
    pub lifetime: (f32, f32),
    pub size: Curve,
    pub color: Gradient,
    pub opacity: f32,
    pub drag: f32,
    pub gravity: f32,
}

impl ShardBurstOpts {
    pub fn frost() -> Self {
        Self {
            count: 20,
            speed: (4.0, 9.0),
            lifetime: (0.26, 0.48),
            size: Curve::from_stops([(0.00, 0.09), (1.00, 0.0)]),
            color: Gradient::from_stops([
                (0.00, [3.5, 5.5, 7.0, 1.0]),
                (0.50, [1.0, 2.0, 3.0, 0.9]),
                (1.00, [0.10, 0.20, 0.40, 0.0]),
            ]),
            opacity: 1.0,
            drag: 1.4,
            gravity: 4.0,
        }
    }

    pub fn frost_tick() -> Self {
        Self {
            count: 8,
            speed: (3.0, 6.0),
            lifetime: (0.18, 0.28),
            size: Curve::from_stops([(0.0, 0.10), (1.0, 0.0)]),
            color: Gradient::from_stops([
                (0.0, [5.0, 7.0, 9.0, 1.0]),
                (1.0, [0.3, 0.5, 0.7, 0.0]),
            ]),
            opacity: 1.0,
            drag: 3.0,
            gravity: 0.0,
        }
    }

    pub fn fire_tick() -> Self {
        Self {
            count: 8,
            speed: (3.0, 6.0),
            lifetime: (0.18, 0.28),
            size: Curve::from_stops([(0.0, 0.10), (1.0, 0.0)]),
            color: Gradient::from_stops([
                (0.0, [8.0, 3.4, 0.8, 1.0]),
                (1.0, [0.8, 0.15, 0.02, 0.0]),
            ]),
            opacity: 1.0,
            drag: 3.0,
            gravity: 0.0,
        }
    }
}

pub fn shard_burst(opts: ShardBurstOpts) -> Layer {
    let mut forces = vec![ForceField::Drag {
        coefficient: opts.drag,
    }];
    if opts.gravity > 0.0 {
        forces.push(ForceField::Gravity {
            axis: -Vec3::Y,
            strength: opts.gravity,
        });
    }
    Layer::Particles(ParticleSpec {
        spawn: SpawnShape::Sphere,
        emission: EmissionMode::Burst { count: opts.count },
        speed: opts.speed,
        lifetime: opts.lifetime,
        forces,
        size: opts.size,
        color: opts.color,
        sprite: SpriteShape::Shard,
        blend: BlendMode::Additive,
        opacity: opts.opacity,
        hybrid: None,
        vfx_role: 0,
    })
}

// ─── Sky portal ring ──────────────────────────────────────────────────────

pub fn sky_portal_ring() -> Layer {
    Layer::Particles(ParticleSpec {
        spawn: SpawnShape::Point,
        emission: EmissionMode::Burst { count: 1 },
        speed: (0.0, 0.0),
        lifetime: (0.45, 0.45),
        forces: vec![],
        size: Curve::from_stops([(0.00, 1.0), (0.30, 3.5), (1.00, 4.0)]),
        color: Gradient::from_stops([
            (0.00, [4.2, 2.2, 0.55, 0.70]),
            (0.50, [2.1, 0.85, 0.18, 0.42]),
            (1.00, [0.6, 0.10, 0.02, 0.0]),
        ]),
        sprite: SpriteShape::Ring,
        blend: BlendMode::Additive,
        opacity: 0.62,
        hybrid: None,
        vfx_role: 0,
    })
}

pub fn heal_ground_ring() -> Layer {
    Layer::Particles(ParticleSpec {
        spawn: SpawnShape::Point,
        emission: EmissionMode::Burst { count: 1 },
        speed: (0.0, 0.0),
        lifetime: (0.40, 0.40),
        forces: vec![],
        size: Curve::from_stops([(0.00, 0.40), (0.50, 1.60), (1.00, 2.20)]),
        color: Gradient::from_stops([
            (0.00, [1.4, 2.4, 1.4, 0.85]),
            (1.00, [0.4, 1.0, 0.5, 0.0]),
        ]),
        sprite: SpriteShape::Ring,
        blend: BlendMode::Additive,
        opacity: 1.0,
        hybrid: None,
        vfx_role: 0,
    })
}
