//! Engine-side combat runtime.
//!
//! Declarative ability/class/talent/attribute/experience types live in
//! `rift_game`. This module only contains the engine-side pieces:
//!  - `ability_runtime`: client-side cast FSM + particle dispatch
//!  - `projectile`: in-engine arrow geometry (rendering only)

pub mod ability_runtime;
pub mod projectile;

pub use ability_runtime::{
    effect_for_vfx, execute_ability, execute_ability_instant, execute_ability_placed,
    mesh_for_kind, AbilityCtx,
};
pub use projectile::{Projectile, ProjectileKind};
