use super::affix::Affix;

/// Item rarity tiers — determines affix count and drop weight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ItemRarity {
    Common,    // white — no affixes, just base stats
    Magic,     // blue — 1-2 affixes
    Rare,      // yellow — 3-4 affixes
    Epic,      // purple — 4-5 affixes, higher rolls
    Legendary, // orange — 5-6 affixes, unique properties
    Ascended,  // red — legendary with boosted stats (~30% stronger rolls)
    Eternal,   // cyan/white glow — perfect rolls, extremely rare
}

impl ItemRarity {
    /// Number of affixes to roll for this rarity.
    pub fn affix_count_range(self) -> (u8, u8) {
        match self {
            ItemRarity::Common => (0, 0),
            ItemRarity::Magic => (1, 2),
            ItemRarity::Rare => (3, 4),
            ItemRarity::Epic => (4, 5),
            ItemRarity::Legendary => (5, 6),
            ItemRarity::Ascended => (5, 6),
            ItemRarity::Eternal => (6, 6),
        }
    }

    /// Roll quality multiplier (affixes roll higher values at higher rarity).
    pub fn quality_mult(self) -> f32 {
        match self {
            ItemRarity::Common => 1.0,
            ItemRarity::Magic => 1.0,
            ItemRarity::Rare => 1.15,
            ItemRarity::Epic => 1.3,
            ItemRarity::Legendary => 1.5,
            ItemRarity::Ascended => 1.95,
            ItemRarity::Eternal => 2.5,  // perfect rolls
        }
    }

    /// Display color as [R, G, B].
    pub fn color(self) -> [f32; 3] {
        match self {
            ItemRarity::Common => [0.8, 0.8, 0.8],
            ItemRarity::Magic => [0.3, 0.5, 1.0],
            ItemRarity::Rare => [1.0, 1.0, 0.2],
            ItemRarity::Epic => [0.7, 0.3, 1.0],
            ItemRarity::Legendary => [1.0, 0.5, 0.0],
            ItemRarity::Ascended => [0.9, 0.15, 0.15],
            ItemRarity::Eternal => [0.4, 1.0, 1.0],
        }
    }

    /// Whether this rarity has a legendary power.
    pub fn has_legendary_power(self) -> bool {
        matches!(self, Self::Legendary | Self::Ascended | Self::Eternal)
    }

    /// Display name suffix for enhanced legendaries.
    pub fn tier_label(self) -> &'static str {
        match self {
            ItemRarity::Ascended => " [Ascended]",
            ItemRarity::Eternal => " [Eternal]",
            _ => "",
        }
    }
}

/// Equipment slot on the player.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemSlot {
    Weapon,
    Helmet,
    Chest,
    Boots,
    Ring,
    Amulet,
}

/// What broad category is this item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemKind {
    Equipment(ItemSlot),
    Potion(PotionType),
}

/// Potion subtypes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PotionType {
    Health,      // Instant heal
    Speed,       // Temporary speed boost
    Damage,      // Temporary damage boost
}

/// Icon identifier for rendering items in the inventory/ground.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemIcon {
    // Weapons
    Sword,
    Axe,
    Mace,
    Dagger,
    Greatsword,
    Bow,
    Crossbow,
    // Armor
    ClothHelm,
    PlateHelm,
    Crown,
    LeatherVest,
    Chainmail,
    PlateArmor,
    Sandals,
    LeatherBoots,
    PlateGreaves,
    // Accessories
    Ring,
    GoldRing,
    Pendant,
    Amulet,
    // Potions
    HealthPotion,
    SpeedPotion,
    DamagePotion,
    // Legendary-specific
    VoidBlade,
    StormCrown,
    PhoenixHeart,
    SerpentFang,
    FrostbiteGreaves,
    SoulboundRing,
}

/// Model identifier for equipped items (affects character appearance).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemModel {
    None,
    // Weapons
    BasicSword,
    BasicAxe,
    BasicBow,
    LegendarySword,
    VoidBlade,
    StormBow,
    // Armor (visual overlay on player)
    LeatherArmor,
    ChainArmor,
    PlateArmor,
    PhoenixPlate,
    // Helmets
    ClothHood,
    IronHelm,
    CrystalCrown,
}

/// Legendary power — unique gameplay-altering effect on legendary+ items.
#[derive(Clone, Debug, PartialEq)]
pub enum LegendaryPower {
    /// Arrows pierce +N additional targets.
    PiercingShots(u32),
    /// X% chance for attacks to fire a second projectile.
    SplitShot(f32),
    /// Kills explode for X% weapon damage in an area.
    ExplosiveDeath(f32),
    /// +X% movement speed while at full HP.
    WindRunner(f32),
    /// Damage taken reduced by X% while moving.
    DodgeRoll(f32),
    /// Crits restore X HP.
    LifeSteal(f32),
    /// X% chance to freeze enemies on hit for Y seconds.
    FrostNova(f32, f32),
    /// All cooldowns reduced by X%.
    Haste(f32),
    /// +X% damage to enemies below 30% HP (execute).
    Executioner(f32),
    /// Every 5th hit deals X% bonus lightning damage.
    ChainLightning(f32),
    /// Gain X% damage for each enemy within 10 units.
    PackHunter(f32),
    /// X% of overkill damage hits another nearby enemy.
    Ricochet(f32),
}

/// Set identity — items with the same SetId form a set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SetId {
    VoidWalker,     // shadow/void themed
    StormCaller,    // lightning themed
    BloodOath,      // life-steal themed
    FrostSentinel,  // ice/slow themed
    PhoenixAscent,  // fire/rebirth themed
}

/// A bonus granted when N pieces of a set are equipped.
#[derive(Clone, Debug)]
pub struct SetBonus {
    pub pieces_required: u8,
    pub description: &'static str,
    pub effect: SetEffect,
}

/// The actual effect of a set bonus.
#[derive(Clone, Debug, PartialEq)]
pub enum SetEffect {
    /// +X% to a stat.
    StatBonus { damage_pct: f32, defense_pct: f32, speed_pct: f32 },
    /// Gain a legendary power.
    GrantPower(LegendaryPower),
    /// Special proc: X% chance on hit to trigger an effect.
    ProcOnHit { chance: f32, description: &'static str },
}

/// Base item template (before affixes).
#[derive(Clone, Debug)]
pub struct ItemBase {
    pub name: &'static str,
    pub kind: ItemKind,
    /// Base damage (weapons) or base defense (armor) or heal amount (potions).
    pub base_value: f32,
    /// Item level — determines affix roll ranges. Usually matches floor level.
    pub item_level: u32,
    /// Visual icon for UI rendering.
    pub icon: ItemIcon,
    /// 3D model when equipped on character.
    pub model: ItemModel,
}

/// A fully generated item with base + affixes + optional legendary/set properties.
#[derive(Clone, Debug)]
pub struct Item {
    pub base: ItemBase,
    pub rarity: ItemRarity,
    pub affixes: Vec<Affix>,
    /// Generated display name (e.g. "Blazing Sword of the Tiger").
    pub display_name: String,
    /// Unique ID if this is a dictionary legendary (None for random items).
    pub legendary_id: Option<&'static str>,
    /// Legendary power (only on Legendary/Ascended/Eternal).
    pub legendary_power: Option<LegendaryPower>,
    /// Set membership (if any).
    pub set_id: Option<SetId>,
}

impl Item {
    /// Total flat damage bonus from all affixes + base.
    pub fn total_damage(&self) -> f32 {
        let base = match self.base.kind {
            ItemKind::Equipment(ItemSlot::Weapon) => self.base.base_value,
            _ => 0.0,
        };
        base + self.affixes.iter().map(|a| a.flat_damage()).sum::<f32>()
    }

    /// Total flat defense from all affixes + base.
    pub fn total_defense(&self) -> f32 {
        let base = match self.base.kind {
            ItemKind::Equipment(slot) if slot != ItemSlot::Weapon => self.base.base_value,
            _ => 0.0,
        };
        base + self.affixes.iter().map(|a| a.flat_defense()).sum::<f32>()
    }

    /// Total percent speed bonus.
    pub fn speed_bonus_pct(&self) -> f32 {
        self.affixes.iter().map(|a| a.speed_pct()).sum()
    }

    /// Total percent attack speed bonus.
    pub fn attack_speed_pct(&self) -> f32 {
        self.affixes.iter().map(|a| a.attack_speed_pct()).sum()
    }

    /// Total crit chance bonus (0.0–1.0).
    pub fn crit_chance(&self) -> f32 {
        self.affixes.iter().map(|a| a.crit_chance()).sum()
    }

    /// Total max HP bonus.
    pub fn max_hp_bonus(&self) -> f32 {
        self.affixes.iter().map(|a| a.max_hp()).sum()
    }

    /// Total HP regen per second.
    pub fn hp_regen(&self) -> f32 {
        self.affixes.iter().map(|a| a.hp_regen()).sum()
    }

    /// Slot this item goes into (None for potions).
    pub fn slot(&self) -> Option<ItemSlot> {
        match self.base.kind {
            ItemKind::Equipment(slot) => Some(slot),
            ItemKind::Potion(_) => None,
        }
    }

    /// Whether this item is part of a set.
    pub fn is_set_item(&self) -> bool {
        self.set_id.is_some()
    }

    /// Whether this item has a legendary power.
    pub fn has_power(&self) -> bool {
        self.legendary_power.is_some()
    }
}
