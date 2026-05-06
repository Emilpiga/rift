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
    CastAbility(AbilityId),
    /// Spawn a one-shot AoE explosion of `radius` doing `damage`.
    Explosion { radius: f32, damage: f32 },
    /// Chain a small bolt to up to `max_targets` nearby enemies.
    ChainLightning { max_targets: u32, damage: f32 },
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
    // ════════ Common-tier: pure stats ═══════════════════════════════
    AffixDef {
        id: "flat_power",
        name_template: "{} Power",
        effect: AffixEffect::Stat(Stat::Power),
        roll: (3.0, 6.0),
        ilvl_scale: 1.5,
        tags: MELEE | CASTER,
        min_ilvl: 1,
        rarity_min: Rarity::Common,
        weight: 100,
    },
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
        name_template: "{} Resource Regen",
        effect: AffixEffect::Stat(Stat::ResourceRegen),
        roll: (0.05, 0.12),
        ilvl_scale: 0.004,
        tags: UTILITY | CASTER,
        min_ilvl: 1,
        rarity_min: Rarity::Common,
        weight: 50,
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
        id: "mod_multi_shot_extra_proj",
        name_template: "Multi-Shot fires {} extra arrows",
        effect: AffixEffect::ExtraProjectiles(ab::MULTI_SHOT),
        roll: (1.0, 2.0),
        ilvl_scale: 0.0,
        tags: SPEED | CRIT,
        min_ilvl: 10,
        rarity_min: Rarity::Legendary,
        weight: 15,
    },
    AffixDef {
        id: "mod_steady_shot_extra_proj",
        name_template: "Steady Shot fires {} extra arrows",
        effect: AffixEffect::ExtraProjectiles(ab::STEADY_SHOT),
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
        effect: AffixEffect::TransformAbility(
            ab::FROST_RAY,
            AbilityVariant::FrostRayShatter,
        ),
        roll: (0.0, 0.0),
        ilvl_scale: 0.0,
        tags: ICE | CASTER,
        min_ilvl: 15,
        rarity_min: Rarity::Legendary,
        weight: 8,
    },
    AffixDef {
        id: "transform_whirlwind_vortex",
        name_template: "Whirlwind pulls enemies inward",
        effect: AffixEffect::TransformAbility(
            ab::WHIRLWIND,
            AbilityVariant::WhirlwindVortex,
        ),
        roll: (0.0, 0.0),
        ilvl_scale: 0.0,
        tags: MELEE,
        min_ilvl: 15,
        rarity_min: Rarity::Legendary,
        weight: 8,
    },
    AffixDef {
        id: "proc_oncrit_explosion",
        name_template: "{} chance on crit to detonate",
        effect: AffixEffect::Proc(
            ProcEvent::OnCrit,
            ProcAction::Explosion {
                radius: 2.5,
                damage: 12.0,
            },
        ),
        roll: (0.10, 0.25),
        ilvl_scale: 0.005,
        tags: FIRE | CRIT,
        min_ilvl: 10,
        rarity_min: Rarity::Legendary,
        weight: 12,
    },
    AffixDef {
        id: "proc_onhit_chain",
        name_template: "{} chance on hit to chain lightning",
        effect: AffixEffect::Proc(
            ProcEvent::OnHit,
            ProcAction::ChainLightning {
                max_targets: 3,
                damage: 8.0,
            },
        ),
        roll: (0.05, 0.12),
        ilvl_scale: 0.002,
        tags: LIGHTNING,
        min_ilvl: 10,
        rarity_min: Rarity::Legendary,
        weight: 12,
    },
    AffixDef {
        id: "proc_ondodge_ray",
        name_template: "{} chance on dodge to free-cast Frost Ray",
        effect: AffixEffect::Proc(
            ProcEvent::OnDodge,
            ProcAction::CastAbility(ab::FROST_RAY),
        ),
        roll: (0.10, 0.20),
        ilvl_scale: 0.005,
        tags: ICE | SPEED,
        min_ilvl: 15,
        rarity_min: Rarity::Legendary,
        weight: 8,
    },
];

/// Look up an affix by stable id. `O(n)` — used for save-game
/// rehydration, not hot paths.
pub fn lookup(id: &str) -> Option<&'static AffixDef> {
    AFFIX_POOL.iter().find(|a| a.id == id)
}
