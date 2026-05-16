//! Arcane / violet ranged-attack presets — used for caster
//! enemy projectiles.

use crate::renderer::vfx::builder::{
    projectile_trail_arcane, EffectBuilder, ImpactTheme, StylePreset,
};
use crate::renderer::vfx::spec::Effect;

/// Caster bolt trail — enemy ranged-attack visual.
pub fn arcane_bolt_trail() -> Effect {
    EffectBuilder::persistent()
        .style(StylePreset::ArcLightning)
        .layers(projectile_trail_arcane())
        .finish()
}

/// Arcane bolt impact — flash, cloud, sparks, ground ring.
pub fn arcane_bolt_impact() -> Effect {
    EffectBuilder::oneshot()
        .style(StylePreset::ArcLightning)
        .impact_burst(ImpactTheme::Arcane)
        .finish()
}
