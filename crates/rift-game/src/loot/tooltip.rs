//! Typed tooltip rendering for [`super::item::Item`] and
//! [`super::item::RolledAffix`].
//!
//! [`Item::tooltip`] returns a vector of typed [`TooltipLine`]s.
//! Each line carries:
//!
//! - its rendered `text` (with any prefix glyph / band suffix
//!   already composed in),
//! - a semantic [`TooltipKind`] so the renderer doesn't have to
//!   sniff prefixes,
//! - an optional `percentile` (0..1) for affix rows so the
//!   renderer can derive a roll-quality band without parsing
//!   the text suffix.
//!
//! Host crates (rift-client, rift-ui) map [`TooltipKind`] to
//! their own `TooltipLineKind` / `RollBand` palettes at view
//! build time. Keeping the typed primitives here means the
//! kind / percentile values are decided at the producer site
//! (where context is unambiguous) and the consumer side
//! becomes a trivial enum adapter — no string sniffing, no
//! prefix lookup tables.

use super::affixes::AffixEffect;
use super::item::{Item, RolledAffix};
use super::roll::roll_percentile;

/// Semantic role of a single tooltip line.
///
/// Mirrors `rift_ui_types::inventory::TooltipLineKind` but lives
/// in rift-game so the producer can stamp the kind directly
/// instead of forcing the host to recover it from prefix bytes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TooltipKind {
    /// Item name (first line). Coloured by rarity downstream.
    Name,
    /// Plain stat / affix / implicit row.
    Stat,
    /// Blank spacer line.
    Blank,
    /// Thin `───` divider between blocks.
    Divider,
    /// `Item Level N` header row.
    ItemLevel,
    /// `Requires Level N` header row. `required` is the
    /// numeric level the equipper must meet; the host
    /// renderer decides red-vs-default by comparing against
    /// the viewing character's level.
    RequiresLevel { required: u32 },
    /// `★ …` legendary effect row inside the gold-edge banner.
    Legendary,
    /// `╔` / `╚` sentinel row marking the top/bottom edge of
    /// the legendary banner.
    LegendaryBannerEdge,
    /// `"…"` flavour string inside the legendary banner.
    LegendaryFlavor,
    /// `◆ …` resonance affix line (cross-family axis).
    Resonance,
    /// `✦ …` rift-touched memento line.
    RiftTouched,
    /// `⚓ …` anchored trait line.
    Anchored,
    /// `⚠ …` warning line (unstable rift loot, …).
    Warning,
    /// `→ Boosts …` synergy footer.
    Synergy,
}

/// One pre-classified tooltip line produced by
/// [`Item::tooltip`]. Owns its `text` so the host can pass it
/// straight into UI views without an intermediate classify pass.
#[derive(Clone, Debug)]
pub struct TooltipLine {
    pub text: String,
    pub kind: TooltipKind,
    /// `Some(p)` for affix rows whose roll has a non-degenerate
    /// range; `p` is the 0..1 percentile of the roll inside that
    /// range. `None` for header rows (Name, ItemLevel,
    /// RequiresLevel, Divider, Blank, banner edges) and for
    /// affix effects with degenerate ranges (Transform) where
    /// percentile is meaningless.
    pub percentile: Option<f32>,
}

impl TooltipLine {
    fn new(text: impl Into<String>, kind: TooltipKind) -> Self {
        Self {
            text: text.into(),
            kind,
            percentile: None,
        }
    }

    fn with_percentile(mut self, p: Option<f32>) -> Self {
        self.percentile = p;
        self
    }
}

impl RolledAffix {
    /// Render the line for tooltips. Effects with no numeric value
    /// (Transform) ignore the template's `{}` placeholder.
    ///
    /// `ilvl` is the item-level the affix was rolled at — needed
    /// to recover the per-ilvl roll range and append a roll-quality
    /// band suffix so the player can see how high in the range
    /// the drop landed.
    pub fn tooltip(&self, ilvl: u32) -> String {
        let (text, pct) = self.tooltip_parts(ilvl);
        if let Some(p) = pct {
            let (glyph, name) = band_label(p);
            format!("{}  {} {}", text, glyph, name)
        } else {
            text
        }
    }

    /// Inner helper used by both [`Self::tooltip`] (string form)
    /// and the typed builder. Returns the un-suffixed line plus
    /// the roll percentile, so the typed contract can carry the
    /// percentile alongside without round-tripping through a
    /// parsed band-name suffix.
    pub fn tooltip_parts(&self, ilvl: u32) -> (String, Option<f32>) {
        let value_str = match self.def.effect {
            AffixEffect::Stat(stat) => {
                if stat.is_percent() {
                    format!("{:+.1}%", self.value * 100.0)
                } else {
                    format!("{:+.0}", self.value)
                }
            }
            AffixEffect::AmplifyAbilityDamage(_) | AffixEffect::ReduceAbilityCooldown(_) => {
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
        (line, roll_percentile(self.def, ilvl, self.value))
    }
}

/// Map a 0..1 roll percentile to the `(glyph, name)` pair used
/// by [`RolledAffix::tooltip`]. Thresholds and literals mirror
/// `rift_ui_types::inventory::RollBand` — the UI crate consumes
/// the name as the wire contract back into the typed enum.
fn band_label(p: f32) -> (&'static str, &'static str) {
    if p < 0.20 {
        ("▾", "Crude")
    } else if p < 0.50 {
        ("▸", "Fair")
    } else if p < 0.80 {
        ("▴", "Fine")
    } else if p < 0.95 {
        ("▴▴", "Pristine")
    } else {
        ("▴▴▴", "Perfect")
    }
}

impl Item {
    pub fn display_name(&self) -> String {
        // Authored uniques own their headline — no rarity prefix,
        // no base name. The legendary tier is implied by the name
        // being authored at all. Falls back to the procedural
        // rarity-prefix path when the unique id doesn't resolve
        // (catalogue pruned, future build).
        if let Some(def) = self.unique_id.and_then(super::uniques::find) {
            let prefix = match (self.unstable, self.anchored) {
                (true, _) => "Unstable ",
                (false, true) => "Anchored ",
                (false, false) => "",
            };
            return format!("{}{}", prefix, def.name);
        }
        // Prefix order: Unstable > Anchored > plain. Unstable
        // is the most action-relevant tag ("will shatter on
        // death") so it leads.
        let prefix = match (self.unstable, self.anchored) {
            (true, _) => "Unstable ",
            (false, true) => "Anchored ",
            (false, false) => "",
        };
        format!("{}{} {}", prefix, self.rarity.name(), self.base.name)
    }

    /// Multi-line tooltip ready for UI rendering, with each line
    /// carrying a typed [`TooltipKind`] so the host doesn't have
    /// to sniff prefix glyphs.
    ///
    /// Structured top-down for readability:
    /// 1. Name (rarity-coloured by the renderer)
    /// 2. `Item Level N`
    /// 3. `Requires Level N` — the minimum character level to equip
    ///    this item (see [`Item::required_level`]). The renderer
    ///    can colour this red when the viewing player can't meet it.
    /// 4. Implicits (base-item lines, e.g. "+24 Armor")
    /// 5. **Signature block** — the slot-defining lines (helm
    ///    CDR, boots move speed, gloves crit pair, etc.). Rendered
    ///    in authored order.
    /// 6. **Bonus block** — separator (`───`) then any
    ///    rarity-rolled stat affixes.
    /// 7. Amplify / cooldown affixes.
    /// 8. Legendary effect (when present) — prefixed `★ ` so the
    ///    UI / player can pick it out at a glance.
    /// 9. Synergy footer (when `loadout.is_some()`) — a one-line
    ///    `→ Boosts <ability> (slot N)` for each slotted ability
    ///    this item benefits.
    pub fn tooltip(&self, loadout: Option<&crate::loadout::Loadout>) -> Vec<TooltipLine> {
        let mut out: Vec<TooltipLine> = Vec::with_capacity(8 + self.affixes.len());
        out.push(TooltipLine::new(self.display_name(), TooltipKind::Name));
        // Unique subtitle: the base name as an italic-ish second
        // line (the renderer can style it). Lets the player see
        // *what kind of thing* the unique actually is when its
        // authored name doesn't telegraph it — "Embercrown" alone
        // doesn't say "helm".
        if self.unique_id.is_some() {
            out.push(TooltipLine::new(
                format!("({})", self.base.name),
                TooltipKind::Stat,
            ));
        }
        out.push(TooltipLine::new(
            format!("Item Level {}", self.ilvl),
            TooltipKind::ItemLevel,
        ));
        let req = self.required_level();
        out.push(TooltipLine::new(
            format!("Requires Level {}", req),
            TooltipKind::RequiresLevel { required: req },
        ));
        if self.unstable {
            out.push(TooltipLine::new(
                "⚠ Unstable — extract to stabilise",
                TooltipKind::Warning,
            ));
        }
        if self.anchored {
            out.push(TooltipLine::new(
                "⚓ Anchored — survives death",
                TooltipKind::Anchored,
            ));
        }

        // Legendary banner — promoted to the top of the body so
        // the most build-defining line of the item lands where
        // the player's eye is already focused. `╔` / `╚` are
        // sentinels the renderer detects to paint the gradient-
        // gold edge + dark inset backdrop. Uniques own the effect;
        // the procedural fallback shares the same banner so the
        // visual contract is consistent regardless of which path
        // produced the line.
        let mut legendary_lines: Vec<TooltipLine> = Vec::new();
        if let Some(def) = self.unique_id.and_then(super::uniques::find) {
            if let Some(eff) = def.build(self.unique_pick) {
                legendary_lines.push(TooltipLine::new(
                    format!("\u{2605} {}", super::uniques::tooltip_line(&eff)),
                    TooltipKind::Legendary,
                ));
                if !def.flavor.is_empty() {
                    legendary_lines.push(TooltipLine::new(
                        format!("\u{201c}{}\u{201d}", def.flavor),
                        TooltipKind::LegendaryFlavor,
                    ));
                }
            }
        } else {
            for a in &self.affixes {
                if super::affixes::is_legendary_effect(&a.def.effect) {
                    let (text, _pct) = a.tooltip_parts(self.ilvl);
                    legendary_lines.push(TooltipLine::new(
                        format!("\u{2605} {}", text),
                        TooltipKind::Legendary,
                    ));
                }
            }
        }
        if !legendary_lines.is_empty() {
            out.push(TooltipLine::new(
                "\u{2554}\u{2554}\u{2554}",
                TooltipKind::LegendaryBannerEdge,
            ));
            for line in legendary_lines {
                out.push(line);
            }
            out.push(TooltipLine::new(
                "\u{255A}\u{255A}\u{255A}",
                TooltipKind::LegendaryBannerEdge,
            ));
        }

        // Implicits.
        if !self.base.implicit.is_empty() {
            out.push(TooltipLine::new("", TooltipKind::Blank));
            for &(stat, value) in self.base.implicit {
                out.push(TooltipLine::new(stat.format(value), TooltipKind::Stat));
            }
        }

        // Partition stat affixes into [signatures | bonus]. The
        // first N entries are always signatures (see
        // `signature_count`); we render them with a deliberate
        // primary → Vitality → secondary order so every item's
        // headline stat is the topmost line of the stat block.
        let sig_n = super::affixes::signature_count(self.base.equip_slot).min(self.affixes.len());
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

        // Trio block (Attribute → Element → Archetype). These
        // are the item's damage / identity axis lines and lead
        // the stat list. Rendered as a single contiguous group
        // — no inner dividers — so the trio reads as one block.
        use super::affixes::{category, AffixCategory};
        let by_cat = |cat: AffixCategory| -> Option<&RolledAffix> {
            rest.iter().find(|a| category(a.def) == cat)
        };
        let trio: Vec<&RolledAffix> = [
            AffixCategory::Attribute,
            AffixCategory::Element,
            AffixCategory::Archetype,
        ]
        .into_iter()
        .filter_map(by_cat)
        .collect();
        if !trio.is_empty() {
            out.push(TooltipLine::new("", TooltipKind::Blank));
            for a in &trio {
                let (_, pct) = a.tooltip_parts(self.ilvl);
                out.push(
                    TooltipLine::new(a.tooltip(self.ilvl), TooltipKind::Stat).with_percentile(pct),
                );
            }
        }

        // Resonance line — a cross-family damage axis that
        // intentionally breaks the trio's family lock. Prefixed
        // with `◆ ` so the renderer can tint it with a distinct
        // colour; shares the damage-axes block above so no
        // divider is inserted.
        let resonance_lines: Vec<&RolledAffix> = rest
            .iter()
            .filter(|a| category(a.def) == AffixCategory::Resonance)
            .collect();
        if !resonance_lines.is_empty() {
            if trio.is_empty() {
                out.push(TooltipLine::new("", TooltipKind::Blank));
            }
            for a in &resonance_lines {
                let (_, pct) = a.tooltip_parts(self.ilvl);
                out.push(
                    TooltipLine::new(
                        format!("◆ {}", a.tooltip(self.ilvl)),
                        TooltipKind::Resonance,
                    )
                    .with_percentile(pct),
                );
            }
        }

        // Defensives / utility block — signature followed by
        // bonus stat rolls partitioned into offensive (Crit /
        // Crit Damage / Attack Speed) and defensive / utility
        // (everything else). Single divider separates the
        // damage-axes block above; no inner dividers within.
        let is_offensive_bonus = |a: &&RolledAffix| {
            matches!(
                a.def.effect,
                AffixEffect::Stat(s) if s.is_offensive_bonus()
            )
        };
        let bonus_stats_all: Vec<&RolledAffix> = rest
            .iter()
            .filter(|a| {
                matches!(a.def.effect, AffixEffect::Stat(_))
                    && !matches!(
                        category(a.def),
                        AffixCategory::Attribute
                            | AffixCategory::Element
                            | AffixCategory::Archetype
                            | AffixCategory::Resonance
                    )
            })
            .collect();
        let offensive_bonus: Vec<&RolledAffix> = bonus_stats_all
            .iter()
            .copied()
            .filter(is_offensive_bonus)
            .collect();
        let defensive_bonus: Vec<&RolledAffix> = bonus_stats_all
            .iter()
            .copied()
            .filter(|a| !is_offensive_bonus(a))
            .collect();
        let has_defutil =
            !signatures.is_empty() || !offensive_bonus.is_empty() || !defensive_bonus.is_empty();
        if has_defutil {
            if !trio.is_empty() || !resonance_lines.is_empty() {
                out.push(TooltipLine::new("───", TooltipKind::Divider));
            } else {
                out.push(TooltipLine::new("", TooltipKind::Blank));
            }
            for a in &signatures {
                let (_, pct) = a.tooltip_parts(self.ilvl);
                out.push(
                    TooltipLine::new(a.tooltip(self.ilvl), TooltipKind::Stat).with_percentile(pct),
                );
            }
            if !signatures.is_empty()
                && (!offensive_bonus.is_empty() || !defensive_bonus.is_empty())
            {
                out.push(TooltipLine::new("───", TooltipKind::Divider));
            }
            for a in &offensive_bonus {
                let (_, pct) = a.tooltip_parts(self.ilvl);
                out.push(
                    TooltipLine::new(a.tooltip(self.ilvl), TooltipKind::Stat).with_percentile(pct),
                );
            }
            if !offensive_bonus.is_empty() && !defensive_bonus.is_empty() {
                out.push(TooltipLine::new("───", TooltipKind::Divider));
            }
            for a in &defensive_bonus {
                let (_, pct) = a.tooltip_parts(self.ilvl);
                out.push(
                    TooltipLine::new(a.tooltip(self.ilvl), TooltipKind::Stat).with_percentile(pct),
                );
            }
        }

        // Non-Stat, non-legendary effect lines (Amplify / CDR).
        let amp_affixes: Vec<&RolledAffix> = self
            .affixes
            .iter()
            .filter(|a| {
                matches!(
                    a.def.effect,
                    AffixEffect::AmplifyAbilityDamage(_) | AffixEffect::ReduceAbilityCooldown(_)
                )
            })
            .collect();
        if !amp_affixes.is_empty() {
            out.push(TooltipLine::new("", TooltipKind::Blank));
            for a in amp_affixes {
                let (_, pct) = a.tooltip_parts(self.ilvl);
                out.push(
                    TooltipLine::new(a.tooltip(self.ilvl), TooltipKind::Stat).with_percentile(pct),
                );
            }
        }

        // Rift-touched memento. One dedicated bonus line that
        // survives extraction and carries the floor depth it
        // was earned at. Always sits **below** the legendary
        // effect / unique flavour and **above** the synergy
        // footer.
        if let Some(rt) = &self.rift_touched {
            out.push(TooltipLine::new("", TooltipKind::Blank));
            let value_str = match rt.def.effect {
                AffixEffect::Stat(stat) => {
                    if stat.is_percent() {
                        format!("{:+.1}%", rt.value * 100.0)
                    } else {
                        format!("{:+.0}", rt.value)
                    }
                }
                _ => format!("{:+.2}", rt.value),
            };
            let line = if rt.def.name_template.contains("{}") {
                rt.def.name_template.replace("{}", &value_str)
            } else {
                rt.def.name_template.to_string()
            };
            out.push(TooltipLine::new(
                format!("\u{2726} {}  (Floor {})", line, rt.depth),
                TooltipKind::RiftTouched,
            ));
        }

        // Synergy footer: list slotted abilities this item helps.
        if let Some(lo) = loadout {
            let mut hits = self.synergy_against(lo);
            hits.sort();
            hits.dedup();
            if !hits.is_empty() {
                out.push(TooltipLine::new("", TooltipKind::Blank));
                for line in hits {
                    out.push(TooltipLine::new(line, TooltipKind::Synergy));
                }
            }
        }

        out
    }

    /// Build the synergy-footer lines for `loadout`. One line per
    /// slotted ability this item helps. Pure read-only — no
    /// allocation beyond the returned `Vec`.
    fn synergy_against(&self, loadout: &crate::loadout::Loadout) -> Vec<String> {
        use crate::abilities::{Archetype, Element};
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
        let push_match =
            |out: &mut Vec<String>, label: &str, pred: fn(&crate::abilities::Ability) -> bool| {
                for (i, ab) in &slotted {
                    if pred(ab) {
                        out.push(format!("→ Boosts {} (slot {}) [{}]", ab.name, i + 1, label));
                    }
                }
            };

        for a in &self.affixes {
            match a.def.effect {
                AffixEffect::Stat(Stat::PhysicalDamage) => push_match(&mut out, "Physical", |x| {
                    matches!(x.element, Element::Physical)
                }),
                AffixEffect::Stat(Stat::FireDamage) => {
                    push_match(&mut out, "Fire", |x| matches!(x.element, Element::Fire))
                }
                AffixEffect::Stat(Stat::IceDamage) => {
                    push_match(&mut out, "Ice", |x| matches!(x.element, Element::Ice))
                }
                AffixEffect::Stat(Stat::LightningDamage) => {
                    push_match(&mut out, "Lightning", |x| {
                        matches!(x.element, Element::Lightning)
                    })
                }
                AffixEffect::Stat(Stat::ProjectileDamage) => {
                    push_match(&mut out, "Projectile", |x| {
                        matches!(x.archetype, Archetype::Projectile)
                    })
                }
                AffixEffect::Stat(Stat::BeamDamage) => {
                    push_match(&mut out, "Beam", |x| matches!(x.archetype, Archetype::Beam))
                }
                AffixEffect::Stat(Stat::AoeDamage) => {
                    push_match(&mut out, "AoE", |x| matches!(x.archetype, Archetype::Aoe))
                }
                AffixEffect::Stat(Stat::MeleeDamage) => push_match(&mut out, "Melee", |x| {
                    matches!(x.archetype, Archetype::Melee)
                }),
                AffixEffect::AmplifyAbilityDamage(id)
                | AffixEffect::ReduceAbilityCooldown(id)
                | AffixEffect::ExtraProjectiles(id)
                | AffixEffect::TransformAbility(id, _) => {
                    for (i, ab) in &slotted {
                        if ab.id == id {
                            out.push(format!("→ Affects {} (slot {})", ab.name, i + 1));
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }
}
