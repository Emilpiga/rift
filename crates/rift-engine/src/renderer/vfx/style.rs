//! VFX art-direction palette — Phase 1 (Rust-only baking); pairs with
//! [`crate::renderer::vfx::builder::EmissionProfile`] / [`crate::renderer::vfx::builder::MotionProfile`].
//!
//! [`StylePreset`] names an identity (void frost, ember, arc lightning).
//! [`StyleProfile`] holds tunable knobs that reshape gradients, opacity,
//! forces, and lights when [`crate::renderer::vfx::builder::EffectBuilder`]
//! pushes layers. Phase 3 packs presets into per-instance `style_pack` for
//! `evaluate_sprite()` in the particle fragment shader.

use glam::Vec3;

use crate::renderer::vfx::builder::tint::tint_gradient;
use crate::renderer::vfx::spec::{
    Curve, EffectLight, ForceField, Gradient, Layer, ParticleSpec, RibbonSpec,
};

/// GPU preset ids — must match `evaluate_sprite.glsl` / `ribbon_style.glsl`.
pub mod gpu_id {
    pub const LEGACY: f32 = 0.0;
    pub const VOID_FROST: f32 = 1.0;
    pub const EMBER_VOID: f32 = 2.0;
    pub const ARC_LIGHTNING: f32 = 3.0;
}

/// Named visual identity shared across trails, impacts, and beams.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StylePreset {
    /// Cold vacuum frost — cyan core, violet edge, crisp HDR, restless curl.
    VoidFrost,
    /// Ember / infernal heat — warm bias, dense core, high emissive punch.
    EmberVoid,
    /// Arcane lightning — violet-teal, sharp filaments, moderate turbulence.
    ArcLightning,
}

impl StylePreset {
    pub fn profile(self) -> StyleProfile {
        match self {
            Self::VoidFrost => StyleProfile {
                rgb_tint: [0.88, 1.05, 1.22],
                energy: 1.08,
                density: 0.92,
                sharpness: 0.72,
                turbulence: 1.28,
                emissive_bias: 1.38,
                edge_softness: 0.18,
            },
            Self::EmberVoid => StyleProfile {
                rgb_tint: [1.18, 0.92, 0.78],
                energy: 1.12,
                density: 1.05,
                sharpness: 0.55,
                turbulence: 1.15,
                emissive_bias: 1.45,
                edge_softness: 0.32,
            },
            Self::ArcLightning => StyleProfile {
                rgb_tint: [1.05, 0.82, 1.28],
                energy: 1.06,
                density: 0.95,
                sharpness: 0.82,
                turbulence: 1.08,
                emissive_bias: 1.32,
                edge_softness: 0.22,
            },
        }
    }

    /// Numeric id written to the instance buffer (`1`..`3`).
    pub fn gpu_id(self) -> f32 {
        use gpu_id::{ARC_LIGHTNING, EMBER_VOID, VOID_FROST};
        match self {
            Self::VoidFrost => VOID_FROST,
            Self::EmberVoid => EMBER_VOID,
            Self::ArcLightning => ARC_LIGHTNING,
        }
    }

    pub fn gpu_pack(self) -> StyleGpuPack {
        let p = self.profile();
        [self.gpu_id(), p.energy, p.sharpness, p.emissive_bias]
    }

    pub const GPU_NONE: StyleGpuPack = [0.0; 4];
    pub const GPU_AUX_NONE: StyleGpuAux = [0.0; 4];

    pub fn gpu_pack_aux(self) -> StyleGpuAux {
        let p = self.profile();
        [p.turbulence, p.density, p.edge_softness, 0.0]
    }
}

/// Tunable art-direction knobs (Phase 1: baked into layer data).
#[derive(Clone, Copy, Debug)]
pub struct StyleProfile {
    /// Per-channel RGB multiplier on gradients / lights.
    pub rgb_tint: [f32; 3],
    /// Global HDR brightness scale.
    pub energy: f32,
    /// Opacity / alpha density multiplier.
    pub density: f32,
    /// 0 = soft blobs, 1 = crisp cores (scales size curve stops).
    pub sharpness: f32,
    /// Multiplier on curl / turbulence force strength.
    pub turbulence: f32,
    /// Extra punch on already-bright gradient stops (bloom catch).
    pub emissive_bias: f32,
    /// Shader edge falloff — baked into preset constants in GLSL when the
    /// GPU style path is active.
    pub edge_softness: f32,
}

/// Per-instance GPU pack (matches `evaluate_sprite.glsl` unpack).
///
/// `x` = preset id (`0` = legacy), `y` = energy, `z` = sharpness,
/// `w` = emissive bias.
pub type StyleGpuPack = [f32; 4];

/// Secondary instance pack: `x` = turbulence, `y` = density, `z` = edge softness.
pub type StyleGpuAux = [f32; 4];

impl StyleProfile {
    pub fn stylize_layer(self, layer: Layer) -> Layer {
        match layer {
            Layer::Particles(spec) => Layer::Particles(self.stylize_particle_spec(spec)),
            Layer::Ribbon(spec) => Layer::Ribbon(self.stylize_ribbon_spec(spec)),
        }
    }

    /// CPU bake when [`StylePreset`] is also packed into `style_pack` for the GPU.
    ///
    /// Keeps colour tint, opacity density, and motion turbulence; skips mask shaping
    /// (`sharpness` size scale, gradient emissive punch) that `evaluate_sprite()` applies.
    pub fn stylize_layer_gpu(self, layer: Layer) -> Layer {
        match layer {
            Layer::Particles(spec) => Layer::Particles(self.stylize_particle_spec_gpu(spec)),
            Layer::Ribbon(spec) => Layer::Ribbon(self.stylize_ribbon_spec(spec)),
        }
    }

    pub fn stylize_particle_spec(self, mut spec: ParticleSpec) -> ParticleSpec {
        spec.color = self.stylize_gradient(&spec.color);
        spec.opacity = (spec.opacity * self.density).clamp(0.0, 4.0);
        spec.size = self.stylize_size_curve(&spec.size);
        spec.forces = spec
            .forces
            .iter()
            .copied()
            .map(|f| self.stylize_force(f))
            .collect();
        spec
    }

    pub fn stylize_particle_spec_gpu(self, mut spec: ParticleSpec) -> ParticleSpec {
        spec.color = self.stylize_gradient_gpu(&spec.color);
        spec.opacity = (spec.opacity * self.density).clamp(0.0, 4.0);
        spec.forces = spec
            .forces
            .iter()
            .copied()
            .map(|f| self.stylize_force(f))
            .collect();
        spec
    }

    pub fn stylize_ribbon_spec(self, mut spec: RibbonSpec) -> RibbonSpec {
        spec.cross_gradient = self.stylize_gradient(&spec.cross_gradient);
        spec.length_gradient = spec
            .length_gradient
            .as_ref()
            .map(|g| self.stylize_gradient(g));
        if let Some(ref mut noise) = spec.noise {
            noise.strength = (noise.strength * self.turbulence.sqrt()).clamp(0.0, 1.0);
        }
        spec
    }

    pub fn stylize_light(self, mut light: EffectLight) -> EffectLight {
        light.color = Vec3::new(
            light.color.x * self.rgb_tint[0] * self.energy,
            light.color.y * self.rgb_tint[1] * self.energy,
            light.color.z * self.rgb_tint[2] * self.energy,
        ) * self.emissive_bias;
        light.intensity *= self.energy;
        light
    }

    fn stylize_gradient(self, g: &Gradient) -> Gradient {
        let g = tint_gradient(g, self.rgb_tint);
        Gradient {
            stops: g
                .stops
                .iter()
                .map(|s| {
                    let rgba = self.stylize_rgba(s.color);
                    crate::renderer::vfx::spec::GradientStop {
                        t: s.t,
                        color: rgba,
                    }
                })
                .collect(),
        }
    }

    fn stylize_gradient_gpu(self, g: &Gradient) -> Gradient {
        let g = tint_gradient(g, self.rgb_tint);
        Gradient {
            stops: g
                .stops
                .iter()
                .map(|s| {
                    let rgba = self.stylize_rgba_gpu(s.color);
                    crate::renderer::vfx::spec::GradientStop {
                        t: s.t,
                        color: rgba,
                    }
                })
                .collect(),
        }
    }

    fn stylize_rgba(self, c: [f32; 4]) -> [f32; 4] {
        let mut rgb = [
            c[0] * self.energy,
            c[1] * self.energy,
            c[2] * self.energy,
        ];
        let peak = rgb[0].max(rgb[1]).max(rgb[2]);
        if peak > 0.4 {
            rgb = rgb.map(|x| x * self.emissive_bias);
        }
        [rgb[0], rgb[1], rgb[2], (c[3] * self.density).clamp(0.0, 1.0)]
    }

    fn stylize_rgba_gpu(self, c: [f32; 4]) -> [f32; 4] {
        [
            c[0] * self.rgb_tint[0] * self.energy,
            c[1] * self.rgb_tint[1] * self.energy,
            c[2] * self.rgb_tint[2] * self.energy,
            (c[3] * self.density).clamp(0.0, 1.0),
        ]
    }

    fn stylize_size_curve(self, curve: &Curve) -> Curve {
        let scale = 0.82 + self.sharpness * 0.28;
        Curve {
            stops: curve
                .stops
                .iter()
                .map(|s| crate::renderer::vfx::spec::CurveStop {
                    t: s.t,
                    value: s.value * scale,
                })
                .collect(),
        }
    }

    fn stylize_force(self, f: ForceField) -> ForceField {
        match f {
            ForceField::Curl {
                frequency,
                strength,
            } => ForceField::Curl {
                frequency,
                strength: strength * self.turbulence,
            },
            ForceField::Turbulence {
                frequency,
                strength,
            } => ForceField::Turbulence {
                frequency,
                strength: strength * self.turbulence,
            },
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::vfx::spec::Gradient;

    #[test]
    fn void_frost_boosts_cold_channel() {
        let p = StylePreset::VoidFrost.profile();
        let g = Gradient::from_stops([(0.0, [1.0, 2.0, 3.0, 1.0])]);
        let out = p.stylize_gradient(&g);
        assert!(out.stops[0].color[2] > out.stops[0].color[0]);
    }

    #[test]
    fn gpu_pack_encodes_preset_id() {
        let pack = StylePreset::VoidFrost.gpu_pack();
        assert!((pack[0] - 1.0).abs() < 1e-5);
        assert!(pack[1] > 1.0);
        assert!(pack[3] > 1.0);
        assert_eq!(StylePreset::GPU_NONE[0], 0.0);
    }

    #[test]
    fn gpu_pack_aux_carries_motion_knobs() {
        let aux = StylePreset::EmberVoid.gpu_pack_aux();
        assert!(aux[0] > 1.0);
        assert!(aux[1] > 1.0);
        assert!(aux[2] > 0.0);
    }

    #[test]
    fn gpu_authoring_skips_size_sharpness() {
        use crate::renderer::vfx::builder::chunks::{self, FlashOpts};
        use crate::renderer::vfx::spec::Layer;

        let p = StylePreset::VoidFrost.profile();
        let spec = match chunks::flash(FlashOpts::frost()) {
            Layer::Particles(s) => s,
            _ => panic!("expected particle layer"),
        };
        let base_size = spec.size.stops[0].value;
        let full = p.stylize_particle_spec(spec.clone());
        let gpu = p.stylize_particle_spec_gpu(spec);
        assert!((full.size.stops[0].value - base_size).abs() > 0.01);
        assert_eq!(gpu.size.stops[0].value, base_size);
        assert!(gpu.color.stops[0].color[2] > 4.0);
    }
}
