use super::attributes::AttributeType;

/// Player class — determines primary attribute, base stats, and available abilities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Class {
    Hunter,
    // Future: Warrior, Mage, etc.
}

/// Static configuration for a class.
#[derive(Clone, Debug)]
pub struct ClassConfig {
    pub class: Class,
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

impl Class {
    pub fn config(self) -> ClassConfig {
        match self {
            Class::Hunter => ClassConfig {
                class: Class::Hunter,
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
            },
        }
    }
}
