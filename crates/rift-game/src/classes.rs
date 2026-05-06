//! Class roster — pure content. Owns `ClassConfig` / `ClassId` shapes,
//! plus the Hunter base config and avatar model paths.

use crate::attributes::AttributeType;
use crate::character::Gender;

/// Opaque class identifier. Treated as a hashable key only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ClassId(pub &'static str);

/// Static configuration for a class.
#[derive(Clone, Debug)]
pub struct ClassConfig {
    pub class: ClassId,
    pub name: &'static str,
    pub primary_attribute: AttributeType,
    /// Base HP at level 1.
    pub base_hp: f32,
    /// HP gained per level.
    pub hp_per_level: f32,
    /// Base damage (before weapon/attributes).
    pub base_damage: f32,
    /// Base defense (before armor/attributes).
    pub base_defense: f32,
    /// Base attack speed (attacks per second).
    pub base_attack_speed: f32,
    /// Base crit chance (0.0 - 1.0).
    pub base_crit_chance: f32,
    /// Base movement speed.
    pub base_move_speed: f32,
    /// Attack range.
    pub base_range: f32,
}

pub const HUNTER: ClassId = ClassId("hunter");

/// Skinned glTF + base albedo for the player avatar, picked by gender.
/// Shared across all classes for now (one rig, multiple outfits via the
/// modular outfit system).
pub fn base_model_paths(gender: Gender) -> (&'static str, &'static str) {
    match gender {
        Gender::Female => (
            "assets/models/base-characters/Base Characters/Godot - UE/Superhero_Female_FullBody.gltf",
            "assets/models/modular-character-outfits/Textures/Base/T_Regular_Female_Dark_BaseColor.png",
        ),
        Gender::Male => (
            "assets/models/base-characters/Base Characters/Godot - UE/Superhero_Male_FullBody.gltf",
            "assets/models/base-characters/Base Characters/Godot - UE/T_Superhero_Male_Dark.png",
        ),
    }
}

pub fn hunter_config() -> ClassConfig {
    ClassConfig {
        class: HUNTER,
        name: "Hunter",
        primary_attribute: AttributeType::Agility,
        base_hp: 80.0,
        hp_per_level: 6.0,
        base_damage: 8.0,
        base_defense: 3.0,
        base_attack_speed: 1.4,
        base_crit_chance: 0.05,
        base_move_speed: 6.0,
        base_range: 12.0,
    }
}

/// Resolve a class id to its static config.
pub fn config_for(class: ClassId) -> ClassConfig {
    match class {
        HUNTER => hunter_config(),
        // New classes here.
        _ => hunter_config(),
    }
}

/// Resolve a wire / persisted class id string back to a `ClassId`.
/// Falls back to [`HUNTER`] for unknown strings — the server uses
/// this on the Hello path so a stale client can't crash us.
pub fn class_from_str(s: &str) -> ClassId {
    match s {
        "hunter" => HUNTER,
        _ => HUNTER,
    }
}
