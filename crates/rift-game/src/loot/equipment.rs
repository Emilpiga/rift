//! Per-character equipped-item set.
//!
//! Mirrors the [`crate::loot::EquipSlot`] table: nine logical slots,
//! each independently `Option<Item>`. Equipped items contribute to
//! the resolved [`crate::stats::CharacterStats`] via
//! [`Equipment::active_affix_sum`]; the bag (a separate
//! `Vec<Item>`) holds anything not equipped.
//!
//! Authoritative on the server; the client carries an identical
//! mirror that's rebuilt from `ServerMsg::EquipmentSync`.
//!
//! # Invariants
//!
//! - `slots[i]` corresponds to `EquipSlot::ALL[i]` — `to_u8` /
//!   `from_u8` round-trip via that index.
//! - The server validates that an item placed in a given slot
//!   actually has a matching `BaseItem::equip_slot` (rings / ring2
//!   are interchangeable since both map to `Ring1`/`Ring2` and any
//!   ring is allowed in either ring slot — see [`Equipment::accepts`]).

use crate::loot::{EquipSlot, Item};
use crate::stats::StatBlock;

/// All currently-equipped items, keyed by [`EquipSlot`]. Empty
/// slots are `None`. Stored as a fixed-size array so iteration is
/// branchless and the wire shape is trivial to derive.
#[derive(Clone, Debug, Default)]
pub struct Equipment {
    slots: [Option<Item>; EquipSlot::COUNT],
}

impl Equipment {
    /// Construct an empty equipment set — every slot `None`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Borrow the item currently in `slot`, if any.
    pub fn get(&self, slot: EquipSlot) -> Option<&Item> {
        self.slots[slot.to_u8() as usize].as_ref()
    }

    /// Place `item` into `slot`, returning whatever was previously
    /// there (so the caller can move it back to the bag). The
    /// caller is responsible for validating
    /// [`Equipment::accepts`] beforehand — `set` itself does not
    /// re-check the slot kind.
    pub fn set(&mut self, slot: EquipSlot, item: Option<Item>) -> Option<Item> {
        std::mem::replace(&mut self.slots[slot.to_u8() as usize], item)
    }

    /// Drop the item out of `slot` (no-op if it's already empty),
    /// returning it to the caller.
    pub fn take(&mut self, slot: EquipSlot) -> Option<Item> {
        self.slots[slot.to_u8() as usize].take()
    }

    /// Iterate every `(slot, item)` pair currently filled. Order
    /// follows [`EquipSlot::ALL`].
    pub fn iter(&self) -> impl Iterator<Item = (EquipSlot, &Item)> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, opt)| opt.as_ref().map(|it| (EquipSlot::ALL[i], it)))
    }

    /// Number of filled slots.
    pub fn count(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    /// Whether `item` is allowed in `slot`. Currently a strict
    /// match (`item.base.equip_slot == slot`) with the single
    /// exception that any ring may live in either ring slot —
    /// `Ring1` and `Ring2` are interchangeable from the
    /// validation perspective.
    pub fn accepts(slot: EquipSlot, item: &Item) -> bool {
        let want = item.base.equip_slot;
        match (slot, want) {
            (EquipSlot::Ring1 | EquipSlot::Ring2, EquipSlot::Ring1 | EquipSlot::Ring2) => true,
            (a, b) => a == b,
        }
    }

    /// Default slot to drop `item` into when the user hasn't
    /// picked one explicitly. Mirrors the base item's
    /// `equip_slot`, except that rings prefer the empty ring slot
    /// (or `Ring1` if both are full).
    pub fn default_slot(&self, item: &Item) -> EquipSlot {
        let base = item.base.equip_slot;
        match base {
            EquipSlot::Ring1 | EquipSlot::Ring2 => {
                if self.slots[EquipSlot::Ring1.to_u8() as usize].is_none() {
                    EquipSlot::Ring1
                } else if self.slots[EquipSlot::Ring2.to_u8() as usize].is_none() {
                    EquipSlot::Ring2
                } else {
                    EquipSlot::Ring1
                }
            }
            other => other,
        }
    }

    /// Sum every equipped item's [`Item::stats`] into a single
    /// [`StatBlock`], suitable for feeding into
    /// `CharacterStats::compute`. Pure / cheap — recomputed on
    /// every HUD frame is fine.
    pub fn active_affix_sum(&self) -> StatBlock {
        let mut out = StatBlock::new();
        for (_, item) in self.iter() {
            for (stat, value) in item.stats().iter() {
                out.add(stat, value);
            }
        }
        out
    }

    /// Aggregate every equipped item's affix-driven ability mods
    /// (Amplify / Modify / Transform / Trigger / per-ability
    /// damage / cooldown). The combat layer caches the result on
    /// the player and rebuilds it on equip / unequip — see
    /// `ServerPlayer::recompute_stats`. Pure / allocates a small
    /// number of `HashMap` entries.
    pub fn ability_mods(&self) -> super::AbilityMods {
        let mut mods = super::AbilityMods::new();
        for (_, item) in self.iter() {
            for affix in &item.affixes {
                mods.apply(affix);
            }
            // Phase 4: hand-authored unique effect. Resolves the
            // stable id through the static `UNIQUES` table and
            // folds the resulting [`LegendaryEffect`] through
            // the same dispatch paths the affix variants use.
            // Unknown ids degrade silently (catalogue drift) —
            // the item's stats / affixes still apply.
            if let Some(def) = item.unique_id.and_then(super::uniques::find) {
                if let Some(eff) = def.build(item.unique_pick) {
                    mods.apply_legendary_effect(&eff);
                }
            }
        }
        mods
    }
}
