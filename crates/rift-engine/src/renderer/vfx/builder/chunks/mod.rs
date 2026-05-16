//! Reusable VFX layer chunks.

mod beam;
mod burst;
mod loot;
mod recipes;
mod shockwave;
mod smoke;
mod trail;

pub use beam::{fire_beam_ribbons, frost_beam_ribbons, ribbon, RibbonOpts};
pub use burst::{
    flash, heal_ground_ring, plasma_core, radial_burst, shard_burst, sky_portal_ring, spark_burst,
    sphere_burst, streak_burst, FlashOpts, PlasmaCoreOpts, RadialBurstOpts, ShardBurstOpts,
    SparkBurstOpts, SphereBurstOpts, StreakBurstOpts,
};
pub use loot::{loot_beam_base_layer, loot_beam_layers};
pub use recipes::{
    beam_tick_impact_layers, impact_burst_layers, ImpactTheme,
};
pub use shockwave::{shockwave, shockwave_spec, ShockwaveOpts};
pub use smoke::{
    smoke_billow, smoke_residue, smoke_wake, SmokeBillowOpts, SmokeResidueOpts, SmokeWakeOpts,
};
pub use trail::{
    continuous, projectile_trail_arcane, projectile_trail_fire, projectile_trail_frost,
    ContinuousOpts,
};

pub use super::particle::{particle, ParticleOpts};
