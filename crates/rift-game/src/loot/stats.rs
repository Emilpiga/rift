//! The complete stat vocabulary.
//!
//! Keep this list **small** — every variant ends up driving UI,
//! affixes, tooltips, and combat formulas. New stats require a
//! gameplay reason; "+% magic find" without drop-table support adds
//! noise, not depth.
//!
//! Stats fall into three semantic groups (informational only — the
//! enum is flat to keep matching cheap):
//!
//! - **Offensive** — `Power`, `CritChance`, `CritDamage`, `AttackSpeed`.
//! - **Defensive** — `Health`, `Armor`, `Evasion`.
//! - **Utility** — `CooldownReduction`, `ResourceRegen`, `MoveSpeed`.
//! - **Elemental** — `FireDamage`, `IceDamage`, `LightningDamage`
//!   (each a percent-multiplier applied per element).
//!
//! Some stats are **flat** (`Health: +120`) and some are **percent**
//! (`CritChance: +0.05` = +5 %). Use [`Stat::is_percent`] to decide
//! how to display / multiply downstream.

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

/// Sparse stat container — sums duplicates on read.
///
/// Used both as a per-item rolled-stat block and as the aggregated
/// character total. Cheap to clone; never indexed by hashing because
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
