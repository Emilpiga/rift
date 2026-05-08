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
//! - **Offensive** — `CritChance`, `CritDamage`, `AttackSpeed`, plus the
//!   typed damage buckets (`WeaponDamage`, `SpellDamage`,
//!   `PhysicalDamage`, `FireDamage`, `IceDamage`, `LightningDamage`,
//!   `ProjectileDamage`, `BeamDamage`, `AoeDamage`, `MeleeDamage`).
//! - **Defensive** — `Health`, `Armor`, `Evasion`.
//! - **Utility** — `CooldownReduction`, `ResourceRegen`, `MoveSpeed`.
//! - **Elemental** — `FireDamage`, `IceDamage`, `LightningDamage`.
//!
//! Some stats are **flat** (`Health: +120`) and some are **percent**
//! (`CritChance: +0.05` = +5 %). Use [`Stat::is_percent`] to decide
//! how to display / multiply downstream.

use crate::attributes::Attributes;
use crate::hero::HERO;

// ---------------------------------------------------------------------------
// Stat enum
// ---------------------------------------------------------------------------

/// Every stat that can appear on an item or character sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stat {
    // Offensive
    CritChance,
    CritDamage,
    AttackSpeed,
    // Defensive
    Health,
    /// Flat life pool bonus. Distinct from Health so guaranteed
    /// "+N Vitality" item lines stack with rare "+N Health"
    /// rolls instead of overwriting each other in tooltips.
    Vitality,
    Armor,
    Evasion,
    /// Maximum essence pool (flat). Authored on items as e.g.
    /// `+12 Max Essence`. Stacks with class base + level.
    MaxEssence,
    /// Essence regen bonus, percent. Multiplies the class's
    /// base regen rate the same way `Stat::Health` percent
    /// channels work.
    EssenceRegen,
    // Utility
    CooldownReduction,
    ResourceRegen,
    MoveSpeed,
    // Damage-bucket scaling — multiplies abilities whose
    // `Scaling` matches.
    WeaponDamage,
    SpellDamage,
    // Elemental scaling — multiplies abilities whose
    // `Element` matches.
    PhysicalDamage,
    FireDamage,
    IceDamage,
    LightningDamage,
    // Archetype scaling — multiplies abilities whose
    // `Archetype` matches.
    ProjectileDamage,
    BeamDamage,
    AoeDamage,
    MeleeDamage,
}

impl Stat {
    /// Display name (singular, capitalised).
    pub fn name(self) -> &'static str {
        match self {
            Stat::CritChance => "Crit Chance",
            Stat::CritDamage => "Crit Damage",
            Stat::AttackSpeed => "Attack Speed",
            Stat::Health => "Health",
            Stat::Vitality => "Vitality",
            Stat::Armor => "Armor",
            Stat::Evasion => "Evasion",
            Stat::MaxEssence => "Max Essence",
            Stat::EssenceRegen => "Essence Regen",
            Stat::CooldownReduction => "Cooldown Reduction",
            Stat::ResourceRegen => "Resource Regen",
            Stat::MoveSpeed => "Move Speed",
            Stat::WeaponDamage => "Weapon Damage",
            Stat::SpellDamage => "Spell Damage",
            Stat::PhysicalDamage => "Physical Damage",
            Stat::FireDamage => "Fire Damage",
            Stat::IceDamage => "Ice Damage",
            Stat::LightningDamage => "Lightning Damage",
            Stat::ProjectileDamage => "Projectile Damage",
            Stat::BeamDamage => "Beam Damage",
            Stat::AoeDamage => "AoE Damage",
            Stat::MeleeDamage => "Melee Damage",
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
                | Stat::EssenceRegen
                | Stat::MoveSpeed
                | Stat::Evasion
                | Stat::WeaponDamage
                | Stat::SpellDamage
                | Stat::PhysicalDamage
                | Stat::FireDamage
                | Stat::IceDamage
                | Stat::LightningDamage
                | Stat::ProjectileDamage
                | Stat::BeamDamage
                | Stat::AoeDamage
                | Stat::MeleeDamage
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
    /// Maximum essence pool (the universal ability resource).
    /// Class base + per-level scaling + flat `Stat::MaxEssence`.
    /// Drives [`Ability::resource_cost`] gating on the server
    /// and the essence bar on the HUD.
    pub max_essence: f32,
    /// Essence per second restored while not actively spending.
    /// Class base * (1 + `Stat::EssenceRegen`).
    pub essence_regen: f32,
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
    /// Base outgoing damage — class base scaled by primary
    /// attribute. Typed-damage gear (`WeaponDamage`, `SpellDamage`,
    /// element / archetype) multiplies on top at the cast site
    /// via [`CharacterStats::ability_damage_mult`].
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
    pub physical_damage: f32,

    // --- Damage buckets --------------------------------------------
    /// Multiplies abilities whose `Scaling` is `Weapon`. 0..1.
    pub weapon_damage: f32,
    /// Multiplies abilities whose `Scaling` is `Spell`. 0..1.
    pub spell_damage: f32,

    // --- Archetype scaling -----------------------------------------
    pub projectile_damage: f32,
    pub beam_damage: f32,
    pub aoe_damage: f32,
    pub melee_damage: f32,
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
        attrs: &Attributes,
        level: u32,
        equipment: &StatBlock,
        modifiers: &StatModifiers,
    ) -> Self {
        let class = &HERO;
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
            max_hp: (class_hp
                + attr_hp_bonus
                + flat(Stat::Health)
                + flat(Stat::Vitality))
                * pct(Stat::Health),
            max_essence: (class.base_essence
                + class.essence_per_level * level as f32
                + flat(Stat::MaxEssence))
                * pct(Stat::MaxEssence),
            essence_regen: class.base_essence_regen
                * (1.0 + flat(Stat::EssenceRegen)),
            armor: flat(Stat::Armor) * pct(Stat::Armor),
            evasion: flat(Stat::Evasion),
            defense: class.base_defense * attr_defense_mult,

            damage: class.base_damage * primary_dmg_mult,
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
            physical_damage: flat(Stat::PhysicalDamage),

            weapon_damage: flat(Stat::WeaponDamage),
            spell_damage: flat(Stat::SpellDamage),

            projectile_damage: flat(Stat::ProjectileDamage),
            beam_damage: flat(Stat::BeamDamage),
            aoe_damage: flat(Stat::AoeDamage),
            melee_damage: flat(Stat::MeleeDamage),
        }
    }

    /// Quick-path snapshot for a freshly-created character with no
    /// gear. Used by the character-select preview and any test
    /// fixture that just wants "what does the hero look like at
    /// level 1?".
    pub fn baseline() -> Self {
        let attrs = Attributes::for_class(HERO.primary_attribute);
        Self::compute(&attrs, 1, &StatBlock::new(), &StatModifiers::new())
    }

    /// Compose the full ability-aware damage multiplier for one
    /// cast. Reads the ability's scaling-bucket / element /
    /// archetype tags and stacks the matching gear bonuses
    /// multiplicatively. Pure read-only — call site multiplies
    /// the result onto base damage in the server cast pipe.
    ///
    /// Order matches the design doc:
    /// `(1 + scaling_bucket) × (1 + element) × (1 + archetype)`.
    /// Each unmatched tag (`Scaling::None`, `Element::None`,
    /// `Archetype::Movement`/`Utility`) contributes `× 1`, so
    /// utility abilities pass through untouched.
    pub fn ability_damage_mult(&self, ability: &crate::abilities::Ability) -> f32 {
        use crate::abilities::{Archetype, Element, Scaling};
        let scaling = match ability.scaling {
            Scaling::Weapon => 1.0 + self.weapon_damage,
            Scaling::Spell => 1.0 + self.spell_damage,
            Scaling::None => 1.0,
        };
        let element = match ability.element {
            Element::Physical => 1.0 + self.physical_damage,
            Element::Fire => 1.0 + self.fire_damage,
            Element::Ice => 1.0 + self.ice_damage,
            Element::Lightning => 1.0 + self.lightning_damage,
            Element::None => 1.0,
        };
        let archetype = match ability.archetype {
            Archetype::Projectile => 1.0 + self.projectile_damage,
            Archetype::Beam => 1.0 + self.beam_damage,
            Archetype::Aoe => 1.0 + self.aoe_damage,
            Archetype::Melee => 1.0 + self.melee_damage,
            Archetype::Movement | Archetype::Utility => 1.0,
        };
        scaling * element * archetype
    }
}
