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
    // ════════ Slot-signature affixes ════════════════════════════════
    //
    // These are the deterministic lines injected by
    // `Item::roll` based on `EquipSlot`. They're full members of
    // the pool (so save/load + tooltip rendering work uniformly)
    // but normally never roll as bonuses because every item slot
    // already carries them. The bonus-roll path filters them
    // out by `id` in `Item::roll`.
    AffixDef {
        id: "flat_vitality",
        name_template: "{} Vitality",
        effect: AffixEffect::Stat(Stat::Vitality),
        roll: (8.0, 16.0),
        ilvl_scale: 3.0,
        tags: ALL,
        min_ilvl: 1,
        rarity_min: Rarity::Common,
        weight: 0,
    },
    AffixDef {
        id: "pct_weapon_damage",
        name_template: "{} Weapon Damage",
        effect: AffixEffect::Stat(Stat::WeaponDamage),
        roll: (0.05, 0.12),
        ilvl_scale: 0.004,
        tags: ALL,
        min_ilvl: 1,
        rarity_min: Rarity::Common,
        weight: 0,
    },
    AffixDef {
        id: "pct_spell_damage",
        name_template: "{} Spell Damage",
        effect: AffixEffect::Stat(Stat::SpellDamage),
        roll: (0.05, 0.12),
        ilvl_scale: 0.004,
        tags: ALL,
        min_ilvl: 1,
        rarity_min: Rarity::Common,
        weight: 0,
    },
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
        id: "pct_beam_damage",
        name_template: "{} Beam Damage",
        effect: AffixEffect::Stat(Stat::BeamDamage),
        roll: (0.06, 0.14),
        ilvl_scale: 0.005,
        tags: ALL,
        min_ilvl: 1,
        rarity_min: Rarity::Common,
        weight: 0,
    },
    AffixDef {
        id: "pct_aoe_damage",
        name_template: "{} AoE Damage",
        effect: AffixEffect::Stat(Stat::AoeDamage),
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

/// Look up an affix by stable id. `O(n)` — used for save-game
/// rehydration, not hot paths.
pub fn lookup(id: &str) -> Option<&'static AffixDef> {
    AFFIX_POOL.iter().find(|a| a.id == id)
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

/// Number of leading affix entries on a fresh roll that come from
/// the slot's signature ([`signature_for`]). The first N entries
/// of [`super::Item::affixes`] are always signatures \u2014 reading
/// this is how tooltip rendering tells "guaranteed lines" from
/// "bonus rolls" without storing an extra flag per affix.
pub fn signature_count(slot: super::items::EquipSlot) -> usize {
    use super::items::EquipSlot::*;
    match slot {
        // Vitality + WeaponDamage + SpellDamage
        Weapon => 3,
        // Vitality + CritChance + CritDamage
        Hands => 3,
        // Vitality + slot-specific defensive / utility line
        Helm | Shoulders | Chest | Legs | Boots => 2,
        // Vitality + one random element / archetype line
        Ring1 | Ring2 | Amulet => 2,
    }
}

/// Deterministic per-slot affix ids that every item gets injected
/// regardless of rarity. Returned as a small `Vec` because some
/// slots (Ring/Amulet) randomise the *kind* of element / archetype
/// while still always producing exactly one such line.
///
/// Vitality is always slot 0 — read top-down on every tooltip the
/// player sees `+N Vitality` first, then the slot signature.
pub fn signature_for(
    slot: super::items::EquipSlot,
    rng: &mut super::rng::LootRng,
) -> Vec<&'static str> {
    use super::items::EquipSlot::*;
    let mut out: Vec<&'static str> = Vec::with_capacity(3);
    out.push("flat_vitality");
    match slot {
        Weapon => {
            out.push("pct_weapon_damage");
            out.push("pct_spell_damage");
        }
        Helm => out.push("pct_cooldown"),
        Shoulders => out.push("pct_armor"),
        Chest => out.push("flat_health"),
        Legs => out.push("flat_armor"),
        Hands => {
            out.push("pct_crit_chance");
            out.push("pct_crit_damage");
        }
        Boots => out.push("pct_move_speed"),
        Ring1 | Ring2 => {
            // One random elemental damage line per ring. Each
            // ring rolls independently, so a player can stack
            // matching elements or hedge across two.
            const ELEMENTS: [&str; 4] = [
                "pct_fire_damage",
                "pct_ice_damage",
                "pct_lightning_damage",
                "pct_physical_damage",
            ];
            let pick = rng.range(0, ELEMENTS.len() as u32) as usize;
            out.push(ELEMENTS[pick]);
        }
        Amulet => {
            // One random archetype damage line. Maps to which
            // ability shape (projectile / beam / AoE / melee)
            // benefits.
            const ARCHETYPES: [&str; 4] = [
                "pct_projectile_damage",
                "pct_beam_damage",
                "pct_aoe_damage",
                "pct_melee_damage",
            ];
            let pick = rng.range(0, ARCHETYPES.len() as u32) as usize;
            out.push(ARCHETYPES[pick]);
        }
    }
    out
}
