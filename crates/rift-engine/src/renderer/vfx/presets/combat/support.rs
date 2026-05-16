//! Friendly support presets — heals, buffs, regen auras.

use crate::renderer::vfx::builder::{
    heal_ground_ring, ContinuousOpts, EffectBuilder, SphereBurstOpts,
};
use crate::renderer::vfx::spec::Effect;

/// Heal-burst — single golden-green pulse played on a target
/// that just received an instant heal. Reads as a quick
/// upward shimmer rather than an impact.
pub fn heal_burst() -> Effect {
    EffectBuilder::oneshot()
        .sphere_burst(SphereBurstOpts::heal_sparkles())
        .layer(heal_ground_ring())
        .finish()
}

/// Heal-over-time aura — sustained gentle green sparkle that
/// stays on a target while a `Rejuvenation` buff is ticking.
pub fn heal_over_time_aura() -> Effect {
    EffectBuilder::timed(10.0)
        .continuous(ContinuousOpts::heal_aura())
        .finish()
}
