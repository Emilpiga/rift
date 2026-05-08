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
pub mod equipment;
pub mod inventory;
pub mod item;
pub mod items;
pub mod rarity;
pub mod rng;

pub use crate::stats::{Stat, StatBlock};
pub use ability_mods::AbilityMods;
pub use affixes::{
    AbilityVariant, AffixDef, AffixEffect, ProcAction, ProcEvent, AFFIX_POOL,
};
pub use drops::{
    table_for, BaseItemSelector, LootEntry, LootTable, SlotFilter, BOSS_TABLE,
    BRUTE_TABLE, CASTER_TABLE, ELITE_TABLE, STALKER_TABLE,
};
pub use equipment::Equipment;
pub use inventory::{Inventory, Loadout};
pub use item::{Item, RolledAffix};
pub use items::{
    tag, AccessoryKind, ArmorKind, BaseItem, EquipSlot, ItemSlot, WeaponKind, BASE_ITEMS,
};
pub use rarity::Rarity;
pub use rarity::salvage_yield;
pub use rng::LootRng;
