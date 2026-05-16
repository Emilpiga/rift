//! Anvil enchanting helpers for mutating one affix slot on an item.

use std::collections::HashSet;

use crate::stats::Stat;

use super::affixes::{
    affix_attribute, affix_element, category, is_legendary_effect, resonance_attribute,
    resonance_element, AffixCategory, AffixDef, AffixEffect, AFFIX_POOL, RESONANCE_POOL,
};
use super::item::{Item, RolledAffix};
use super::items::{BaseItem, EquipSlot};
use super::rng::LootRng;
use super::roll::roll_range;
use super::tooltip::enchant_candidate_preview;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnchantError {
    InvalidAffixIndex,
    LockedToDifferentAffix,
    NoCandidate,
}

pub fn reroll_affix(
    item: &mut Item,
    affix_index: u8,
    rng: &mut LootRng,
) -> Result<(), EnchantError> {
    let idx = affix_index as usize;
    let candidates = reroll_candidates(item, affix_index)?;
    if candidates.is_empty() {
        return Err(EnchantError::NoCandidate);
    }

    let weights: Vec<u32> = candidates
        .iter()
        .map(|def| {
            let mut w = def.weight.max(1);
            if (def.tags & item.base.favored_tags) != 0 {
                w = w.saturating_mul(2);
            }
            w
        })
        .collect();
    let pick = rng
        .weighted_index(&weights)
        .unwrap_or_else(|| rng.range(0, candidates.len() as u32) as usize);
    let def = candidates[pick];
    let (lo, hi) = roll_range(def, item.ilvl);
    let value = if (hi - lo).abs() < 1e-6 {
        lo
    } else {
        rng.frange(lo, hi)
    };

    // Hands signatures are CritChance + CritDamage (`signature_for`). Rolling one axis
    // into the other's *stat* while the sibling already holds that stat would duplicate
    // `CritChance`/`CritDamage` across two lines — illegal. Catch that outcome here and
    // **swap** the old axis onto the sibling with a fresh magnitude roll instead.
    let mut applied_swap = false;
    if let Some(sib) = glove_crit_sibling_idx(item, idx) {
        if let Some(srolled) = item.affixes.get(sib) {
            if let (AffixEffect::Stat(new_st), AffixEffect::Stat(sib_st)) =
                (def.effect, srolled.def.effect)
            {
                if new_st == sib_st && matches!(new_st, Stat::CritChance | Stat::CritDamage) {
                    let old_current =
                        std::mem::replace(&mut item.affixes[idx], RolledAffix { def, value });
                    item.affixes[sib] = old_current;
                    let sdef = item.affixes[sib].def;
                    let (lo2, hi2) = roll_range(sdef, item.ilvl);
                    item.affixes[sib].value = if (hi2 - lo2).abs() < 1e-6 {
                        lo2
                    } else {
                        rng.frange(lo2, hi2)
                    };
                    applied_swap = true;
                }
            }
        }
    }
    if !applied_swap {
        item.affixes[idx] = RolledAffix { def, value };
    }
    item.enchanted_affix_index = Some(affix_index);
    Ok(())
}

pub fn reroll_candidate_tooltips(
    item: &Item,
    affix_index: u8,
) -> Result<Vec<String>, EnchantError> {
    Ok(reroll_candidates(item, affix_index)?
        .into_iter()
        .map(|def| enchant_candidate_preview(def, item.ilvl))
        .collect())
}

/// Affix lines in the reroll pool that are **not** valid for this slot, with a
/// compact UX label explaining the first gate that rejects them (mirrors
/// [`candidate_compatibility`]).
pub fn reroll_excluded_previews(
    item: &Item,
    affix_index: u8,
) -> Result<Vec<(String, &'static str)>, EnchantError> {
    let idx = affix_index as usize;
    if idx >= item.affixes.len() {
        return Err(EnchantError::InvalidAffixIndex);
    }
    if let Some(locked) = item.enchanted_affix_index {
        if locked != affix_index {
            return Err(EnchantError::LockedToDifferentAffix);
        }
    }

    let current = item.affixes[idx].def;
    let cat = category(current);
    if cat == AffixCategory::RiftTouched {
        return Err(EnchantError::NoCandidate);
    }

    let used_stats = used_stats_excluding(item, idx);
    let pool: Vec<&'static AffixDef> = match cat {
        AffixCategory::Resonance => RESONANCE_POOL.iter().collect(),
        _ => AFFIX_POOL.iter().collect(),
    };

    let mut excluded: Vec<(String, &'static str)> = Vec::new();
    for candidate in pool {
        if let Err(reason) =
            candidate_compatibility(item, idx, current, candidate, cat, &used_stats)
        {
            excluded.push((enchant_candidate_preview(candidate, item.ilvl), reason));
        }
    }
    Ok(excluded)
}

fn reroll_candidates(item: &Item, affix_index: u8) -> Result<Vec<&'static AffixDef>, EnchantError> {
    let idx = affix_index as usize;
    if idx >= item.affixes.len() {
        return Err(EnchantError::InvalidAffixIndex);
    }
    if let Some(locked) = item.enchanted_affix_index {
        if locked != affix_index {
            return Err(EnchantError::LockedToDifferentAffix);
        }
    }

    let current = item.affixes[idx].def;
    let cat = category(current);
    if cat == AffixCategory::RiftTouched {
        return Err(EnchantError::NoCandidate);
    }

    let used_stats = used_stats_excluding(item, idx);
    let pool: Vec<&'static AffixDef> = match cat {
        AffixCategory::Resonance => RESONANCE_POOL.iter().collect(),
        _ => AFFIX_POOL.iter().collect(),
    };
    Ok(pool
        .into_iter()
        .filter(|def| candidate_compatibility(item, idx, current, def, cat, &used_stats).is_ok())
        .collect())
}

fn fnv1a64(data: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 14695981039346656037;
    const PRIME: u64 = 1099511628211;
    let mut hash = OFFSET_BASIS;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// Stable entropy for [`reroll_affix`] — mixes sim tick, player id,
/// enchant source coordinates, **and** a snapshot of the target item
/// so two rerolls on the same tick cannot share an RNG stream.
///
/// `source_tag`: `0` = bag (`source_detail` = inventory index),
/// `1` = equipped (`source_detail` = equip-slot wire byte).
pub fn reroll_entropy_seed(
    tick: u32,
    player_key: u64,
    source_tag: u8,
    source_detail: u32,
    affix_index: u8,
    item: &Item,
) -> u64 {
    let mut buf: Vec<u8> = Vec::with_capacity(192);
    buf.extend_from_slice(&tick.to_le_bytes());
    buf.extend_from_slice(&player_key.to_le_bytes());
    buf.push(source_tag);
    buf.extend_from_slice(&source_detail.to_le_bytes());
    buf.push(affix_index);
    buf.extend_from_slice(item.base.id.as_bytes());
    buf.push(0);
    buf.push(item.rarity as u8);
    buf.extend_from_slice(&item.ilvl.to_le_bytes());
    buf.push(item.anchored as u8);
    buf.push(item.unstable as u8);
    match item.enchanted_affix_index {
        None => buf.push(0),
        Some(i) => {
            buf.push(1);
            buf.push(i);
        }
    }
    match item.unique_pick {
        None => buf.push(0),
        Some(p) => {
            buf.push(1);
            buf.push(p);
        }
    }
    if let Some(uid) = item.unique_id {
        buf.extend_from_slice(uid.as_bytes());
        buf.push(0);
    }
    for a in &item.affixes {
        buf.extend_from_slice(a.def.id.as_bytes());
        buf.push(0);
        buf.extend_from_slice(&a.value.to_bits().to_le_bytes());
    }
    fnv1a64(&buf)
}

/// Indices of the glove crit signature lines — located by **affix id**, not vec position,
/// so rerolls stay correct even if older saves or tooling reorder [`Item::affixes`].
fn glove_crit_pair_indices(item: &Item) -> Option<(usize, usize)> {
    if item.base.equip_slot != Some(EquipSlot::Hands) {
        return None;
    }
    let mut idx_chance = None;
    let mut idx_damage = None;
    for (i, r) in item.affixes.iter().enumerate() {
        match r.def.id {
            "pct_crit_chance" => idx_chance = Some(i),
            "pct_crit_damage" => idx_damage = Some(i),
            _ => {}
        }
    }
    Some((idx_chance?, idx_damage?))
}

fn glove_crit_sibling_idx(item: &Item, skip_idx: usize) -> Option<usize> {
    let (cc, cd) = glove_crit_pair_indices(item)?;
    if skip_idx == cc {
        Some(cd)
    } else if skip_idx == cd {
        Some(cc)
    } else {
        None
    }
}

/// Crit pair occupies two Hands lines; rolling e.g. Crit Damage on the Crit Chance row
/// while the sibling already shows Crit Damage matches **affix id** / **stat** with the
/// sibling — reroll applies a swap ([`reroll_affix`]) instead of duplicating.
fn gloves_crit_pair_swap_candidate(item: &Item, skip_idx: usize, candidate: &AffixDef) -> bool {
    let Some(sib) = glove_crit_sibling_idx(item, skip_idx) else {
        return false;
    };
    if !matches!(candidate.id, "pct_crit_chance" | "pct_crit_damage") {
        return false;
    }
    let Some(srolled) = item.affixes.get(sib) else {
        return false;
    };
    if srolled.def.id != candidate.id {
        return false;
    }
    let mut dup_count = 0usize;
    for (i, r) in item.affixes.iter().enumerate() {
        if i != skip_idx && r.def.id == candidate.id {
            dup_count += 1;
            if i != sib {
                return false;
            }
        }
    }
    dup_count == 1
}

fn used_stats_excluding(item: &Item, skip_idx: usize) -> HashSet<Stat> {
    let mut used = HashSet::new();
    for &(stat, _) in item.base.implicit {
        used.insert(stat);
    }
    for (i, rolled) in item.affixes.iter().enumerate() {
        if i == skip_idx {
            continue;
        }
        if let AffixEffect::Stat(stat) = rolled.def.effect {
            used.insert(stat);
        }
    }
    used
}

/// Align rerolls with the tooltip bonus block partition (`tooltip.rs`):
/// offensive bonus stats (`Crit*`, `AttackSpeed`, key minion stats) stay in that
/// lane; sustain / mobility / mitigation bonuses (`MoveSpeed`, regens, bulk etc.)
/// stay in the other. Without this, every `Bonus` Stat line could reroll into any
/// other tag-compatible stat — e.g. signature gloves crit damage previewing move speed.
fn bonus_stat_reroll_lane_compatible(current: &AffixDef, candidate: &AffixDef) -> bool {
    match (&current.effect, &candidate.effect) {
        (AffixEffect::Stat(a), AffixEffect::Stat(b)) => {
            a.is_offensive_bonus() == b.is_offensive_bonus()
        }
        _ => true,
    }
}

/// Bonus rerolls normally mirror drops ([`BaseItem::allowed_tags`]). Signature lines
/// bypass that filter (`signature_for` injects ids directly), so an affix can exist on
/// a base whose `allowed_tags` omits its tag mask — e.g. glove crit pair uses tag `CRIT`
/// only while many glove bases list `CASTER | UTILITY | SUMMON` but not `CRIT`. Without
/// this OR branch, rerolling crit could only offer stats that hit `allowed_tags` alone
/// (often just attack speed via `CASTER`).
///
/// Finally, **offensive bonus** stats ([`Stat::is_offensive_bonus`]) share one reroll lane
/// ([`bonus_stat_reroll_lane_compatible`]) but their tag masks often don't intersect —
/// `CRIT` vs `SPEED|CASTER`, etc. Allow reshuffling within that lane regardless of tags so
/// e.g. glove attack speed can reroll into crit chance/damage.
fn bonus_reroll_tags_allow(base: &BaseItem, current: &AffixDef, candidate: &AffixDef) -> bool {
    if (candidate.tags & base.allowed_tags) != 0 || (candidate.tags & current.tags) != 0 {
        return true;
    }
    matches!(
        (&current.effect, &candidate.effect),
        (AffixEffect::Stat(a), AffixEffect::Stat(b))
            if a.is_offensive_bonus() && b.is_offensive_bonus()
    )
}

/// Glove signatures always inject both crit lines ([`super::affixes::signature_for`]),
/// but `pct_crit_damage` carries `min_ilvl = 5` for **drops**. Low-ilvl gloves can still
/// show both lines — any reroll **between the two crit affix ids** (refresh **or** swap)
/// must not reject `pct_crit_damage` for `min_ilvl` alone.
fn gloves_crit_line_reroll_skips_candidate_min_ilvl(
    current: &AffixDef,
    candidate: &AffixDef,
) -> bool {
    matches!(current.id, "pct_crit_chance" | "pct_crit_damage")
        && matches!(candidate.id, "pct_crit_chance" | "pct_crit_damage")
}

/// Whether `candidate` can replace the rolled affix at `skip_idx`. Returns `Ok(())`
/// when allowed; otherwise `Err(short_reason)` matches the **first** failing gate
/// (same order as the historical [`compatible_candidate`] conjunction).
fn candidate_compatibility(
    item: &Item,
    skip_idx: usize,
    current: &AffixDef,
    candidate: &AffixDef,
    cat: AffixCategory,
    used_stats: &HashSet<Stat>,
) -> Result<(), &'static str> {
    let min_ilvl_ok = candidate.min_ilvl <= item.ilvl
        || (item.base.equip_slot == Some(EquipSlot::Hands)
            && gloves_crit_line_reroll_skips_candidate_min_ilvl(current, candidate));
    if !min_ilvl_ok {
        return Err("needs ilvl");
    }
    if !item.rarity.at_least(candidate.rarity_min) {
        return Err("needs rarity");
    }
    let dup_elsewhere = item
        .affixes
        .iter()
        .enumerate()
        .any(|(i, r)| i != skip_idx && r.def.id == candidate.id);
    if dup_elsewhere && !gloves_crit_pair_swap_candidate(item, skip_idx, candidate) {
        return Err("duplicate line");
    }
    if let AffixEffect::Stat(stat) = candidate.effect {
        if used_stats.contains(&stat) && !gloves_crit_pair_swap_candidate(item, skip_idx, candidate)
        {
            return Err("stat used");
        }
    }
    if !same_effect_type(current.effect, candidate.effect) {
        return Err("wrong effect family");
    }
    match cat {
        AffixCategory::Element => {
            // Damage-axis % lines (Physical / Fire / Ice / Lightning). Drops respect
            // [`BaseFamily::allows_element`]; hub anvil rerolls intentionally allow
            // pivoting across these four so one slot can roll any elemental amplifier.
            if !(category(candidate) == AffixCategory::Element
                && affix_element(candidate).is_some())
            {
                return Err("not elemental %");
            }
        }
        AffixCategory::Attribute => {
            // Drops stay family-locked via [`Item::roll`]; the anvil may pivot across the
            // primary trio so attribute slots aren't stuck mirroring the base forever.
            if !(category(candidate) == cat && affix_attribute(candidate).is_some()) {
                return Err("not attribute");
            }
        }
        AffixCategory::Bonus => {
            if category(candidate) != cat {
                return Err("not bonus");
            }
            if candidate.weight == 0 {
                return Err("disabled affix");
            }
            if !bonus_reroll_tags_allow(item.base, current, candidate) {
                return Err("tag mismatch");
            }
            if is_legendary_effect(&candidate.effect) {
                return Err("legendary only");
            }
            if !bonus_stat_reroll_lane_compatible(current, candidate) {
                return Err("bonus lane");
            }
        }
        AffixCategory::Resonance => {
            let ok = category(candidate) == cat
                && resonance_element(candidate)
                    .map(|e| !item.base.family.allows_element(e))
                    .or_else(|| {
                        resonance_attribute(candidate)
                            .map(|a| !item.base.family.allows_attribute(a))
                    })
                    .unwrap_or(false);
            if !ok {
                return Err("resonance rule");
            }
        }
        AffixCategory::RiftTouched => {
            return Err("rift-touched");
        }
    }
    Ok(())
}

fn same_effect_type(a: AffixEffect, b: AffixEffect) -> bool {
    matches!(
        (a, b),
        (AffixEffect::Stat(_), AffixEffect::Stat(_))
            | (
                AffixEffect::AmplifyAbilityDamage(_),
                AffixEffect::AmplifyAbilityDamage(_)
            )
            | (
                AffixEffect::ReduceAbilityCooldown(_),
                AffixEffect::ReduceAbilityCooldown(_)
            )
            | (
                AffixEffect::ExtraProjectiles(_),
                AffixEffect::ExtraProjectiles(_)
            )
            | (
                AffixEffect::TransformAbility(_, _),
                AffixEffect::TransformAbility(_, _)
            )
            | (AffixEffect::Proc(_, _), AffixEffect::Proc(_, _))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loot::{BaseItem, Rarity, BASE_ITEMS};

    fn base(id: &str) -> &'static BaseItem {
        BASE_ITEMS
            .iter()
            .find(|b| b.id == id)
            .unwrap_or_else(|| panic!("missing base item `{id}`"))
    }

    fn rerollable_item() -> (Item, u8) {
        for seed in 1..500 {
            let item = Item::roll(
                base("staff_basic"),
                Rarity::Rare,
                20,
                &mut LootRng::new(seed),
            );
            for idx in 0..item.affixes.len() {
                let mut probe = item.clone();
                if reroll_affix(&mut probe, idx as u8, &mut LootRng::new(seed + 10_000)).is_ok() {
                    return (item, idx as u8);
                }
            }
        }
        panic!("test pool should produce a rerollable rare staff");
    }

    #[test]
    fn first_reroll_locks_slot_and_same_slot_can_repeat() {
        let (mut item, idx) = rerollable_item();
        assert_eq!(item.enchanted_affix_index, None);

        reroll_affix(&mut item, idx, &mut LootRng::new(42)).unwrap();
        assert_eq!(item.enchanted_affix_index, Some(idx));

        reroll_affix(&mut item, idx, &mut LootRng::new(43)).unwrap();
        assert_eq!(item.enchanted_affix_index, Some(idx));
    }

    #[test]
    fn locked_item_rejects_other_affix_slot() {
        let (mut item, idx) = rerollable_item();
        reroll_affix(&mut item, idx, &mut LootRng::new(42)).unwrap();

        let other = (0..item.affixes.len())
            .find(|i| *i != idx as usize)
            .expect("rare test item should have another affix") as u8;
        let before = item.affixes[other as usize].def.id;
        assert_eq!(
            reroll_affix(&mut item, other, &mut LootRng::new(99)),
            Err(EnchantError::LockedToDifferentAffix)
        );
        assert_eq!(item.affixes[other as usize].def.id, before);
    }

    #[test]
    fn attribute_reroll_any_primary_line_ok_and_locks_slot() {
        use crate::loot::affixes::AffixEffect;
        use crate::stats::Stat;

        let def = crate::loot::affixes::lookup("flat_intellect").unwrap();
        let mut item = Item {
            base: base("staff_basic"),
            rarity: Rarity::Rare,
            ilvl: 20,
            affixes: vec![RolledAffix { def, value: 10.0 }],
            anchored: false,
            unstable: false,
            provenance: None,
            unique_id: None,
            unique_pick: None,
            rift_touched: None,
            enchanted_affix_index: None,
        };

        assert_eq!(reroll_affix(&mut item, 0, &mut LootRng::new(1)), Ok(()));
        assert_eq!(item.enchanted_affix_index, Some(0));
        match item.affixes[0].def.effect {
            AffixEffect::Stat(Stat::Strength | Stat::Agility | Stat::Intellect) => {}
            _ => panic!("expected primary-attribute trio roll"),
        }
    }

    #[test]
    fn reroll_entropy_seed_split_collisions() {
        let def = crate::loot::affixes::lookup("flat_intellect").unwrap();
        let item_a = Item {
            base: base("staff_basic"),
            rarity: Rarity::Rare,
            ilvl: 20,
            affixes: vec![RolledAffix { def, value: 10.0 }],
            anchored: false,
            unstable: false,
            provenance: None,
            unique_id: None,
            unique_pick: None,
            rift_touched: None,
            enchanted_affix_index: None,
        };
        let item_b = Item {
            base: base("staff_basic"),
            rarity: Rarity::Rare,
            ilvl: 21,
            affixes: vec![RolledAffix { def, value: 10.0 }],
            anchored: false,
            unstable: false,
            provenance: None,
            unique_id: None,
            unique_pick: None,
            rift_touched: None,
            enchanted_affix_index: None,
        };
        assert_ne!(
            reroll_entropy_seed(42, 7, 0, 0, 0, &item_a),
            reroll_entropy_seed(42, 7, 0, 0, 0, &item_b),
            "same tick/player/source — items must diverge",
        );
        assert_ne!(
            reroll_entropy_seed(42, 7, 0, 0, 0, &item_a),
            reroll_entropy_seed(42, 7, 0, 1, 0, &item_a),
            "bag slot coordinate must affect entropy",
        );
    }

    #[test]
    fn bonus_reroll_keeps_offensive_lane_on_gloves_crit_damage() {
        let cd = crate::loot::affixes::lookup("pct_crit_damage").unwrap();
        let item = Item {
            base: base("light_gloves"),
            rarity: Rarity::Rare,
            ilvl: 20,
            affixes: vec![RolledAffix {
                def: cd,
                value: 0.166,
            }],
            anchored: false,
            unstable: false,
            provenance: None,
            unique_id: None,
            unique_pick: None,
            rift_touched: None,
            enchanted_affix_index: None,
        };

        let cands = reroll_candidates(&item, 0).expect("reroll candidates");
        assert!(
            cands.iter().any(|d| d.id == "pct_crit_chance"),
            "crit chance shares CRIT tags with crit damage — must preview on glove rerolls",
        );
        assert!(
            cands.iter().any(|d| d.id == "pct_attack_speed"),
            "attack speed should remain a legal reroll from crit damage (same offensive bonus lane)",
        );
        assert!(
            !cands.iter().any(|d| d.id == "pct_move_speed"),
            "move speed must not reroll from crit damage — different tooltip lane",
        );
        assert!(
            !cands.iter().any(|d| d.id == "pct_resource_regen"),
            "essence regen must not reroll from crit damage — sustain lane",
        );
        assert!(
            !cands.iter().any(|d| d.id == "flat_health_regen"),
            "health regen must not reroll from crit damage — sustain lane",
        );
    }

    #[test]
    fn glove_crit_signature_pair_lists_opposite_crit_in_pool() {
        let cc = crate::loot::affixes::lookup("pct_crit_chance").unwrap();
        let cd = crate::loot::affixes::lookup("pct_crit_damage").unwrap();
        let item = Item {
            base: base("light_gloves"),
            rarity: Rarity::Rare,
            ilvl: 20,
            affixes: vec![
                RolledAffix {
                    def: cc,
                    value: 0.03,
                },
                RolledAffix {
                    def: cd,
                    value: 0.15,
                },
            ],
            anchored: false,
            unstable: false,
            provenance: None,
            unique_id: None,
            unique_pick: None,
            rift_touched: None,
            enchanted_affix_index: None,
        };

        let c0 = reroll_candidates(&item, 0).expect("idx 0");
        assert!(
            c0.iter().any(|d| d.id == "pct_crit_damage"),
            "crit chance row must offer crit damage (swap semantics): {:?}",
            c0.iter().map(|d| d.id).collect::<Vec<_>>()
        );
        let c1 = reroll_candidates(&item, 1).expect("idx 1");
        assert!(
            c1.iter().any(|d| d.id == "pct_crit_chance"),
            "crit damage row must offer crit chance (swap semantics): {:?}",
            c1.iter().map(|d| d.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn glove_crit_swap_pool_when_crit_lines_not_at_indices_zero_one() {
        let cc = crate::loot::affixes::lookup("pct_crit_chance").unwrap();
        let cd = crate::loot::affixes::lookup("pct_crit_damage").unwrap();
        let fi = crate::loot::affixes::lookup("flat_intellect").unwrap();
        let fh = crate::loot::affixes::lookup("flat_health").unwrap();
        let item = Item {
            base: base("light_gloves"),
            rarity: Rarity::Rare,
            ilvl: 20,
            affixes: vec![
                RolledAffix {
                    def: fi,
                    value: 8.0,
                },
                RolledAffix {
                    def: cc,
                    value: 0.039,
                },
                RolledAffix {
                    def: cd,
                    value: 0.166,
                },
                RolledAffix {
                    def: fh,
                    value: 22.0,
                },
            ],
            anchored: false,
            unstable: false,
            provenance: None,
            unique_id: None,
            unique_pick: None,
            rift_touched: None,
            enchanted_affix_index: None,
        };

        let idx_cc = item
            .affixes
            .iter()
            .position(|a| a.def.id == "pct_crit_chance")
            .expect("crit chance line");
        let pool = reroll_candidates(&item, idx_cc as u8).expect("reroll pool");
        assert!(
            pool.iter().any(|d| d.id == "pct_crit_damage"),
            "must locate glove crit pair by id — previews cannot be attack-speed-only: {:?}",
            pool.iter().map(|d| d.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn glove_low_ilvl_crit_reroll_includes_sibling_crit_despite_damage_min_ilvl() {
        let cc = crate::loot::affixes::lookup("pct_crit_chance").unwrap();
        let cd = crate::loot::affixes::lookup("pct_crit_damage").unwrap();
        let item = Item {
            base: base("light_gloves"),
            rarity: Rarity::Rare,
            ilvl: 2,
            affixes: vec![
                RolledAffix {
                    def: cc,
                    value: 0.025,
                },
                RolledAffix {
                    def: cd,
                    value: 0.12,
                },
            ],
            anchored: false,
            unstable: false,
            provenance: None,
            unique_id: None,
            unique_pick: None,
            rift_touched: None,
            enchanted_affix_index: None,
        };

        let idx_cc = 0;
        let pool = reroll_candidates(&item, idx_cc as u8).expect("pool");
        assert!(
            pool.iter().any(|d| d.id == "pct_crit_damage"),
            "signature crit damage exists at ilvl 2 — reroll must not gate it out by min_ilvl: {:?}",
            pool.iter().map(|d| d.id).collect::<Vec<_>>()
        );

        let pool_cd = reroll_candidates(&item, 1).expect("crit damage row");
        assert!(
            pool_cd.iter().any(|d| d.id == "pct_crit_damage"),
            "crit damage refresh must bypass min_ilvl 5 when item already has signature damage: {:?}",
            pool_cd.iter().map(|d| d.id).collect::<Vec<_>>()
        );
        assert!(
            pool_cd.iter().any(|d| d.id == "pct_crit_chance"),
            "crit damage row must still offer chance (swap): {:?}",
            pool_cd.iter().map(|d| d.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn glove_crit_swap_roll_preserves_one_chance_one_damage() {
        let cc = crate::loot::affixes::lookup("pct_crit_chance").unwrap();
        let cd = crate::loot::affixes::lookup("pct_crit_damage").unwrap();
        let item_orig = Item {
            base: base("light_gloves"),
            rarity: Rarity::Rare,
            ilvl: 30,
            affixes: vec![
                RolledAffix {
                    def: cc,
                    value: 0.04,
                },
                RolledAffix {
                    def: cd,
                    value: 0.18,
                },
            ],
            anchored: false,
            unstable: false,
            provenance: None,
            unique_id: None,
            unique_pick: None,
            rift_touched: None,
            enchanted_affix_index: None,
        };

        let mut saw_swap = false;
        for seed in 0_u64..250_000 {
            let mut probe = item_orig.clone();
            let mut rng = LootRng::new(seed);
            if reroll_affix(&mut probe, 0, &mut rng).is_err() {
                continue;
            }
            let ids = probe
                .affixes
                .iter()
                .take(2)
                .map(|a| a.def.id)
                .collect::<Vec<_>>();
            if ids == ["pct_crit_damage", "pct_crit_chance"] {
                saw_swap = true;
                break;
            }
        }
        assert!(
            saw_swap,
            "expected some RNG seed to roll swap onto damage while sibling held damage",
        );
    }

    #[test]
    fn glove_signature_crit_pair_offers_multiple_bonus_candidates() {
        let cc = crate::loot::affixes::lookup("pct_crit_chance").unwrap();
        let cd = crate::loot::affixes::lookup("pct_crit_damage").unwrap();
        let item = Item {
            base: base("light_gloves"),
            rarity: Rarity::Rare,
            ilvl: 20,
            affixes: vec![
                RolledAffix {
                    def: cc,
                    value: 0.03,
                },
                RolledAffix {
                    def: cd,
                    value: 0.15,
                },
            ],
            anchored: false,
            unstable: false,
            provenance: None,
            unique_id: None,
            unique_pick: None,
            rift_touched: None,
            enchanted_affix_index: None,
        };

        let c0 = reroll_candidates(&item, 0).expect("idx 0");
        let ids0: Vec<_> = c0.iter().map(|d| d.id).collect();
        assert!(
            ids0.contains(&"pct_crit_chance"),
            "refresh same stat must stay legal (sibling holds other crit stat): {:?}",
            ids0
        );
        assert!(
            ids0.iter().any(|id| *id != "pct_attack_speed"),
            "expected candidates beyond attack speed alone: {:?}",
            ids0
        );

        let c1 = reroll_candidates(&item, 1).expect("idx 1");
        let ids1: Vec<_> = c1.iter().map(|d| d.id).collect();
        assert!(
            ids1.contains(&"pct_crit_damage"),
            "refresh same stat must stay legal: {:?}",
            ids1
        );
        assert!(
            ids1.iter().any(|id| *id != "pct_attack_speed"),
            "expected candidates beyond attack speed alone: {:?}",
            ids1
        );
    }

    #[test]
    fn bonus_reroll_attack_speed_offers_crit_pair_on_gloves() {
        let spd = crate::loot::affixes::lookup("pct_attack_speed").unwrap();
        let item = Item {
            base: base("light_gloves"),
            rarity: Rarity::Rare,
            ilvl: 20,
            affixes: vec![RolledAffix {
                def: spd,
                value: 0.07,
            }],
            anchored: false,
            unstable: false,
            provenance: None,
            unique_id: None,
            unique_pick: None,
            rift_touched: None,
            enchanted_affix_index: None,
        };

        let cands = reroll_candidates(&item, 0).expect("reroll candidates");
        assert!(
            cands.iter().any(|d| d.id == "pct_crit_chance"),
            "crit must reroll from attack speed — same offensive lane, CRIT-only tags",
        );
        assert!(
            cands.iter().any(|d| d.id == "pct_crit_damage"),
            "crit damage must reroll from attack speed — same offensive lane",
        );
    }

    #[test]
    fn attribute_reroll_pivots_across_primary_trio_despite_family_lock() {
        let fi = crate::loot::affixes::lookup("flat_intellect").unwrap();
        let item = Item {
            base: base("robe_chest"),
            rarity: Rarity::Rare,
            ilvl: 20,
            affixes: vec![RolledAffix {
                def: fi,
                value: 12.0,
            }],
            anchored: false,
            unstable: false,
            provenance: None,
            unique_id: None,
            unique_pick: None,
            rift_touched: None,
            enchanted_affix_index: None,
        };

        let cands = reroll_candidates(&item, 0).expect("reroll candidates");
        assert!(cands.iter().any(|d| d.id == "flat_strength"));
        assert!(cands.iter().any(|d| d.id == "flat_agility"));
        assert!(cands.iter().any(|d| d.id == "flat_intellect"));
    }

    #[test]
    fn bonus_reroll_keeps_sustain_lane_from_essence_regen() {
        let rg = crate::loot::affixes::lookup("pct_resource_regen").unwrap();
        let item = Item {
            base: base("light_gloves"),
            rarity: Rarity::Rare,
            ilvl: 20,
            affixes: vec![RolledAffix {
                def: rg,
                value: 0.08,
            }],
            anchored: false,
            unstable: false,
            provenance: None,
            unique_id: None,
            unique_pick: None,
            rift_touched: None,
            enchanted_affix_index: None,
        };

        let cands = reroll_candidates(&item, 0).expect("reroll candidates");
        assert!(
            !cands.iter().any(|d| d.id == "pct_crit_damage"),
            "crit damage must not reroll from essence regen — offensive lane",
        );
        assert!(
            !cands.iter().any(|d| d.id == "pct_attack_speed"),
            "attack speed must not reroll from essence regen — offensive lane",
        );
        assert!(
            cands.iter().any(|d| d.id == "pct_move_speed"),
            "move speed may reroll within sustain/mobility lane",
        );
    }

    #[test]
    fn damage_percent_enchant_pool_spans_all_elements() {
        let phys = crate::loot::affixes::lookup("pct_physical_damage").unwrap();
        let item = Item {
            base: base("dagger_basic"),
            rarity: Rarity::Rare,
            ilvl: 30,
            affixes: vec![RolledAffix {
                def: phys,
                value: 0.10,
            }],
            anchored: false,
            unstable: false,
            provenance: None,
            unique_id: None,
            unique_pick: None,
            rift_touched: None,
            enchanted_affix_index: None,
        };

        let cands = reroll_candidates(&item, 0).expect("reroll candidates");
        assert!(
            cands.iter().any(|d| d.id == "pct_fire_damage"),
            "fire damage % should appear even when the weapon family only rolls physical drops",
        );
    }

    #[test]
    fn reroll_pool_partitions_into_included_and_excluded_previews() {
        use crate::loot::affixes::{category, AffixCategory, AFFIX_POOL, RESONANCE_POOL};

        let (item, idx) = rerollable_item();
        let idx_usize = idx as usize;
        let n_in = reroll_candidates(&item, idx).expect("candidates").len();
        let n_ex = reroll_excluded_previews(&item, idx)
            .expect("excluded")
            .len();
        let cat = category(item.affixes[idx_usize].def);
        let pool_n = match cat {
            AffixCategory::Resonance => RESONANCE_POOL.len(),
            _ => AFFIX_POOL.len(),
        };
        assert_eq!(
            n_in + n_ex,
            pool_n,
            "every pool row must land in either reroll outcomes or excluded previews",
        );
    }
}
