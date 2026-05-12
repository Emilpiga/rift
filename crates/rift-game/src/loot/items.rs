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

use super::families::BaseFamily;

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
    pub const ALL: u32 = FIRE | ICE | LIGHTNING | CRIT | SPEED | DEFENSE | CASTER | MELEE | UTILITY;

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
///
/// **Wire / persistence note:** the discriminant of each variant is
/// its position in this declaration (`self as u8`) and is mirrored
/// 1:1 by [`EquipSlot::ALL`]. New variants must be appended at the
/// end — inserting in the middle silently shifts every later
/// `equipped_slot` SMALLINT in the database to the wrong slot.
/// Display order on the paperdoll is decoupled and lives in
/// `loot::inventory::ALL_SLOTS`.
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
    Shoulders,
}

impl EquipSlot {
    /// Stable wire / persistence ordering of every slot. Index in
    /// this array doubles as the `u8` discriminant on the wire and
    /// in the `equipped_slot` SMALLINT column.
    pub const ALL: [EquipSlot; 10] = [
        EquipSlot::Weapon,
        EquipSlot::Helm,
        EquipSlot::Chest,
        EquipSlot::Legs,
        EquipSlot::Hands,
        EquipSlot::Boots,
        EquipSlot::Ring1,
        EquipSlot::Ring2,
        EquipSlot::Amulet,
        EquipSlot::Shoulders,
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
            EquipSlot::Shoulders => "Shoulders",
            EquipSlot::Chest => "Chest",
            EquipSlot::Legs => "Legs",
            EquipSlot::Hands => "Hands",
            EquipSlot::Boots => "Boots",
            EquipSlot::Ring1 => "Ring 1",
            EquipSlot::Ring2 => "Ring 2",
            EquipSlot::Amulet => "Amulet",
        }
    }

    /// Default `(width, height)` an item targeting this slot
    /// occupies in the bag grid. Mirrors the visual size used
    /// on the paperdoll so picking up an item from an equip
    /// slot lands on a bag tile of the same shape. All values
    /// are in inventory cells (1 cell = one bag grid square).
    pub fn inventory_size(self) -> (u8, u8) {
        match self {
            EquipSlot::Weapon => (2, 3),
            EquipSlot::Shoulders => (2, 2),
            EquipSlot::Chest => (2, 3),
            EquipSlot::Legs => (2, 2),
            EquipSlot::Helm => (2, 2),
            EquipSlot::Hands => (2, 2),
            EquipSlot::Boots => (2, 2),
            EquipSlot::Ring1 | EquipSlot::Ring2 => (1, 1),
            EquipSlot::Amulet => (1, 1),
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
    /// Damage-axis lock for this base — `Source × Element ×
    /// Archetype`. Authored per row; Phase 2 of the itemisation
    /// refactor (see `ITEMS.md` §3) makes the affix-roll pipeline
    /// honour this and lets the legacy `allowed_tags` / `favored_tags`
    /// bitmasks retire. During Phase 1 both fields coexist so the
    /// rolling code keeps working unchanged.
    pub family: BaseFamily,
    /// Registry key for the inventory icon, matching the relative
    /// stem produced by the engine's icon-discovery pass
    /// (e.g. `"loot/Boots/Boots_1"`). Look-ups go through the
    /// shared `IconUvRegistry`; an unknown key falls back to the
    /// rarity-coloured placeholder.
    pub icon: &'static str,
    /// Optional per-gender glTF/GLB worn on the character avatar
    /// when this item is equipped. The mesh is expected to be
    /// skinned against the same logical skeleton as the matching
    /// gender's base player rig (modular outfit pipeline). `None`
    /// items render no visual; either gender slot can be `None`
    /// independently while art catches up.
    pub models: Option<GenderedModel>,
}

/// Gendered art override for a `BaseItem`. Each field is the
/// path to a glTF/GLB rigged against the corresponding base
/// player skeleton. `None` for a gender means "no visual on
/// avatars of that gender" — the equipment is still functional,
/// it just doesn't dress the model. Use [`GenderedModel::for_gender`]
/// at the call site so the lookup stays in one place.
#[derive(Clone, Copy, Debug)]
pub struct GenderedModel {
    pub female: Option<&'static str>,
    pub male: Option<&'static str>,
}

impl GenderedModel {
    /// Pick the path matching `gender`, or `None` when the
    /// matching gender's art hasn't been authored yet.
    pub fn for_gender(&self, gender: crate::character::Gender) -> Option<&'static str> {
        match gender {
            crate::character::Gender::Female => self.female,
            crate::character::Gender::Male => self.male,
        }
    }
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
//   Shoulders → armor, defense, utility

use tag::*;

use super::families::{Attribute, ELEMENTS_CASTER, ELEMENTS_PHYSICAL};

pub const BASE_ITEMS: &[BaseItem] = &[
    // ---- Weapons ------------------------------------------------------
    BaseItem {
        id: "staff_basic",
        name: "Apprentice Staff",
        slot: ItemSlot::Weapon(WeaponKind::Staff),
        equip_slot: EquipSlot::Weapon,
        allowed_tags: ANY_ELEMENT | CASTER | UTILITY | CRIT,
        favored_tags: ANY_ELEMENT | CASTER,
        implicit: &[(Stat::FireDamage, 0.08)],
        min_ilvl: 1,
        // Staff = caster: spell scaling, any element, archetype
        // wildcard. Beam / AoE shapes (Frost Ray, Fire Wave,
        // Fireball) are covered by their element axis — the
        // archetype trio line for a staff just opens up to
        // Projectile or Melee resonance instead of locking.
        family: BaseFamily {
            attribute: Some(Attribute::Intellect),
            element: Some(ELEMENTS_CASTER),
        },
        icon: "loot/Weapons/43",
        models: None,
    },
    BaseItem {
        id: "sword_basic",
        name: "Iron Sword",
        slot: ItemSlot::Weapon(WeaponKind::Sword),
        equip_slot: EquipSlot::Weapon,
        allowed_tags: MELEE | CRIT | SPEED | DEFENSE | UTILITY,
        favored_tags: MELEE | CRIT,
        implicit: &[(Stat::PhysicalDamage, 0.10)],
        min_ilvl: 1,
        // Sword = physical melee weapon.
        family: BaseFamily {
            attribute: Some(Attribute::Strength),
            element: Some(ELEMENTS_PHYSICAL),
        },
        icon: "loot/Weapons/25",
        models: None,
    },
    BaseItem {
        id: "dagger_basic",
        name: "Hunter's Dagger",
        slot: ItemSlot::Weapon(WeaponKind::Dagger),
        equip_slot: EquipSlot::Weapon,
        allowed_tags: MELEE | CRIT | SPEED | UTILITY,
        favored_tags: CRIT | SPEED,
        implicit: &[(Stat::Strength, 3.0), (Stat::CritChance, 0.05)],
        min_ilvl: 1,
        // Dagger = physical melee weapon, crit-flavoured.
        family: BaseFamily {
            attribute: Some(Attribute::Strength),
            element: Some(ELEMENTS_PHYSICAL),
        },
        icon: "loot/Weapons/1",
        models: None,
    },
    BaseItem {
        id: "wand_basic",
        name: "Carved Wand",
        slot: ItemSlot::Weapon(WeaponKind::Wand),
        equip_slot: EquipSlot::Weapon,
        allowed_tags: ANY_ELEMENT | CASTER | UTILITY | SPEED,
        favored_tags: CASTER | UTILITY,
        implicit: &[(Stat::Agility, 3.0), (Stat::CooldownReduction, 0.04)],
        min_ilvl: 1,
        // Wand = caster, any element, projectile shape (Fireball,
        // Multi-fireball-style spell projectiles).
        family: BaseFamily {
            attribute: Some(Attribute::Agility),
            element: Some(ELEMENTS_CASTER),
        },
        icon: "loot/Weapons/15",
        // Weapons are external props rather than skinned outfit
        // pieces — the same authored mesh is used regardless of
        // the wielder's gender, attached to the casting-hand
        // joint each frame. The renderer-side path lives in
        // `rift-client::game::weapon_visuals`; both gender slots
        // point at the same source file because nothing in the
        // mesh is gendered.
        models: Some(GenderedModel {
            female: Some("assets/models/loot/weapons/wands/wand_01.glb"),
            male: Some("assets/models/loot/weapons/wands/wand_01.glb"),
        }),
    },
    // ---- Armor — Helm, Chest, Legs, Hands, Boots ---------------------
    // Each base picks one EquipSlot. New bases (different art / name /
    // implicit) can target the same slot to give players choice.
    BaseItem {
        id: "light_helm",
        name: "Leather Helm",
        slot: ItemSlot::Armor(ArmorKind::Light),
        equip_slot: EquipSlot::Helm,
        allowed_tags: DEFENSE | MELEE | CRIT | UTILITY,
        favored_tags: DEFENSE | MELEE,
        implicit: &[(Stat::Armor, 12.0), (Stat::Health, 15.0)],
        min_ilvl: 1,
        // Light helm — wildcard attribute; physical element vibe
        // comes from the implicit, not the family lock.
        family: BaseFamily {
            attribute: None,
            element: Some(ELEMENTS_PHYSICAL),
        },
        icon: "loot/Helmets/Helmet_1",
        models: Some(GenderedModel {
            female: Some("assets/models/loot/helm/armor_helm_leather_01_female.glb"),
            male: Some("assets/models/loot/helm/armor_helm_leather_01_male.glb"),
        }),
    },
    BaseItem {
        id: "light_shoulders",
        name: "Leather Spaulders",
        slot: ItemSlot::Armor(ArmorKind::Light),
        equip_slot: EquipSlot::Shoulders,
        allowed_tags: SPEED | CRIT | DEFENSE | UTILITY,
        favored_tags: SPEED | CRIT,
        implicit: &[(Stat::Evasion, 0.02), (Stat::Health, 12.0)],
        min_ilvl: 1,
        // Light shoulders — flexible across all axes; identity
        // comes from the evasion implicit, not the family lock.
        family: BaseFamily::WILDCARD,
        icon: "loot/Shoulders/Shoulders_1",
        models: Some(GenderedModel {
            female: Some(
                "assets/models/loot/shoulderpads/armor_shoulderpads_leather_01_female.glb",
            ),
            male: Some("assets/models/loot/shoulderpads/armor_shoulderpads_leather_01_male.glb"),
        }),
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
        // Heavy plate — Strength, physical, melee archetype.
        family: BaseFamily {
            attribute: Some(Attribute::Strength),
            element: Some(ELEMENTS_PHYSICAL),
        },
        icon: "loot/BodyArmor/BodyArmor_1",
        models: None,
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
        // Light chest — flexible. Suits rogue / ranger / hybrid.
        family: BaseFamily::WILDCARD,
        icon: "loot/BodyArmor/BodyArmor_2",
        // Bring-up: first modular armor visual. Studded Vest is the
        // closest fit thematically; once we have more art each base
        // gets its own mesh. Male model not yet authored — male
        // avatars equip it for stats but render bare-chested.
        models: Some(GenderedModel {
            female: Some("assets/models/loot/chest_pieces/armor_chest_leather_01_female.glb"),
            male: Some("assets/models/loot/chest_pieces/armor_chest_leather_01_male.glb"),
        }),
    },
    BaseItem {
        id: "light_boots",
        name: "Leather Boots",
        slot: ItemSlot::Armor(ArmorKind::Light),
        equip_slot: EquipSlot::Boots,
        allowed_tags: SPEED | CRIT | DEFENSE | UTILITY,
        favored_tags: SPEED,
        implicit: &[(Stat::MoveSpeed, 0.05), (Stat::Evasion, 0.03)],
        min_ilvl: 1,
        // Boots are identity-neutral — wildcard family.
        family: BaseFamily::WILDCARD,
        icon: "loot/Boots/Boots_1",
        models: Some(GenderedModel {
            female: Some("assets/models/loot/feet/armor_boots_leather_01_female.glb"),
            male: Some("assets/models/loot/feet/armor_boots_leather_01_male.glb"),
        }),
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
        // Robe — Intellect, element-flex, no archetype lock.
        family: BaseFamily {
            attribute: Some(Attribute::Intellect),
            element: None,
        },
        icon: "loot/BodyArmor/BodyArmor_3",
        models: None,
    },
    BaseItem {
        id: "light_gloves",
        name: "Leather Gloves",
        slot: ItemSlot::Armor(ArmorKind::Light),
        equip_slot: EquipSlot::Hands,
        allowed_tags: ANY_ELEMENT | CASTER | UTILITY,
        favored_tags: CASTER | ANY_ELEMENT,
        implicit: &[(Stat::CooldownReduction, 0.03)],
        min_ilvl: 1,
        // Light gloves lean caster — Intellect, element-flex.
        family: BaseFamily {
            attribute: Some(Attribute::Intellect),
            element: None,
        },
        icon: "loot/Gloves/Gloves_1",
        models: Some(GenderedModel {
            female: Some("assets/models/loot/gloves/armor_gloves_leather_01_female.glb"),
            male: Some("assets/models/loot/gloves/armor_gloves_leather_01_male.glb"),
        }),
    },
    BaseItem {
        id: "light_legs",
        name: "Leather Leggings",
        slot: ItemSlot::Armor(ArmorKind::Light),
        equip_slot: EquipSlot::Legs,
        allowed_tags: DEFENSE | MELEE | UTILITY,
        favored_tags: DEFENSE,
        implicit: &[(Stat::Armor, 16.0), (Stat::Health, 20.0)],
        min_ilvl: 1,
        // Heavy-ish legs — Strength, physical, melee archetype.
        family: BaseFamily {
            attribute: Some(Attribute::Strength),
            element: Some(ELEMENTS_PHYSICAL),
        },
        icon: "loot/Pants/Pants_1",
        models: Some(GenderedModel {
            female: Some("assets/models/loot/legs/armor_legs_leather_01_female.glb"),
            male: Some("assets/models/loot/legs/armor_legs_leather_01_male.glb"),
        }),
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
        // Accessories — the build-completion slots. Wildcard.
        family: BaseFamily::WILDCARD,
        icon: "loot/Rings/Ring_1",
        models: None,
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
        // Accessories — wildcard. Amulet completes elemental sets.
        family: BaseFamily::WILDCARD,
        icon: "loot/Necklaces/Necklace_1",
        models: None,
    },
];

#[cfg(test)]
mod family_tests {
    use super::super::families::Element;
    use super::*;

    /// Every `BaseItem` row declares a `family`. Source-less
    /// (wildcard) and source-locked entries are both legal; what
    /// we enforce is that a non-empty element slice is just that
    /// — non-empty — so the Phase 2 roll path can pick at least
    /// one option per axis when the lock is set.
    #[test]
    fn every_base_declares_a_coherent_family() {
        for b in BASE_ITEMS {
            if let Some(list) = b.family.element {
                assert!(
                    !list.is_empty(),
                    "base `{}` has an empty element slice",
                    b.id,
                );
            }
        }
    }

    /// Weapons should always declare an attribute — they are the
    /// damage-producing slot, and the attribute axis drives class
    /// scaling. Armor / accessories may legitimately be wildcard.
    #[test]
    fn weapons_lock_an_attribute() {
        for b in BASE_ITEMS {
            if matches!(b.slot, ItemSlot::Weapon(_)) {
                assert!(
                    b.family.attribute.is_some(),
                    "weapon base `{}` has no attribute lock — \
                     weapons must commit to one of Str / Agi / Int",
                    b.id,
                );
            }
        }
    }

    /// Wildcard sanity: an accessory base should permit every
    /// element and every archetype, so the build-completion slot
    /// can finish any build.
    #[test]
    fn accessories_are_wildcard_friendly() {
        for b in BASE_ITEMS {
            if matches!(b.slot, ItemSlot::Accessory(_)) {
                for e in [
                    Element::Physical,
                    Element::Fire,
                    Element::Ice,
                    Element::Lightning,
                ] {
                    assert!(
                        b.family.allows_element(e),
                        "accessory `{}` rejects element {:?}",
                        b.id,
                        e,
                    );
                }
            }
        }
    }

    /// Every `BaseItem::id` must be unique across the catalogue.
    /// Persistence keys items by the stable string id, so a
    /// duplicate would alias two different bases onto the same
    /// row on save / load and silently overwrite a player's gear.
    #[test]
    fn every_base_id_is_unique() {
        let mut seen: std::collections::HashSet<&'static str> = Default::default();
        for b in BASE_ITEMS {
            assert!(
                seen.insert(b.id),
                "base item id `{}` appears more than once in BASE_ITEMS",
                b.id
            );
        }
    }

    /// Every `BaseItem::icon` must be non-empty. An empty icon
    /// key forces the renderer down the fallback-glyph path every
    /// frame for that base; usually a content-authoring slip.
    #[test]
    fn every_base_declares_an_icon() {
        for b in BASE_ITEMS {
            assert!(
                !b.icon.is_empty(),
                "base item `{}` has an empty icon key",
                b.id
            );
        }
    }
}
