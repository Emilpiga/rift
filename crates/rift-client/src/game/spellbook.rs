//! Spellbook host adapter.
//!
//! The visual spellbook lives in `rift-ui`; this module owns the
//! persistent UI state and converts authoritative `rift-game`
//! data into `rift-ui-types` view models each frame.

use rift_engine::ui::im::Ui;
use rift_game::abilities::{Ability, Category};
use rift_game::loadout::{
    is_ability_unlocked, is_slot_unlocked, player_abilities, Loadout, SLOT_COUNT,
    SLOT_UNLOCK_LEVELS,
};
use rift_ui_types::spellbook::{
    SpellbookAbilityView, SpellbookCategoryView, SpellbookSlotView, SpellbookView,
};

pub use rift_ui_types::spellbook::SpellbookAction;

#[derive(Default)]
pub struct SpellbookUi {
    pub state: rift_ui_types::spellbook::SpellbookState,
}

impl SpellbookUi {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&self) -> bool {
        self.state.open
    }

    pub fn toggle(&mut self) {
        self.state.open = !self.state.open;
        if !self.state.open {
            self.state.selected_ability = None;
            self.state.target_slot = None;
        }
    }

    pub fn open_for_slot(&mut self, slot_index: u8) {
        self.state.open = true;
        self.state.target_slot = Some(slot_index);
        self.state.selected_ability = None;
    }

    pub fn close(&mut self) {
        self.state.open = false;
        self.state.selected_ability = None;
        self.state.target_slot = None;
    }

    pub fn frame(
        &mut self,
        ui: &mut Ui<'_>,
        loadout: &Loadout,
        player_level: u32,
        talents: &rift_game::talents::TalentTree,
    ) -> Option<SpellbookAction> {
        if !self.state.open {
            return None;
        }

        let abilities: Vec<SpellbookAbilityView<'static>> = player_abilities()
            .map(|ability| ability_view(ability, talents))
            .filter(|ability| ability.unlocked)
            .collect();
        let categories: Vec<SpellbookCategoryView<'static>> = Category::all()
            .iter()
            .copied()
            .filter_map(|category| {
                let count = category_count(category, &abilities);
                (count > 0).then_some(SpellbookCategoryView {
                    id: category_id(category),
                    label: category.label(),
                    count,
                })
            })
            .collect();
        if !categories
            .iter()
            .any(|category| category.id == self.state.selected_category)
        {
            self.state.selected_category = 0;
        }
        let slots: Vec<SpellbookSlotView<'static>> = (0..SLOT_COUNT)
            .map(|index| {
                let ability_id = loadout.slots[index].raw();
                SpellbookSlotView {
                    index: index as u8,
                    key_label: SLOT_KEYS[index],
                    ability_id: abilities
                        .iter()
                        .any(|ability| ability.id == ability_id)
                        .then_some(ability_id),
                    unlocked: is_slot_unlocked(index, player_level),
                    unlock_level: SLOT_UNLOCK_LEVELS[index],
                }
            })
            .collect();
        let view = SpellbookView {
            player_level,
            categories: &categories,
            abilities: &abilities,
            slots: &slots,
        };
        rift_ui::spellbook::frame_spellbook(ui, &view, &mut self.state)
    }
}

const SLOT_KEYS: [&str; SLOT_COUNT] = ["LMB", "1", "2", "3", "4", "5"];

fn ability_view(
    ability: &'static Ability,
    talents: &rift_game::talents::TalentTree,
) -> SpellbookAbilityView<'static> {
    SpellbookAbilityView {
        id: ability.wire_id.raw(),
        name: ability.name,
        description: ability.description,
        icon: ability.icon,
        category: category_id(ability.category()),
        unlock_level: ability.unlock_level,
        unlocked: is_ability_unlocked(ability.wire_id, talents),
        cooldown: ability.cooldown,
        resource_cost: ability.resource_cost,
        channel_cost_per_sec: ability.channel_cost_per_sec,
        damage_mult: ability.damage_mult,
        projectile_count: ability.projectile_count() as u32,
    }
}

fn category_count(category: Category, abilities: &[SpellbookAbilityView<'_>]) -> usize {
    let id = category_id(category);
    if category == Category::All {
        abilities.len()
    } else {
        abilities
            .iter()
            .filter(|ability| ability.category == id)
            .count()
    }
}

fn category_id(category: Category) -> u8 {
    match category {
        Category::All => 0,
        Category::Fire => 1,
        Category::Cold => 2,
        Category::Lightning => 3,
        Category::Physical => 4,
        Category::Utility => 5,
    }
}
