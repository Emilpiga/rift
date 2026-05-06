//! Item base types and synergy tag system.
//!
//! Each [`BaseItem`] declares:
//! - A [`ItemSlot`] (semantic taxonomy: weapon kind / armor kind /
//!   accessory kind) — drives stat bias and tooltip strings.
//! - An [`EquipSlot`] — which physical slot it occupies on the body.
//! - **Allowed tags** — affixes outside this mask never roll. This
//!   is how we keep "+% Fire Damage" off a tank chest.
//! - **Favored tags** — affixes inside this mask roll with extra
//!   weight, biasing pools toward the base's identity.
//! - **Implicit stats** — every roll of this base gets these for
//!   free (a staff always has some Power, etc.).
//!
//! ## Why tag bitmasks?
//!
//! Affixes carry a `tags: u32` bitmask. Pool filtering is then a
//! `(affix.tags & base.allowed_tags) != 0` check — fast, and adding
//! a new tag is a single `const`. No per-affix special-casing.

use crate::stats::Stat;

/// Tag constants used by [`BaseItem::allowed_tags`] /
/// [`BaseItem::favored_tags`] and [`crate::loot::AffixDef::tags`].
///
/// A new tag is one `const` line. Eight is plenty for now; the
/// underlying type is `u32` so we have headroom.
pub mod tag {
    pub const FIRE: u32 = 1 << 0;
    pub const ICE: u32 = 1 << 1;
    pub const LIGHTNING: u32 = 1 << 2;
    pub const CRIT: u32 = 1 << 3;
    pub const SPEED: u32 = 1 << 4;
    pub const DEFENSE: u32 = 1 << 5;
    pub const CASTER: u32 = 1 << 6;
    pub const MELEE: u32 = 1 << 7;
    pub const UTILITY: u32 = 1 << 8;

    /// Every tag — used by accessories, which can roll anything.
    pub const ALL: u32 = FIRE
        | ICE
        | LIGHTNING
        | CRIT
        | SPEED
        | DEFENSE
        | CASTER
        | MELEE
        | UTILITY;

    /// Caster gear shorthand.
    pub const ANY_ELEMENT: u32 = FIRE | ICE | LIGHTNING;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WeaponKind {
    Staff,
    Sword,
    Dagger,
    Wand,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ArmorKind {
    Heavy,
    Light,
    Robe,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AccessoryKind {
    Ring,
    Amulet,
}

/// Semantic item type — what *kind* of thing it is. Independent of
/// where on the body it goes (that's [`EquipSlot`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemSlot {
    Weapon(WeaponKind),
    Armor(ArmorKind),
    Accessory(AccessoryKind),
}

/// Physical slot on the character. Defined here (not in `inventory`)
/// because [`BaseItem`] needs to refer to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EquipSlot {
    Weapon,
    Helm,
    Chest,
    Legs,
    Hands,
    Boots,
    Ring1,
    Ring2,
    Amulet,
}

impl EquipSlot {
    /// Stable wire / persistence ordering of every slot. Index in
    /// this array doubles as the `u8` discriminant on the wire and
    /// in the `equipped_slot` SMALLINT column.
    pub const ALL: [EquipSlot; 9] = [
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

    /// Number of physical slots — also the length of
    /// [`crate::loot::Equipment`]'s backing array.
    pub const COUNT: usize = Self::ALL.len();

    /// Stable index into [`EquipSlot::ALL`]. Used as the wire
    /// byte and the `equipped_slot` smallint.
    pub fn to_u8(self) -> u8 {
        self as u8
    }

    /// Inverse of [`EquipSlot::to_u8`]. Returns `None` for bytes
    /// outside the known table — keeps mismatched-build wire
    /// frames from corrupting state.
    pub fn from_u8(byte: u8) -> Option<EquipSlot> {
        Self::ALL.get(byte as usize).copied()
    }

    /// Human-friendly label for HUD / tooltip.
    pub fn label(self) -> &'static str {
        match self {
            EquipSlot::Weapon => "Weapon",
            EquipSlot::Helm => "Helm",
            EquipSlot::Chest => "Chest",
            EquipSlot::Legs => "Legs",
            EquipSlot::Hands => "Hands",
            EquipSlot::Boots => "Boots",
            EquipSlot::Ring1 => "Ring 1",
            EquipSlot::Ring2 => "Ring 2",
            EquipSlot::Amulet => "Amulet",
        }
    }
}

/// One row in the base-item table. All fields are `'static` so the
/// table can live in a `pub const`.
#[derive(Clone, Copy, Debug)]
pub struct BaseItem {
    pub id: &'static str,
    pub name: &'static str,
    pub slot: ItemSlot,
    pub equip_slot: EquipSlot,
    /// Affix tags this base is willing to roll. Affixes outside the
    /// mask never appear on it.
    pub allowed_tags: u32,
    /// Affix tags this base prefers — pool weight ×2 inside the mask.
    pub favored_tags: u32,
    /// Always-present stats. Don't count against the rarity affix budget.
    pub implicit: &'static [(Stat, f32)],
    /// Minimum item-level at which this base can drop.
    pub min_ilvl: u32,
    /// Registry key for the inventory icon, matching the relative
    /// stem produced by the engine's icon-discovery pass
    /// (e.g. `"loot/Boots/Boots_1"`). Look-ups go through the
    /// shared `IconUvRegistry`; an unknown key falls back to the
    /// rarity-coloured placeholder.
    pub icon: &'static str,
}

// ---------------------------------------------------------------------
// Starter base-item table
// ---------------------------------------------------------------------
//
// Bias logic — each base has a clear identity:
//   Staff   → caster, elemental scaling
//   Sword   → melee, balanced
//   Dagger  → crit + speed
//   Wand    → hybrid caster (utility + element)
//   Heavy   → armor, defense, melee
//   Light   → evasion, speed
//   Robe    → caster, utility, mana regen
//   Ring    → wildcard (ALL tags allowed)
//   Amulet  → wildcard (ALL tags allowed)

use tag::*;

pub const BASE_ITEMS: &[BaseItem] = &[
    // ---- Weapons ------------------------------------------------------
    BaseItem {
        id: "staff_basic",
        name: "Apprentice Staff",
        slot: ItemSlot::Weapon(WeaponKind::Staff),
        equip_slot: EquipSlot::Weapon,
        allowed_tags: ANY_ELEMENT | CASTER | UTILITY | CRIT,
        favored_tags: ANY_ELEMENT | CASTER,
        implicit: &[(Stat::Power, 6.0)],
        min_ilvl: 1,
        icon: "",
    },
    BaseItem {
        id: "sword_basic",
        name: "Iron Sword",
        slot: ItemSlot::Weapon(WeaponKind::Sword),
        equip_slot: EquipSlot::Weapon,
        allowed_tags: MELEE | CRIT | SPEED | DEFENSE | UTILITY,
        favored_tags: MELEE | CRIT,
        implicit: &[(Stat::Power, 8.0)],
        min_ilvl: 1,
        icon: "",
    },
    BaseItem {
        id: "dagger_basic",
        name: "Hunter's Dagger",
        slot: ItemSlot::Weapon(WeaponKind::Dagger),
        equip_slot: EquipSlot::Weapon,
        allowed_tags: MELEE | CRIT | SPEED | UTILITY,
        favored_tags: CRIT | SPEED,
        implicit: &[(Stat::Power, 5.0), (Stat::CritChance, 0.05)],
        min_ilvl: 1,
        icon: "",
    },
    BaseItem {
        id: "wand_basic",
        name: "Carved Wand",
        slot: ItemSlot::Weapon(WeaponKind::Wand),
        equip_slot: EquipSlot::Weapon,
        allowed_tags: ANY_ELEMENT | CASTER | UTILITY | SPEED,
        favored_tags: CASTER | UTILITY,
        implicit: &[(Stat::Power, 5.0), (Stat::CooldownReduction, 0.04)],
        min_ilvl: 1,
        icon: "",
    },
    // ---- Armor — Helm, Chest, Legs, Hands, Boots ---------------------
    // Each base picks one EquipSlot. New bases (different art / name /
    // implicit) can target the same slot to give players choice.
    BaseItem {
        id: "heavy_helm",
        name: "Plated Helm",
        slot: ItemSlot::Armor(ArmorKind::Heavy),
        equip_slot: EquipSlot::Helm,
        allowed_tags: DEFENSE | MELEE | CRIT | UTILITY,
        favored_tags: DEFENSE | MELEE,
        implicit: &[(Stat::Armor, 12.0), (Stat::Health, 15.0)],
        min_ilvl: 1,
        icon: "loot/Helmets/Helmet_1",
    },
    BaseItem {
        id: "heavy_chest",
        name: "Plated Cuirass",
        slot: ItemSlot::Armor(ArmorKind::Heavy),
        equip_slot: EquipSlot::Chest,
        allowed_tags: DEFENSE | MELEE | UTILITY,
        favored_tags: DEFENSE | MELEE,
        implicit: &[(Stat::Armor, 24.0), (Stat::Health, 30.0)],
        min_ilvl: 1,
        icon: "loot/BodyArmor/BodyArmor_1",
    },
    BaseItem {
        id: "light_chest",
        name: "Studded Vest",
        slot: ItemSlot::Armor(ArmorKind::Light),
        equip_slot: EquipSlot::Chest,
        allowed_tags: DEFENSE | SPEED | CRIT | UTILITY,
        favored_tags: SPEED | CRIT,
        implicit: &[(Stat::Evasion, 0.05), (Stat::Health, 18.0)],
        min_ilvl: 1,
        icon: "loot/BodyArmor/BodyArmor_2",
    },
    BaseItem {
        id: "light_boots",
        name: "Swift Boots",
        slot: ItemSlot::Armor(ArmorKind::Light),
        equip_slot: EquipSlot::Boots,
        allowed_tags: SPEED | CRIT | DEFENSE | UTILITY,
        favored_tags: SPEED,
        implicit: &[(Stat::MoveSpeed, 0.05), (Stat::Evasion, 0.03)],
        min_ilvl: 1,
        icon: "loot/Boots/Boots_1",
    },
    BaseItem {
        id: "robe_chest",
        name: "Mage Robe",
        slot: ItemSlot::Armor(ArmorKind::Robe),
        equip_slot: EquipSlot::Chest,
        allowed_tags: ANY_ELEMENT | CASTER | UTILITY | DEFENSE,
        favored_tags: CASTER | UTILITY,
        implicit: &[(Stat::Health, 14.0), (Stat::ResourceRegen, 0.08)],
        min_ilvl: 1,
        icon: "loot/BodyArmor/BodyArmor_3",
    },
    BaseItem {
        id: "robe_hands",
        name: "Silken Gloves",
        slot: ItemSlot::Armor(ArmorKind::Robe),
        equip_slot: EquipSlot::Hands,
        allowed_tags: ANY_ELEMENT | CASTER | UTILITY,
        favored_tags: CASTER | ANY_ELEMENT,
        implicit: &[(Stat::CooldownReduction, 0.03)],
        min_ilvl: 1,
        icon: "loot/Gloves/Gloves_1",
    },
    BaseItem {
        id: "heavy_legs",
        name: "Plated Greaves",
        slot: ItemSlot::Armor(ArmorKind::Heavy),
        equip_slot: EquipSlot::Legs,
        allowed_tags: DEFENSE | MELEE | UTILITY,
        favored_tags: DEFENSE,
        implicit: &[(Stat::Armor, 16.0), (Stat::Health, 20.0)],
        min_ilvl: 1,
        icon: "loot/Pants/Pants_1",
    },
    // ---- Accessories — wildcards -------------------------------------
    BaseItem {
        id: "ring_basic",
        name: "Plain Ring",
        slot: ItemSlot::Accessory(AccessoryKind::Ring),
        equip_slot: EquipSlot::Ring1,
        allowed_tags: ALL,
        favored_tags: 0,
        implicit: &[],
        min_ilvl: 1,
        icon: "loot/Rings/Ring_1",
    },
    BaseItem {
        id: "amulet_basic",
        name: "Plain Amulet",
        slot: ItemSlot::Accessory(AccessoryKind::Amulet),
        equip_slot: EquipSlot::Amulet,
        allowed_tags: ALL,
        favored_tags: 0,
        implicit: &[(Stat::Health, 10.0)],
        min_ilvl: 1,
        icon: "loot/Necklaces/Necklace_1",
    },
];
