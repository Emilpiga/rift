use super::affix::{Affix, AffixPool, AffixTier};
use super::dictionary::LootDictionary;
use super::item::{Item, ItemBase, ItemIcon, ItemKind, ItemModel, ItemRarity, ItemSlot, PotionType};

/// Seeded RNG for item generation.
pub(crate) struct ItemRng {
    state: u64,
}

impl ItemRng {
    fn new(seed: u64) -> Self {
        Self { state: if seed == 0 { 1 } else { seed } }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.state
    }

    /// Random f32 in [0.0, 1.0).
    fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Random f32 in [min, max].
    fn range_f32(&mut self, min: f32, max: f32) -> f32 {
        min + self.next_f32() * (max - min)
    }

    /// Random u32 in [0, max).
    fn range_u32(&mut self, max: u32) -> u32 {
        (self.next_u64() % max as u64) as u32
    }

    /// Weighted selection from a list of items. Returns index.
    fn weighted_select(&mut self, weights: &[u32]) -> usize {
        let total: u32 = weights.iter().sum();
        if total == 0 { return 0; }
        let mut roll = self.range_u32(total);
        for (i, &w) in weights.iter().enumerate() {
            if roll < w {
                return i;
            }
            roll -= w;
        }
        weights.len() - 1
    }
}

/// Roll a rarity based on floor level and luck.
fn roll_rarity(rng: &mut ItemRng, floor: u32) -> ItemRarity {
    // Higher floors increase rare+ chances
    let luck_bonus = (floor as f32 * 2.0).min(20.0);
    let roll = rng.next_f32() * 100.0;

    if roll < 1.0 + luck_bonus * 0.1 {
        ItemRarity::Legendary
    } else if roll < 5.0 + luck_bonus * 0.3 {
        ItemRarity::Epic
    } else if roll < 15.0 + luck_bonus * 0.5 {
        ItemRarity::Rare
    } else if roll < 45.0 + luck_bonus {
        ItemRarity::Magic
    } else {
        ItemRarity::Common
    }
}

/// Base item templates per slot: (name, base_value, icon, model)
const WEAPON_BASES: &[(&str, f32, ItemIcon, ItemModel)] = &[
    ("Sword", 8.0, ItemIcon::Sword, ItemModel::BasicSword),
    ("Axe", 10.0, ItemIcon::Axe, ItemModel::BasicAxe),
    ("Mace", 12.0, ItemIcon::Mace, ItemModel::BasicSword),
    ("Dagger", 5.0, ItemIcon::Dagger, ItemModel::BasicSword),
    ("Greatsword", 15.0, ItemIcon::Greatsword, ItemModel::BasicSword),
];

const HELMET_BASES: &[(&str, f32, ItemIcon, ItemModel)] = &[
    ("Cap", 3.0, ItemIcon::ClothHelm, ItemModel::ClothHood),
    ("Helm", 5.0, ItemIcon::PlateHelm, ItemModel::IronHelm),
    ("Crown", 7.0, ItemIcon::Crown, ItemModel::CrystalCrown),
];

const CHEST_BASES: &[(&str, f32, ItemIcon, ItemModel)] = &[
    ("Vest", 5.0, ItemIcon::LeatherVest, ItemModel::LeatherArmor),
    ("Chainmail", 8.0, ItemIcon::Chainmail, ItemModel::ChainArmor),
    ("Plate", 12.0, ItemIcon::PlateArmor, ItemModel::PlateArmor),
];

const BOOTS_BASES: &[(&str, f32, ItemIcon, ItemModel)] = &[
    ("Sandals", 2.0, ItemIcon::Sandals, ItemModel::LeatherArmor),
    ("Boots", 4.0, ItemIcon::LeatherBoots, ItemModel::LeatherArmor),
    ("Greaves", 6.0, ItemIcon::PlateGreaves, ItemModel::PlateArmor),
];

const RING_BASES: &[(&str, f32, ItemIcon, ItemModel)] = &[
    ("Band", 0.0, ItemIcon::Ring, ItemModel::None),
    ("Ring", 0.0, ItemIcon::Ring, ItemModel::None),
    ("Signet", 0.0, ItemIcon::GoldRing, ItemModel::None),
];

const AMULET_BASES: &[(&str, f32, ItemIcon, ItemModel)] = &[
    ("Pendant", 0.0, ItemIcon::Pendant, ItemModel::None),
    ("Amulet", 0.0, ItemIcon::Amulet, ItemModel::None),
    ("Talisman", 0.0, ItemIcon::Amulet, ItemModel::None),
];

/// Generate a random item for a given floor level and item slot.
pub fn generate_item(floor: u32, slot: ItemSlot, seed: u64) -> Item {
    let mut rng = ItemRng::new(seed);
    let mut rarity = roll_rarity(&mut rng, floor);
    let pool = AffixPool::standard();

    // Legendary+ items may roll from the dictionary
    if rarity.has_legendary_power() {
        let dict = LootDictionary::new();
        let candidates = dict.legendaries_for(slot, floor);
        if !candidates.is_empty() {
            let weights: Vec<u32> = candidates.iter().map(|c| c.weight).collect();
            let idx = rng.weighted_select(&weights);
            let def = candidates[idx];

            // Chance to upgrade: Legendary → Ascended (10%) → Eternal (1%)
            let upgrade_roll = rng.next_f32();
            if upgrade_roll < 0.01 {
                rarity = ItemRarity::Eternal;
            } else if upgrade_roll < 0.10 {
                rarity = ItemRarity::Ascended;
            } else {
                rarity = ItemRarity::Legendary;
            }

            return generate_legendary_from_def(def, floor, rarity, &pool, &mut rng);
        }
    }

    generate_item_with_rarity(floor, slot, rarity, &pool, &mut rng)
}

/// Generate a legendary item from a dictionary definition.
fn generate_legendary_from_def(
    def: &super::dictionary::LegendaryDef,
    floor: u32,
    rarity: ItemRarity,
    pool: &AffixPool,
    rng: &mut ItemRng,
) -> Item {
    let scaled_value = def.base_damage_or_defense * (1.0 + (floor.saturating_sub(1)) as f32 * 0.15);

    let base = ItemBase {
        name: def.name,
        kind: ItemKind::Equipment(def.slot),
        base_value: scaled_value,
        item_level: floor,
        icon: def.icon,
        model: def.model,
    };

    let (min_affixes, max_affixes) = rarity.affix_count_range();
    let num_affixes = if min_affixes == max_affixes {
        min_affixes
    } else {
        min_affixes + (rng.range_u32((max_affixes - min_affixes + 1) as u32) as u8)
    };

    let quality_mult = rarity.quality_mult();
    let affixes = roll_affixes(rng, pool, num_affixes, floor, quality_mult);

    let display_name = format!("{}{}", def.name, rarity.tier_label());

    Item {
        base,
        rarity,
        affixes,
        display_name,
        legendary_id: Some(def.id),
        legendary_power: Some(def.power.clone()),
        set_id: def.set_id,
    }
}

/// Generate a random item with a specific rarity (non-legendary path).
pub(crate) fn generate_item_with_rarity(
    floor: u32,
    slot: ItemSlot,
    rarity: ItemRarity,
    pool: &AffixPool,
    rng: &mut ItemRng,
) -> Item {
    // Pick a base item
    let bases = match slot {
        ItemSlot::Weapon => WEAPON_BASES,
        ItemSlot::Helmet => HELMET_BASES,
        ItemSlot::Chest => CHEST_BASES,
        ItemSlot::Boots => BOOTS_BASES,
        ItemSlot::Ring => RING_BASES,
        ItemSlot::Amulet => AMULET_BASES,
    };

    let base_idx = rng.range_u32(bases.len() as u32) as usize;
    let (base_name, base_value, icon, model) = bases[base_idx];

    // Scale base value with floor level
    let scaled_value = base_value * (1.0 + (floor.saturating_sub(1)) as f32 * 0.15);

    let base = ItemBase {
        name: base_name,
        kind: ItemKind::Equipment(slot),
        base_value: scaled_value,
        item_level: floor,
        icon,
        model,
    };

    // Roll affixes
    let (min_affixes, max_affixes) = rarity.affix_count_range();
    let num_affixes = if min_affixes == max_affixes {
        min_affixes
    } else {
        min_affixes + (rng.range_u32((max_affixes - min_affixes + 1) as u32) as u8)
    };

    let quality_mult = rarity.quality_mult();
    let affixes = roll_affixes(rng, pool, num_affixes, floor, quality_mult);

    // Generate display name
    let display_name = build_display_name(base_name, &affixes, rarity);

    Item {
        base,
        rarity,
        affixes,
        display_name,
        legendary_id: None,
        legendary_power: None,
        set_id: None,
    }
}

/// Generate a potion item.
pub fn generate_potion(floor: u32, potion_type: PotionType, _seed: u64) -> Item {
    let heal_amount = match potion_type {
        PotionType::Health => 30.0 + floor as f32 * 10.0,
        PotionType::Speed => 30.0, // duration in percent boost
        PotionType::Damage => 25.0, // percent damage boost
    };

    let name = match potion_type {
        PotionType::Health => "Health Potion",
        PotionType::Speed => "Speed Potion",
        PotionType::Damage => "Damage Potion",
    };

    let icon = match potion_type {
        PotionType::Health => ItemIcon::HealthPotion,
        PotionType::Speed => ItemIcon::SpeedPotion,
        PotionType::Damage => ItemIcon::DamagePotion,
    };

    Item {
        base: ItemBase {
            name,
            kind: ItemKind::Potion(potion_type),
            base_value: heal_amount,
            item_level: floor,
            icon,
            model: ItemModel::None,
        },
        rarity: ItemRarity::Common,
        affixes: Vec::new(),
        display_name: name.to_string(),
        legendary_id: None,
        legendary_power: None,
        set_id: None,
    }
}

fn roll_affixes(
    rng: &mut ItemRng,
    pool: &AffixPool,
    count: u8,
    item_level: u32,
    quality_mult: f32,
) -> Vec<Affix> {
    let mut affixes = Vec::with_capacity(count as usize);
    let mut used_types = Vec::new();

    // Alternate between prefixes and suffixes
    for i in 0..count {
        let pick_prefix = i % 2 == 0;
        let tier_list = if pick_prefix { &pool.prefixes } else { &pool.suffixes };

        // Filter out already-used affix types
        let available: Vec<(usize, &AffixTier)> = tier_list
            .iter()
            .enumerate()
            .filter(|(_, t)| !used_types.contains(&t.affix_type))
            .collect();

        if available.is_empty() {
            continue;
        }

        let weights: Vec<u32> = available.iter().map(|(_, t)| t.weight).collect();
        let selected = rng.weighted_select(&weights);
        let (_, tier) = &available[selected];

        let (min, max) = tier.roll_range(item_level);
        let value = rng.range_f32(min, max) * quality_mult;

        used_types.push(tier.affix_type);
        affixes.push(Affix {
            affix_type: tier.affix_type,
            value,
            is_prefix: tier.is_prefix,
        });
    }

    affixes
}

fn build_display_name(base_name: &str, affixes: &[Affix], rarity: ItemRarity) -> String {
    if rarity == ItemRarity::Common || affixes.is_empty() {
        return base_name.to_string();
    }

    let prefix = affixes.iter()
        .find(|a| a.is_prefix)
        .and_then(|a| {
            let pool = AffixPool::standard();
            pool.prefixes.iter()
                .find(|t| t.affix_type == a.affix_type)
                .map(|t| t.name_fragment)
        });

    let suffix = affixes.iter()
        .find(|a| !a.is_prefix)
        .and_then(|a| {
            let pool = AffixPool::standard();
            pool.suffixes.iter()
                .find(|t| t.affix_type == a.affix_type)
                .map(|t| t.name_fragment)
        });

    match (prefix, suffix) {
        (Some(p), Some(s)) => format!("{} {} {}", p, base_name, s),
        (Some(p), None) => format!("{} {}", p, base_name),
        (None, Some(s)) => format!("{} {}", base_name, s),
        (None, None) => base_name.to_string(),
    }
}
