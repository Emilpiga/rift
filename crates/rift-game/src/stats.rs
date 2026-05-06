//! Unified stat vocabulary, sparse storage, and resolved character
//! sheet.
//!
//! This module is the single source of truth for what a "stat" is
//! across both items and characters. Three layers, all in one
//! file because keeping them apart caused vocabulary drift
//! (`loot::stats::Health` vs `CharacterStats::max_hp` etc.):
//!
//! 1. [`Stat`] — the fixed enum every affix, tooltip, and combat
//!    formula keys off of. Keep it small; new variants need a
//!    gameplay reason.
//! 2. [`StatBlock`] — sparse `Vec<(Stat, f32)>` container used for
//!    rolled affix stacks on items and aggregate equipment sums.
//! 3. [`CharacterStats`] — the resolved sheet (class base ×
//!    attributes ⊕ equipment ⊕ buffs). Derived; recompute via
//!    [`CharacterStats::compute`] whenever an input changes.
//!
//! Stats fall into four semantic groups (informational only — the
//! enum stays flat to keep matching cheap):
//!
//! - **Offensive** — `Power`, `CritChance`, `CritDamage`, `AttackSpeed`.
//! - **Defensive** — `Health`, `Armor`, `Evasion`.
//! - **Utility** — `CooldownReduction`, `ResourceRegen`, `MoveSpeed`.
//! - **Elemental** — `FireDamage`, `IceDamage`, `LightningDamage`.
//!
//! Some stats are **flat** (`Health: +120`) and some are **percent**
//! (`CritChance: +0.05` = +5 %). Use [`Stat::is_percent`] to decide
//! how to display / multiply downstream.

use crate::attributes::Attributes;
use crate::classes::ClassConfig;

// ---------------------------------------------------------------------------
// Stat enum
// ---------------------------------------------------------------------------

/// Every stat that can appear on an item or character sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stat {
    // Offensive
    Power,
    CritChance,
    CritDamage,
    AttackSpeed,
    // Defensive
    Health,
    Armor,
    Evasion,
    // Utility
    CooldownReduction,
    ResourceRegen,
    MoveSpeed,
    // Elemental scaling
    FireDamage,
    IceDamage,
    LightningDamage,
}

impl Stat {
    /// Display name (singular, capitalised).
    pub fn name(self) -> &'static str {
        match self {
            Stat::Power => "Power",
            Stat::CritChance => "Crit Chance",
            Stat::CritDamage => "Crit Damage",
            Stat::AttackSpeed => "Attack Speed",
            Stat::Health => "Health",
            Stat::Armor => "Armor",
            Stat::Evasion => "Evasion",
            Stat::CooldownReduction => "Cooldown Reduction",
            Stat::ResourceRegen => "Resource Regen",
            Stat::MoveSpeed => "Move Speed",
            Stat::FireDamage => "Fire Damage",
            Stat::IceDamage => "Ice Damage",
            Stat::LightningDamage => "Lightning Damage",
        }
    }

    /// `true` if this stat is naturally expressed as a percentage
    /// (rolls in 0..1 space, displayed as `+12 %`). `false` for flat
    /// scalars displayed bare (`+120 Health`).
    pub fn is_percent(self) -> bool {
        matches!(
            self,
            Stat::CritChance
                | Stat::CritDamage
                | Stat::AttackSpeed
                | Stat::CooldownReduction
                | Stat::ResourceRegen
                | Stat::MoveSpeed
                | Stat::FireDamage
                | Stat::IceDamage
                | Stat::LightningDamage
        )
    }

    /// Format `value` for tooltip display (with sign prefix and unit).
    pub fn format(self, value: f32) -> String {
        if self.is_percent() {
            format!("{:+.1}% {}", value * 100.0, self.name())
        } else {
            format!("{:+.0} {}", value, self.name())
        }
    }
}

// ---------------------------------------------------------------------------
// StatBlock
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// StatModifiers
// ---------------------------------------------------------------------------

/// Aggregate of buff / debuff / talent contributions on top of
/// equipment. Two channels:
///
/// - `flat`: added before percent multipliers (treated like
///   another equipment-affix sum).
/// - `percent`: applied at the end as `value *= 1 + sum`. Multiple
///   sources (talents + buffs) on the same stat add together
///   before the multiply.
///
/// Stats that are themselves chances or multipliers
/// (`CritChance`, `CritDamage`, `CooldownReduction`,
/// `MoveSpeed`, `AttackSpeed`, elemental dmg) only honour the
/// `flat` channel — adding +5 % crit-chance is the same as adding
/// +0.05 in raw units, so we don't double-multiply.
#[derive(Clone, Debug, Default)]
pub struct StatModifiers {
    pub flat: StatBlock,
    pub percent: StatBlock,
}

impl StatModifiers {
    pub fn new() -> Self {
        Self::default()
    }

    /// Merge another modifier set into this one. Both channels
    /// concatenate — `StatBlock::get` sums on read.
    pub fn extend(&mut self, other: &StatModifiers) {
        self.flat.extend(&other.flat);
        self.percent.extend(&other.percent);
    }
}

/// Sparse stat container — sums duplicates on read.
///
/// Used both as a per-item rolled-stat block and as the aggregated
/// equipment total. Cheap to clone; never indexed by hashing because
/// the cardinality is tiny (≤ 13 entries).
#[derive(Clone, Debug, Default)]
pub struct StatBlock {
    entries: Vec<(Stat, f32)>,
}

impl StatBlock {
    pub fn new() -> Self {
        Self { entries: Vec::new() }
    }

    /// Append `(stat, value)`. Duplicates are kept; [`Self::get`]
    /// sums them on read.
    pub fn add(&mut self, stat: Stat, value: f32) {
        self.entries.push((stat, value));
    }

    /// Sum of every entry matching `stat`. Returns `0.0` if none.
    pub fn get(&self, stat: Stat) -> f32 {
        self.entries
            .iter()
            .filter(|(s, _)| *s == stat)
            .map(|(_, v)| *v)
            .sum()
    }

    /// Iterate the raw entries (each affix typically pushes one).
    pub fn iter(&self) -> impl Iterator<Item = (Stat, f32)> + '_ {
        self.entries.iter().copied()
    }

    /// Merge another block in (sums by appending).
    pub fn extend(&mut self, other: &StatBlock) {
        self.entries.extend(other.entries.iter().copied());
    }

    /// Coalesce duplicate stats into one entry each (for clean
    /// character-sheet display).
    pub fn collapsed(&self) -> Vec<(Stat, f32)> {
        let mut out: Vec<(Stat, f32)> = Vec::new();
        for &(s, v) in &self.entries {
            if let Some(slot) = out.iter_mut().find(|(s2, _)| *s2 == s) {
                slot.1 += v;
            } else {
                out.push((s, v));
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// CharacterStats
// ---------------------------------------------------------------------------

/// Final, post-aggregation character sheet. Each field corresponds
/// to one [`Stat`] variant (plus class-only values like `range`).
///
/// All fields are flat scalars in their native units (HP in raw
/// points, percentages as 0..1 multipliers, durations in seconds).
///
/// Derived state — never mutate a field in place. Recompute via
/// [`CharacterStats::compute`] whenever an input changes.
#[derive(Clone, Debug, Default)]
pub struct CharacterStats {
    // --- Defensive --------------------------------------------------
    /// Maximum hit points. Built from class HP + vitality + flat
    /// `Stat::Health` from gear.
    pub max_hp: f32,
    /// Flat armor — mirrors `Stat::Armor`.
    pub armor: f32,
    /// Evasion chance, 0..1 — mirrors `Stat::Evasion`.
    pub evasion: f32,
    /// Total defense rating (class base × primary-attr scaling).
    /// Kept separate from `armor` because classes still talk about
    /// "defense" as a single number; once the combat layer is
    /// unified we can collapse the two.
    pub defense: f32,

    // --- Offensive --------------------------------------------------
    /// Base outgoing damage (class base + flat `Stat::Power`)
    /// scaled by primary attribute. Ability `damage_mult`
    /// multiplies on top at the cast site.
    pub damage: f32,
    /// Crit roll, 0..1 — class base + agility + `Stat::CritChance`.
    pub crit_chance: f32,
    /// Crit damage multiplier (e.g. `0.5` = +50 %). Built from a
    /// 0.5 baseline + `Stat::CritDamage`.
    pub crit_damage: f32,
    /// Attacks-per-second multiplier — class base × (1 + agility +
    /// `Stat::AttackSpeed`).
    pub attack_speed: f32,

    // --- Utility ----------------------------------------------------
    /// Movement speed in m/s — class base × (1 + `Stat::MoveSpeed`).
    pub move_speed: f32,
    /// Cooldown reduction, 0..1 (capped at 0.75). Mirrors
    /// `Stat::CooldownReduction`.
    pub cooldown_reduction: f32,
    /// Resource (mana / focus) regen multiplier — 1 +
    /// `Stat::ResourceRegen`.
    pub resource_regen: f32,
    /// Effective ability range in metres (class base, no affix
    /// source yet).
    pub range: f32,

    // --- Elemental -------------------------------------------------
    /// Per-element damage bonus, 0..1.
    pub fire_damage: f32,
    pub ice_damage: f32,
    pub lightning_damage: f32,
}

impl CharacterStats {
    /// Compute a fresh snapshot from the inputs.
    ///
    /// `equipment` is the summed [`StatBlock`] from every equipped
    /// item (the planned `Equipment::active_affix_sum`). Pass
    /// [`StatBlock::new`]\(\) for an unarmoured character.
    ///
    /// Pure — no `&mut self`, no global state — so it's trivially
    /// testable and safe to call from either the client (HUD) or
    /// the server (authoritative combat).
    pub fn compute(
        class: &ClassConfig,
        attrs: &Attributes,
        level: u32,
        equipment: &StatBlock,
        modifiers: &StatModifiers,
    ) -> Self {
        let primary = class.primary_attribute;
        let primary_value = attrs.get(primary) as f32;

        // Per-attribute scaling. Each point of primary -> +1 % damage.
        let primary_dmg_mult = 1.0 + primary_value * 0.01;
        // Primary +0.5 %, Strength +0.8 % defense per point.
        let attr_defense_mult =
            1.0 + primary_value * 0.005 + attrs.strength as f32 * 0.008;
        // Vitality -> +3 flat HP per point.
        let attr_hp_bonus = attrs.vitality as f32 * 3.0;
        // Agility -> +0.1 % crit, +0.5 % attack speed per point.
        let attr_crit_bonus = attrs.agility as f32 * 0.001;
        let attr_aspd_bonus = attrs.agility as f32 * 0.005;

        // Class-level scaling.
        let class_hp = class.base_hp + class.hp_per_level * level as f32;

        // Helper: equipment + modifiers.flat sum for a stat.
        let flat = |s: Stat| equipment.get(s) + modifiers.flat.get(s);
        // Helper: percent multiplier (1 + sum of percent
        // contributions). Talents / buffs only.
        let pct = |s: Stat| 1.0 + modifiers.percent.get(s);

        Self {
            max_hp: (class_hp + attr_hp_bonus + flat(Stat::Health))
                * pct(Stat::Health),
            armor: flat(Stat::Armor) * pct(Stat::Armor),
            evasion: flat(Stat::Evasion),
            defense: class.base_defense * attr_defense_mult,

            damage: (class.base_damage + flat(Stat::Power))
                * primary_dmg_mult
                * pct(Stat::Power),
            crit_chance: class.base_crit_chance
                + attr_crit_bonus
                + flat(Stat::CritChance),
            crit_damage: 0.5 + flat(Stat::CritDamage),
            attack_speed: class.base_attack_speed
                * (1.0 + attr_aspd_bonus + flat(Stat::AttackSpeed)),

            move_speed: class.base_move_speed * (1.0 + flat(Stat::MoveSpeed)),
            cooldown_reduction: flat(Stat::CooldownReduction).min(0.75),
            resource_regen: 1.0 + flat(Stat::ResourceRegen),
            range: class.base_range,

            fire_damage: flat(Stat::FireDamage),
            ice_damage: flat(Stat::IceDamage),
            lightning_damage: flat(Stat::LightningDamage),
        }
    }

    /// Quick-path snapshot for a freshly-created character with no
    /// gear. Used by the character-select preview and any test
    /// fixture that just wants "what does this class look like at
    /// level 1?".
    pub fn baseline(class: &ClassConfig) -> Self {
        let attrs = Attributes::for_class(class.primary_attribute);
        Self::compute(class, &attrs, 1, &StatBlock::new(), &StatModifiers::new())
    }
}
