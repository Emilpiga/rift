//! Affix definitions, effect taxonomy, and the global affix pool.
//!
//! # The four ways an item can affect gameplay
//!
//! Inspired by ARPG design lore, [`AffixEffect`] enumerates the
//! four *interaction patterns* an affix can have with the combat
//! layer:
//!
//! | Pattern    | Variant                                   | Example                                              |
//! |------------|-------------------------------------------|------------------------------------------------------|
//! | (Stat)     | [`AffixEffect::Stat`]                     | `+15 % Fire Damage`                                  |
//! | Amplify    | [`AffixEffect::AmplifyAbilityDamage`]     | `+25 % Fireball damage`                              |
//! | Amplify    | [`AffixEffect::ReduceAbilityCooldown`]    | `Frost Ray cooldown -10 %`                           |
//! | Modify     | [`AffixEffect::ExtraProjectiles`]         | `Fireball fires +2 extra projectiles`                |
//! | Transform  | [`AffixEffect::TransformAbility`]         | `Fireball becomes a beam`                            |
//! | Trigger    | [`AffixEffect::Proc`]                     | `On crit: cast a free mini-fireball`                 |
//!
//! Stats are number-only ammo. The other four are what make builds
//! interesting — they live in the same affix pool but are gated by
//! `rarity_min` so they only appear on higher-rarity drops.
//!
//! # Filtering & rolling
//!
//! When [`crate::loot::Item::roll`] picks an affix:
//!
//! 1. `tags & base.allowed_tags != 0` — synergy with the base.
//! 2. `min_ilvl <= ilvl` — gated by item-level.
//! 3. `rarity_min <= item_rarity` — gated by rarity tier.
//! 4. Already-rolled affix ids are excluded (no duplicates).
//! 5. Weight ×2 if `tags & base.favored_tags != 0` — base bias.

use crate::abilities::AbilityId;

use super::items::tag::*;
use super::rarity::Rarity;
use crate::stats::Stat;

/// What an affix actually *does*.
///
/// Adding a new pattern: add a variant here, then teach the combat
/// layer (likely via [`super::ability_mods::AbilityMods`]) how to
/// match on it.
#[derive(Clone, Copy, Debug)]
pub enum AffixEffect {
    /// Plain stat line. Rolled value goes into the character's
    /// [`super::stats::StatBlock`].
    Stat(Stat),

    /// **Amplify** — multiply a specific ability's damage by
    /// `1 + value`. Combined multiplicatively across affixes.
    AmplifyAbilityDamage(AbilityId),

    /// **Amplify** — multiply a specific ability's cooldown by
    /// `1 - value`. Clamped at the runtime layer.
    ReduceAbilityCooldown(AbilityId),

    /// **Modify** — add `value` extra projectiles to a projectile
    /// ability (the rolled value is integer-rounded at apply time).
    ExtraProjectiles(AbilityId),

    /// **Transform** — replace the ability's behaviour with the
    /// named [`AbilityVariant`]. Mutually exclusive — a single
    /// transform wins (last-equipped, by convention).
    TransformAbility(AbilityId, AbilityVariant),

    /// **Trigger / Proc** — when `event` fires, invoke `action`. The
    /// rolled value is the proc chance in 0..1.
    Proc(ProcEvent, ProcAction),
}

/// Discrete behavioural reskins of an ability. Combat layer matches
/// on these. Adding a variant is a one-line change here plus a
/// matching arm wherever the ability is implemented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbilityVariant {
    /// Fireball morphs from a projectile into a piercing beam.
    FireballToBeam,
    /// Frost Ray detonates at its terminal point into shards.
    FrostRayShatter,
    /// Whirlwind pulls enemies inward each tick instead of just hitting.
    WhirlwindVortex,
}

/// Game events that can trigger a proc affix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcEvent {
    OnCrit,
    OnHit,
    OnKill,
    OnDodge,
    OnLowHealth, // < 30 % HP threshold
}

/// What a proc actually does. Concrete payloads so the combat layer
/// can dispatch without a string lookup.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ProcAction {
    /// Cast `ability` for free, ignoring cooldown & resource cost.
    /// This is the generic "proc casts a spell" payload — pick any
    /// player-castable ability id and the dispatcher will route it
    /// through the standard cast pipeline.
    CastAbility(AbilityId),
    /// Spawn a one-shot AoE explosion of `radius` doing `damage`.
    Explosion { radius: f32, damage: f32 },
}

/// One affix the loot system can roll. `'static` so the pool can
/// live in a `pub const`.
#[derive(Clone, Copy, Debug)]
pub struct AffixDef {
    pub id: &'static str,
    /// Tooltip template — `{}` is replaced by the formatted value.
    /// Effects without a numeric value (Transform) ignore `{}`.
    pub name_template: &'static str,
    pub effect: AffixEffect,
    /// Roll range at `ilvl = 1`. For [`AffixEffect::Stat`] the
    /// units follow [`Stat::is_percent`]. For Amplify / Cooldown /
    /// Proc-chance it's a fraction (0.25 = 25 %). For
    /// [`AffixEffect::ExtraProjectiles`] it's an integer count
    /// rounded after rolling. Transform ignores it.
    pub roll: (f32, f32),
    /// Linear scaling per item-level above 1. `0.0` = static range.
    pub ilvl_scale: f32,
    /// Bitmask filtered against [`super::items::BaseItem::allowed_tags`].
    pub tags: u32,
    /// Affix doesn't appear below this item-level.
    pub min_ilvl: u32,
    /// Affix doesn't appear below this **rarity tier**. This is the
    /// behavioural-gating knob: gameplay-changing patterns
    /// (Transform, Trigger) sit at [`Rarity::Legendary`] and never
    /// roll on lower tiers, so rarity changes *what* you get, not
    /// just *how big*.
    pub rarity_min: Rarity,
    /// Base selection weight before favoured-tag bonus.
    pub weight: u32,
}

// ---------------------------------------------------------------------
// The pool
// ---------------------------------------------------------------------

use crate::abilities as ab;

pub const AFFIX_POOL: &[AffixDef] = &[
    // ════════ Attribute axis — flat points ═══════════════════════════
    //
    // The third trio axis is Attribute. These affixes don't draw
    // from the bonus pool (their category is `Attribute`, which
    // the bonus filter rejects); they roll only when the trio
    // pipeline asks for an attribute line. Range / scale matches
    // the `flat_health` shape — boring, predictable, comparable.
    AffixDef {
        id: "flat_strength",
        name_template: "{} Strength",
        effect: AffixEffect::Stat(Stat::Strength),
        roll: (4.0, 8.0),
        ilvl_scale: 0.6,
        tags: ALL,
        min_ilvl: 1,
        rarity_min: Rarity::Common,
        weight: 1,
    },
    AffixDef {
        id: "flat_agility",
        name_template: "{} Agility",
        effect: AffixEffect::Stat(Stat::Agility),
        roll: (4.0, 8.0),
        ilvl_scale: 0.6,
        tags: ALL,
        min_ilvl: 1,
        rarity_min: Rarity::Common,
        weight: 1,
    },
    AffixDef {
        id: "flat_intellect",
        name_template: "{} Intellect",
        effect: AffixEffect::Stat(Stat::Intellect),
        roll: (4.0, 8.0),
        ilvl_scale: 0.6,
        tags: ALL,
        min_ilvl: 1,
        rarity_min: Rarity::Common,
        weight: 1,
    },
    // ════════ Common-tier: pure stats ═══════════════════════════════
    AffixDef {
        id: "flat_health",
        name_template: "{} Health",
        effect: AffixEffect::Stat(Stat::Health),
        roll: (10.0, 25.0),
        ilvl_scale: 4.0,
        tags: DEFENSE | UTILITY,
        min_ilvl: 1,
        rarity_min: Rarity::Common,
        weight: 100,
    },
    AffixDef {
        id: "flat_armor",
        name_template: "{} Armor",
        effect: AffixEffect::Stat(Stat::Armor),
        roll: (4.0, 9.0),
        ilvl_scale: 2.0,
        tags: DEFENSE | MELEE,
        min_ilvl: 1,
        rarity_min: Rarity::Common,
        weight: 80,
    },
    AffixDef {
        id: "pct_attack_speed",
        name_template: "{} Attack Speed",
        effect: AffixEffect::Stat(Stat::AttackSpeed),
        roll: (0.04, 0.08),
        ilvl_scale: 0.003,
        tags: SPEED | MELEE | CASTER,
        min_ilvl: 1,
        rarity_min: Rarity::Common,
        weight: 60,
    },
    AffixDef {
        id: "pct_move_speed",
        name_template: "{} Move Speed",
        effect: AffixEffect::Stat(Stat::MoveSpeed),
        roll: (0.03, 0.07),
        ilvl_scale: 0.001,
        tags: SPEED | UTILITY,
        min_ilvl: 1,
        rarity_min: Rarity::Common,
        weight: 50,
    },
    AffixDef {
        id: "pct_evasion",
        name_template: "{} Evasion",
        effect: AffixEffect::Stat(Stat::Evasion),
        roll: (0.03, 0.07),
        ilvl_scale: 0.002,
        tags: SPEED | DEFENSE,
        min_ilvl: 1,
        rarity_min: Rarity::Common,
        weight: 50,
    },
    AffixDef {
        id: "pct_resource_regen",
        name_template: "{} Essence Regen",
        effect: AffixEffect::Stat(Stat::ResourceRegen),
        roll: (0.05, 0.12),
        ilvl_scale: 0.004,
        tags: UTILITY | CASTER,
        min_ilvl: 1,
        rarity_min: Rarity::Common,
        weight: 50,
    },
    AffixDef {
        id: "flat_health_regen",
        name_template: "{} Health Regen",
        effect: AffixEffect::Stat(Stat::HealthRegen),
        roll: (1.0, 3.0),
        ilvl_scale: 0.4,
        tags: DEFENSE | UTILITY,
        min_ilvl: 1,
        rarity_min: Rarity::Common,
        weight: 50,
    },
    AffixDef {
        id: "pct_elemental_resist",
        name_template: "{} Elemental Resist",
        effect: AffixEffect::Stat(Stat::ElementalResist),
        roll: (0.03, 0.06),
        ilvl_scale: 0.002,
        tags: DEFENSE | CASTER,
        min_ilvl: 3,
        rarity_min: Rarity::Magic,
        weight: 45,
    },
    AffixDef {
        id: "pct_healing_received",
        name_template: "{} Healing Received",
        effect: AffixEffect::Stat(Stat::HealingReceived),
        roll: (0.05, 0.12),
        ilvl_scale: 0.003,
        tags: DEFENSE | UTILITY,
        min_ilvl: 5,
        rarity_min: Rarity::Magic,
        weight: 35,
    },
    // ════════ Magic-tier: synergistic clusters ══════════════════════
    AffixDef {
        id: "pct_crit_chance",
        name_template: "{} Crit Chance",
        effect: AffixEffect::Stat(Stat::CritChance),
        roll: (0.02, 0.05),
        ilvl_scale: 0.002,
        tags: CRIT,
        min_ilvl: 1,
        rarity_min: Rarity::Magic,
        weight: 60,
    },
    AffixDef {
        id: "pct_crit_damage",
        name_template: "{} Crit Damage",
        effect: AffixEffect::Stat(Stat::CritDamage),
        roll: (0.10, 0.25),
        ilvl_scale: 0.01,
        tags: CRIT,
        min_ilvl: 5,
        rarity_min: Rarity::Magic,
        weight: 50,
    },
    AffixDef {
        id: "pct_cooldown",
        name_template: "{} Cooldown Reduction",
        effect: AffixEffect::Stat(Stat::CooldownReduction),
        roll: (0.03, 0.06),
        ilvl_scale: 0.002,
        tags: UTILITY | CASTER,
        min_ilvl: 5,
        rarity_min: Rarity::Magic,
        weight: 50,
    },
    AffixDef {
        id: "pct_fire_damage",
        name_template: "{} Fire Damage",
        effect: AffixEffect::Stat(Stat::FireDamage),
        roll: (0.06, 0.14),
        ilvl_scale: 0.005,
        tags: FIRE | CASTER,
        min_ilvl: 1,
        rarity_min: Rarity::Magic,
        weight: 70,
    },
    AffixDef {
        id: "pct_ice_damage",
        name_template: "{} Ice Damage",
        effect: AffixEffect::Stat(Stat::IceDamage),
        roll: (0.06, 0.14),
        ilvl_scale: 0.005,
        tags: ICE | CASTER,
        min_ilvl: 1,
        rarity_min: Rarity::Magic,
        weight: 70,
    },
    AffixDef {
        id: "pct_lightning_damage",
        name_template: "{} Lightning Damage",
        effect: AffixEffect::Stat(Stat::LightningDamage),
        roll: (0.06, 0.14),
        ilvl_scale: 0.005,
        tags: LIGHTNING | CASTER,
        min_ilvl: 1,
        rarity_min: Rarity::Magic,
        weight: 70,
    },
    // ════════ Rare-tier: ability AMPLIFIERS ═════════════════════════
    AffixDef {
        id: "amp_frost_ray_dmg",
        name_template: "Frost Ray damage {}",
        effect: AffixEffect::AmplifyAbilityDamage(ab::FROST_RAY),
        roll: (0.10, 0.20),
        ilvl_scale: 0.005,
        tags: ICE | CASTER,
        min_ilvl: 5,
        rarity_min: Rarity::Rare,
        weight: 30,
    },
    AffixDef {
        id: "amp_whirlwind_dmg",
        name_template: "Whirlwind damage {}",
        effect: AffixEffect::AmplifyAbilityDamage(ab::WHIRLWIND),
        roll: (0.10, 0.20),
        ilvl_scale: 0.005,
        tags: MELEE,
        min_ilvl: 5,
        rarity_min: Rarity::Rare,
        weight: 30,
    },
    AffixDef {
        id: "cdr_frost_ray",
        name_template: "Frost Ray cooldown {}",
        effect: AffixEffect::ReduceAbilityCooldown(ab::FROST_RAY),
        roll: (0.05, 0.12),
        ilvl_scale: 0.003,
        tags: ICE | CASTER | UTILITY,
        min_ilvl: 5,
        rarity_min: Rarity::Rare,
        weight: 25,
    },
    AffixDef {
        id: "cdr_evasive_roll",
        name_template: "Evasive Roll cooldown {}",
        effect: AffixEffect::ReduceAbilityCooldown(ab::EVASIVE_ROLL),
        roll: (0.05, 0.12),
        ilvl_scale: 0.003,
        tags: SPEED | UTILITY,
        min_ilvl: 5,
        rarity_min: Rarity::Rare,
        weight: 25,
    },
    // ════════ Legendary-tier: gameplay-changing ═════════════════════
    AffixDef {
        id: "mod_fire_ball_extra_proj",
        name_template: "Fireball fires {} extra projectiles",
        effect: AffixEffect::ExtraProjectiles(ab::FIRE_BALL),
        roll: (1.0, 1.0),
        ilvl_scale: 0.0,
        tags: SPEED,
        min_ilvl: 15,
        rarity_min: Rarity::Legendary,
        weight: 10,
    },
    AffixDef {
        id: "transform_frost_ray_shatter",
        name_template: "Frost Ray detonates into shards",
        effect: AffixEffect::TransformAbility(ab::FROST_RAY, AbilityVariant::FrostRayShatter),
        roll: (0.0, 0.0),
        ilvl_scale: 0.0,
        tags: ICE | CASTER,
        min_ilvl: 15,
        rarity_min: Rarity::Legendary,
        weight: 8,
    },
    // ════════ Slot-signature affixes ════════════════════════════════
    //
    // These are the deterministic lines injected by
    // `Item::roll` based on `EquipSlot`. They're full members of
    // the pool (so save/load + tooltip rendering work uniformly)
    // but normally never roll as bonuses because every item slot
    // already carries them. The bonus-roll path filters them
    // out by `id` in `Item::roll`.
    AffixDef {
        id: "pct_physical_damage",
        name_template: "{} Physical Damage",
        effect: AffixEffect::Stat(Stat::PhysicalDamage),
        roll: (0.06, 0.14),
        ilvl_scale: 0.005,
        tags: ALL,
        min_ilvl: 1,
        rarity_min: Rarity::Common,
        weight: 0,
    },
    AffixDef {
        id: "pct_projectile_damage",
        name_template: "{} Projectile Damage",
        effect: AffixEffect::Stat(Stat::ProjectileDamage),
        roll: (0.06, 0.14),
        ilvl_scale: 0.005,
        tags: ALL,
        min_ilvl: 1,
        rarity_min: Rarity::Common,
        weight: 0,
    },
    AffixDef {
        id: "pct_melee_damage",
        name_template: "{} Melee Damage",
        effect: AffixEffect::Stat(Stat::MeleeDamage),
        roll: (0.06, 0.14),
        ilvl_scale: 0.005,
        tags: ALL,
        min_ilvl: 1,
        rarity_min: Rarity::Common,
        weight: 0,
    },
];

// ---------------------------------------------------------------------
// Resonance pool (ITEMS.md §2.5 / §3 Phase 3)
// ---------------------------------------------------------------------
//
// "Cross-family" damage-axis lines. Every entry mirrors a normal
// axis affix but rolls at ~60 % of its sibling's magnitudes
// (roll-range and ilvl_scale both scaled). The roll pipeline
// draws from this pool *only* in the resonance phase and filters
// to axes the base's `BaseFamily` would otherwise reject \u2014 so a
// resonance line is by construction something the item shouldn't
// "naturally" have. That's the design: a tiny, exciting,
// off-archetype bonus that rewards specific build pivots.
//
// `min_ilvl: 5` everywhere keeps resonance off the very earliest
// Rare drops; `rarity_min: Rarity::Rare` enforces the rarity gate
// at the def level too even though the pipeline chance-gates it
// separately. `weight: 1` everywhere because the pipeline picks
// uniformly from the filtered subset \u2014 weight differentiation
// would just bias which off-family pivot the player gets.
pub const RESONANCE_POOL: &[AffixDef] = &[
    // ── Element resonance ───────────────────────────────────────
    AffixDef {
        id: "res_physical_damage",
        name_template: "Resonant {} Physical Damage",
        effect: AffixEffect::Stat(Stat::PhysicalDamage),
        roll: (0.04, 0.08),
        ilvl_scale: 0.003,
        tags: ALL,
        min_ilvl: 5,
        rarity_min: Rarity::Rare,
        weight: 1,
    },
    AffixDef {
        id: "res_fire_damage",
        name_template: "Resonant {} Fire Damage",
        effect: AffixEffect::Stat(Stat::FireDamage),
        roll: (0.04, 0.08),
        ilvl_scale: 0.003,
        tags: ALL,
        min_ilvl: 5,
        rarity_min: Rarity::Rare,
        weight: 1,
    },
    AffixDef {
        id: "res_ice_damage",
        name_template: "Resonant {} Ice Damage",
        effect: AffixEffect::Stat(Stat::IceDamage),
        roll: (0.04, 0.08),
        ilvl_scale: 0.003,
        tags: ALL,
        min_ilvl: 5,
        rarity_min: Rarity::Rare,
        weight: 1,
    },
    AffixDef {
        id: "res_lightning_damage",
        name_template: "Resonant {} Lightning Damage",
        effect: AffixEffect::Stat(Stat::LightningDamage),
        roll: (0.04, 0.08),
        ilvl_scale: 0.003,
        tags: ALL,
        min_ilvl: 5,
        rarity_min: Rarity::Rare,
        weight: 1,
    },
    // ── Archetype resonance ─────────────────────────────────────
    AffixDef {
        id: "res_projectile_damage",
        name_template: "Resonant {} Projectile Damage",
        effect: AffixEffect::Stat(Stat::ProjectileDamage),
        roll: (0.04, 0.08),
        ilvl_scale: 0.003,
        tags: ALL,
        min_ilvl: 5,
        rarity_min: Rarity::Rare,
        weight: 1,
    },
    AffixDef {
        id: "res_melee_damage",
        name_template: "Resonant {} Melee Damage",
        effect: AffixEffect::Stat(Stat::MeleeDamage),
        roll: (0.04, 0.08),
        ilvl_scale: 0.003,
        tags: ALL,
        min_ilvl: 5,
        rarity_min: Rarity::Rare,
        weight: 1,
    },
    // ── Attribute resonance ──────────────────────────
    // Flat attribute points at ~60 % of the in-axis roll — a
    // resonance line is supposed to feel like a bonus, not a
    // straight upgrade over the family-locked attribute slot.
    AffixDef {
        id: "res_strength",
        name_template: "Resonant {} Strength",
        effect: AffixEffect::Stat(Stat::Strength),
        roll: (2.0, 5.0),
        ilvl_scale: 0.4,
        tags: ALL,
        min_ilvl: 5,
        rarity_min: Rarity::Rare,
        weight: 1,
    },
    AffixDef {
        id: "res_agility",
        name_template: "Resonant {} Agility",
        effect: AffixEffect::Stat(Stat::Agility),
        roll: (2.0, 5.0),
        ilvl_scale: 0.4,
        tags: ALL,
        min_ilvl: 5,
        rarity_min: Rarity::Rare,
        weight: 1,
    },
    AffixDef {
        id: "res_intellect",
        name_template: "Resonant {} Intellect",
        effect: AffixEffect::Stat(Stat::Intellect),
        roll: (2.0, 5.0),
        ilvl_scale: 0.4,
        tags: ALL,
        min_ilvl: 5,
        rarity_min: Rarity::Rare,
        weight: 1,
    },
];

/// Per-rarity probability that an item gets an extra Resonance
/// affix appended after the bonus block. ITEMS.md §2.5: Rare 5 %,
/// Legendary 25 %. Common / Magic never resonate.
pub fn resonance_chance(rarity: Rarity) -> f32 {
    match rarity {
        Rarity::Common | Rarity::Magic => 0.0,
        Rarity::Rare => 0.05,
        Rarity::Legendary => 0.25,
    }
}

// ---------------------------------------------------------------------
// Rift-touched pool — ITEMS.md §2.6 / §3 Phase 5
// ---------------------------------------------------------------------
//
// A single extra slot awarded only on drops that come from inside
// a rift instance, gated by [`RIFT_TOUCHED_MIN_FLOOR`]. The defs
// here roll with `ilvl_scale = 0.0` — magnitude scaling is done
// at the drop site by multiplying the rolled value by a
// floor-depth factor, so a deeper rift produces a stronger line
// without touching the ilvl axis (which is gated by drop ilvl).
//
// All six entries use existing `Stat` axes so no new compute
// path is required — the "rift-touched" identity comes from the
// dedicated slot, glyph, colour and depth-scaling, not from a
// parallel stat hierarchy.
//
// `min_ilvl: 1` because the floor gate is the effective limiter;
// `rarity_min: Common` because rift-touched stacks **on top of**
// whatever rarity the item rolled. The roll pipeline never picks
// from this pool — it's sampled exclusively by
// `drop_for_enemy` via [`roll_rift_touched`].
pub const RIFT_TOUCHED_POOL: &[AffixDef] = &[
    AffixDef {
        id: "rt_crit_chance",
        name_template: "{} Crit Chance",
        effect: AffixEffect::Stat(Stat::CritChance),
        roll: (0.03, 0.06),
        ilvl_scale: 0.0,
        tags: ALL,
        min_ilvl: 1,
        rarity_min: Rarity::Common,
        weight: 1,
    },
    AffixDef {
        id: "rt_elemental_resist",
        name_template: "{} Elemental Resist",
        effect: AffixEffect::Stat(Stat::ElementalResist),
        roll: (0.04, 0.08),
        ilvl_scale: 0.0,
        tags: ALL,
        min_ilvl: 1,
        rarity_min: Rarity::Common,
        weight: 1,
    },
    AffixDef {
        id: "rt_cooldown_reduction",
        name_template: "{} Cooldown Reduction",
        effect: AffixEffect::Stat(Stat::CooldownReduction),
        roll: (0.03, 0.06),
        ilvl_scale: 0.0,
        tags: ALL,
        min_ilvl: 1,
        rarity_min: Rarity::Common,
        weight: 1,
    },
    AffixDef {
        id: "rt_move_speed",
        name_template: "{} Move Speed",
        effect: AffixEffect::Stat(Stat::MoveSpeed),
        roll: (0.03, 0.06),
        ilvl_scale: 0.0,
        tags: ALL,
        min_ilvl: 1,
        rarity_min: Rarity::Common,
        weight: 1,
    },
    AffixDef {
        id: "rt_resource_regen",
        name_template: "{} Essence Regen",
        effect: AffixEffect::Stat(Stat::ResourceRegen),
        roll: (0.05, 0.10),
        ilvl_scale: 0.0,
        tags: ALL,
        min_ilvl: 1,
        rarity_min: Rarity::Common,
        weight: 1,
    },
    AffixDef {
        id: "rt_range",
        name_template: "{} Range",
        effect: AffixEffect::Stat(Stat::Range),
        roll: (0.04, 0.08),
        ilvl_scale: 0.0,
        tags: ALL,
        min_ilvl: 1,
        rarity_min: Rarity::Common,
        weight: 1,
    },
];

/// Minimum rift floor at which a kill is eligible for a
/// rift-touched line. ITEMS.md §3 Phase 5 originally specified
/// floor 20 but the design contract is "easily configurable" —
/// for the first ship we set it to `1` (every rift kill is
/// eligible) so the feature gets exercised in playtests, with
/// the single named constant here as the obvious knob to push it
/// later. Hub kills (`floor_index == 0`) never qualify because
/// they're below this threshold by construction.
pub const RIFT_TOUCHED_MIN_FLOOR: u32 = 1;

/// Independent per-drop probability that a rift-floor kill
/// awards a rift-touched line. Kept low so the slot stays
/// special — most rift drops still come back with the usual
/// trio + bonus shape and the extra slot is the visible
/// "this came from deep" cue.
pub const RIFT_TOUCHED_CHANCE: f32 = 0.20;

/// Per-floor magnitude multiplier on top of the rift-touched
/// def's base roll range. A drop from floor
/// `RIFT_TOUCHED_MIN_FLOOR` gets `1.0×`, each additional floor
/// stacks `RIFT_TOUCHED_DEPTH_SCALE` on top — so a kill at floor
/// `MIN + 10` rolls with `1.0 + 10 * SCALE` applied to the rolled
/// value. Independent of ilvl scaling so deep rifts feel
/// meaningfully different from a level-matched hub drop.
pub const RIFT_TOUCHED_DEPTH_SCALE: f32 = 0.10;

/// Element targeted by a resonance affix; `None` if not in the
/// resonance pool or not an element axis.
pub fn resonance_element(def: &AffixDef) -> Option<families::Element> {
    if !is_resonance(def) {
        return None;
    }
    affix_element(def)
}

/// Archetype targeted by a resonance affix.
pub fn resonance_archetype(def: &AffixDef) -> Option<families::Archetype> {
    if !is_resonance(def) {
        return None;
    }
    affix_archetype(def)
}

/// Attribute targeted by a resonance affix.
pub fn resonance_attribute(def: &AffixDef) -> Option<families::Attribute> {
    if !is_resonance(def) {
        return None;
    }
    affix_attribute(def)
}

/// Look up an affix by stable id. `O(n)` — used for save-game
/// rehydration, not hot paths. Searches both [`AFFIX_POOL`] and
/// [`RESONANCE_POOL`] so a persisted resonance line rehydrates
/// transparently.
pub fn lookup(id: &str) -> Option<&'static AffixDef> {
    AFFIX_POOL
        .iter()
        .chain(RESONANCE_POOL.iter())
        .chain(RIFT_TOUCHED_POOL.iter())
        .find(|a| a.id == id)
}

/// `true` if `def` lives in [`RESONANCE_POOL`]. Resonance lines
/// are identified by the `res_` id prefix — a convention the
/// const pool authors maintain, since `AffixDef` itself has no
/// data flag for it. Cheap byte-prefix check, no pointer
/// comparison, works for both freshly-rolled and persisted-then-
/// rehydrated items.
pub fn is_resonance(def: &AffixDef) -> bool {
    def.id.as_bytes().starts_with(b"res_")
}

/// `true` if `def` lives in [`RIFT_TOUCHED_POOL`]. Identified by
/// the `rt_` id prefix — same convention as resonance, no data
/// flag needed on `AffixDef`. Works post-rehydration because the
/// `&'static AffixDef` pointer always points back into one of
/// the static pools and the id is preserved.
pub fn is_rift_touched(def: &AffixDef) -> bool {
    def.id.as_bytes().starts_with(b"rt_")
}

/// `true` if `effect` is a "legendary" effect — i.e. one of the
/// gameplay-changing patterns (Transform, Trigger/Proc, extra
/// projectiles). The roll pipeline reserves these for the
/// dedicated legendary-effect slot on Legendary-rarity items.
pub fn is_legendary_effect(effect: &AffixEffect) -> bool {
    matches!(
        effect,
        AffixEffect::TransformAbility(_, _)
            | AffixEffect::Proc(_, _)
            | AffixEffect::ExtraProjectiles(_)
    )
}

// ---------------------------------------------------------------------
// Axis classification — Element × Archetype
// ---------------------------------------------------------------------
//
// Phase 2 of the itemisation refactor (see `ITEMS.md` §3 Phase 2)
// classifies every affix as one of three logical categories. The
// trio rolling pipeline reads these helpers to filter the
// `AFFIX_POOL` into per-axis sub-pools without growing the data
// table — the affixes themselves don't carry the category, it
// falls out of the `Stat` they wrap.
//
// The historical "Source" axis (WeaponDamage / SpellDamage) is
// retired — those stats overlap completely with Element and
// Archetype scaling. The future Attribute axis
// (Strength / Agility / Intellect) replaces it; for now the
// pipeline is an Element × Archetype duo.

use super::families;

/// Logical category of an affix, used by [`super::Item::roll`] to
/// split affixes into pipeline phases. Falls out of
/// `AffixDef::effect` plus the `res_` id-prefix convention —
/// affixes don't carry the category as data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AffixCategory {
    /// `Stat(PhysicalDamage | FireDamage | IceDamage | LightningDamage)`.
    Element,
    /// `Stat(ProjectileDamage | MeleeDamage)`. Beam / AoE damage
    /// stats are not gated on the archetype axis — they're
    /// covered by element scaling. Their `Stat` values are still
    /// applied at runtime by abilities; they just never roll as
    /// affixes.
    Archetype,
    /// `Stat(Strength | Agility | Intellect)` — flat attribute
    /// points. The third trio axis; family-locked to the base's
    /// declared attribute (or wildcard).
    Attribute,
    /// Anything in [`RESONANCE_POOL`] — a damage-axis line that
    /// **breaks the family lock by design**. Rolled in its own
    /// dedicated slot with `{Rare: 5 %, Legendary: 25 %}` chance.
    Resonance,
    /// Anything in [`RIFT_TOUCHED_POOL`] — a single extra slot
    /// awarded only on drops from inside a rift instance,
    /// gated by [`RIFT_TOUCHED_MIN_FLOOR`]. Magnitudes scale
    /// with floor depth, not ilvl. Survives extraction.
    RiftTouched,
    /// Everything else — defensives, crit, utility, ability mods,
    /// legendary effects. The bonus pool draws from here.
    Bonus,
}

/// Classify `def` for the trio pipeline. `is_legendary_effect`
/// still gates the legendary-effect slot independently; a
/// legendary effect is always `Bonus` by category since it never
/// appears on a damage-axis line. Resonance affixes are
/// classified by pool membership ([`is_resonance`]) regardless
/// of which `Stat` they wrap.
pub fn category(def: &AffixDef) -> AffixCategory {
    use crate::stats::Stat::*;
    if is_resonance(def) {
        return AffixCategory::Resonance;
    }
    if is_rift_touched(def) {
        return AffixCategory::RiftTouched;
    }
    match def.effect {
        AffixEffect::Stat(PhysicalDamage | FireDamage | IceDamage | LightningDamage) => {
            AffixCategory::Element
        }
        AffixEffect::Stat(ProjectileDamage | MeleeDamage) => AffixCategory::Archetype,
        AffixEffect::Stat(Strength | Agility | Intellect) => AffixCategory::Attribute,
        _ => AffixCategory::Bonus,
    }
}

/// Item-family `Attribute` an affix targets.
pub fn affix_attribute(def: &AffixDef) -> Option<families::Attribute> {
    use crate::stats::Stat::*;
    match def.effect {
        AffixEffect::Stat(Strength) => Some(families::Attribute::Strength),
        AffixEffect::Stat(Agility) => Some(families::Attribute::Agility),
        AffixEffect::Stat(Intellect) => Some(families::Attribute::Intellect),
        _ => None,
    }
}

/// Item-family `Element` an affix targets.
pub fn affix_element(def: &AffixDef) -> Option<families::Element> {
    use crate::stats::Stat::*;
    match def.effect {
        AffixEffect::Stat(PhysicalDamage) => Some(families::Element::Physical),
        AffixEffect::Stat(FireDamage) => Some(families::Element::Fire),
        AffixEffect::Stat(IceDamage) => Some(families::Element::Ice),
        AffixEffect::Stat(LightningDamage) => Some(families::Element::Lightning),
        _ => None,
    }
}

/// Item-family `Archetype` an affix targets. Beam / AoE damage
/// stats exist on the [`crate::stats::Stat`] enum (abilities still
/// apply them at runtime) but no affix rolls them — they're
/// covered by the element axis. Hence the narrow match.
pub fn affix_archetype(def: &AffixDef) -> Option<families::Archetype> {
    use crate::stats::Stat::*;
    match def.effect {
        AffixEffect::Stat(ProjectileDamage) => Some(families::Archetype::Projectile),
        AffixEffect::Stat(MeleeDamage) => Some(families::Archetype::Melee),
        _ => None,
    }
}

/// Number of leading affix entries on a fresh roll that come from
/// the slot's signature ([`signature_for`]). The first N entries
/// of [`super::Item::affixes`] are always signatures \u2014 reading
/// this is how tooltip rendering tells "guaranteed lines" from
/// "bonus rolls" without storing an extra flag per affix.
///
/// **Phase 2 update:** signatures no longer carry damage-axis
/// lines (those move to the trio pipeline). Each slot's signature
/// is now `Vitality + slot-defensive-stat`, with `Hands` keeping
/// the crit pair because crit identity is what gloves *mean* in
/// this game.
pub fn signature_count(slot: super::items::EquipSlot) -> usize {
    use super::items::EquipSlot::*;
    match slot {
        // CritChance + CritDamage — gloves are the crit slot in
        // this game; the pair stays as a signature.
        Hands => 2,
        // Slot defensive / utility line only.
        Helm | Shoulders | Chest | Legs | Boots => 1,
        // No signature lines — identity comes from the
        // family-locked Element × Archetype trio.
        Weapon | Ring1 | Ring2 | Amulet => 0,
    }
}

/// Deterministic per-slot affix ids that every item gets injected
/// regardless of rarity.
///
/// **Phase 2 update:** signatures are now slim and deterministic
/// — one slot-defensive / utility line (Hands keeps the crit
/// pair). The damage-axis trio (Element × Archetype) is rolled
/// separately by [`super::Item::roll`] from the family-locked
/// sub-pools. Weapon and accessories have no signature at all.
///
/// The `_rng` parameter is retained on the signature because future
/// signatures may randomise (e.g. a Helm rolling CDR vs.
/// MaxResource); none of today's signatures need it.
pub fn signature_for(
    slot: super::items::EquipSlot,
    _rng: &mut super::rng::LootRng,
) -> Vec<&'static str> {
    use super::items::EquipSlot::*;
    let mut out: Vec<&'static str> = Vec::with_capacity(2);
    match slot {
        Helm => out.push("pct_cooldown"),
        Shoulders => out.push("flat_armor"),
        Chest => out.push("flat_health"),
        Legs => out.push("flat_armor"),
        Hands => {
            out.push("pct_crit_chance");
            out.push("pct_crit_damage");
        }
        Boots => out.push("pct_move_speed"),
        Weapon | Ring1 | Ring2 | Amulet => {
            // No signature line — identity comes entirely from
            // the family-locked Element × Archetype trio.
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::items::EquipSlot;
    use super::super::rng::LootRng;
    use super::*;

    /// Every id returned by [`signature_for`] must resolve in
    /// [`AFFIX_POOL`]. A `None` return from [`lookup`] is silently
    /// dropped by `Item::roll` and produces an item missing its
    /// guaranteed slot line — exactly the regression we just fixed
    /// for Shoulders. Tests every slot, and (for Ring/Amulet) every
    /// random branch so a future content change can't sneak an
    /// unknown id past us.
    #[test]
    fn every_signature_id_resolves_in_pool() {
        for slot in EquipSlot::ALL {
            // Drive the slot through enough seeds that every
            // random branch (Ring/Amulet pick 1-of-4) is hit.
            for seed in 0..32u64 {
                let mut rng = LootRng::new(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
                let ids = signature_for(slot, &mut rng);
                for id in ids {
                    assert!(
                        lookup(id).is_some(),
                        "signature id `{id}` for slot {slot:?} (seed {seed}) \
                         not found in AFFIX_POOL — `Item::roll` would silently \
                         drop this line",
                    );
                }
            }
        }
    }

    /// Every id in every pool must be unique within its pool and
    /// across pools — the `lookup` chain assumes id-uniqueness so
    /// a duplicate would silently shadow.
    #[test]
    fn every_affix_id_is_globally_unique() {
        let mut seen: std::collections::HashSet<&'static str> = Default::default();
        for d in AFFIX_POOL
            .iter()
            .chain(RESONANCE_POOL.iter())
            .chain(RIFT_TOUCHED_POOL.iter())
        {
            assert!(
                seen.insert(d.id),
                "affix id `{}` appears more than once across AFFIX_POOL / RESONANCE_POOL / RIFT_TOUCHED_POOL",
                d.id
            );
        }
    }

    /// Resonance pool authoring contract: every id must use the
    /// `res_` prefix that [`is_resonance`] keys off, and must
    /// resolve via [`lookup`] so persistence round-trips work.
    #[test]
    fn resonance_pool_prefix_and_lookup() {
        for d in RESONANCE_POOL {
            assert!(
                d.id.starts_with("res_"),
                "resonance affix id `{}` is missing the `res_` prefix that \
                 `is_resonance` matches against — it would classify as `Bonus` \
                 and roll through the wrong slot",
                d.id
            );
            assert!(
                lookup(d.id).is_some(),
                "resonance affix id `{}` doesn't resolve via `lookup` — \
                 persisted resonance lines would fail to rehydrate",
                d.id
            );
        }
    }

    /// Rift-touched pool authoring contract: every id must use the
    /// `rt_` prefix that [`is_rift_touched`] keys off, must resolve
    /// via [`lookup`], and must keep `ilvl_scale = 0.0` so depth
    /// scaling at the drop site is the only magnitude knob.
    #[test]
    fn rift_touched_pool_contract() {
        assert!(
            !RIFT_TOUCHED_POOL.is_empty(),
            "RIFT_TOUCHED_POOL must contain at least one entry — \
             `roll_rift_touched` would return `None` for every kill"
        );
        for d in RIFT_TOUCHED_POOL {
            assert!(
                d.id.starts_with("rt_"),
                "rift-touched affix id `{}` is missing the `rt_` prefix",
                d.id
            );
            assert!(
                lookup(d.id).is_some(),
                "rift-touched affix id `{}` doesn't resolve via `lookup`",
                d.id
            );
            assert_eq!(
                d.ilvl_scale, 0.0,
                "rift-touched affix `{}` declares ilvl_scale={}; only \
                 floor-depth scaling at the drop site should drive magnitude",
                d.id, d.ilvl_scale
            );
        }
    }

    /// Every `AffixDef::name_template` must either contain exactly
    /// one `{}` placeholder (numeric effects) or none at all
    /// (`TransformAbility`). Anything else would mis-render at
    /// tooltip time.
    #[test]
    fn every_name_template_has_correct_placeholder_count() {
        for d in AFFIX_POOL
            .iter()
            .chain(RESONANCE_POOL.iter())
            .chain(RIFT_TOUCHED_POOL.iter())
        {
            let count = d.name_template.matches("{}").count();
            let expected = match d.effect {
                AffixEffect::TransformAbility(_, _) => 0,
                _ => 1,
            };
            assert_eq!(
                count, expected,
                "affix `{}` has {count} `{{}}` placeholder(s) in `name_template` \
                 but its effect wants {expected}",
                d.id
            );
        }
    }
}
