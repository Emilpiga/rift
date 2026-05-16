//! Roll pipeline for [`super::item::Item`].
//!
//! Split out of `item.rs` so the affix-roll formulae, the
//! signature/trio/bonus/legendary/resonance dispatch in
//! [`Item::roll`], and the small `roll_range` / `roll_percentile`
//! helpers used by tooltip rendering live next to each other
//! rather than mixed in with the struct definitions and the
//! tooltip builder.

use super::affixes::{AffixDef, AffixEffect, AFFIX_POOL};
use super::item::{Item, RolledAffix, RolledRiftTouched};
use super::items::BaseItem;
use super::rarity::Rarity;
use super::rng::LootRng;

/// Roll a single affix's magnitude, scaled by item-level. Pulled
/// out of `Item::roll` because both phases (signature + bonus +
/// legendary effect) need the same formula.
fn roll_value(def: &AffixDef, ilvl: u32, rng: &mut LootRng) -> f32 {
    let scale = ilvl.saturating_sub(1) as f32 * def.ilvl_scale;
    if def.roll.0 == def.roll.1 {
        def.roll.0
    } else {
        rng.frange(def.roll.0 + scale, def.roll.1 + scale)
    }
}

/// The `[min, max]` magnitude range an affix can roll at the given
/// item-level (rolls sample **inclusively** at both ends — see
/// [`super::rng::LootRng::frange`]). Used both by [`Item::roll`]
/// (indirectly via [`roll_value`]) and tooltip rendering — the
/// player sees what part of the range a drop landed on.
pub fn roll_range(def: &AffixDef, ilvl: u32) -> (f32, f32) {
    let scale = ilvl.saturating_sub(1) as f32 * def.ilvl_scale;
    (def.roll.0 + scale, def.roll.1 + scale)
}

/// Where `value` lands in `[lo, hi]`, expressed as a 0..1 fraction.
/// Returns `None` when the range is degenerate (Transform / fixed
/// rolls — there's no scale to talk about).
///
/// For **flat** (non-percent) [`Stat`] affixes, uses the same rounding as
/// tooltip display (`round()` on lo/hi/value) so tier bands align with
/// values shown as whole numbers (e.g. +9 max matches `Perfect`).
pub fn roll_percentile(def: &AffixDef, ilvl: u32, value: f32) -> Option<f32> {
    let (lo, hi) = roll_range(def, ilvl);
    if (hi - lo).abs() < 1e-6 {
        return None;
    }
    if let AffixEffect::Stat(stat) = def.effect {
        if !stat.is_percent() {
            let lo_d = lo.round();
            let hi_d = hi.round();
            let v_d = value.round();
            let denom = hi_d - lo_d;
            if denom.abs() < 1e-6 {
                return None;
            }
            return Some(((v_d - lo_d) / denom).clamp(0.0, 1.0));
        }
    }
    Some(((value - lo) / (hi - lo)).clamp(0.0, 1.0))
}

/// Sample a rift-touched line for a kill at `floor_index`. Returns
/// `None` unless **both** gates pass:
///
/// 1. **Floor gate** — the kill happened at or beyond
///    [`super::affixes::RIFT_TOUCHED_MIN_FLOOR`]. Hub kills
///    (`floor_index == 0`) and any future "below-min" floors are
///    filtered out here so the call site doesn't need to repeat
///    the check.
/// 2. **Chance gate** — independent
///    [`super::affixes::RIFT_TOUCHED_CHANCE`] per qualifying kill.
///    Kept low so the slot stays a meaningful "this came from
///    deep" signal rather than a free-slot every drop.
///
/// Magnitude is rolled from the def's base `(roll.0, roll.1)`
/// range (the pool authors all set `ilvl_scale = 0.0` so ilvl
/// doesn't enter the formula) and then scaled by floor depth
/// via [`super::affixes::RIFT_TOUCHED_DEPTH_SCALE`]. A deeper
/// kill therefore produces a stronger line — the "rift-touched"
/// identity is partly the slot, partly the depth-driven scale.
pub fn roll_rift_touched(
    rng: &mut super::rng::LootRng,
    floor_index: u32,
) -> Option<RolledRiftTouched> {
    if floor_index < super::affixes::RIFT_TOUCHED_MIN_FLOOR {
        return None;
    }
    if rng.frange(0.0, 1.0) >= super::affixes::RIFT_TOUCHED_CHANCE {
        return None;
    }
    let pool = super::affixes::RIFT_TOUCHED_POOL;
    if pool.is_empty() {
        return None;
    }
    let pick = rng.range(0, pool.len() as u32) as usize;
    let def = &pool[pick];
    let depth_steps = floor_index - super::affixes::RIFT_TOUCHED_MIN_FLOOR;
    let depth_mult = 1.0 + (depth_steps as f32) * super::affixes::RIFT_TOUCHED_DEPTH_SCALE;
    let base_value = rng.frange(def.roll.0, def.roll.1);
    let value = base_value * depth_mult;
    Some(RolledRiftTouched {
        def,
        value,
        depth: floor_index.min(u16::MAX as u32) as u16,
    })
}

/// Per-roll chance that a Legendary drop is also Anchored.
/// 1 in 5 000 — the chase trait is meant to be a long-term
/// goal, not something every farming session produces.
pub const ANCHORED_CHANCE: f32 = 1.0 / 5_000.0;

impl Item {
    /// Roll a fresh drop of `base` at the given rarity / item-level.
    ///
    /// Pipeline (see `ITEMS.md` §2.1 and §3):
    ///
    /// 1. **Signature injection** — deterministic Vitality + slim
    ///    per-`EquipSlot` defensive line (helm CDR, boots move
    ///    speed, gloves crit pair, etc.). Damage-axis lines no
    ///    longer live here; the trio owns those.
    ///
    /// 2. **Source × Element × Archetype trio** — family-locked
    ///    axis lines, gated by rarity:
    ///
    ///    | Rarity    | Source | Element | Archetype |
    ///    | --------- | :----: | :-----: | :-------: |
    ///    | Common    | ✓      |         |           |
    ///    | Magic     | ✓      | one of {Element, Archetype}      ||
    ///    | Rare      | ✓      | ✓       | ✓         |
    ///    | Legendary | ✓      | ✓       | ✓         |
    ///
    ///    Each line is sampled uniformly from the corresponding
    ///    axis sub-pool filtered by [`super::BaseFamily`]. Wildcard
    ///    axes (e.g. accessories) permit the full pool; locked
    ///    axes (e.g. Staff → `{Fire, Ice, Lightning}`) permit only
    ///    the declared subset. Cross-family rolls are impossible
    ///    by construction.
    ///
    /// 3. **Bonus rolls** — `rarity.affix_count_range()` extra
    ///    lines from the *bonus* sub-pool only (legendary effects
    ///    excluded; damage-axis affixes excluded — those live in
    ///    the trio). Excludes any `Stat` already touched by the
    ///    signature or trio, so e.g. a Chest can't double up on
    ///    `+Health`.
    ///
    /// 4. **Legendary effect** — Legendary rarity additionally
    ///    rolls one effect: hand-authored uniques win when a
    ///    `(slot, base)` match exists; otherwise procedural fallback.
    ///
    /// 5. **Resonance** — Rare/Legendary chance for an extra
    ///    cross-family axis line.
    ///
    /// 6. **Anchored roll** — Legendary 1/5000, independent.
    pub fn roll(base: &'static BaseItem, rarity: Rarity, ilvl: u32, rng: &mut LootRng) -> Self {
        use super::affixes::{affix_attribute, affix_element, category, AffixCategory};
        use crate::stats::Stat;

        // Consumables are inert items \u2014 effect comes from the
        // `ConsumableKind` discriminator on the base, not from
        // a stat / affix block. Short-circuit the entire roll
        // pipeline so a respec token is just `(base, rarity,
        // ilvl)` with no affix surface to (mis)handle later.
        if matches!(base.slot, super::ItemSlot::Consumable(_)) {
            return Self {
                base,
                rarity,
                ilvl,
                affixes: Vec::new(),
                anchored: false,
                unstable: false,
                provenance: None,
                unique_id: None,
                unique_pick: None,
                rift_touched: None,
                enchanted_affix_index: None,
            };
        }

        let mut rolled: Vec<RolledAffix> = Vec::new();
        let mut used_stats: std::collections::HashSet<Stat> = Default::default();
        // Seed `used_stats` with the base item's implicit stats so
        // signature / trio / bonus rolls can't double-up on a
        // stat the implicit already carries. Without this a chest
        // with implicit `+18 Health` could still roll
        // `flat_health` as a signature and ship two Health lines.
        for &(stat, _) in base.implicit {
            used_stats.insert(stat);
        }

        // Track a rolled affix; updates `used_stats` so later
        // phases never duplicate the same `Stat`.
        let push = |def: &'static AffixDef,
                    value: f32,
                    rolled: &mut Vec<RolledAffix>,
                    used: &mut std::collections::HashSet<Stat>| {
            if let AffixEffect::Stat(s) = def.effect {
                used.insert(s);
            }
            rolled.push(RolledAffix { def, value });
        };

        // Signature injection. The consumable short-circuit
        // above means every base reaching this point has a
        // real equip slot — unwrap is safe and documents
        // the invariant.
        let equip_slot = base
            .equip_slot
            .expect("non-consumable base must declare an equip slot");
        let sig_ids = super::affixes::signature_for(equip_slot, rng);
        for id in &sig_ids {
            if let Some(def) = super::affixes::lookup(id) {
                // Skip signature lines whose `Stat` already
                // appears in the implicit set — e.g. a Chest
                // with `+18 Health` implicit no longer also
                // ships a `flat_health` signature line. Without
                // this guard the tooltip would show two
                // separate Health rows that sum at equip time.
                if let AffixEffect::Stat(s) = def.effect {
                    if used_stats.contains(&s) {
                        continue;
                    }
                }
                let value = roll_value(def, ilvl, rng);
                push(def, value, &mut rolled, &mut used_stats);
            }
        }

        // Duo: Attribute × Element damage axes.
        //
        // Decide which axes this rarity activates.
        //   Common    — Attribute only (one identity line).
        //   Magic     — Attribute + Element.
        //   Rare/Leg  — Attribute + Element (same as Magic; the
        //               extra slot budget is spent on bonus rolls).
        let (do_attribute, do_element) = match rarity {
            Rarity::Common => (true, false),
            Rarity::Magic | Rarity::Rare | Rarity::Legendary => (true, true),
        };

        // Uniform pick from a family-locked axis sub-pool. Returns
        // `None` when no candidate clears the family + ilvl
        // filters (uncommon but possible at very low ilvl).
        fn pick_axis(
            base: &BaseItem,
            ilvl: u32,
            rng: &mut LootRng,
            cat: AffixCategory,
        ) -> Option<&'static AffixDef> {
            let candidates: Vec<&'static AffixDef> = AFFIX_POOL
                .iter()
                .filter(|d| {
                    if d.min_ilvl > ilvl {
                        return false;
                    }
                    if category(d) != cat {
                        return false;
                    }
                    match cat {
                        AffixCategory::Element => affix_element(d)
                            .map(|e| base.family.allows_element(e))
                            .unwrap_or(false),
                        AffixCategory::Attribute => affix_attribute(d)
                            .map(|a| base.family.allows_attribute(a))
                            .unwrap_or(false),
                        AffixCategory::Bonus => false,
                        AffixCategory::Resonance => false,
                        // Rift-touched lines roll only via the
                        // drop-site helper (`roll_rift_touched`),
                        // never through the trio pipeline.
                        AffixCategory::RiftTouched => false,
                    }
                })
                .collect();
            if candidates.is_empty() {
                return None;
            }
            let idx = rng.range(0, candidates.len() as u32) as usize;
            Some(candidates[idx])
        }

        // Magic's xor: try the chosen axis; if it produces nothing
        // (family rejected every candidate), try the other before
        // giving up — better to silently produce the trio's third
        // option than to ship a stat-thin Magic drop.
        if do_attribute {
            if let Some(def) = pick_axis(base, ilvl, rng, AffixCategory::Attribute) {
                let value = roll_value(def, ilvl, rng);
                push(def, value, &mut rolled, &mut used_stats);
            }
        }
        if do_element {
            if let Some(def) = pick_axis(base, ilvl, rng, AffixCategory::Element) {
                let value = roll_value(def, ilvl, rng);
                push(def, value, &mut rolled, &mut used_stats);
            }
        }

        // Bonus rolls.
        let (lo, hi) = rarity.affix_count_range();
        let bonus_count = rng.range(lo, hi + 1) as usize;

        // Bonus pool: only the `Bonus` category, weight > 0,
        // legendary effects filtered out, and excluding any Stat
        // already touched by signature or trio. Legacy
        // `allowed_tags` / `favored_tags` masks still drive the
        // bonus-flavour bias.
        let bonus_pool_filter =
            |a: &&'static AffixDef,
             rolled: &[RolledAffix],
             used: &std::collections::HashSet<Stat>| {
                if category(a) != AffixCategory::Bonus {
                    return false;
                }
                if super::affixes::is_legendary_effect(&a.effect) {
                    return false;
                }
                if a.weight == 0 {
                    return false;
                }
                if a.min_ilvl > ilvl {
                    return false;
                }
                if !rarity.at_least(a.rarity_min) {
                    return false;
                }
                if (a.tags & base.allowed_tags) == 0 {
                    return false;
                }
                // Dedupe — by affix id (for non-stat lines) and by
                // Stat (for the no-duplicate-stat invariant).
                if rolled.iter().any(|r| r.def.id == a.id) {
                    return false;
                }
                if let AffixEffect::Stat(s) = a.effect {
                    if used.contains(&s) {
                        return false;
                    }
                    // Weapons are an offensive slot — gate bonus
                    // **stat** rolls to the offensive set
                    // (Crit / Attack Speed) so a sword never
                    // shows up with `+Armor` or `+Evasion`. Non-
                    // Stat effects (Amplify / CDR / Proc /
                    // ExtraProjectiles) are unaffected because
                    // they're inherently offensive utility on
                    // a weapon.
                    if equip_slot == super::items::EquipSlot::Weapon && !s.is_offensive_bonus() {
                        return false;
                    }
                }
                true
            };

        for _ in 0..bonus_count {
            let candidates: Vec<&'static AffixDef> = AFFIX_POOL
                .iter()
                .filter(|a| bonus_pool_filter(a, &rolled, &used_stats))
                .collect();
            if candidates.is_empty() {
                break;
            }
            let weights: Vec<u32> = candidates
                .iter()
                .map(|a| {
                    let base_w = a.weight;
                    if (a.tags & base.favored_tags) != 0 {
                        base_w * 2
                    } else {
                        base_w
                    }
                })
                .collect();
            let Some(pick) = rng.weighted_index(&weights) else {
                break;
            };
            let def = candidates[pick];
            let value = roll_value(def, ilvl, rng);
            push(def, value, &mut rolled, &mut used_stats);
        }

        // Legendary effect.
        // Hand-authored uniques win over the procedural pool
        // when one exists for this `(slot, base)`. Falling back
        // to the procedural roll keeps Phase-2 behaviour for
        // slots / bases that don't have an authored entry yet —
        // every Legendary still produces *some* legendary effect.
        let mut unique_id: Option<&'static str> = None;
        let mut unique_pick: Option<u8> = None;
        if rarity == Rarity::Legendary {
            let candidates = super::uniques::candidates_for(base);
            if !candidates.is_empty() {
                let pick_idx = rng.range(0, candidates.len() as u32) as usize;
                let def = candidates[pick_idx];
                unique_id = Some(def.id);
                if def.needs_pick() {
                    let len = def.pool_len().max(1) as u32;
                    unique_pick = Some(rng.range(0, len) as u8);
                }
            } else {
                // Procedural fallback: legacy legendary-effect
                // roll. Kept so bases without an authored unique
                // still feel legendary.
                let mut effect_candidates: Vec<(&'static AffixDef, u32)> = AFFIX_POOL
                    .iter()
                    .filter(|a| {
                        super::affixes::is_legendary_effect(&a.effect)
                            && (a.tags & base.allowed_tags) != 0
                            && a.min_ilvl <= ilvl
                            && a.weight > 0
                    })
                    .map(|a| (a, a.weight))
                    .collect();
                effect_candidates.retain(|(a, _)| !rolled.iter().any(|r| r.def.id == a.id));
                if !effect_candidates.is_empty() {
                    let weights: Vec<u32> = effect_candidates.iter().map(|(_, w)| *w).collect();
                    if let Some(pick) = rng.weighted_index(&weights) {
                        let def = effect_candidates[pick].0;
                        let value = roll_value(def, ilvl, rng);
                        push(def, value, &mut rolled, &mut used_stats);
                    }
                }
            }
        }

        // Resonance.
        // ITEMS.md §2.5: with `{Rare: 5 %, Legendary: 25 %}` the
        // item gets an extra cross-family axis line drawn from
        // `RESONANCE_POOL`. Filter to axes the base's
        // `BaseFamily` would normally *reject* — the whole point
        // of resonance is that it's "off-archetype". Also skip
        // stats already on the item to keep the no-duplicate-
        // stat invariant intact.
        let res_chance = super::affixes::resonance_chance(rarity);
        if res_chance > 0.0 && rng.next_f32() < res_chance {
            let candidates: Vec<&'static AffixDef> = super::affixes::RESONANCE_POOL
                .iter()
                .filter(|a| {
                    if a.min_ilvl > ilvl {
                        return false;
                    }
                    if !rarity.at_least(a.rarity_min) {
                        return false;
                    }
                    // No-duplicate-stat: skip if the stat is
                    // already present (from signature or trio).
                    if let AffixEffect::Stat(s) = a.effect {
                        if used_stats.contains(&s) {
                            return false;
                        }
                    }
                    // Cross-family rule: keep only axes the family
                    // *rejects*. A wildcard family rejects nothing
                    // and therefore cannot resonate — accessories
                    // pull from the normal trio for their flavour
                    // (see §2.3). Element / Archetype checked
                    // independently; one match suffices.
                    if let Some(e) = super::affixes::resonance_element(a) {
                        if !base.family.allows_element(e) {
                            return true;
                        }
                    }
                    if let Some(at) = super::affixes::resonance_attribute(a) {
                        if !base.family.allows_attribute(at) {
                            return true;
                        }
                    }
                    false
                })
                .collect();
            if !candidates.is_empty() {
                let pick = rng.range(0, candidates.len() as u32) as usize;
                let def = candidates[pick];
                let value = roll_value(def, ilvl, rng);
                push(def, value, &mut rolled, &mut used_stats);
            }
        }

        // Anchored roll.
        // Legendary-only and *very* rare. Decided after the
        // affix block so the stat roll is independent.
        let anchored = rarity == Rarity::Legendary && rng.next_f32() < ANCHORED_CHANCE;

        Self {
            base,
            rarity,
            ilvl,
            affixes: rolled,
            anchored,
            // Freshly-rolled items are stable by default. The
            // server flips `unstable = true` at pickup time iff
            // the picker is in a rift (see
            // `Sim::try_pickup_loot`). Roll-time tests, debug
            // seeding, and any future hub-spawn drops therefore
            // come out stable for free.
            unstable: false,
            // Caller (server `drop_for_enemy`) is responsible for
            // attaching provenance after the roll — it owns the
            // current Sim's character roster, which `rift-game`
            // doesn't know about. Items rolled via tests / debug
            // seeding leave it `None` and self-bind on first
            // interaction.
            provenance: None,
            unique_id,
            unique_pick,
            // Rift-touched is rolled separately by the drop site
            // (`server::sim::loot::drop_for_enemy`) after the
            // base `Item::roll` completes, so the constructor
            // always leaves it `None`. The caller attaches a
            // `RolledRiftTouched` when the floor gate + chance
            // gate both pass.
            rift_touched: None,
            enchanted_affix_index: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frange_inclusive_formula_hits_hi_at_max_unit() {
        // Mirrors [`LootRng::frange`]: `t = u / MASK` reaches `1.0` when `u == MASK`.
        const MASK: u64 = (1u64 << 24) - 1;
        let t_max = MASK as f32 / MASK as f32;
        assert!((t_max - 1.0).abs() < 1e-6);
        let lo = 10.0f32;
        let hi = 20.0f32;
        let at_max = lo + (hi - lo) * t_max;
        assert!((at_max - hi).abs() < 1e-4);
    }

    #[test]
    fn roll_percentile_flat_stat_matches_display_rounding() {
        let def = AFFIX_POOL
            .iter()
            .find(|d| d.id == "flat_strength")
            .expect("flat_strength affix");
        // ilvl 3 → lo=5.2 hi=9.2; value 8.94 rounds to +9 like the tooltip.
        let p = roll_percentile(def, 3, 8.94).expect("non-degenerate");
        assert!(
            (p - 1.0).abs() < 1e-5,
            "rounded bands treat +9 at hi_round=9 as perfect (got {p})"
        );
        // Continuous mapping would be ~(8.94 - 5.2) / 4.0 ≈ 0.935 < 0.95.
        let continuous = (8.94_f32 - 5.2) / (9.2 - 5.2);
        assert!(continuous < 0.95);
    }
}
