//! Frost-themed combat presets.

use glam::Vec3;

use crate::renderer::vfx::builder::{
    projectile_trail_frost, EffectBuilder, ImpactTheme, RibbonOpts, StylePreset,
};
use crate::renderer::vfx::spec::*;

/// Frost Ray — channeled cyan beam with hand swirl + dual ribbons.
pub fn frost_ray() -> EffectBundle {
    EffectBuilder::persistent()
        .style(StylePreset::VoidFrost)
        .channel_hand_swirl()
        .ribbon(RibbonOpts::frost_outer())
        .ribbon(RibbonOpts::frost_inner())
        .finish_bundle()
        .with_light(EffectLight {
            color: Vec3::new(1.6, 2.6, 3.6),
            radius: 4.0,
            intensity: 1.4,
            intensity_curve: None,
            lifetime: None,
            flicker_amp: 0.03,
            flicker_hz: 12.0,
            offset: Vec3::new(0.0, 0.15, 0.0),
            follow_particles: true,
        })
        .with_tip_light(EffectLight {
            color: Vec3::new(1.8, 3.0, 4.2),
            radius: 3.5,
            intensity: 1.5,
            intensity_curve: None,
            lifetime: None,
            flicker_amp: 0.04,
            flicker_hz: 14.0,
            offset: Vec3::ZERO,
            follow_particles: true,
        })
}

/// Frost-impact burst at beam pierce / wall clip points.
pub fn frost_impact() -> EffectBundle {
    EffectBuilder::oneshot()
        .style(StylePreset::VoidFrost)
        .beam_tick_impact(ImpactTheme::Frost)
        .finish_bundle()
}

/// Trailing wake for a Frost Shatter shard projectile.
pub fn frost_shard_trail() -> Effect {
    EffectBuilder::persistent()
        .style(StylePreset::VoidFrost)
        .layers(projectile_trail_frost())
        .finish()
}

/// Shard projectile detonation — full frost impact recipe.
pub fn frost_shard_impact() -> Effect {
    EffectBuilder::oneshot()
        .style(StylePreset::VoidFrost)
        .impact_burst(ImpactTheme::Frost)
        .finish()
}
