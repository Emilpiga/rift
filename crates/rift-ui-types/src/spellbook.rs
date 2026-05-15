//! Spellbook view models + action/state types.
//!
//! The host flattens `rift-game` ability/loadout/talent data into
//! these plain structs. `rift_ui::spellbook` owns only rendering and
//! returns [`SpellbookAction`] intents.

#[derive(Clone, Copy, Debug, Default)]
pub struct SpellbookState {
    pub open: bool,
    pub selected_ability: Option<u8>,
    pub target_slot: Option<u8>,
    pub selected_category: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct SpellbookCategoryView<'a> {
    pub id: u8,
    pub label: &'a str,
    pub count: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct SpellbookAbilityView<'a> {
    pub id: u8,
    pub name: &'a str,
    pub description: &'a str,
    pub icon: Option<&'a str>,
    pub category: u8,
    pub unlock_level: u32,
    pub unlocked: bool,
    pub cooldown: f32,
    pub resource_cost: f32,
    pub channel_cost_per_sec: f32,
    pub damage_mult: f32,
    pub effective_damage: f32,
    pub avg_damage: f32,
    pub crit_chance: f32,
    pub crit_damage: f32,
    pub projectile_count: u32,
    pub pierce_count: u32,
    pub minion_count: u32,
    pub minion_damage: f32,
    pub minion_health: f32,
    pub minion_attack_interval: f32,
    pub minion_duration: f32,
    pub minion_inherits_crit: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct SpellbookSlotView<'a> {
    pub index: u8,
    pub key_label: &'a str,
    pub ability_id: Option<u8>,
    pub unlocked: bool,
    pub unlock_level: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct SpellbookView<'a> {
    pub player_level: u32,
    pub categories: &'a [SpellbookCategoryView<'a>],
    pub abilities: &'a [SpellbookAbilityView<'a>],
    pub slots: &'a [SpellbookSlotView<'a>],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpellbookAction {
    AssignSlot { slot_index: u8, ability_id: u8 },
}
