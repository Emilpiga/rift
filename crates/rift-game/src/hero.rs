//! Hero — single, universal player archetype.
//!
//! There is one player class. Customization happens through the
//! ability loadout (see [`crate::abilities`]) and gear, not class
//! choice. This module owns the base stat block ([`HeroConfig`])
//! that the stat / damage formulas read, plus the avatar model
//! paths used at spawn.

use crate::attributes::AttributeType;
use crate::character::Gender;

/// Static configuration for the hero archetype. All stat formulas
/// (HP, damage, crit, move speed, …) read from this single
/// constant.
#[derive(Clone, Debug)]
pub struct HeroConfig {
    pub name: &'static str,
    pub primary_attribute: AttributeType,
    /// Base HP at level 1.
    pub base_hp: f32,
    /// HP gained per level.
    pub hp_per_level: f32,
    /// Base essence pool (the universal ability resource) at
    /// level 1.
    pub base_resource: f32,
    /// Essence pool gained per level.
    pub resource_per_level: f32,
    /// Essence per second restored while the player is not
    /// actively spending. Server pauses regen briefly after
    /// every cast / channel tick (see
    /// `ServerPlayer::resource_regen_pause`).
    pub base_resource_regen: f32,
    /// Baseline passive HP regen per second before any
    /// gear / talents. Intentionally tiny — enough that an
    /// out-of-combat player slowly tops off rather than being
    /// stuck at low HP forever, but never a substitute for
    /// healing skills or potions. Gear / talents stack on top
    /// via `Stat::HealthRegen`.
    pub base_health_regen: f32,
    /// Base damage (before weapon/attributes).
    pub base_damage: f32,
    /// Base attack speed (attacks per second).
    pub base_attack_speed: f32,
    /// Base crit chance (0.0 - 1.0).
    pub base_crit_chance: f32,
    /// Base movement speed.
    pub base_move_speed: f32,
}

/// The single player config. Stats and damage formulas all
/// derive from this constant. Tweak here to globally rebalance
/// hero baseline values.
pub const HERO: HeroConfig = HeroConfig {
    name: "Hero",
    primary_attribute: AttributeType::Agility,
    base_hp: 80.0,
    hp_per_level: 6.0,
    base_resource: 100.0,
    resource_per_level: 5.0,
    base_resource_regen: 8.0,
    // ~0.5 HP/s on an 80 HP baseline — roughly a full bar in
    // ~160 s of standing still. Slow enough that combat
    // healing still matters, fast enough that exploration
    // between rooms doesn't feel like an HP penalty.
    base_health_regen: 0.5,
    base_damage: 8.0,
    base_attack_speed: 1.4,
    base_crit_chance: 0.05,
    base_move_speed: 6.0,
};

/// Skinned glTF + base albedo for the player avatar, picked by
/// gender. Shared across every player; visual variety comes
/// from the modular outfit system, not class.
pub fn base_model_paths(gender: Gender) -> (&'static str, &'static str) {
    match gender {
        Gender::Female => (
            "assets/models/base-characters/Base Characters/Godot - UE/Superhero_Female_FullBody.gltf",
            "assets/models/base-characters/Base Characters/Godot - UE/T_Superhero_Female_Dark_BaseColor.png",
        ),
        Gender::Male => (
            "assets/models/base-characters/Base Characters/Godot - UE/Superhero_Male_FullBody.gltf",
            "assets/models/base-characters/Base Characters/Godot - UE/T_Superhero_Male_Dark.png",
        ),
    }
}

/// Convenience accessor for the (only) hero config.
pub fn config() -> &'static HeroConfig {
    &HERO
}
