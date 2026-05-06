//! A rolled drop: base item + rarity + a list of [`RolledAffix`].
//!
//! Items are immutable once rolled — [`Item::roll`] is the only way
//! to produce one. Save/load rehydrates by serialising
//! `(base_id, rarity, ilvl, [(affix_id, value)])` and reconstructing
//! here.

use super::affixes::{AffixDef, AffixEffect, AFFIX_POOL};
use super::items::BaseItem;
use super::rarity::Rarity;
use super::rng::LootRng;
use super::stats::StatBlock;

/// One realised affix on an item.
#[derive(Clone, Debug)]
pub struct RolledAffix {
    pub def: &'static AffixDef,
    /// Magnitude rolled within the affix's range. Meaning depends
    /// on `def.effect` (see [`AffixEffect`] docs).
    pub value: f32,
}

impl RolledAffix {
    /// Render the line for tooltips. Effects with no numeric value
    /// (Transform) ignore the template's `{}` placeholder.
    pub fn tooltip(&self) -> String {
        let value_str = match self.def.effect {
            AffixEffect::Stat(stat) => {
                if stat.is_percent() {
                    format!("{:+.1}%", self.value * 100.0)
                } else {
                    format!("{:+.0}", self.value)
                }
            }
            AffixEffect::AmplifyAbilityDamage(_)
            | AffixEffect::ReduceAbilityCooldown(_) => {
                format!("{:+.0}%", self.value * 100.0)
            }
            AffixEffect::ExtraProjectiles(_) => format!("+{}", self.value.round() as i32),
            AffixEffect::Proc(_, _) => format!("{:.0}%", self.value * 100.0),
            AffixEffect::TransformAbility(_, _) => String::new(),
        };
        if self.def.name_template.contains("{}") {
            self.def.name_template.replace("{}", &value_str)
        } else {
            self.def.name_template.to_string()
        }
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    pub base: &'static BaseItem,
    pub rarity: Rarity,
    pub ilvl: u32,
    pub affixes: Vec<RolledAffix>,
}

impl Item {
    /// Roll a fresh drop of `base` at the given rarity / item-level.
    ///
    /// Filtering rules (in order):
    /// 1. `affix.tags & base.allowed_tags != 0` — synergy gate.
    /// 2. `affix.min_ilvl <= ilvl` — ilvl gate.
    /// 3. `rarity.at_least(affix.rarity_min)` — behavioural gate.
    ///    This is what stops Common items from ever rolling
    ///    Transform/Proc affixes.
    /// 4. No duplicate affix ids on one item.
    /// 5. Weight ×2 if `affix.tags & base.favored_tags != 0`.
    pub fn roll(
        base: &'static BaseItem,
        rarity: Rarity,
        ilvl: u32,
        rng: &mut LootRng,
    ) -> Self {
        let (lo, hi) = rarity.affix_count_range();
        let count = rng.range(lo, hi + 1) as usize;

        let mut candidates: Vec<(&'static AffixDef, u32)> = AFFIX_POOL
            .iter()
            .filter(|a| {
                (a.tags & base.allowed_tags) != 0
                    && a.min_ilvl <= ilvl
                    && rarity.at_least(a.rarity_min)
            })
            .map(|a| {
                let favored = (a.tags & base.favored_tags) != 0;
                let weight = if favored { a.weight * 2 } else { a.weight };
                (a, weight)
            })
            .collect();

        let mut rolled: Vec<RolledAffix> = Vec::with_capacity(count);
        for _ in 0..count {
            if candidates.is_empty() {
                break;
            }
            let weights: Vec<u32> = candidates.iter().map(|(_, w)| *w).collect();
            let Some(pick) = rng.weighted_index(&weights) else {
                break;
            };
            let def = candidates[pick].0;
            // No duplicate affix lines on the same item.
            candidates.retain(|(c, _)| c.id != def.id);

            let scale = (ilvl.saturating_sub(1)) as f32 * def.ilvl_scale;
            let value = if def.roll.0 == def.roll.1 {
                def.roll.0
            } else {
                rng.frange(def.roll.0 + scale, def.roll.1 + scale)
            };
            rolled.push(RolledAffix { def, value });
        }

        Self {
            base,
            rarity,
            ilvl,
            affixes: rolled,
        }
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

    pub fn display_name(&self) -> String {
        format!("{} {}", self.rarity.name(), self.base.name)
    }

    /// Multi-line tooltip ready for UI rendering.
    pub fn tooltip(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(2 + self.affixes.len());
        out.push(self.display_name());
        out.push(format!("Item Level {}", self.ilvl));
        if !self.base.implicit.is_empty() {
            for &(stat, value) in self.base.implicit {
                out.push(stat.format(value));
            }
            out.push(String::new()); // visual separator
        }
        for a in &self.affixes {
            out.push(a.tooltip());
        }
        out
    }

    /// Pack the rolled item into a wire-friendly tuple of static-pool
    /// indices: `(base_id, rarity_byte, ilvl, [(affix_id, value)])`.
    /// `rift-game` is dependency-free of the wire crate by design,
    /// so the network layer wraps this tuple in its own struct.
    ///
    /// # Panics
    ///
    /// Panics if `self.base` doesn't live inside [`super::BASE_ITEMS`]
    /// or one of the rolled affix defs doesn't live inside
    /// [`AFFIX_POOL`]. Both invariants are guaranteed for items
    /// produced by [`Item::roll`].
    pub fn to_wire(&self) -> (u16, u8, u16, Vec<(u16, f32)>) {
        // Match by `id` rather than pointer identity — `BASE_ITEMS`
        // and `AFFIX_POOL` are `pub const` slices, so each access
        // can produce a fresh copy with different addresses.
        let base_id = super::items::BASE_ITEMS
            .iter()
            .position(|b| b.id == self.base.id)
            .expect("base item id not in BASE_ITEMS") as u16;
        let affixes = self
            .affixes
            .iter()
            .map(|a| {
                let id = AFFIX_POOL
                    .iter()
                    .position(|d| d.id == a.def.id)
                    .expect("affix id not in AFFIX_POOL") as u16;
                (id, a.value)
            })
            .collect();
        (base_id, self.rarity as u8, self.ilvl as u16, affixes)
    }

    /// Inverse of [`Item::to_wire`]. Returns `None` if any index is
    /// out of bounds (mismatched build / corrupted save).
    pub fn from_wire(
        base_id: u16,
        rarity_byte: u8,
        ilvl: u16,
        affixes: &[(u16, f32)],
    ) -> Option<Self> {
        let base = super::items::BASE_ITEMS.get(base_id as usize)?;
        let rarity = match rarity_byte {
            0 => Rarity::Common,
            1 => Rarity::Magic,
            2 => Rarity::Rare,
            3 => Rarity::Legendary,
            _ => return None,
        };
        let mut rolled = Vec::with_capacity(affixes.len());
        for &(id, value) in affixes {
            let def = AFFIX_POOL.get(id as usize)?;
            rolled.push(RolledAffix { def, value });
        }
        Some(Self {
            base,
            rarity,
            ilvl: ilvl as u32,
            affixes: rolled,
        })
    }

    /// Pack the rolled item into a tuple keyed by *stable* string
    /// ids (`BaseItem.id`, `AffixDef.id`) suitable for long-term
    /// storage. Unlike [`Item::to_wire`] this does not depend on
    /// the in-process pool ordering, so saved rows survive a
    /// rebuild that reorders `BASE_ITEMS` / `AFFIX_POOL`.
    ///
    /// # Panics
    ///
    /// Panics if `self.base` or any affix def doesn't carry an id
    /// — both invariants hold for items produced by [`Item::roll`].
    pub fn to_persisted(&self) -> (String, u8, u16, Vec<(String, f32)>) {
        let affixes = self
            .affixes
            .iter()
            .map(|a| (a.def.id.to_string(), a.value))
            .collect();
        (
            self.base.id.to_string(),
            self.rarity as u8,
            self.ilvl as u16,
            affixes,
        )
    }

    /// Inverse of [`Item::to_persisted`]. Returns `None` if any
    /// id is unknown (item dropped from a pool that has since
    /// been pruned, or a corrupt row).
    pub fn from_persisted(
        base_id: &str,
        rarity_byte: u8,
        ilvl: u16,
        affixes: &[(String, f32)],
    ) -> Option<Self> {
        let base = super::items::BASE_ITEMS.iter().find(|b| b.id == base_id)?;
        let rarity = match rarity_byte {
            0 => Rarity::Common,
            1 => Rarity::Magic,
            2 => Rarity::Rare,
            3 => Rarity::Legendary,
            _ => return None,
        };
        let mut rolled = Vec::with_capacity(affixes.len());
        for (id, value) in affixes {
            let def = AFFIX_POOL.iter().find(|d| d.id == id.as_str())?;
            rolled.push(RolledAffix {
                def,
                value: *value,
            });
        }
        Some(Self {
            base,
            rarity,
            ilvl: ilvl as u32,
            affixes: rolled,
        })
    }
}
