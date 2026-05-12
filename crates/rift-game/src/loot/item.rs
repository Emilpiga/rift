//! A rolled drop: base item + rarity + a list of [`RolledAffix`].
//!
//! Items are immutable once rolled — [`Item::roll`] (in
//! [`super::roll`]) is the only way to produce one. Save/load
//! rehydrates via [`Item::from_wire`] / [`Item::from_persisted`]
//! (in [`super::wire`]).
//!
//! This file holds the struct definitions and the small,
//! gameplay-derived accessors (`footprint`, `stats`,
//! `required_level`) plus the regression-test module that
//! exercises the whole pipeline. The actual roll dispatch lives
//! in [`super::roll`]; tooltip rendering lives in
//! [`super::tooltip`]; wire / persistence (de)serialisation lives
//! in [`super::wire`].

use super::affixes::{AffixDef, AffixEffect};
use super::items::BaseItem;
use super::rarity::Rarity;
use crate::stats::StatBlock;

/// One realised affix on an item.
#[derive(Clone, Debug)]
pub struct RolledAffix {
    pub def: &'static AffixDef,
    /// Magnitude rolled within the affix's range. Meaning depends
    /// on `def.effect` (see [`AffixEffect`] docs).
    pub value: f32,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub base: &'static BaseItem,
    pub rarity: Rarity,
    pub ilvl: u32,
    pub affixes: Vec<RolledAffix>,
    /// `true` when this drop rolled the rare "Anchored" trait.
    /// Anchored items survive the wipe-on-death floor reset
    /// (see `Sim::wipe_dead_loot`) so the player keeps them
    /// across runs. Legendaries only, gated behind
    /// [`super::roll::ANCHORED_CHANCE`] inside [`Item::roll`].
    /// Purely additive — no stat impact, just persistence.
    pub anchored: bool,
    /// `true` while this item is "unstable rift loot" — picked
    /// up inside an active rift instance and not yet stabilised
    /// by extraction. Unstable items live only in the
    /// authoritative `ServerPlayer.inventory` / `equipment`
    /// snapshots; they are *never* written to persistence and
    /// are stripped from a player's bag + equipment whenever the
    /// player leaves a rift through any path other than the
    /// extraction vote (death-by-rift-exit, return-to-hub,
    /// disconnect). Walking a successful extraction flips this
    /// to `false` ("purified"), at which point the item behaves
    /// like every other piece of loot — persisted, equippable,
    /// stash-able. The flag is the diegetic surface for the
    /// "loot acquired in a rift is unstable, must be purified by
    /// extraction, death shatters unstable loot" fantasy.
    pub unstable: bool,
    /// Eligibility lineage for ground-loot pickup. Snapshotted
    /// once at the moment the item is first generated (monster
    /// kill in a rift) and **carried with the item forever** —
    /// across stash, equip, drop, and re-pickup. The set holds
    /// every character (by 16-byte UUID) that shared the
    /// originating expedition.
    ///
    /// `None` is the legacy / unprovenanced state for items
    /// that predate this system. The server self-binds a
    /// `None` to the current holder on first interaction
    /// (equip, drop, pickup) so the loophole closes the moment
    /// a legacy item is touched.
    ///
    /// rift-game intentionally does **not** depend on the
    /// `uuid` crate; the bytes are passed through as raw
    /// `[u8; 16]` and converted at the `rift-persistence` /
    /// server boundary. Wire / persistence formats default to
    /// `None` so old payloads decode cleanly.
    pub provenance: Option<LootProvenance>,
    /// When the legendary roll matched a [`super::uniques::UniqueDef`]
    /// the unique's stable string id is stamped here. `None` for
    /// every non-Legendary item and for Legendaries whose
    /// `(slot, base)` matched no authored row (procedural-only
    /// fallback). See `loot/uniques.rs`.
    pub unique_id: Option<&'static str>,
    /// Per-instance pool index for uniques whose
    /// [`super::uniques::UniqueRoll::Pool`] sampled an ability at
    /// roll time (today: Mirrorglass). Persists across save / load
    /// so the same drop reproduces the same proc target. `None`
    /// for `Fixed` uniques and for non-unique drops.
    pub unique_pick: Option<u8>,
    /// Rift-touched bonus line (ITEMS.md §2.6).
    /// `Some` only on drops that came from a kill inside a rift
    /// at or beyond
    /// [`super::affixes::RIFT_TOUCHED_MIN_FLOOR`]. Survives
    /// extraction — the line is a permanent record of how deep
    /// the item came from. Magnitude scales with the
    /// `depth` field at drop time, not item-level. `None` is
    /// the default for hub drops, legacy items, and rift drops
    /// that didn't pass the per-drop chance gate
    /// ([`super::affixes::RIFT_TOUCHED_CHANCE`]).
    pub rift_touched: Option<RolledRiftTouched>,
}

/// One realised rift-touched bonus line. Carries the floor
/// depth it was rolled at so a deeper drop can carry a
/// stronger version of the same line — see
/// [`super::roll::roll_rift_touched`].
#[derive(Clone, Debug)]
pub struct RolledRiftTouched {
    pub def: &'static AffixDef,
    pub value: f32,
    /// Floor index at the moment of the kill. Stored so the
    /// tooltip can display the provenance ("Earned in Floor N")
    /// and so save-load round-trips preserve the depth identity
    /// even if the scaling formula changes between builds.
    pub depth: u16,
}

/// 16-byte little-endian UUID payload identifying a character.
/// Used as the eligibility key inside [`LootProvenance`]. Kept as
/// a plain byte array so `rift-game` doesn't need to depend on
/// the `uuid` crate; the persistence / network layers convert
/// to and from `uuid::Uuid` at their boundary.
pub type CharacterIdBytes = [u8; 16];

/// Eligibility lineage attached to an [`Item`]. See
/// [`Item::provenance`] for the high-level rules. Today this
/// only carries the `eligible` set, but the type is named so the
/// payload can grow (timestamp, originating rift seed, etc.)
/// without churning every call site.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LootProvenance {
    /// Snapshot of every character UUID that shared the
    /// originating expedition. A pickup is allowed when the
    /// pickup'er's character id appears in this list — or when
    /// the per-ground-instance share window has expired (handled
    /// outside this struct, on the server's `ServerLoot`).
    ///
    /// Stored as a `Vec` rather than `HashSet` because the set
    /// is tiny (party sizes are bounded) and `contains` on a
    /// short vec wins every time. Order is irrelevant.
    pub eligible: Vec<CharacterIdBytes>,
}

impl LootProvenance {
    /// Build a provenance snapshot from a slice of character ids.
    /// Empty input is allowed (produces an empty `eligible` set
    /// — equivalent to a no-pickup gate, used by tests and as a
    /// defensive fallback).
    pub fn from_ids(ids: impl IntoIterator<Item = CharacterIdBytes>) -> Self {
        let mut eligible: Vec<CharacterIdBytes> = ids.into_iter().collect();
        // De-dup so a buggy caller that passes the same id twice
        // doesn't bloat the persisted payload. Sort first so the
        // dedup is O(n log n) rather than O(n^2).
        eligible.sort_unstable();
        eligible.dedup();
        Self { eligible }
    }

    /// `true` if `who` shared the originating expedition. The
    /// time-window gate (after which any peer can pick up) lives
    /// on the server's `ServerLoot` row, not here — provenance
    /// itself is timeless.
    pub fn allows(&self, who: &CharacterIdBytes) -> bool {
        self.eligible.contains(who)
    }
}

impl Item {
    /// Bag-grid footprint `(width_cells, height_cells)`. The
    /// item anchors at its inventory storage index and
    /// extends down + right by these dimensions; all covered
    /// cells must remain empty in the storage `Vec`.
    pub fn footprint(&self) -> (u8, u8) {
        self.base.equip_slot.inventory_size()
    }

    /// Stat-only contribution of this item (implicits + Stat affixes).
    /// Ability mods come from [`super::AbilityMods`] separately.
    pub fn stats(&self) -> StatBlock {
        let mut block = StatBlock::new();
        for &(stat, value) in self.base.implicit {
            block.add(stat, value);
        }
        for a in &self.affixes {
            if let AffixEffect::Stat(stat) = a.def.effect {
                block.add(stat, a.value);
            }
        }
        block
    }

    /// Minimum character level required to equip this item.
    ///
    /// Derived (not stored), so the wire / persistence formats stay
    /// unchanged: it's the larger of the item's own item-level and
    /// the highest `min_ilvl` of any rolled affix. The ilvl term
    /// keeps the rift-tier signal — a level-30 rift drops gear that
    /// asks for level 30 to wear, even when its affix block happens
    /// to be modest. The affix term defends against future affixes
    /// that gate themselves above their host item's ilvl.
    ///
    /// Floored at 1 so freshly-created characters always have *some*
    /// equippable starter.
    pub fn required_level(&self) -> u32 {
        let from_ilvl = self.ilvl.max(1);
        let from_affixes = self
            .affixes
            .iter()
            .map(|a| a.def.min_ilvl)
            .max()
            .unwrap_or(0);
        from_ilvl.max(from_affixes).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::super::affixes::AFFIX_POOL;
    use super::super::items::BASE_ITEMS;
    use super::super::rng::LootRng;
    use super::*;

    /// Pick a base by static id, panicking with a useful message
    /// if the catalog drifts out from under the test.
    fn base(id: &str) -> &'static super::super::items::BaseItem {
        BASE_ITEMS
            .iter()
            .find(|b| b.id == id)
            .unwrap_or_else(|| panic!("base item `{id}` missing from BASE_ITEMS"))
    }

    #[test]
    fn required_level_floors_at_one_for_ilvl_zero() {
        // Synthetic item with no affixes and a degenerate ilvl.
        // Floor guarantees fresh characters always have *some*
        // equippable starter, even if a future code path produces
        // an ilvl-0 drop by accident.
        let item = Item {
            base: base("staff_basic"),
            rarity: Rarity::Common,
            ilvl: 0,
            affixes: Vec::new(),
            anchored: false,
            provenance: None,
            unstable: false,
            unique_id: None,
            unique_pick: None,
            rift_touched: None,
        };
        assert_eq!(item.required_level(), 1);
    }

    #[test]
    fn required_level_tracks_item_level_when_affixes_are_low_tier() {
        // Common drop from a level-30 rift with no affixes that
        // outscale the item-level: requirement equals the ilvl,
        // i.e. "you cleared rift 30, this asks you to be level 30".
        let item = Item {
            base: base("staff_basic"),
            rarity: Rarity::Common,
            ilvl: 30,
            affixes: Vec::new(),
            anchored: false,
            provenance: None,
            unstable: false,
            unique_id: None,
            unique_pick: None,
            rift_touched: None,
        };
        assert_eq!(item.required_level(), 30);
    }

    #[test]
    fn required_level_uses_highest_affix_min_ilvl_when_above_ilvl() {
        // Pick any two affixes with distinct min_ilvls and verify
        // the requirement tracks the highest one when it exceeds
        // the item's ilvl.
        let a_low = AFFIX_POOL
            .iter()
            .find(|a| a.min_ilvl == 1)
            .expect("AFFIX_POOL should contain at least one min_ilvl=1 entry");
        let a_high = AFFIX_POOL
            .iter()
            .filter(|a| a.min_ilvl > 1)
            .max_by_key(|a| a.min_ilvl)
            .expect("AFFIX_POOL should contain at least one min_ilvl>1 entry");
        let item = Item {
            base: base("staff_basic"),
            rarity: Rarity::Rare,
            // ilvl deliberately low so the affix term wins.
            ilvl: 1,
            affixes: vec![
                RolledAffix {
                    def: a_low,
                    value: 0.0,
                },
                RolledAffix {
                    def: a_high,
                    value: 0.0,
                },
            ],
            anchored: false,
            provenance: None,
            unstable: false,
            unique_id: None,
            unique_pick: None,
            rift_touched: None,
        };
        assert_eq!(item.required_level(), a_high.min_ilvl.max(1));
    }

    #[test]
    fn required_level_takes_max_of_ilvl_and_affix_terms() {
        // ilvl=20, affix at min_ilvl<=20 → ilvl term wins.
        // ilvl=20, affix at min_ilvl>20  → affix term wins.
        // Pick affixes from the live pool so this test stays
        // immune to renames.
        let a_low = AFFIX_POOL
            .iter()
            .filter(|a| a.min_ilvl <= 20)
            .next()
            .expect("AFFIX_POOL should contain at least one min_ilvl<=20 entry");
        let item_a = Item {
            base: base("staff_basic"),
            rarity: Rarity::Common,
            ilvl: 20,
            affixes: vec![RolledAffix {
                def: a_low,
                value: 0.0,
            }],
            anchored: false,
            provenance: None,
            unstable: false,
            unique_id: None,
            unique_pick: None,
            rift_touched: None,
        };
        assert_eq!(item_a.required_level(), 20);

        // Find any affix gated at >20 to drive the second branch.
        if let Some(high) = AFFIX_POOL.iter().find(|a| a.min_ilvl > 20) {
            let item_b = Item {
                base: base("staff_basic"),
                rarity: Rarity::Common,
                ilvl: 20,
                affixes: vec![RolledAffix {
                    def: high,
                    value: 0.0,
                }],
                anchored: false,
                provenance: None,
                unstable: false,
                unique_id: None,
                unique_pick: None,
                rift_touched: None,
            };
            assert_eq!(item_b.required_level(), high.min_ilvl);
        }
    }

    #[test]
    fn required_level_for_rolled_item_never_exceeds_max_of_ilvl_and_pool_max() {
        // End-to-end: roll a Legendary at a moderate ilvl with a
        // deterministic seed and verify the requirement is bounded
        // by max(ilvl, pool max min_ilvl). This catches regressions
        // where a future field starts contributing to the
        // requirement without being reflected in the tests.
        let pool_max = AFFIX_POOL.iter().map(|a| a.min_ilvl).max().unwrap_or(0);
        let mut rng = LootRng::new(0xDEAD_BEEF_CAFE);
        let item = Item::roll(base("staff_basic"), Rarity::Legendary, 25, &mut rng);
        let req = item.required_level();
        assert!(
            req <= 25u32.max(pool_max),
            "req={req} exceeds bound max(25, pool_max={pool_max})",
        );
        assert!(req >= 1);
    }

    #[test]
    fn tooltip_includes_requires_level_line() {
        let mut rng = LootRng::new(1);
        let item = Item::roll(base("staff_basic"), Rarity::Magic, 12, &mut rng);
        let lines = item.tooltip(None);
        let req = item.required_level();
        let expected = format!("Requires Level {}", req);
        assert!(
            lines.iter().any(|l| l.text == expected),
            "tooltip missing `{expected}`; got {lines:?}",
        );
    }

    /// Every `BaseItem` × `Rarity` combo must produce a roll with
    /// at least the per-slot signature lines. Catches the class of
    /// regressions where a base's tag mask filters every bonus
    /// candidate out, or a slot's signature ids drift out of the
    /// pool.
    #[test]
    fn every_base_rolls_at_every_rarity() {
        use super::super::affixes::signature_count;
        let rarities = [
            Rarity::Common,
            Rarity::Magic,
            Rarity::Rare,
            Rarity::Legendary,
        ];
        // Drive several seeds per cell so Ring/Amulet's random
        // signature pick is exercised in every branch.
        for b in BASE_ITEMS {
            for r in rarities {
                for seed in 0..8u64 {
                    let mut rng = LootRng::new(
                        (b.id.len() as u64)
                            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                            .wrapping_add((r as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9))
                            .wrapping_add(seed),
                    );
                    let it = Item::roll(b, r, b.min_ilvl.max(1), &mut rng);
                    let sig = signature_count(b.equip_slot);
                    assert!(
                        it.affixes.len() >= sig,
                        "base `{}` rarity {:?}: rolled {} affixes, \
                         expected at least {} signature lines",
                        b.id,
                        r,
                        it.affixes.len(),
                        sig,
                    );
                    // Every rolled affix must round-trip through the
                    // pool — the persisted-id path depends on this.
                    for a in &it.affixes {
                        assert!(
                            super::super::affixes::lookup(a.def.id).is_some(),
                            "base `{}` rolled affix id `{}` that \
                             does not resolve in AFFIX_POOL",
                            b.id,
                            a.def.id,
                        );
                    }
                }
            }
        }
    }

    // Trio-pipeline invariants (ITEMS.md §3 Phase 2).

    /// Helper: iterate `Item::roll` over every base × rarity × N
    /// seeds and hand each rolled `Item` to `check`. Trio
    /// invariants are statistical — we exercise enough seeds that
    /// rare branches (Magic's element-vs-archetype flip,
    /// family-empty-element fallbacks) get covered.
    fn for_every_roll(seeds: u64, mut check: impl FnMut(&Item)) {
        let rarities = [
            Rarity::Common,
            Rarity::Magic,
            Rarity::Rare,
            Rarity::Legendary,
        ];
        for b in BASE_ITEMS {
            for r in rarities {
                for seed in 0..seeds {
                    let mut rng = LootRng::new(
                        (b.id.len() as u64)
                            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                            .wrapping_add((r as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9))
                            .wrapping_add(seed),
                    );
                    let it = Item::roll(b, r, b.min_ilvl.max(1), &mut rng);
                    check(&it);
                }
            }
        }
    }

    /// Every damage-axis affix on a rolled item must satisfy the
    /// base's `BaseFamily` lock. A sword (Source::Weapon) must
    /// never roll a Spell-source line; a staff (Element ∈ {Fire,
    /// Ice, Lightning}) must never roll Physical; a bow
    /// (Archetype::Projectile) must never roll Beam / AoE / Melee.
    /// This is *the* invariant the trio pipeline buys us — it's
    /// why the pipeline exists.
    ///
    /// Resonance lines ([`super::super::affixes::is_resonance`])
    /// are excluded from this assertion — they break the family
    /// lock on purpose and carry their own invariant
    /// ([`resonance_lines_are_always_cross_family`]).
    #[test]
    fn axis_lines_respect_family_lock() {
        use super::super::affixes::{
            affix_archetype, affix_attribute, affix_element, is_resonance,
        };
        for_every_roll(16, |it| {
            for a in &it.affixes {
                if is_resonance(a.def) {
                    continue;
                }
                if let Some(e) = affix_element(a.def) {
                    assert!(
                        it.base.family.allows_element(e),
                        "base `{}` (family {:?}) rolled out-of-family \
                         Element line `{}`",
                        it.base.id,
                        it.base.family,
                        a.def.id,
                    );
                }
                if let Some(ar) = affix_archetype(a.def) {
                    assert!(
                        it.base.family.allows_archetype(ar),
                        "base `{}` (family {:?}) rolled out-of-family \
                         Archetype line `{}`",
                        it.base.id,
                        it.base.family,
                        a.def.id,
                    );
                }
                if let Some(at) = affix_attribute(a.def) {
                    assert!(
                        it.base.family.allows_attribute(at),
                        "base `{}` (family {:?}) rolled out-of-family \
                         Attribute line `{}`",
                        it.base.id,
                        it.base.family,
                        a.def.id,
                    );
                }
            }
        });
    }

    /// No `Stat` may appear twice on the same item — signature,
    /// trio, and bonus rolls all share the dedupe set. A Chest
    /// that signatures `+Health` must never bonus-roll another
    /// `+Health`; a Weapon that trios `+Weapon Damage` must never
    /// pick up a second `+Weapon Damage` line.
    #[test]
    fn no_stat_appears_twice() {
        use crate::stats::Stat;
        for_every_roll(16, |it| {
            let mut seen: std::collections::HashSet<Stat> = Default::default();
            for a in &it.affixes {
                if let AffixEffect::Stat(s) = a.def.effect {
                    assert!(
                        seen.insert(s),
                        "base `{}` rolled stat {:?} twice (affixes: {:?})",
                        it.base.id,
                        s,
                        it.affixes.iter().map(|x| x.def.id).collect::<Vec<_>>(),
                    );
                }
            }
        });
    }

    /// Common rolls exactly one attribute line. Magic rolls
    /// attribute + one of {Element, Archetype}. Rare/Legendary
    /// rolls the full Attribute × Element × Archetype trio.
    #[test]
    fn rarity_gates_trio_shape() {
        use super::super::affixes::{
            affix_archetype, affix_attribute, affix_element, is_resonance,
        };
        for_every_roll(16, |it| {
            let mut elements = 0;
            let mut archetypes = 0;
            let mut attributes = 0;
            for a in &it.affixes {
                if is_resonance(a.def) {
                    continue;
                }
                if affix_element(a.def).is_some() {
                    elements += 1;
                }
                if affix_archetype(a.def).is_some() {
                    archetypes += 1;
                }
                if affix_attribute(a.def).is_some() {
                    attributes += 1;
                }
            }
            match it.rarity {
                Rarity::Common => {
                    assert_eq!(
                        attributes, 1,
                        "base `{}` Common: expected 1 attribute line, got {}",
                        it.base.id, attributes
                    );
                    assert_eq!(
                        elements + archetypes,
                        0,
                        "base `{}` Common: expected 0 element/archetype lines, got {}+{}",
                        it.base.id,
                        elements,
                        archetypes
                    );
                }
                Rarity::Magic => {
                    assert_eq!(
                        attributes, 1,
                        "base `{}` Magic: expected 1 attribute line, got {}",
                        it.base.id, attributes
                    );
                    assert_eq!(
                        elements + archetypes,
                        1,
                        "base `{}` Magic: expected exactly one of element/archetype, got {}+{}",
                        it.base.id,
                        elements,
                        archetypes
                    );
                }
                Rarity::Rare | Rarity::Legendary => {
                    assert_eq!(
                        attributes, 1,
                        "base `{}` {:?}: expected 1 Attribute line, got {}",
                        it.base.id, it.rarity, attributes
                    );
                    assert_eq!(
                        elements, 1,
                        "base `{}` {:?}: expected 1 Element line, got {}",
                        it.base.id, it.rarity, elements
                    );
                    assert_eq!(
                        archetypes, 1,
                        "base `{}` {:?}: expected 1 Archetype line, got {}",
                        it.base.id, it.rarity, archetypes
                    );
                }
            }
        });
    }

    // Resonance invariants.

    /// Every rolled resonance line must target an axis the base's
    /// `BaseFamily` **rejects**. This is the design contract for
    /// resonance — it's the slot where the family lock is broken
    /// on purpose. A sword (Source::Weapon) resonating must pick
    /// a Spell-source line; a staff (no Physical element) must
    /// pick Physical; a bow must pick Beam/AoE/Melee or wrong
    /// element. If this fails, the resonance filter is leaking
    /// in-family lines and the cross-family flavour is gone.
    #[test]
    fn resonance_lines_are_always_cross_family() {
        use super::super::affixes::{
            is_resonance, resonance_archetype, resonance_attribute, resonance_element,
        };
        for_every_roll(64, |it| {
            for a in &it.affixes {
                if !is_resonance(a.def) {
                    continue;
                }
                let cross = resonance_element(a.def)
                    .map(|e| !it.base.family.allows_element(e))
                    .unwrap_or(false)
                    || resonance_archetype(a.def)
                        .map(|ar| !it.base.family.allows_archetype(ar))
                        .unwrap_or(false)
                    || resonance_attribute(a.def)
                        .map(|at| !it.base.family.allows_attribute(at))
                        .unwrap_or(false);
                assert!(
                    cross,
                    "base `{}` (family {:?}) rolled in-family \
                     resonance line `{}` — resonance must always \
                     target a family-rejected axis",
                    it.base.id, it.base.family, a.def.id,
                );
            }
        });
    }

    /// Resonance can fire at most once per item — the pipeline
    /// has exactly one probabilistic check after the bonus block.
    /// A failure here would mean the resonance phase re-entered
    /// or the pool was being double-sampled, breaking the slot
    /// budget and the tooltip layout.
    #[test]
    fn at_most_one_resonance_line_per_item() {
        use super::super::affixes::is_resonance;
        for_every_roll(64, |it| {
            let count = it.affixes.iter().filter(|a| is_resonance(a.def)).count();
            assert!(
                count <= 1,
                "base `{}` rolled {} resonance lines (max 1)",
                it.base.id,
                count
            );
        });
    }

    /// Common and Magic rarities never produce a resonance line.
    /// The chance gate ([`super::super::affixes::resonance_chance`])
    /// returns 0 for those tiers; this test pins that behaviour
    /// so a future tuning pass can't silently extend resonance
    /// to lower rarities.
    #[test]
    fn common_and_magic_never_resonate() {
        use super::super::affixes::is_resonance;
        for_every_roll(64, |it| {
            if matches!(it.rarity, Rarity::Common | Rarity::Magic) {
                let any = it.affixes.iter().any(|a| is_resonance(a.def));
                assert!(
                    !any,
                    "base `{}` rarity {:?} resonated — should never happen",
                    it.base.id, it.rarity,
                );
            }
        });
    }

    /// Every id in [`RESONANCE_POOL`] must round-trip through
    /// [`super::super::affixes::lookup`] — persistence stores
    /// items by id, and a missing resonance id would silently
    /// drop the line on rehydration.
    #[test]
    fn every_resonance_id_resolves() {
        use super::super::affixes::{lookup, RESONANCE_POOL};
        for def in RESONANCE_POOL {
            let found = lookup(def.id);
            assert!(
                found.is_some(),
                "resonance id `{}` does not resolve through `lookup`",
                def.id
            );
        }
    }
}
