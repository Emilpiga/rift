//! Warrior route — melee, sword swings, charges, parries.
//!
//! See `TALENT_TREE.md` §8.1.
//!
//! Reserved `TalentId` range: **1000 .. 1999**.
//!
//! The §8.1 sample lists a "Charge" ability that hasn't been
//! authored yet (`AbilityId` would be dangling); we ship the
//! route without it for now. Keystones reference variants already
//! reserved in [`super::KeystoneId`].

use super::hub::CONNECTOR_WARRIOR_TAIL;
use super::{AbilityModifier, KeystoneId, Route, TalentEffect, TalentId, TalentNode, TalentStat};

pub fn nodes() -> Vec<TalentNode> {
    let mut out = Vec::new();

    // ─── Ring 1: Sword Slash + its support cluster ────────────────────
    // Entry ability — every other warrior pick walks through it.
    // Its support cluster (Heavy Strikes, Reach) hangs directly
    // off it so the UI can lay them out as satellites; one
    // generic progression stat (Toughness) hangs off it as the
    // gateway to Whirlwind.
    out.push(TalentNode {
        id: TalentId(1000),
        name: "Sword Slash",
        description: "Unlock the Melee Attack ability.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Warrior,
        prerequisites: vec![CONNECTOR_WARRIOR_TAIL],
        effect: TalentEffect::UnlockAbility {
            ability: crate::abilities::MELEE_ATTACK,
        },
    });
    // ─── Sword Slash support cluster ───
    out.push(TalentNode {
        id: TalentId(1011),
        name: "Heavy Strikes",
        description: "Melee Attack deals +15% damage.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Warrior,
        prerequisites: vec![TalentId(1000)],
        effect: TalentEffect::AbilityMod {
            ability: crate::abilities::MELEE_ATTACK,
            modifier: AbilityModifier::DamageBonus(0.15),
        },
    });
    // "Reach": §8.1 lists "MELEE_ATTACK arc radius +10%". No
    // arc-radius `AbilityModifier` variant exists yet, so this
    // ships as a +10 % melee damage bonus on the same ability —
    // a tuning placeholder that still scales the same build
    // axis. Replace with a proper arc modifier when the
    // `AbilityModifier` enum grows one.
    out.push(TalentNode {
        id: TalentId(1012),
        name: "Reach",
        description: "Melee Attack deals +10% damage.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Warrior,
        prerequisites: vec![TalentId(1000)],
        effect: TalentEffect::AbilityMod {
            ability: crate::abilities::MELEE_ATTACK,
            modifier: AbilityModifier::DamageBonus(0.10),
        },
    });

    // ─── Progression toward Whirlwind ────────────────────────────────
    // Two small stat nodes the player must traverse before
    // Whirlwind opens up — the §8.1 sketch had Whirlwind hang
    // directly off the entry ability which made the spine read
    // as "ability → ability" with no investment between.
    out.push(stat(
        1001,
        "Toughness",
        "+5% maximum health per rank.",
        TalentStat::MaxHp,
        0.05,
        3,
        &[TalentId(1000)],
    ));
    out.push(stat(
        1002,
        "Conditioning",
        "+5% damage per rank.",
        TalentStat::Damage,
        0.05,
        2,
        &[TalentId(1001)],
    ));

    // ─── Ring 2: Whirlwind + its support cluster ──────────────────────
    out.push(TalentNode {
        id: TalentId(1010),
        name: "Whirlwind",
        description: "Unlock the Whirlwind ability.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Warrior,
        prerequisites: vec![TalentId(1002)],
        effect: TalentEffect::UnlockAbility {
            ability: crate::abilities::WHIRLWIND,
        },
    });
    // ─── Whirlwind support cluster ───
    out.push(TalentNode {
        id: TalentId(1013),
        name: "Wider Spin",
        description: "Whirlwind deals +15% damage.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Warrior,
        prerequisites: vec![TalentId(1010)],
        effect: TalentEffect::AbilityMod {
            ability: crate::abilities::WHIRLWIND,
            modifier: AbilityModifier::DamageBonus(0.15),
        },
    });
    out.push(TalentNode {
        id: TalentId(1014),
        name: "Endless Rotation",
        description: "Whirlwind cooldown reduced by 0.5s.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Warrior,
        prerequisites: vec![TalentId(1010)],
        effect: TalentEffect::AbilityMod {
            ability: crate::abilities::WHIRLWIND,
            modifier: AbilityModifier::CooldownReduction(0.5),
        },
    });

    // ─── Progression toward Berserker ────────────────────────────────
    out.push(stat(
        1015,
        "War Trance",
        "+5% damage per rank.",
        TalentStat::Damage,
        0.05,
        2,
        &[TalentId(1010)],
    ));

    // ─── Ring 3 ────────────────────────────────────────────────────────
    out.push(TalentNode {
        id: TalentId(1020),
        name: "Berserker",
        description: "Below 50% HP: +30% melee damage, −15% defense.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Warrior,
        prerequisites: vec![TalentId(1015)],
        effect: TalentEffect::Keystone {
            keystone: KeystoneId::Berserker,
        },
    });

    // ─── Ring 4 ────────────────────────────────────────────────────────
    out.push(TalentNode {
        id: TalentId(1030),
        name: "Executioner",
        description: "Melee crits below 30% HP execute the target.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Warrior,
        prerequisites: vec![TalentId(1020)],
        effect: TalentEffect::Keystone {
            keystone: KeystoneId::Executioner,
        },
    });

    out
}

fn stat(
    id: u16,
    name: &'static str,
    description: &'static str,
    stat: TalentStat,
    per_rank: f32,
    max_rank: u8,
    prerequisites: &[TalentId],
) -> TalentNode {
    TalentNode {
        id: TalentId(id),
        name,
        description,
        max_rank,
        current_rank: 0,
        route: Route::Warrior,
        prerequisites: prerequisites.to_vec(),
        effect: TalentEffect::PercentBonus { stat, per_rank },
    }
}
