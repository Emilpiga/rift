use super::super::tint::gradient_from_tinted_stops;
use crate::renderer::vfx::spec::*;

#[derive(Clone, Debug)]
pub struct ShockwaveOpts {
    pub lifetime: f32,
    pub start_size: f32,
    pub mid_size: f32,
    pub end_size: f32,
    pub mid_t: f32,
    pub opacity: f32,
    pub tint: [f32; 3],
    pub spawn: SpawnShape,
    pub gradient: ShockwaveGradient,
}

#[derive(Clone, Copy, Debug)]
pub enum ShockwaveGradient {
    FireWave,
    FireImpact,
    Arcane,
    Frost,
    Proc,
}

impl Default for ShockwaveOpts {
    fn default() -> Self {
        Self::fire_wave()
    }
}

impl ShockwaveOpts {
    pub fn fire_wave() -> Self {
        Self {
            lifetime: 0.55,
            start_size: 0.50,
            mid_size: 6.00,
            end_size: 15.00,
            mid_t: 0.30,
            opacity: 0.58,
            tint: [1.0, 1.0, 1.0],
            spawn: SpawnShape::Point,
            gradient: ShockwaveGradient::FireWave,
        }
    }

    pub fn fire_impact() -> Self {
        Self {
            lifetime: 0.35,
            start_size: 0.40,
            mid_size: 2.20,
            end_size: 3.60,
            mid_t: 0.0,
            opacity: 0.52,
            tint: [1.0, 1.0, 1.0],
            spawn: SpawnShape::Point,
            gradient: ShockwaveGradient::FireImpact,
        }
    }

    pub fn arcane() -> Self {
        Self {
            lifetime: 0.32,
            start_size: 0.35,
            mid_size: 0.0,
            end_size: 2.60,
            mid_t: 0.0,
            opacity: 1.0,
            tint: [1.0, 1.0, 1.0],
            spawn: SpawnShape::Point,
            gradient: ShockwaveGradient::Arcane,
        }
    }

    pub fn frost() -> Self {
        Self {
            lifetime: 0.32,
            start_size: 0.35,
            mid_size: 0.0,
            end_size: 2.50,
            mid_t: 0.0,
            opacity: 1.0,
            tint: [1.0, 1.0, 1.0],
            spawn: SpawnShape::Point,
            gradient: ShockwaveGradient::Frost,
        }
    }

    pub fn proc_explosion() -> Self {
        Self {
            lifetime: 0.45,
            start_size: 0.40,
            mid_size: 4.5,
            end_size: 7.0,
            mid_t: 0.40,
            opacity: 1.0,
            tint: [1.0, 1.0, 1.0],
            spawn: SpawnShape::Point,
            gradient: ShockwaveGradient::Proc,
        }
    }

    pub fn lifetime(mut self, s: f32) -> Self {
        self.lifetime = s;
        self
    }

    pub fn opacity(mut self, o: f32) -> Self {
        self.opacity = o;
        self
    }

    pub fn tint(mut self, rgb: [f32; 3]) -> Self {
        self.tint = rgb;
        self
    }

    pub fn scale_sizes(mut self, factor: f32) -> Self {
        self.start_size *= factor;
        self.mid_size *= factor;
        self.end_size *= factor;
        self
    }
}

const FIRE_WAVE: &[(f32, [f32; 4])] = &[
    (0.00, [4.0, 2.0, 0.48, 0.68]),
    (0.40, [2.1, 0.72, 0.16, 0.44]),
    (1.00, [0.8, 0.10, 0.02, 0.0]),
];
const FIRE_IMPACT: &[(f32, [f32; 4])] = &[(0.00, [2.4, 1.25, 0.36, 0.46]), (1.00, [0.45, 0.12, 0.04, 0.0])];
const ARCANE: &[(f32, [f32; 4])] = &[(0.00, [2.4, 1.0, 3.6, 0.9]), (1.00, [0.30, 0.10, 0.55, 0.0])];
const FROST: &[(f32, [f32; 4])] = &[(0.00, [1.6, 3.0, 4.4, 0.9]), (1.00, [0.10, 0.25, 0.55, 0.0])];
const PROC: &[(f32, [f32; 4])] = &[
    (0.00, [4.2, 2.0, 0.55, 0.70]),
    (0.50, [2.0, 0.68, 0.16, 0.42]),
    (1.00, [0.6, 0.10, 0.02, 0.0]),
];

fn gradient_for(kind: ShockwaveGradient, tint: [f32; 3]) -> Gradient {
    let stops = match kind {
        ShockwaveGradient::FireWave => FIRE_WAVE,
        ShockwaveGradient::FireImpact => FIRE_IMPACT,
        ShockwaveGradient::Arcane => ARCANE,
        ShockwaveGradient::Frost => FROST,
        ShockwaveGradient::Proc => PROC,
    };
    gradient_from_tinted_stops(stops, tint)
}

pub fn shockwave_spec(opts: ShockwaveOpts) -> ParticleSpec {
    let size_stops = if opts.mid_t > 0.0 {
        vec![
            (0.00, opts.start_size),
            (opts.mid_t, opts.mid_size),
            (1.00, opts.end_size),
        ]
    } else {
        vec![(0.00, opts.start_size), (1.00, opts.end_size)]
    };

    ParticleSpec {
        spawn: opts.spawn,
        emission: EmissionMode::Burst { count: 1 },
        speed: (0.0, 0.0),
        lifetime: (opts.lifetime, opts.lifetime),
        forces: vec![],
        size: Curve::from_stops(size_stops),
        color: gradient_for(opts.gradient, opts.tint),
        sprite: SpriteShape::Ring,
        blend: BlendMode::Additive,
        opacity: opts.opacity,
        hybrid: None,
        vfx_role: 0,
    }
}

pub fn shockwave(opts: ShockwaveOpts) -> Layer {
    Layer::Particles(shockwave_spec(opts))
}
