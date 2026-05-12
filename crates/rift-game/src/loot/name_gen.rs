//! Procedural item names for non-unique drops (ITEMS.md Phase 7 §1).
//!
//! Pattern: `<Adjective?> <BaseName> <of-Suffix?>`.
//!
//! - **Adjective** is drawn from the item's highest-percentile
//!   *axis* affix — Element / Archetype / Attribute / Resonance.
//!   These are the "what flavour of damage is this item" lines,
//!   so they sit naturally at the front of the name.
//! - **Suffix** is drawn from the item's highest-percentile
//!   *bonus* affix — defensive / utility / ability-mod lines.
//!   These read better trailing "of ...".
//!
//! Pure presentation: the rolled item state is untouched, the
//! function is deterministic given the same `Item`, and the
//! affix → word lookup is a `match` (no per-frame allocations
//! beyond the final `String`).
//!
//! Legendaries with an authored `unique_id` are handled at the
//! [`crate::loot::Item::display_name`] site; legendaries without
//! one fall through to this module the same as any other rolled
//! item.

use crate::stats::Stat;

use super::affixes::{category, AffixCategory, AffixDef, AffixEffect};
use super::item::{Item, RolledAffix};
use super::roll::roll_percentile;

/// Build `"<Adjective?> <BaseName> <of-Suffix?>"`. When the item
/// has no affixes of the relevant category the corresponding
/// fragment is dropped.
pub fn procedural_name(item: &Item) -> String {
    let base = item.base.name;
    let adj = best_axis_affix(item).and_then(|a| adjective_for(a.def));
    let suffix = best_bonus_affix(item).and_then(|a| suffix_for(a.def));
    match (adj, suffix) {
        (Some(a), Some(s)) => format!("{a} {base} {s}"),
        (Some(a), None) => format!("{a} {base}"),
        (None, Some(s)) => format!("{base} {s}"),
        (None, None) => base.to_string(),
    }
}

/// Highest-roll axis affix (Element / Archetype / Attribute /
/// Resonance). Falls back to the first match when percentile
/// computation isn't applicable (e.g. ExtraProjectiles).
fn best_axis_affix(item: &Item) -> Option<&RolledAffix> {
    best_by_percentile(item, |c| {
        matches!(
            c,
            AffixCategory::Element | AffixCategory::Attribute | AffixCategory::Resonance
        )
    })
}

/// Highest-roll bonus affix.
fn best_bonus_affix(item: &Item) -> Option<&RolledAffix> {
    best_by_percentile(item, |c| matches!(c, AffixCategory::Bonus))
}

fn best_by_percentile(item: &Item, keep: impl Fn(AffixCategory) -> bool) -> Option<&RolledAffix> {
    item.affixes
        .iter()
        .filter(|a| keep(category(a.def)))
        .max_by(|x, y| {
            let px = roll_percentile(x.def, item.ilvl, x.value).unwrap_or(0.0);
            let py = roll_percentile(y.def, item.ilvl, y.value).unwrap_or(0.0);
            px.partial_cmp(&py).unwrap_or(std::cmp::Ordering::Equal)
        })
}

/// Adjective for an axis affix. Words lean into the rift /
/// void aesthetic — nothing too "high fantasy heroic". Returns
/// `None` for affix shapes that don't map to a fronting word
/// (defensive — every current axis affix has a mapping).
fn adjective_for(def: &AffixDef) -> Option<&'static str> {
    match def.effect {
        AffixEffect::Stat(Stat::PhysicalDamage) => Some("Forged"),
        AffixEffect::Stat(Stat::FireDamage) => Some("Molten"),
        AffixEffect::Stat(Stat::IceDamage) => Some("Frozen"),
        AffixEffect::Stat(Stat::LightningDamage) => Some("Charged"),
        AffixEffect::Stat(Stat::Strength) => Some("Titanbound"),
        AffixEffect::Stat(Stat::Agility) => Some("Veilrunner's"),
        AffixEffect::Stat(Stat::Intellect) => Some("Voidseer's"),
        _ => None,
    }
}

/// Suffix phrase for a bonus affix (read after "of "). Returns
/// `None` for effects without a sensible trailing word (e.g.
/// raw Transform — those only roll on Legendary uniques which
/// take their authored name anyway).
fn suffix_for(def: &AffixDef) -> Option<&'static str> {
    let s = match def.effect {
        AffixEffect::Stat(stat) => match stat {
            Stat::CritChance => "of Execution",
            Stat::CritDamage => "of Annihilation",
            Stat::AttackSpeed => "of the Riftwind",
            Stat::Health => "of Undeath",
            Stat::HealthRegen => "of the Ouroboros",
            Stat::Armor => "of the Unbroken",
            Stat::Evasion => "of Shadows",
            Stat::ElementalResist => "of the Veil",
            Stat::HealingReceived => "of Communion",
            Stat::MaxResource => "of the Maelstrom",
            Stat::CooldownReduction => "of the Hourglass",
            Stat::ResourceRegen => "of the Tide",
            Stat::MoveSpeed => "of the Wraith",
            Stat::Range => "of the Horizon",
            // Damage-axis stats never reach `suffix_for`
            // because they're classified as Element /
            // Archetype categories, not Bonus.
            _ => return None,
        },
        AffixEffect::AmplifyAbilityDamage(_) => "of Ruination",
        AffixEffect::ReduceAbilityCooldown(_) => "of Echoing Time",
        AffixEffect::ExtraProjectiles(_) => "of Prism",
        AffixEffect::Proc(_, _) => "of Resonance",
        // Transform / Trigger are Legendary-only patterns that
        // belong to uniques in practice; defensively, drop them
        // out of the suffix so they don't double-name an
        // already-Legendary item.
        AffixEffect::TransformAbility(_, _) => return None,
    };
    Some(s)
}
