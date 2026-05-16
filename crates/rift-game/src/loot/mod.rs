//! Loot system — items, affixes, stats, ability modifiers, inventory.
//!
//! # Design philosophy
//!
//! 1. **Stats change decisions or interact with abilities.** No filler
//!    "+2 % loot rarity on Tuesdays".
//! 2. **Base items bias affixes via tags.** A staff favours elemental
//!    scaling, a dagger favours crit/speed. The pool filter does the
//!    "synergy" work without per-affix special-casing.
//! 3. **Affix effects are typed.** Beyond flat stats, an affix can
//!    *amplify*, *modify*, *transform*, or *trigger* an ability. See
//!    [`AffixEffect`].
//! 4. **Rarity changes behaviour, not just numbers.** Each affix
//!    declares a `rarity_min`; gameplay-changing effects (Transform,
//!    Trigger, ExtraProjectiles) only roll on Legendary items, so
//!    higher rarity unlocks new patterns rather than only bigger
//!    multipliers.
//!
//! # Module layout
//!
//! - [`crate::stats`] — the fixed [`crate::stats::Stat`] enum + sparse
//!   [`crate::stats::StatBlock`] (re-exported here for back-compat).
//! - [`items`] — [`items::BaseItem`] table, [`items::ItemSlot`] /
//!   [`items::EquipSlot`], bias-tag constants.
//! - [`affixes`] — [`affixes::AffixDef`], [`affixes::AffixEffect`],
//!   [`affixes::AFFIX_POOL`].
//! - [`ability_mods`] — runtime aggregation
//!   ([`ability_mods::AbilityMods`]) the combat layer queries.
//! - [`rarity`] — [`rarity::Rarity`] + per-rarity affix-count rules.
//! - [`item`] — [`item::Item`] (a rolled drop) + `Item::roll`.
//! - [`inventory`] — [`inventory::Loadout`], [`inventory::Inventory`].
//! - [`rng`] — [`rng::LootRng`] — tiny seeded xorshift.
//!
//! # Adding things later
//!
//! - **New stat?** Add a variant to [`crate::stats::Stat`] + a wiring
//!   affix in [`affixes::AFFIX_POOL`].
//! - **New base?** Append a row to [`items::BaseItem`].
//! - **New affix?** Append an [`affixes::AffixDef`] row, picking
//!   `tags` for synergy and `rarity_min` for behavioural gating.
//! - **New ability transform / proc?** Add a variant to
//!   [`affixes::AbilityVariant`] / [`affixes::ProcAction`] and let
//!   the combat layer match on it.

pub mod ability_mods;
pub mod affixes;
pub mod drops;
pub mod enchant;
pub mod equipment;
pub mod families;
pub mod inventory;
pub mod item;
pub mod items;
pub mod name_gen;
pub mod rarity;
pub mod rng;
pub mod roll;
pub mod tooltip;
pub mod uniques;
pub mod wire;

pub use crate::stats::{Stat, StatBlock};
pub use ability_mods::AbilityMods;
pub use affixes::{
    AbilityVariant, AffixDef, AffixEffect, ProcAction, ProcEvent, AFFIX_POOL,
    RIFT_TOUCHED_MIN_FLOOR, RIFT_TOUCHED_POOL,
};
pub use drops::{
    table_for, BaseItemSelector, LootEntry, LootTable, SlotFilter, BOSS_TABLE, BRUTE_TABLE,
    CASTER_TABLE, ELITE_TABLE, STALKER_TABLE,
};
pub use enchant::{
    reroll_affix, reroll_candidate_tooltips, reroll_entropy_seed, reroll_excluded_previews,
    EnchantError,
};
pub use equipment::Equipment;
pub use families::{Attribute, BaseFamily, Element};
pub use inventory::{Inventory, Loadout};
pub use item::{CharacterIdBytes, Item, LootProvenance, RolledAffix, RolledRiftTouched};
pub use items::{
    tag, AccessoryKind, ArmorKind, BaseItem, ConsumableKind, EquipSlot, GenderedModel, ItemSlot,
    WeaponKind, BASE_ITEMS,
};
pub use rarity::salvage_yield;
pub use rarity::Rarity;
pub use rng::LootRng;
pub use roll::{roll_percentile, roll_range, roll_rift_touched, ANCHORED_CHANCE};
pub use tooltip::{enchant_candidate_preview, TooltipKind, TooltipLine};
pub use uniques::{BespokeId, LegendaryEffect, UniqueDef, UniqueRoll, UNIQUES};
