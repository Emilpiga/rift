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
use crate::stats::StatBlock;

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

/// The `(min, max)` magnitude range an affix can roll at the given
/// item-level. Used both by [`Item::roll`] (indirectly via
/// [`roll_value`]) and tooltip rendering — the player sees what
/// part of the range a drop landed on.
pub fn roll_range(def: &AffixDef, ilvl: u32) -> (f32, f32) {
    let scale = ilvl.saturating_sub(1) as f32 * def.ilvl_scale;
    (def.roll.0 + scale, def.roll.1 + scale)
}

/// Where `value` lands in `(lo, hi)`, expressed as a 0..1 fraction.
/// Returns `None` when the range is degenerate (Transform / fixed
/// rolls — there's no scale to talk about).
pub fn roll_percentile(def: &AffixDef, ilvl: u32, value: f32) -> Option<f32> {
    let (lo, hi) = roll_range(def, ilvl);
    if (hi - lo).abs() < 1e-6 {
        None
    } else {
        Some(((value - lo) / (hi - lo)).clamp(0.0, 1.0))
    }
}

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
    ///
    /// `ilvl` is the item-level the affix was rolled at \u2014 needed
    /// to recover the per-ilvl roll range and append a roll-quality
    /// percentile (`[NN%]`) so the player can see how high in the
    /// range the drop landed.
    pub fn tooltip(&self, ilvl: u32) -> String {
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
        let line = if self.def.name_template.contains("{}") {
            self.def.name_template.replace("{}", &value_str)
        } else {
            self.def.name_template.to_string()
        };
        // Append a roll-quality percentile when the affix has a
        // non-degenerate range. ExtraProjectiles / Proc / Stat all
        // qualify; Transform doesn't.
        if let Some(p) = roll_percentile(self.def, ilvl, self.value) {
            format!("{}  [{:.0}%]", line, p * 100.0)
        } else {
            line
        }
    }
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
    /// [`ANCHORED_CHANCE`] inside [`Item::roll`]. Purely
    /// additive — no stat impact, just persistence.
    pub anchored: bool,
}

/// Per-roll chance that a Legendary drop is also Anchored.
/// 1 in 5 000 — the chase trait is meant to be a long-term
/// goal, not something every farming session produces.
pub const ANCHORED_CHANCE: f32 = 1.0 / 5_000.0;

impl Item {
    /// Roll a fresh drop of `base` at the given rarity / item-level.
    ///
    /// Two-phase pipeline:
    ///
    /// 1. **Signature injection** — every item gets the
    ///    per-`EquipSlot` signature (Vitality + slot-specific
    ///    guaranteed lines, see [`super::affixes::signature_for`]).
    ///    These are rolled deterministically; rarity doesn't gate
    ///    them. Ring / Amulet randomise *which* element / archetype
    ///    they roll while still always producing one signature
    ///    line of that family.
    ///
    /// 2. **Bonus rolls** — `rarity.affix_count_range()` extra
    ///    lines from the bonus pool (filtered by tags + ilvl +
    ///    rarity_min, signature ids excluded so we never
    ///    double-roll a guaranteed line). Legendary additionally
    ///    rolls one effect affix from the legendary pool
    ///    (Transform / Proc / ExtraProjectiles).
    ///
    /// The result: every item is readable top-down — Vitality,
    /// slot signature, then the bonus block — and a Legendary
    /// always carries one truly build-changing line on top.
    pub fn roll(
        base: &'static BaseItem,
        rarity: Rarity,
        ilvl: u32,
        rng: &mut LootRng,
    ) -> Self {
        let mut rolled: Vec<RolledAffix> = Vec::new();

        // ── Phase 1: signature injection ────────────────────────
        let sig_ids = super::affixes::signature_for(base.equip_slot, rng);
        for id in &sig_ids {
            if let Some(def) = super::affixes::lookup(id) {
                let value = roll_value(def, ilvl, rng);
                rolled.push(RolledAffix { def, value });
            }
        }

        // ── Phase 2: bonus rolls ────────────────────────────────
        let (lo, hi) = rarity.affix_count_range();
        let bonus_count = rng.range(lo, hi + 1) as usize;

        // Bonus pool: stat affixes + non-legendary effects (amp,
        // cdr) that pass the base's tag mask and aren't already
        // in the signature.
        let mut bonus_candidates: Vec<(&'static AffixDef, u32)> = AFFIX_POOL
            .iter()
            .filter(|a| {
                !sig_ids.contains(&a.id)
                    && !super::affixes::is_legendary_effect(&a.effect)
                    && (a.tags & base.allowed_tags) != 0
                    && a.min_ilvl <= ilvl
                    && rarity.at_least(a.rarity_min)
                    // Signature-only entries have weight 0 to keep
                    // them out of bonus rolling.
                    && a.weight > 0
            })
            .map(|a| {
                let favored = (a.tags & base.favored_tags) != 0;
                let weight = if favored { a.weight * 2 } else { a.weight };
                (a, weight)
            })
            .collect();

        for _ in 0..bonus_count {
            if bonus_candidates.is_empty() {
                break;
            }
            let weights: Vec<u32> = bonus_candidates.iter().map(|(_, w)| *w).collect();
            let Some(pick) = rng.weighted_index(&weights) else {
                break;
            };
            let def = bonus_candidates[pick].0;
            bonus_candidates.retain(|(c, _)| c.id != def.id);
            let value = roll_value(def, ilvl, rng);
            rolled.push(RolledAffix { def, value });
        }

        // ── Phase 3: legendary effect ───────────────────────────
        if rarity == Rarity::Legendary {
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
            // Drop already-rolled ids (paranoia — none should be in
            // bonus pool, but a future refactor might overlap).
            effect_candidates.retain(|(a, _)| !rolled.iter().any(|r| r.def.id == a.id));
            if !effect_candidates.is_empty() {
                let weights: Vec<u32> =
                    effect_candidates.iter().map(|(_, w)| *w).collect();
                if let Some(pick) = rng.weighted_index(&weights) {
                    let def = effect_candidates[pick].0;
                    let value = roll_value(def, ilvl, rng);
                    rolled.push(RolledAffix { def, value });
                }
            }
        }

        // ── Phase 4: anchored roll ──────────────────────────────
        // Legendary-only and *very* rare. Decided after the
        // affix block so the stat roll is independent — players
        // see the same legendary they would have rolled, just
        // occasionally tagged Anchored on top.
        let anchored = rarity == Rarity::Legendary && rng.next_f32() < ANCHORED_CHANCE;

        Self {
            base,
            rarity,
            ilvl,
            affixes: rolled,
            anchored,
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
        if self.anchored {
            format!("Anchored {} {}", self.rarity.name(), self.base.name)
        } else {
            format!("{} {}", self.rarity.name(), self.base.name)
        }
    }

    /// Multi-line tooltip ready for UI rendering.
    ///
    /// Structured top-down for readability:
    /// 1. Name (rarity-coloured by the renderer)
    /// 2. `Item Level N`
    /// 3. Implicits (base-item lines, e.g. "+24 Armor")
    /// 4. **Signature block** \u2014 the slot-defining lines, ordered
    ///    `[primary, Vitality, secondary?]` so the slot's headline
    ///    stat reads first and the eternal `+N Vitality` anchor
    ///    sits directly under it.
    /// 5. **Bonus block** \u2014 separator (`\u2500\u2500\u2500`) then any
    ///    rarity-rolled stat affixes.
    /// 6. Amplify / cooldown affixes.
    /// 7. Legendary effect (when present) \u2014 prefixed `\u2605 ` so the
    ///    UI / player can pick it out at a glance.
    /// 8. Synergy footer (when `loadout.is_some()`) \u2014 a one-line
    ///    `\u2192 Boosts <ability> (slot N)` for each slotted ability
    ///    this item benefits.
    pub fn tooltip(&self, loadout: Option<&crate::loadout::Loadout>) -> Vec<String> {
        let mut out = Vec::with_capacity(8 + self.affixes.len());
        out.push(self.display_name());
        out.push(format!("Item Level {}", self.ilvl));
        if self.anchored {
            // Tagged so the renderer can colour this line
            // distinctly from regular flavour text.
            out.push("⚓ Anchored — survives death".to_string());
        }

        // Implicits.
        if !self.base.implicit.is_empty() {
            out.push(String::new());
            for &(stat, value) in self.base.implicit {
                out.push(stat.format(value));
            }
        }

        // Partition stat affixes into [signatures | bonus]. The
        // first N entries are always signatures (see
        // `signature_count`); we render them with a deliberate
        // primary \u2192 Vitality \u2192 secondary order so every item's
        // headline stat is the topmost line of the stat block.
        let sig_n = super::affixes::signature_count(self.base.equip_slot)
            .min(self.affixes.len());
        let (raw_signatures, rest) = self.affixes.split_at(sig_n);
        // Defensive filter: skip any legendary effect that
        // somehow lands in the first N positions (older
        // persisted items + future hand-built test items can
        // both end up that way). The dedicated legendary block
        // at the bottom owns rendering for those — letting one
        // slip into the signature slice would print the same
        // line twice (white in signatures, gold in legendary).
        let signatures: Vec<&RolledAffix> = raw_signatures
            .iter()
            .filter(|a| !super::affixes::is_legendary_effect(&a.def.effect))
            .collect();

        // Find the Vitality entry inside the signature slice.
        // Other signatures keep their authored order; primary is
        // whatever comes first that *isn't* Vitality.
        let vitality_idx = signatures.iter().position(|a| {
            matches!(
                a.def.effect,
                AffixEffect::Stat(crate::stats::Stat::Vitality)
            )
        });

        // Render signature block in the requested order.
        if !signatures.is_empty() {
            out.push(String::new());
            // Primary: first non-Vitality signature.
            let mut primary_rendered = false;
            for (i, a) in signatures.iter().enumerate() {
                if Some(i) == vitality_idx {
                    continue;
                }
                out.push(a.tooltip(self.ilvl));
                primary_rendered = true;
                break;
            }
            // Vitality \u2014 always second when present, or first if
            // there's no other signature (shouldn't happen with
            // current slot mapping, but defensive).
            if let Some(vi) = vitality_idx {
                out.push(signatures[vi].tooltip(self.ilvl));
            }
            // Secondary (and any further) non-Vitality signatures.
            // Skip the first non-Vitality entry we already wrote
            // as `primary`.
            let mut skipped_primary = !primary_rendered;
            for (i, a) in signatures.iter().enumerate() {
                if Some(i) == vitality_idx {
                    continue;
                }
                if !skipped_primary {
                    skipped_primary = true;
                    continue;
                }
                out.push(a.tooltip(self.ilvl));
            }
        }

        // Bonus stat affixes (post-signature). Separated visually
        // from the signature block by a thin divider so the
        // player parses "guaranteed" vs "rolled" at a glance.
        let bonus_stats: Vec<&RolledAffix> = rest
            .iter()
            .filter(|a| matches!(a.def.effect, AffixEffect::Stat(_)))
            .collect();
        if !bonus_stats.is_empty() {
            out.push(String::new());
            out.push("───".to_string());
            for a in bonus_stats {
                out.push(a.tooltip(self.ilvl));
            }
        }

        // Non-Stat, non-legendary effect lines (Amplify / CDR).
        // These read as "amplifies my Frost Ray by +12%" \u2014 useful
        // signal so they live above the legendary effect.
        let amp_affixes: Vec<&RolledAffix> = self
            .affixes
            .iter()
            .filter(|a| {
                matches!(
                    a.def.effect,
                    AffixEffect::AmplifyAbilityDamage(_)
                        | AffixEffect::ReduceAbilityCooldown(_)
                )
            })
            .collect();
        if !amp_affixes.is_empty() {
            out.push(String::new());
            for a in amp_affixes {
                out.push(a.tooltip(self.ilvl));
            }
        }

        // Legendary effect \u2014 exactly one (or zero) per item.
        for a in &self.affixes {
            if super::affixes::is_legendary_effect(&a.def.effect) {
                out.push(String::new());
                out.push(format!("★ {}", a.tooltip(self.ilvl)));
            }
        }

        // Synergy footer: list slotted abilities this item helps.
        if let Some(lo) = loadout {
            let mut hits = self.synergy_against(lo);
            hits.sort();
            hits.dedup();
            if !hits.is_empty() {
                out.push(String::new());
                for line in hits {
                    out.push(line);
                }
            }
        }

        out
    }

    /// Build the synergy-footer lines for `loadout`. One line per
    /// slotted ability this item helps. Pure read-only — no
    /// allocation beyond the returned `Vec`.
    ///
    /// Match rules:
    /// - `Stat::WeaponDamage` / `SpellDamage` → matches abilities
    ///   whose `Scaling` equals it.
    /// - `Stat::PhysicalDamage` / `FireDamage` / `IceDamage` /
    ///   `LightningDamage` → matches abilities whose `Element`
    ///   equals it.
    /// - `Stat::ProjectileDamage` / `BeamDamage` / `AoeDamage` /
    ///   `MeleeDamage` → matches abilities whose `Archetype`
    ///   equals it.
    /// - `AmplifyAbilityDamage(id)` / `ReduceAbilityCooldown(id)` /
    ///   `ExtraProjectiles(id)` / `TransformAbility(id, _)` →
    ///   match if that exact ability is slotted.
    fn synergy_against(&self, loadout: &crate::loadout::Loadout) -> Vec<String> {
        use crate::abilities::{Archetype, Element, Scaling};
        use crate::stats::Stat;
        let mut out: Vec<String> = Vec::new();
        // Gather slotted abilities once so each affix scan is O(6).
        let slotted: Vec<(usize, &'static crate::abilities::Ability)> = loadout
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, &id)| crate::abilities::lookup(id).map(|a| (i, a)))
            .collect();

        // Helper closure-free predicate-driven match. We can't use
        // a `&mut Vec` capturing closure here because each affix
        // arm needs its own predicate, and chaining mutable
        // captures runs into borrow-checker overlap.
        let push_match = |out: &mut Vec<String>,
                          label: &str,
                          pred: fn(&crate::abilities::Ability) -> bool| {
            for (i, ab) in &slotted {
                if pred(ab) {
                    out.push(format!(
                        "→ Boosts {} (slot {}) [{}]",
                        ab.name,
                        i + 1,
                        label
                    ));
                }
            }
        };

        for a in &self.affixes {
            match a.def.effect {
                AffixEffect::Stat(Stat::WeaponDamage) => push_match(
                    &mut out,
                    "Weapon",
                    |x| matches!(x.scaling, Scaling::Weapon),
                ),
                AffixEffect::Stat(Stat::SpellDamage) => push_match(
                    &mut out,
                    "Spell",
                    |x| matches!(x.scaling, Scaling::Spell),
                ),
                AffixEffect::Stat(Stat::PhysicalDamage) => push_match(
                    &mut out,
                    "Physical",
                    |x| matches!(x.element, Element::Physical),
                ),
                AffixEffect::Stat(Stat::FireDamage) => push_match(
                    &mut out,
                    "Fire",
                    |x| matches!(x.element, Element::Fire),
                ),
                AffixEffect::Stat(Stat::IceDamage) => push_match(
                    &mut out,
                    "Ice",
                    |x| matches!(x.element, Element::Ice),
                ),
                AffixEffect::Stat(Stat::LightningDamage) => push_match(
                    &mut out,
                    "Lightning",
                    |x| matches!(x.element, Element::Lightning),
                ),
                AffixEffect::Stat(Stat::ProjectileDamage) => push_match(
                    &mut out,
                    "Projectile",
                    |x| matches!(x.archetype, Archetype::Projectile),
                ),
                AffixEffect::Stat(Stat::BeamDamage) => push_match(
                    &mut out,
                    "Beam",
                    |x| matches!(x.archetype, Archetype::Beam),
                ),
                AffixEffect::Stat(Stat::AoeDamage) => push_match(
                    &mut out,
                    "AoE",
                    |x| matches!(x.archetype, Archetype::Aoe),
                ),
                AffixEffect::Stat(Stat::MeleeDamage) => push_match(
                    &mut out,
                    "Melee",
                    |x| matches!(x.archetype, Archetype::Melee),
                ),
                AffixEffect::AmplifyAbilityDamage(id)
                | AffixEffect::ReduceAbilityCooldown(id)
                | AffixEffect::ExtraProjectiles(id)
                | AffixEffect::TransformAbility(id, _) => {
                    for (i, ab) in &slotted {
                        if ab.id == id {
                            out.push(format!(
                                "→ Affects {} (slot {})",
                                ab.name,
                                i + 1
                            ));
                        }
                    }
                }
                _ => {}
            }
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
    pub fn to_wire(&self) -> (u16, u8, u16, Vec<(u16, f32)>, bool) {
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
        (base_id, self.rarity as u8, self.ilvl as u16, affixes, self.anchored)
    }

    /// Inverse of [`Item::to_wire`]. Returns `None` if any index is
    /// out of bounds (mismatched build / corrupted save).
    pub fn from_wire(
        base_id: u16,
        rarity_byte: u8,
        ilvl: u16,
        affixes: &[(u16, f32)],
        anchored: bool,
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
            anchored,
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
    pub fn to_persisted(&self) -> (String, u8, u16, Vec<(String, f32)>, bool) {
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
            self.anchored,
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
        anchored: bool,
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
            anchored,
        })
    }
}
