use super::attributes::AttributeType;

/// Opaque class identifier. Engine treats this as a hashable key only —
/// concrete IDs (`HUNTER`, `MAGE`, …) are defined by the game crate.
/// Adding a new class is a game-side affair; the engine doesn't change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ClassId(pub &'static str);

/// Static configuration for a class. Constructed by the game crate
/// (one factory function per class) and handed to `PlayerState`.
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
