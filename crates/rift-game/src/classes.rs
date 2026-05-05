//! Class roster — pure content. The engine owns the `ClassConfig`
//! shape; this module declares which classes the game ships and their
//! base stats. New classes get added here without touching the engine.

use rift_engine::combat::{ClassConfig, ClassId};
use rift_engine::combat::attributes::AttributeType;

use crate::character::Gender;

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
