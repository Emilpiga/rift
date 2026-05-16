//! Generic particle layer — escape hatch and base for chunk authors.

use crate::renderer::vfx::spec::*;

/// Full [`ParticleSpec`] description for `.particle()` / [`super::chunks::particle`].
#[derive(Clone, Debug)]
pub struct ParticleOpts {
    pub spawn: SpawnShape,
    pub emission: EmissionMode,
    pub speed: (f32, f32),
    pub lifetime: (f32, f32),
    pub forces: Vec<ForceField>,
    pub size: Curve,
    pub color: Gradient,
    pub sprite: SpriteShape,
    pub blend: BlendMode,
    pub opacity: f32,
    pub hybrid: Option<HybridMaterial>,
}

impl ParticleOpts {
    pub fn into_layer(self) -> Layer {
        Layer::Particles(ParticleSpec {
            spawn: self.spawn,
            emission: self.emission,
            speed: self.speed,
            lifetime: self.lifetime,
            forces: self.forces,
            size: self.size,
            color: self.color,
            sprite: self.sprite,
            blend: self.blend,
            opacity: self.opacity,
            hybrid: self.hybrid,
            vfx_role: 0,
        })
    }
}

impl From<ParticleSpec> for ParticleOpts {
    fn from(s: ParticleSpec) -> Self {
        Self {
            spawn: s.spawn,
            emission: s.emission,
            speed: s.speed,
            lifetime: s.lifetime,
            forces: s.forces,
            size: s.size,
            color: s.color,
            sprite: s.sprite,
            blend: s.blend,
            opacity: s.opacity,
            hybrid: s.hybrid,
        }
    }
}

pub fn particle(opts: impl Into<ParticleOpts>) -> Layer {
    opts.into().into_layer()
}
