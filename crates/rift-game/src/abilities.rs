//! Hunter ability roster — pure content. The engine owns `Ability`,
//! `AbilityEffect`, `Debuff`, and the runtime that interprets them; this
//! module just declares which abilities the Hunter class has.
//!
//! Other classes get their own module here (e.g. `mage.rs`,
//! `paladin.rs`) following the same pattern.

use rift_engine::combat::ability::{Ability, AbilityId, TargetingMode};
use rift_engine::combat::ability_runtime::{
    AbilityEffect, ActionMovement, ParticlePreset, SpawnOffset,
};
use rift_engine::combat::debuff::Debuff;
use rift_engine::ecs::components::PlayerAction;

// ─── Ability identifiers ──────────────────────────────────────────────────
//
// These constants are the source of truth for ability IDs. Talents and
// gameplay code should reference these instead of constructing
// `AbilityId(...)` directly.

pub const STEADY_SHOT: AbilityId = AbilityId("steady_shot");
pub const MULTI_SHOT: AbilityId = AbilityId("multi_shot");
pub const RAPID_FIRE: AbilityId = AbilityId("rapid_fire");
pub const RAIN_OF_ARROWS: AbilityId = AbilityId("rain_of_arrows");
pub const EVASIVE_ROLL: AbilityId = AbilityId("evasive_roll");
pub const MARK_FOR_DEATH: AbilityId = AbilityId("mark_for_death");

// ─── Hunter abilities ─────────────────────────────────────────────────────

pub fn steady_shot() -> Ability {
    Ability {
        id: STEADY_SHOT,
        name: "Steady Shot",
        description: "Fire a precise arrow at the target.",
        cooldown: 0.5,
        resource_cost: 0.0,
        damage_mult: 1.0,
        projectile_count: 1,
        spread_angle: 0.0,
        range: 12.0,
        unlock_level: 1,
        duration: 0.0,
        targeting: TargetingMode::Instant,
        effects: &[AbilityEffect::SpawnProjectiles {
            count: 1,
            spread: 0.0,
            damage_mult: 1.0,
            pierce: 0,
            spawn_offset: SpawnOffset::HAND,
        }],
    }
}

pub fn multi_shot() -> Ability {
    Ability {
        id: MULTI_SHOT,
        name: "Multi-Shot",
        description: "Fire 3 arrows in a wide spread.",
        cooldown: 4.0,
        resource_cost: 15.0,
        damage_mult: 0.7,
        projectile_count: 3,
        spread_angle: 0.5,
        range: 10.0,
        unlock_level: 3,
        duration: 0.0,
        targeting: TargetingMode::Instant,
        effects: &[AbilityEffect::SpawnProjectiles {
            count: 3,
            spread: 0.5,
            damage_mult: 0.7,
            pierce: 0,
            spawn_offset: SpawnOffset::HAND,
        }],
    }
}

pub fn rapid_fire() -> Ability {
    Ability {
        id: RAPID_FIRE,
        name: "Rapid Fire",
        description: "Channel a burst of 6 rapid arrows.",
        cooldown: 8.0,
        resource_cost: 25.0,
        damage_mult: 0.5,
        projectile_count: 6,
        spread_angle: 0.08,
        range: 12.0,
        unlock_level: 7,
        duration: 1.0,
        targeting: TargetingMode::Instant,
        effects: &[AbilityEffect::SpawnProjectiles {
            count: 6,
            spread: 0.08,
            damage_mult: 0.5,
            pierce: 0,
            spawn_offset: SpawnOffset::HAND,
        }],
    }
}

pub fn rain_of_arrows() -> Ability {
    Ability {
        id: RAIN_OF_ARROWS,
        name: "Rain of Arrows",
        description: "Call down a rain of arrows in an area.",
        cooldown: 12.0,
        resource_cost: 35.0,
        damage_mult: 0.4,
        projectile_count: 12,
        spread_angle: 0.0,
        range: 15.0,
        unlock_level: 12,
        duration: 2.0,
        targeting: TargetingMode::Placed { radius: 3.0 },
        effects: &[AbilityEffect::SpawnAoeZone {
            radius: 3.0,
            damage_mult: 0.4,
            duration: 2.0,
            tick_interval: 0.5,
            visual: Some(ParticlePreset::RainOfArrows),
            visual_y: 5.0,
        }],
    }
}

pub fn evasive_roll() -> Ability {
    Ability {
        id: EVASIVE_ROLL,
        name: "Evasive Roll",
        description: "Dodge roll in movement direction. Brief invulnerability.",
        cooldown: 6.0,
        resource_cost: 0.0,
        damage_mult: 0.0,
        projectile_count: 0,
        spread_angle: 0.0,
        range: 0.0,
        unlock_level: 5,
        duration: 0.3,
        targeting: TargetingMode::Instant,
        effects: &[AbilityEffect::SetPlayerAction {
            action: PlayerAction::Roll,
            duration: 0.95,
            clip: &["Roll", "Roll_Forward", "Dodge_Roll", "Dodge"],
            movement: ActionMovement::Forward(11.0),
            cancel_cast: true,
            emitter: Some(ParticlePreset::DodgePuff),
        }],
    }
}

pub fn mark_for_death() -> Ability {
    Ability {
        id: MARK_FOR_DEATH,
        name: "Mark for Death",
        description: "Mark target. They take 25% increased damage for 6s.",
        cooldown: 15.0,
        resource_cost: 20.0,
        damage_mult: 0.0,
        projectile_count: 0,
        spread_angle: 0.0,
        range: 20.0,
        unlock_level: 10,
        duration: 6.0,
        targeting: TargetingMode::Instant,
        effects: &[AbilityEffect::ApplyDebuff {
            radius: 3.0,
            debuff: || Debuff::mark_for_death(6.0),
        }],
    }
}

/// Full hunter roster, ordered for the action bar.
pub fn hunter_roster() -> [Ability; 6] {
    [
        steady_shot(),
        multi_shot(),
        evasive_roll(),
        rapid_fire(),
        mark_for_death(),
        rain_of_arrows(),
    ]
}
