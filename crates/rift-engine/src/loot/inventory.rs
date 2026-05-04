use super::item::{Item, ItemKind, ItemSlot};

/// Player's inventory: equipment slots + backpack.
#[derive(Clone, Debug, Default)]
pub struct Inventory {
    pub backpack: Vec<Item>,
    pub max_backpack_size: usize,
}

impl Inventory {
    pub fn new() -> Self {
        Self {
            backpack: Vec::new(),
            max_backpack_size: 20,
        }
    }

    /// Try to add an item to the backpack. Returns false if full.
    pub fn add_item(&mut self, item: Item) -> bool {
        if self.backpack.len() >= self.max_backpack_size {
            return false;
        }
        self.backpack.push(item);
        true
    }

    /// Remove item at index from backpack. Returns the item.
    pub fn remove_item(&mut self, index: usize) -> Option<Item> {
        if index < self.backpack.len() {
            Some(self.backpack.remove(index))
        } else {
            None
        }
    }
}

/// The player's equipped items (one per slot).
#[derive(Clone, Debug, Default)]
pub struct Equipment {
    pub weapon: Option<Item>,
    pub helmet: Option<Item>,
    pub chest: Option<Item>,
    pub boots: Option<Item>,
    pub ring: Option<Item>,
    pub amulet: Option<Item>,
}

impl Equipment {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get item in a slot.
    pub fn get(&self, slot: ItemSlot) -> Option<&Item> {
        match slot {
            ItemSlot::Weapon => self.weapon.as_ref(),
            ItemSlot::Helmet => self.helmet.as_ref(),
            ItemSlot::Chest => self.chest.as_ref(),
            ItemSlot::Boots => self.boots.as_ref(),
            ItemSlot::Ring => self.ring.as_ref(),
            ItemSlot::Amulet => self.amulet.as_ref(),
        }
    }

    /// Equip an item, returning whatever was previously in that slot.
    pub fn equip(&mut self, item: Item) -> Option<Item> {
        let slot = match item.base.kind {
            ItemKind::Equipment(slot) => slot,
            _ => return Some(item), // Can't equip non-equipment
        };

        let slot_ref = match slot {
            ItemSlot::Weapon => &mut self.weapon,
            ItemSlot::Helmet => &mut self.helmet,
            ItemSlot::Chest => &mut self.chest,
            ItemSlot::Boots => &mut self.boots,
            ItemSlot::Ring => &mut self.ring,
            ItemSlot::Amulet => &mut self.amulet,
        };

        let old = slot_ref.take();
        *slot_ref = Some(item);
        old
    }

    /// Unequip item from a slot, returning it.
    pub fn unequip(&mut self, slot: ItemSlot) -> Option<Item> {
        let slot_ref = match slot {
            ItemSlot::Weapon => &mut self.weapon,
            ItemSlot::Helmet => &mut self.helmet,
            ItemSlot::Chest => &mut self.chest,
            ItemSlot::Boots => &mut self.boots,
            ItemSlot::Ring => &mut self.ring,
            ItemSlot::Amulet => &mut self.amulet,
        };
        slot_ref.take()
    }

    /// Compute total stats from all equipped items.
    pub fn total_stats(&self) -> PlayerStats {
        let mut stats = PlayerStats::default();
        let all_slots = [
            &self.weapon, &self.helmet, &self.chest,
            &self.boots, &self.ring, &self.amulet,
        ];

        for slot in &all_slots {
            if let Some(item) = slot {
                stats.flat_damage += item.total_damage();
                stats.flat_defense += item.total_defense();
                stats.percent_damage += item.affixes.iter().map(|a| a.percent_damage()).sum::<f32>();
                stats.move_speed_pct += item.speed_bonus_pct();
                stats.attack_speed_pct += item.attack_speed_pct();
                stats.crit_chance += item.crit_chance();
                stats.max_hp_bonus += item.max_hp_bonus();
                stats.hp_regen += item.hp_regen();
                stats.damage_reduction += item.affixes.iter().map(|a| a.damage_reduction()).sum::<f32>();
                stats.life_on_hit += item.affixes.iter().map(|a| a.life_on_hit()).sum::<f32>();
            }
        }

        // Cap certain stats
        stats.crit_chance = stats.crit_chance.min(0.75); // 75% max crit
        stats.damage_reduction = stats.damage_reduction.min(0.75); // 75% max DR
        stats.move_speed_pct = stats.move_speed_pct.min(100.0); // 100% max MS bonus

        stats
    }
}

/// Aggregated player stats from equipment.
#[derive(Clone, Debug, Default)]
pub struct PlayerStats {
    pub flat_damage: f32,
    pub flat_defense: f32,
    pub percent_damage: f32,
    pub move_speed_pct: f32,
    pub attack_speed_pct: f32,
    pub crit_chance: f32,
    pub max_hp_bonus: f32,
    pub hp_regen: f32,
    pub damage_reduction: f32,
    pub life_on_hit: f32,
}

impl PlayerStats {
    /// Compute final damage for an attack given base damage.
    pub fn compute_damage(&self, base_damage: f32, is_crit: bool) -> f32 {
        let total = (base_damage + self.flat_damage) * (1.0 + self.percent_damage);
        if is_crit {
            total * 2.0 // Base crit multiplier
        } else {
            total
        }
    }

    /// Compute damage reduction (incoming damage multiplier).
    pub fn incoming_damage_mult(&self) -> f32 {
        (1.0 - self.damage_reduction).max(0.25) // Never reduce more than 75%
    }

    /// Effective max HP.
    pub fn effective_max_hp(&self, base_max: f32) -> f32 {
        base_max + self.max_hp_bonus
    }

    /// Effective move speed multiplier.
    pub fn move_speed_mult(&self) -> f32 {
        1.0 + self.move_speed_pct / 100.0
    }

    /// Effective attack cooldown multiplier (lower = faster).
    pub fn attack_cooldown_mult(&self) -> f32 {
        1.0 / (1.0 + self.attack_speed_pct / 100.0)
    }
}
