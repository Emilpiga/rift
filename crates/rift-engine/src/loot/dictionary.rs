//! Loot Dictionary — defines all legendary/set items and set bonuses.
//!
//! Add new items here. The generation system picks from this dictionary
//! when rolling legendary+ items.

use super::item::*;

/// A dictionary entry for a unique/legendary item.
#[derive(Clone, Debug)]
pub struct LegendaryDef {
    pub id: &'static str,
    pub name: &'static str,
    pub slot: ItemSlot,
    pub base_damage_or_defense: f32,
    pub icon: ItemIcon,
    pub model: ItemModel,
    pub power: LegendaryPower,
    pub set_id: Option<SetId>,
    /// Minimum floor level to drop.
    pub min_floor: u32,
    /// Relative drop weight (higher = more common among legendaries).
    pub weight: u32,
}

/// A set definition with its bonuses.
#[derive(Clone, Debug)]
pub struct SetDef {
    pub id: SetId,
    pub name: &'static str,
    pub bonuses: &'static [SetBonus],
}

/// The global loot dictionary.
pub struct LootDictionary {
    pub legendaries: Vec<LegendaryDef>,
    pub sets: Vec<SetDef>,
}

impl LootDictionary {
    /// Build the complete loot dictionary.
    pub fn new() -> Self {
        Self {
            legendaries: build_legendaries(),
            sets: build_sets(),
        }
    }

    /// Get all legendaries that can drop for a given slot and floor.
    pub fn legendaries_for(&self, slot: ItemSlot, floor: u32) -> Vec<&LegendaryDef> {
        self.legendaries
            .iter()
            .filter(|l| l.slot == slot && floor >= l.min_floor)
            .collect()
    }

    /// Get a specific legendary by ID.
    pub fn get_legendary(&self, id: &str) -> Option<&LegendaryDef> {
        self.legendaries.iter().find(|l| l.id == id)
    }

    /// Get set definition by ID.
    pub fn get_set(&self, id: SetId) -> Option<&SetDef> {
        self.sets.iter().find(|s| s.id == id)
    }

    /// Count how many equipped items belong to a set.
    pub fn count_set_pieces(set_id: SetId, equipped: &[&Item]) -> u8 {
        equipped.iter().filter(|item| item.set_id == Some(set_id)).count() as u8
    }

    /// Get all active set bonuses for currently equipped items.
    pub fn active_set_bonuses(&self, equipped: &[&Item]) -> Vec<(&SetDef, &SetBonus)> {
        let mut results = Vec::new();
        for set_def in &self.sets {
            let count = Self::count_set_pieces(set_def.id, equipped);
            for bonus in set_def.bonuses {
                if count >= bonus.pieces_required {
                    results.push((set_def, bonus));
                }
            }
        }
        results
    }
}

// ─── Legendary Definitions ────────────────────────────────────────────────────

fn build_legendaries() -> Vec<LegendaryDef> {
    vec![
        // ─── Weapons ─────────────────────────────────────────────────────
        LegendaryDef {
            id: "void_blade",
            name: "Void Blade",
            slot: ItemSlot::Weapon,
            base_damage_or_defense: 25.0,
            icon: ItemIcon::VoidBlade,
            model: ItemModel::VoidBlade,
            power: LegendaryPower::PiercingShots(2),
            set_id: Some(SetId::VoidWalker),
            min_floor: 3,
            weight: 10,
        },
        LegendaryDef {
            id: "storm_reaper",
            name: "Storm Reaper",
            slot: ItemSlot::Weapon,
            base_damage_or_defense: 22.0,
            icon: ItemIcon::Sword,
            model: ItemModel::StormBow,
            power: LegendaryPower::ChainLightning(150.0),
            set_id: Some(SetId::StormCaller),
            min_floor: 5,
            weight: 8,
        },
        LegendaryDef {
            id: "serpent_fang",
            name: "Serpent's Fang",
            slot: ItemSlot::Weapon,
            base_damage_or_defense: 18.0,
            icon: ItemIcon::SerpentFang,
            model: ItemModel::LegendarySword,
            power: LegendaryPower::SplitShot(30.0),
            set_id: None,
            min_floor: 2,
            weight: 12,
        },
        LegendaryDef {
            id: "executioners_edge",
            name: "Executioner's Edge",
            slot: ItemSlot::Weapon,
            base_damage_or_defense: 30.0,
            icon: ItemIcon::Greatsword,
            model: ItemModel::BasicAxe,
            power: LegendaryPower::Executioner(50.0),
            set_id: None,
            min_floor: 7,
            weight: 6,
        },
        LegendaryDef {
            id: "ricochet_bow",
            name: "Whisperwind",
            slot: ItemSlot::Weapon,
            base_damage_or_defense: 16.0,
            icon: ItemIcon::Bow,
            model: ItemModel::StormBow,
            power: LegendaryPower::Ricochet(40.0),
            set_id: None,
            min_floor: 4,
            weight: 10,
        },

        // ─── Helmets ─────────────────────────────────────────────────────
        LegendaryDef {
            id: "storm_crown",
            name: "Crown of Tempests",
            slot: ItemSlot::Helmet,
            base_damage_or_defense: 12.0,
            icon: ItemIcon::StormCrown,
            model: ItemModel::CrystalCrown,
            power: LegendaryPower::Haste(20.0),
            set_id: Some(SetId::StormCaller),
            min_floor: 5,
            weight: 8,
        },
        LegendaryDef {
            id: "void_mask",
            name: "Mask of the Void",
            slot: ItemSlot::Helmet,
            base_damage_or_defense: 10.0,
            icon: ItemIcon::PlateHelm,
            model: ItemModel::IronHelm,
            power: LegendaryPower::PackHunter(5.0),
            set_id: Some(SetId::VoidWalker),
            min_floor: 3,
            weight: 10,
        },

        // ─── Chest ───────────────────────────────────────────────────────
        LegendaryDef {
            id: "phoenix_heart",
            name: "Phoenix Heart",
            slot: ItemSlot::Chest,
            base_damage_or_defense: 18.0,
            icon: ItemIcon::PhoenixHeart,
            model: ItemModel::PhoenixPlate,
            power: LegendaryPower::ExplosiveDeath(200.0),
            set_id: Some(SetId::PhoenixAscent),
            min_floor: 6,
            weight: 7,
        },
        LegendaryDef {
            id: "blood_oath_vest",
            name: "Blood Oath Vestments",
            slot: ItemSlot::Chest,
            base_damage_or_defense: 14.0,
            icon: ItemIcon::Chainmail,
            model: ItemModel::ChainArmor,
            power: LegendaryPower::LifeSteal(8.0),
            set_id: Some(SetId::BloodOath),
            min_floor: 4,
            weight: 9,
        },

        // ─── Boots ───────────────────────────────────────────────────────
        LegendaryDef {
            id: "frostbite_greaves",
            name: "Frostbite Greaves",
            slot: ItemSlot::Boots,
            base_damage_or_defense: 8.0,
            icon: ItemIcon::FrostbiteGreaves,
            model: ItemModel::PlateArmor,
            power: LegendaryPower::FrostNova(15.0, 1.5),
            set_id: Some(SetId::FrostSentinel),
            min_floor: 4,
            weight: 9,
        },
        LegendaryDef {
            id: "windrunner_boots",
            name: "Windrunner Treads",
            slot: ItemSlot::Boots,
            base_damage_or_defense: 6.0,
            icon: ItemIcon::LeatherBoots,
            model: ItemModel::LeatherArmor,
            power: LegendaryPower::WindRunner(25.0),
            set_id: None,
            min_floor: 2,
            weight: 12,
        },

        // ─── Rings ───────────────────────────────────────────────────────
        LegendaryDef {
            id: "soulbound_ring",
            name: "Soulbound Circle",
            slot: ItemSlot::Ring,
            base_damage_or_defense: 0.0,
            icon: ItemIcon::SoulboundRing,
            model: ItemModel::None,
            power: LegendaryPower::DodgeRoll(25.0),
            set_id: Some(SetId::BloodOath),
            min_floor: 3,
            weight: 10,
        },

        // ─── Amulets ─────────────────────────────────────────────────────
        LegendaryDef {
            id: "phoenix_pendant",
            name: "Phoenix Feather Pendant",
            slot: ItemSlot::Amulet,
            base_damage_or_defense: 0.0,
            icon: ItemIcon::Amulet,
            model: ItemModel::None,
            power: LegendaryPower::ExplosiveDeath(100.0),
            set_id: Some(SetId::PhoenixAscent),
            min_floor: 6,
            weight: 7,
        },
    ]
}

// ─── Set Definitions ──────────────────────────────────────────────────────────

fn build_sets() -> Vec<SetDef> {
    vec![
        SetDef {
            id: SetId::VoidWalker,
            name: "Void Walker",
            bonuses: &[
                SetBonus {
                    pieces_required: 2,
                    description: "+15% damage, +10% movement speed",
                    effect: SetEffect::StatBonus { damage_pct: 15.0, defense_pct: 0.0, speed_pct: 10.0 },
                },
                SetBonus {
                    pieces_required: 3,
                    description: "Arrows pierce +3 additional targets",
                    effect: SetEffect::GrantPower(LegendaryPower::PiercingShots(3)),
                },
            ],
        },
        SetDef {
            id: SetId::StormCaller,
            name: "Storm Caller",
            bonuses: &[
                SetBonus {
                    pieces_required: 2,
                    description: "+20% attack speed, +10% damage",
                    effect: SetEffect::StatBonus { damage_pct: 10.0, defense_pct: 0.0, speed_pct: 0.0 },
                },
                SetBonus {
                    pieces_required: 3,
                    description: "Every 3rd hit chains lightning to nearby enemies",
                    effect: SetEffect::GrantPower(LegendaryPower::ChainLightning(200.0)),
                },
            ],
        },
        SetDef {
            id: SetId::BloodOath,
            name: "Blood Oath",
            bonuses: &[
                SetBonus {
                    pieces_required: 2,
                    description: "+25% defense, crits heal for 12 HP",
                    effect: SetEffect::StatBonus { damage_pct: 0.0, defense_pct: 25.0, speed_pct: 0.0 },
                },
                SetBonus {
                    pieces_required: 3,
                    description: "Life steal doubled",
                    effect: SetEffect::GrantPower(LegendaryPower::LifeSteal(16.0)),
                },
            ],
        },
        SetDef {
            id: SetId::FrostSentinel,
            name: "Frost Sentinel",
            bonuses: &[
                SetBonus {
                    pieces_required: 2,
                    description: "+20% defense, 10% freeze chance on hit",
                    effect: SetEffect::StatBonus { damage_pct: 0.0, defense_pct: 20.0, speed_pct: 0.0 },
                },
            ],
        },
        SetDef {
            id: SetId::PhoenixAscent,
            name: "Phoenix Ascent",
            bonuses: &[
                SetBonus {
                    pieces_required: 2,
                    description: "+30% damage, kills explode for 200% weapon damage",
                    effect: SetEffect::GrantPower(LegendaryPower::ExplosiveDeath(200.0)),
                },
            ],
        },
    ]
}
