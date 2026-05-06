//! Per-enemy loot tables — the data layer that turns a monster kill
//! into zero-or-more rolled [`super::Item`]s.
//!
//! # Structure
//!
//! - [`LootTable`] = the full drop spec for one enemy archetype:
//!   - `drop_chance`: probability the enemy drops *anything* at all
//!   - `rarity_weights`: when it does drop, the weighted distribution
//!     over [`Rarity`] tiers
//!   - `entries`: a weighted list of [`LootEntry`] candidates the
//!     base item is sampled from
//!
//! - [`LootEntry`] = one candidate drop:
//!   - `selector`: how to pick the base item (specific id, item-slot
//!     filter, or tag filter)
//!   - `weight`: relative weight in the table
//!   - optional `rarity_override` / `ilvl_offset` for boss-style
//!     guaranteed drops
//!
//! # Why selectors instead of a flat id list?
//!
//! Hand-listing every base id per enemy doesn't scale. A `Brute`
//! drops "any DEFENSE-tagged armor"; a `Caster` drops "any
//! CASTER-tagged weapon or wand". A new base item with the right
//! tag automatically joins the pool — no per-enemy edits.
//!
//! # Determinism
//!
//! All rolling goes through [`super::LootRng`]. A loot drop seeded
//! by `(server_tick, enemy_net_id)` produces identical drops on
//! every client when replayed — useful both for shared loot
//! visibility and netcode replay.

use crate::monsters::MonsterRole;

use super::affixes::AFFIX_POOL;
use super::item::Item;
use super::items::{tag, BaseItem, ItemSlot, BASE_ITEMS};
use super::rarity::Rarity;
use super::rng::LootRng;

// ---------------------------------------------------------------------
// Selector
// ---------------------------------------------------------------------

/// How a [`LootEntry`] picks a concrete base item.
#[derive(Clone, Copy, Debug)]
pub enum BaseItemSelector {
    /// Specific base item by id. Falls back to no-drop if the id is
    /// missing (e.g. content removed from `BASE_ITEMS`).
    Exact(&'static str),
    /// Any base item whose [`ItemSlot`] kind matches. The closure-y
    /// matching is done by [`item_matches_slot_filter`].
    SlotKind(SlotFilter),
    /// Any base item whose `allowed_tags & tags != 0`. This is the
    /// "give me anything with a Caster bias" selector.
    Tagged(u32),
}

#[derive(Clone, Copy, Debug)]
pub enum SlotFilter {
    AnyWeapon,
    AnyArmor,
    AnyAccessory,
    /// Match an exact [`ItemSlot`] (e.g. `ItemSlot::Weapon(WeaponKind::Staff)`).
    Exact(ItemSlot),
}

fn slot_filter_matches(filter: SlotFilter, slot: ItemSlot) -> bool {
    match (filter, slot) {
        (SlotFilter::AnyWeapon, ItemSlot::Weapon(_)) => true,
        (SlotFilter::AnyArmor, ItemSlot::Armor(_)) => true,
        (SlotFilter::AnyAccessory, ItemSlot::Accessory(_)) => true,
        (SlotFilter::Exact(a), b) => a == b,
        _ => false,
    }
}

/// Resolve a selector to the concrete pool of allowed bases.
fn resolve(selector: BaseItemSelector, ilvl: u32) -> Vec<&'static BaseItem> {
    match selector {
        BaseItemSelector::Exact(id) => BASE_ITEMS
            .iter()
            .find(|b| b.id == id && b.min_ilvl <= ilvl)
            .into_iter()
            .collect(),
        BaseItemSelector::SlotKind(filter) => BASE_ITEMS
            .iter()
            .filter(|b| b.min_ilvl <= ilvl && slot_filter_matches(filter, b.slot))
            .collect(),
        BaseItemSelector::Tagged(tags) => BASE_ITEMS
            .iter()
            .filter(|b| b.min_ilvl <= ilvl && (b.allowed_tags & tags) != 0)
            .collect(),
    }
}

// ---------------------------------------------------------------------
// Entry & table
// ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct LootEntry {
    pub selector: BaseItemSelector,
    /// Relative weight inside the table.
    pub weight: u32,
    /// If set, forces this entry to drop at exactly this rarity
    /// (ignoring [`LootTable::rarity_weights`]). Used for boss
    /// guaranteed-rare/legendary drops.
    pub rarity_override: Option<Rarity>,
    /// Added to the rolled item-level. Negative values floor at 1.
    pub ilvl_offset: i32,
}

impl LootEntry {
    /// Convenience constructor for a plain entry.
    pub const fn new(selector: BaseItemSelector, weight: u32) -> Self {
        Self {
            selector,
            weight,
            rarity_override: None,
            ilvl_offset: 0,
        }
    }
}

/// Full drop spec for one enemy archetype.
#[derive(Clone, Debug)]
pub struct LootTable {
    /// Probability of *any* drop, in `0..=1`.
    pub drop_chance: f32,
    /// Weighted distribution over rarity tiers when something does drop.
    /// Order matches [`Rarity`]'s discriminant: Common, Magic, Rare, Legendary.
    pub rarity_weights: [u32; 4],
    pub entries: &'static [LootEntry],
    /// How many *guaranteed* picks beyond the drop_chance roll.
    /// Bosses use `extra_rolls = 1..3` so they always cough up
    /// something on top of the regular roll.
    pub extra_rolls: u32,
}

impl LootTable {
    /// Roll once. Returns 0..N items per call.
    ///
    /// Sampling pipeline:
    /// 1. Bernoulli on `drop_chance` (skipped for the `extra_rolls`).
    /// 2. Pick a [`LootEntry`] by weight.
    /// 3. Resolve the entry's selector to a base-item pool, pick one
    ///    uniformly.
    /// 4. Pick a rarity (entry override → table weights).
    /// 5. Hand off to [`Item::roll`].
    pub fn roll(&self, rng: &mut LootRng, ilvl: u32) -> Vec<Item> {
        let mut out = Vec::new();

        // First roll is gated by `drop_chance`.
        if rng.next_f32() < self.drop_chance {
            if let Some(item) = self.roll_one(rng, ilvl) {
                out.push(item);
            }
        }
        // `extra_rolls` always fire (bosses).
        for _ in 0..self.extra_rolls {
            if let Some(item) = self.roll_one(rng, ilvl) {
                out.push(item);
            }
        }
        out
    }

    fn roll_one(&self, rng: &mut LootRng, ilvl: u32) -> Option<Item> {
        if self.entries.is_empty() {
            return None;
        }
        let weights: Vec<u32> = self.entries.iter().map(|e| e.weight).collect();
        let entry_idx = rng.weighted_index(&weights)?;
        let entry = &self.entries[entry_idx];

        let entry_ilvl = ((ilvl as i32) + entry.ilvl_offset).max(1) as u32;
        let bases = resolve(entry.selector, entry_ilvl);
        if bases.is_empty() {
            return None;
        }
        let base = bases[(rng.next_u64() as usize) % bases.len()];

        let rarity = entry
            .rarity_override
            .unwrap_or_else(|| pick_rarity(&self.rarity_weights, rng));

        // Re-affirm the table at the rolled rarity has at least one
        // valid affix candidate; if not, fall back to Common rather
        // than producing an Item with zero affixes.
        let rarity = if any_affix_available(base, rarity, entry_ilvl) {
            rarity
        } else {
            Rarity::Common
        };

        Some(Item::roll(base, rarity, entry_ilvl, rng))
    }
}

fn pick_rarity(weights: &[u32; 4], rng: &mut LootRng) -> Rarity {
    let total: u32 = weights.iter().sum();
    if total == 0 {
        return Rarity::Common;
    }
    let mut pick = rng.range(0, total);
    for (i, &w) in weights.iter().enumerate() {
        if pick < w {
            return match i {
                0 => Rarity::Common,
                1 => Rarity::Magic,
                2 => Rarity::Rare,
                _ => Rarity::Legendary,
            };
        }
        pick -= w;
    }
    Rarity::Common
}

fn any_affix_available(base: &BaseItem, rarity: Rarity, ilvl: u32) -> bool {
    AFFIX_POOL.iter().any(|a| {
        (a.tags & base.allowed_tags) != 0
            && a.min_ilvl <= ilvl
            && rarity.at_least(a.rarity_min)
    })
}

// ---------------------------------------------------------------------
// Default tables
// ---------------------------------------------------------------------

/// Pick the loot table for an enemy by [`MonsterRole`]. The combat
/// layer calls this on death and feeds the result through
/// `LootTable::roll`.
pub fn table_for(role: MonsterRole) -> &'static LootTable {
    match role {
        MonsterRole::Brute => &BRUTE_TABLE,
        MonsterRole::Stalker => &STALKER_TABLE,
        MonsterRole::Caster => &CASTER_TABLE,
        MonsterRole::Elite => &ELITE_TABLE,
        MonsterRole::Boss => &BOSS_TABLE,
    }
}

// Concrete tables. Tweak weights here, not at the call site.
pub const BRUTE_TABLE: LootTable = LootTable {
    drop_chance: 0.18,
    rarity_weights: [70, 25, 5, 0], // common-heavy
    extra_rolls: 0,
    entries: &[
        // Brutes mostly drop heavy-armor pieces or melee weapons.
        LootEntry::new(BaseItemSelector::Tagged(tag::DEFENSE | tag::MELEE), 6),
        LootEntry::new(BaseItemSelector::SlotKind(SlotFilter::AnyAccessory), 1),
    ],
};

pub const STALKER_TABLE: LootTable = LootTable {
    drop_chance: 0.20,
    rarity_weights: [60, 30, 10, 0],
    extra_rolls: 0,
    entries: &[
        LootEntry::new(BaseItemSelector::Tagged(tag::SPEED | tag::CRIT), 5),
        LootEntry::new(BaseItemSelector::SlotKind(SlotFilter::AnyAccessory), 1),
    ],
};

pub const CASTER_TABLE: LootTable = LootTable {
    drop_chance: 0.22,
    rarity_weights: [55, 30, 13, 2], // first whisper of legendary
    extra_rolls: 0,
    entries: &[
        LootEntry::new(BaseItemSelector::Tagged(tag::CASTER | tag::ANY_ELEMENT), 5),
        LootEntry::new(BaseItemSelector::Tagged(tag::UTILITY), 2),
        LootEntry::new(BaseItemSelector::SlotKind(SlotFilter::AnyAccessory), 1),
    ],
};

pub const ELITE_TABLE: LootTable = LootTable {
    drop_chance: 0.85,
    rarity_weights: [20, 45, 30, 5],
    extra_rolls: 0,
    entries: &[
        // Elites are wildcard — any item type.
        LootEntry::new(BaseItemSelector::SlotKind(SlotFilter::AnyWeapon), 3),
        LootEntry::new(BaseItemSelector::SlotKind(SlotFilter::AnyArmor), 3),
        LootEntry::new(BaseItemSelector::SlotKind(SlotFilter::AnyAccessory), 2),
    ],
};

pub const BOSS_TABLE: LootTable = LootTable {
    drop_chance: 1.0,
    rarity_weights: [0, 30, 50, 20],
    extra_rolls: 2, // 1 chance roll + 2 guaranteed = 3 drops typical
    entries: &[
        LootEntry {
            selector: BaseItemSelector::SlotKind(SlotFilter::AnyWeapon),
            weight: 4,
            rarity_override: None,
            ilvl_offset: 1,
        },
        LootEntry {
            selector: BaseItemSelector::SlotKind(SlotFilter::AnyArmor),
            weight: 4,
            rarity_override: None,
            ilvl_offset: 1,
        },
        LootEntry {
            // Bosses always have a shot at a guaranteed legendary
            // accessory — gameplay-changing affixes via rarity_min.
            selector: BaseItemSelector::SlotKind(SlotFilter::AnyAccessory),
            weight: 2,
            rarity_override: Some(Rarity::Legendary),
            ilvl_offset: 2,
        },
    ],
};
