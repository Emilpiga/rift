pub mod class;
pub mod attributes;
pub mod experience;
pub mod ability;
pub mod ability_runtime;
pub mod debuff;
pub mod talent;
pub mod projectile;
pub mod projectile_pool;

pub use class::{ClassId, ClassConfig};
pub use attributes::{Attributes, AttributeType, AttributeScaling};
pub use experience::{Experience, LevelUpReward};
pub use ability::{Ability, AbilityId, AbilitySlot, AbilityState, TargetingMode};
pub use ability_runtime::{
    execute_ability, execute_ability_instant, execute_ability_placed, AbilityCtx, AbilityEffect,
    ActionMovement, ParticlePreset, SpawnOffset,
};
pub use debuff::{
    apply_damage, cleanup_debuff_visuals, debuff_tick_system, Debuff, DebuffKind, DebuffVisual,
    Debuffs,
};
pub use talent::{TalentTree, TalentNode, TalentId};
pub use projectile::{Projectile, ProjectileKind};
pub use projectile_pool::{ProjectilePool, AoeZone};
