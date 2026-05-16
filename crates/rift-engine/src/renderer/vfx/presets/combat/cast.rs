//! Self-cast / movement spark presets — small one-shot
//! visuals that fire on the caster's body when activating
//! abilities or dodging.

use crate::renderer::vfx::builder::{EffectBuilder, SphereBurstOpts};
use crate::renderer::vfx::spec::Effect;

/// Generic ability cast spark — a small omni-directional burst
/// in the caster's tinted ability colour. Replaces the legacy
/// `EmitterConfig::hit_spark(rgb)` call from the ability runtime.
pub fn cast_spark(rgb: [f32; 3]) -> Effect {
    EffectBuilder::oneshot()
        .sphere_burst(SphereBurstOpts::cast_spark(rgb))
        .finish()
}

/// Evasive-roll puff — a short-lived smoky burst left behind
/// when the player dodges. Replaces `EmitterConfig::dodge_puff`.
pub fn dodge_puff() -> Effect {
    EffectBuilder::oneshot()
        .sphere_burst(SphereBurstOpts::dodge_puff())
        .finish()
}
