//! Player inventory: a fixed [`Loadout`] of equipped items + a
//! variable-size [`Inventory::bag`].

use super::ability_mods::AbilityMods;
use super::item::Item;
use super::items::{EquipSlot, ItemSlot};
use super::stats::StatBlock;

/// All slots a character has, in stable display order.
pub const ALL_SLOTS: &[EquipSlot] = &[
    EquipSlot::Weapon,
    EquipSlot::Helm,
    EquipSlot::Chest,
    EquipSlot::Legs,
    EquipSlot::Hands,
    EquipSlot::Boots,
    EquipSlot::Ring1,
    EquipSlot::Ring2,
    EquipSlot::Amulet,
];

#[derive(Clone, Debug, Default)]
pub struct Loadout {
    weapon: Option<Item>,
    helm: Option<Item>,
    chest: Option<Item>,
    legs: Option<Item>,
    hands: Option<Item>,
    boots: Option<Item>,
    ring1: Option<Item>,
    ring2: Option<Item>,
    amulet: Option<Item>,
}

impl Loadout {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, slot: EquipSlot) -> Option<&Item> {
        match slot {
            EquipSlot::Weapon => self.weapon.as_ref(),
            EquipSlot::Helm => self.helm.as_ref(),
            EquipSlot::Chest => self.chest.as_ref(),
            EquipSlot::Legs => self.legs.as_ref(),
            EquipSlot::Hands => self.hands.as_ref(),
            EquipSlot::Boots => self.boots.as_ref(),
            EquipSlot::Ring1 => self.ring1.as_ref(),
            EquipSlot::Ring2 => self.ring2.as_ref(),
            EquipSlot::Amulet => self.amulet.as_ref(),
        }
    }

    /// Place `item` in `slot`, returning whatever was there before.
    /// Caller is responsible for [`can_equip`] validation if needed.
    pub fn set(&mut self, slot: EquipSlot, item: Option<Item>) -> Option<Item> {
        let dst: &mut Option<Item> = match slot {
            EquipSlot::Weapon => &mut self.weapon,
            EquipSlot::Helm => &mut self.helm,
            EquipSlot::Chest => &mut self.chest,
            EquipSlot::Legs => &mut self.legs,
            EquipSlot::Hands => &mut self.hands,
            EquipSlot::Boots => &mut self.boots,
            EquipSlot::Ring1 => &mut self.ring1,
            EquipSlot::Ring2 => &mut self.ring2,
            EquipSlot::Amulet => &mut self.amulet,
        };
        std::mem::replace(dst, item)
    }

    pub fn iter(&self) -> impl Iterator<Item = (EquipSlot, &Item)> {
        ALL_SLOTS
            .iter()
            .copied()
            .filter_map(move |s| self.get(s).map(|it| (s, it)))
    }

    /// Sum of every equipped item's stat affixes + implicits.
    pub fn total_stats(&self) -> StatBlock {
        let mut total = StatBlock::new();
        for (_, item) in self.iter() {
            total.extend(&item.stats());
        }
        total
    }

    /// Aggregated ability modifiers (Amplify / Modify / Transform /
    /// Trigger) from every equipped item. The combat layer caches
    /// this and re-runs it whenever equipment changes.
    pub fn ability_mods(&self) -> AbilityMods {
        let mut mods = AbilityMods::new();
        for (_, item) in self.iter() {
            for affix in &item.affixes {
                mods.apply(affix);
            }
        }
        mods
    }
}

/// `true` if `item` can legally occupy `slot`. Rings can sit in
/// either ring slot; everything else is exact.
pub fn can_equip(item: &Item, slot: EquipSlot) -> bool {
    let target = item.base.equip_slot;
    if target == slot {
        return true;
    }
    matches!(
        (target, slot),
        (EquipSlot::Ring1, EquipSlot::Ring2) | (EquipSlot::Ring2, EquipSlot::Ring1)
    )
}

#[derive(Clone, Debug, Default)]
pub struct Inventory {
    pub equipped: Loadout,
    pub bag: Vec<Item>,
}

impl Inventory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pick_up(&mut self, item: Item) {
        self.bag.push(item);
    }

    /// Move bag entry `bag_index` into `slot`. Whatever was equipped
    /// goes back to the bag's tail. Returns `false` if the slot
    /// rejects the item type or the index is out of range.
    pub fn equip_from_bag(&mut self, bag_index: usize, slot: EquipSlot) -> bool {
        if bag_index >= self.bag.len() {
            return false;
        }
        if !can_equip(&self.bag[bag_index], slot) {
            return false;
        }
        let item = self.bag.swap_remove(bag_index);
        if let Some(prev) = self.equipped.set(slot, Some(item)) {
            self.bag.push(prev);
        }
        true
    }

    pub fn unequip(&mut self, slot: EquipSlot) -> bool {
        match self.equipped.set(slot, None) {
            Some(item) => {
                self.bag.push(item);
                true
            }
            None => false,
        }
    }

    pub fn total_stats(&self) -> StatBlock {
        self.equipped.total_stats()
    }

    pub fn ability_mods(&self) -> AbilityMods {
        self.equipped.ability_mods()
    }
}

/// Default destination slot for a freshly-rolled item — used by UI
/// hover hints.
pub fn default_slot_for(slot: ItemSlot) -> EquipSlot {
    match slot {
        ItemSlot::Weapon(_) => EquipSlot::Weapon,
        ItemSlot::Armor(_) => EquipSlot::Chest,
        ItemSlot::Accessory(super::items::AccessoryKind::Ring) => EquipSlot::Ring1,
        ItemSlot::Accessory(super::items::AccessoryKind::Amulet) => EquipSlot::Amulet,
    }
}
