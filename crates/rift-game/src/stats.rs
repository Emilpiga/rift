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
//!   typed damage buckets (`PhysicalDamage`, `FireDamage`,
//!   `IceDamage`, `LightningDamage`).
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
    // Attributes — flat bonus points that fold into the
    // character's `Attributes` block at compute time, then drive
    // class scaling (damage / HP / crit / etc.).
    Strength,
    Agility,
    Intellect,
    // Offensive
    CritChance,
    CritDamage,
    AttackSpeed,
    // Defensive
    Health,
    Armor,
    Evasion,
    /// Flat health regenerated per second. Authored on items as
    /// e.g. `+3 Health Regen`. Ticks alongside essence regen on
    /// the server, no spend-pause gating.
    HealthRegen,
    /// Damage reduction vs. `Element::{Fire,Ice,Lightning}`
    /// ability hits, percent (0..1, capped at 0.75). One stat
    /// covers all three elements; per-element resists can be
    /// split out later if/when the design needs it. Physical
    /// damage is mitigated solely by `Armor` (soft-capped flat
    /// reduction) — there is no separate `PhysicalResist`.
    ElementalResist,
    /// Bonus to incoming heals, percent (0.20 = +20% heals
    /// received). Multiplies both direct heals and HoT ticks
    /// at apply time. Stacks additively across gear.
    HealingReceived,
    /// Maximum essence pool (flat). Authored on items as e.g.
    /// `+12 Max Essence`. Stacks with class base + level.
    MaxResource,
    // Utility
    CooldownReduction,
    /// Essence regeneration bonus, percent. The in-game name is
    /// "Essence Regen" but the programmatic identifier stays
    /// `ResourceRegen` so future resource systems can share it.
    /// Multiplies the class's base regen rate.
    ResourceRegen,
    MoveSpeed,
    /// Global ability-range multiplier. Each `+1.0` here adds
    /// +100 % to projectile travel distance / beam length / AoE
    /// radius. Roll values are percent (e.g. `0.10` = +10 %).
    Range,
    // Elemental scaling — multiplies abilities whose
    // `Element` matches.
    PhysicalDamage,
    FireDamage,
    IceDamage,
    LightningDamage,
}

impl Stat {
    /// Display name (singular, capitalised).
    pub fn name(self) -> &'static str {
        match self {
            Stat::Strength => "Strength",
            Stat::Agility => "Agility",
            Stat::Intellect => "Intellect",
            Stat::CritChance => "Crit Chance",
            Stat::CritDamage => "Crit Damage",
            Stat::AttackSpeed => "Attack Speed",
            Stat::Health => "Health",
            Stat::Armor => "Armor",
            Stat::Evasion => "Evasion",
            Stat::HealthRegen => "Health Regen",
            Stat::ElementalResist => "Elemental Resist",
            Stat::HealingReceived => "Healing Received",
            Stat::MaxResource => "Max Essence",
            Stat::CooldownReduction => "Cooldown Reduction",
            Stat::ResourceRegen => "Essence Regen",
            Stat::MoveSpeed => "Move Speed",
            Stat::Range => "Range",
            Stat::PhysicalDamage => "Physical Damage",
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
                | Stat::Range
                | Stat::Evasion
                | Stat::ElementalResist
                | Stat::HealingReceived
                | Stat::PhysicalDamage
                | Stat::FireDamage
                | Stat::IceDamage
                | Stat::LightningDamage
        )
    }

    /// `true` if this stat is part of the offensive bonus group.
    /// Used by tooltip rendering (to partition the bonus block
    /// into "offence vs sustain") and by the bonus-roll filter
    /// in `Item::roll` (weapons only ever roll offensive bonus
    /// stats). Single source of truth so the two pipelines
    /// can't drift.
    pub fn is_offensive_bonus(self) -> bool {
        matches!(
            self,
            Stat::CritChance | Stat::CritDamage | Stat::AttackSpeed
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
        Self {
            entries: Vec::new(),
        }
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
    /// Class base + per-level scaling + flat `Stat::MaxResource`.
    /// Drives [`Ability::resource_cost`] gating on the server
    /// and the essence bar on the HUD.
    pub max_resource: f32,
    /// Essence per second restored while not actively spending.
    /// Class base * (1 + `Stat::ResourceRegen`).
    pub resource_regen: f32,
    /// Flat HP per second restored passively. Sourced entirely
    /// from `Stat::HealthRegen`. Ticks every frame on the
    /// server, no spend-pause gating.
    pub health_regen: f32,
    /// Bonus to incoming heals, 0..1 (`+0.20` = +20% heals
    /// received). Multiplied onto direct heals + HoT ticks.
    pub healing_received: f32,
    /// Damage reduction vs. `Element::{Fire,Ice,Lightning}`,
    /// 0..0.75. Physical damage is mitigated solely by `armor`.
    pub elemental_resist: f32,
    /// Flat armor — mirrors `Stat::Armor`, with Strength folded
    /// in as a percent bonus. Consumed via
    /// [`CharacterStats::armor_damage_reduction`].
    pub armor: f32,
    /// Evasion chance, 0..1 — mirrors `Stat::Evasion`.
    pub evasion: f32,

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
    /// Global ability-range multiplier (`1.0` = no change). Scales
    /// projectile travel distance, beam length, and AoE radius
    /// uniformly at the cast site.
    pub range: f32,

    // --- Elemental -------------------------------------------------
    /// Per-element damage bonus, 0..1.
    pub fire_damage: f32,
    pub ice_damage: f32,
    pub lightning_damage: f32,
    pub physical_damage: f32,
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

        // Helper: equipment + modifiers.flat sum for a stat.
        let flat = |s: Stat| equipment.get(s) + modifiers.flat.get(s);
        // Helper: percent multiplier (1 + sum of percent
        // contributions). Talents / buffs only.
        let pct = |s: Stat| 1.0 + modifiers.percent.get(s);

        // Fold item-rolled attribute bonuses into the manual
        // `Attributes` block before any scaling reads from it.
        // Items roll flat points (`+14 Strength`) that stack with
        // the point-spend screen 1:1 — gear and manual investment
        // are interchangeable in the formula, which keeps the
        // numbers honest.
        let strength = attrs.strength as f32 + flat(Stat::Strength);
        let agility = attrs.agility as f32 + flat(Stat::Agility);
        let intellect = attrs.intellect as f32 + flat(Stat::Intellect);
        let primary_value = match primary {
            crate::attributes::AttributeType::Strength => strength,
            crate::attributes::AttributeType::Agility => agility,
            crate::attributes::AttributeType::Intellect => intellect,
            crate::attributes::AttributeType::Vitality => attrs.vitality as f32,
        };

        // Per-attribute scaling. Each point of primary -> +1 % damage.
        let primary_dmg_mult = 1.0 + primary_value * 0.01;
        // Strength -> +0.8 % armor per point. Folds into the
        // flat armor pool as a percent multiplier alongside the
        // talent-channel `Stat::Armor` percent line.
        let strength_armor_mult = 1.0 + strength * 0.008;
        // Vitality -> +3 flat HP per point.
        let attr_hp_bonus = attrs.vitality as f32 * 3.0;
        // Agility -> +0.1 % crit, +0.5 % attack speed per point.
        let attr_crit_bonus = agility * 0.001;
        let attr_aspd_bonus = agility * 0.005;
        // Intellect -> +2 flat max essence per point.
        let attr_resource_bonus = intellect * 2.0;

        // Class-level scaling.
        let class_hp = class.base_hp + class.hp_per_level * level as f32;

        Self {
            max_hp: (class_hp + attr_hp_bonus + flat(Stat::Health)) * pct(Stat::Health),
            max_resource: (class.base_resource
                + class.resource_per_level * level as f32
                + attr_resource_bonus
                + flat(Stat::MaxResource))
                * pct(Stat::MaxResource),
            resource_regen: class.base_resource_regen * (1.0 + flat(Stat::ResourceRegen)),
            // Baseline class regen + any flat bonus from gear /
            // talents. Clamped at zero so a hypothetical
            // negative aura can't drive regen below zero (HP
            // drain is its own debuff path).
            health_regen: (class.base_health_regen + flat(Stat::HealthRegen)).max(0.0),
            healing_received: flat(Stat::HealingReceived),
            elemental_resist: flat(Stat::ElementalResist).clamp(0.0, 0.75),
            armor: flat(Stat::Armor) * pct(Stat::Armor) * strength_armor_mult,
            evasion: flat(Stat::Evasion),

            damage: class.base_damage * primary_dmg_mult,
            crit_chance: class.base_crit_chance + attr_crit_bonus + flat(Stat::CritChance),
            crit_damage: 0.5 + flat(Stat::CritDamage),
            attack_speed: class.base_attack_speed
                * (1.0 + attr_aspd_bonus + flat(Stat::AttackSpeed)),

            move_speed: class.base_move_speed * (1.0 + flat(Stat::MoveSpeed)),
            cooldown_reduction: flat(Stat::CooldownReduction).min(0.75),
            range: 1.0 + flat(Stat::Range),

            fire_damage: flat(Stat::FireDamage),
            ice_damage: flat(Stat::IceDamage),
            lightning_damage: flat(Stat::LightningDamage),
            physical_damage: flat(Stat::PhysicalDamage),
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
    /// cast. Reads the ability's element tag and stacks the
    /// matching gear bonus multiplicatively. Pure read-only —
    /// call site multiplies the result onto base damage in the
    /// server cast pipe.
    ///
    /// Order matches the design doc:
    /// `(1 + scaling_bucket) × (1 + element)`.
    /// Each unmatched tag (`Element::None`) contributes `× 1`, so
    /// utility abilities pass through untouched.
    pub fn ability_damage_mult(&self, ability: &crate::abilities::Ability) -> f32 {
        use crate::abilities::Element;
        let element = match ability.element {
            Element::Physical => 1.0 + self.physical_damage,
            Element::Fire => 1.0 + self.fire_damage,
            Element::Ice => 1.0 + self.ice_damage,
            Element::Lightning => 1.0 + self.lightning_damage,
            Element::None => 1.0,
        };
        element
    }

    /// Fraction of incoming damage absorbed by armor, 0..0.75.
    /// Standard ARPG-style soft cap: `armor / (armor + K)` where
    /// `K` scales with the receiver's level so high-level players
    /// don't get free immortality from low-level armor numbers.
    /// Capped at 0.75 to leave a real damage floor.
    pub fn armor_damage_reduction(&self, level: u32) -> f32 {
        if self.armor <= 0.0 {
            return 0.0;
        }
        let k = 50.0 + 10.0 * level as f32;
        (self.armor / (self.armor + k)).clamp(0.0, 0.75)
    }

    /// Resist multiplier on incoming damage of `element` (1.0 =
    /// no resist, 0.25 = 75 % reduction, the cap). The three
    /// elements share `elemental_resist`. `Physical` is unresisted
    /// here — armor handles physical mitigation via
    /// [`armor_damage_reduction`]. `None` (untyped) is unresisted.
    pub fn incoming_resist_mult(&self, element: crate::abilities::Element) -> f32 {
        use crate::abilities::Element;
        let r = match element {
            Element::Fire | Element::Ice | Element::Lightning => self.elemental_resist,
            Element::Physical | Element::None => 0.0,
        };
        (1.0 - r.clamp(0.0, 0.75)).max(0.0)
    }

    /// Effective per-hit damage of `ability` against an
    /// unmitigated target, *excluding* per-ability affix amplify
    /// mods (`AmplifyAbilityDamage`) which live on
    /// [`AbilityMods`] outside this struct. Reproduces the cast
    /// pipeline's `base_damage × damage_scalar × ability_mult`
    /// — the number the player would deal to a target with no
    /// armor / resist before a crit roll.
    ///
    /// For `Channel` / `AoeZone` abilities the returned value is
    /// per-tick (matches the server's per-tick application);
    /// callers that want a total can multiply by the tick count.
    pub fn ability_effective_damage(&self, ability: &crate::abilities::Ability) -> f32 {
        // `damage_scalar = stats.damage / HERO.base_damage`. We
        // re-derive it here so this fn stays self-contained
        // (no `ServerPlayer` dependency for the client tooltip
        // path).
        let base = HERO.base_damage;
        let dmg_scalar = if base <= 0.0 { 1.0 } else { self.damage / base };
        ability.base_damage * dmg_scalar * self.ability_damage_mult(ability)
    }

    /// Crit-weighted average damage of one hit — the per-hit
    /// number folded by the player's crit chance × crit damage.
    /// Useful for tooltips that want to show "expected" damage
    /// without burying the player in two lines per ability.
    pub fn ability_avg_damage(&self, ability: &crate::abilities::Ability) -> f32 {
        let per_hit = self.ability_effective_damage(ability);
        let chance = self.crit_chance.clamp(0.0, 1.0);
        per_hit * (1.0 + chance * self.crit_damage)
    }
}
