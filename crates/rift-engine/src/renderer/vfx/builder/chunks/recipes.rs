//! High-level recipes — stack several chunks for common ability shapes.

use crate::renderer::vfx::builder::archetype::ImpactArchetype;
use crate::renderer::vfx::spec::*;

/// Standard projectile detonation (flash + cloud + motes + ground ring).
#[derive(Clone, Copy, Debug)]
pub enum ImpactTheme {
    Fire,
    Frost,
    Arcane,
}

impl ImpactTheme {
    pub fn default_style(self) -> crate::renderer::vfx::style::StylePreset {
        use crate::renderer::vfx::style::StylePreset;
        match self {
            Self::Fire => StylePreset::EmberVoid,
            Self::Frost => StylePreset::VoidFrost,
            Self::Arcane => StylePreset::ArcLightning,
        }
    }
}

pub fn impact_burst_layers(theme: ImpactTheme) -> Vec<Layer> {
    ImpactArchetype::Detonation.layers(theme.default_style())
}

pub fn beam_tick_impact_layers(theme: ImpactTheme) -> Vec<Layer> {
    ImpactArchetype::BeamTick.layers(theme.default_style())
}
