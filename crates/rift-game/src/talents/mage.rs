//! Mage route — elemental projectiles and AoE (Fire / Ice /
//! Lightning).
//!
//! See `TALENT_TREE.md` §8.2.
//!
//! Reserved `TalentId` range: **2000 .. 2999**.

use super::hub::CONNECTOR_MAGE_TAIL;
use super::{AbilityModifier, KeystoneId, Route, TalentEffect, TalentId, TalentNode, TalentStat};

pub fn nodes() -> Vec<TalentNode> {
    let mut out = Vec::new();

    // ─── Ring 1: Fireball + its support cluster ──────────────────────
    out.push(TalentNode {
        id: TalentId(2000),
        name: "Fireball",
        description: "Unlock the Fireball ability.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Mage,
        prerequisites: vec![CONNECTOR_MAGE_TAIL],
        effect: TalentEffect::UnlockAbility {
            ability: crate::abilities::FIRE_BALL,
        },
    });
    // ─── Fireball support cluster ───
    out.push(TalentNode {
        id: TalentId(2010),
        name: "Fireball Volley",
        description: "Fireball fires +2 extra projectiles in a fan.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Mage,
        prerequisites: vec![TalentId(2000)],
        effect: TalentEffect::AbilityMod {
            ability: crate::abilities::FIRE_BALL,
            modifier: AbilityModifier::ExtraProjectiles(2),
        },
    });
    out.push(TalentNode {
        id: TalentId(2012),
        name: "Kindling",
        description: "Fireball deals +15% damage.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Mage,
        prerequisites: vec![TalentId(2000)],
        effect: TalentEffect::AbilityMod {
            ability: crate::abilities::FIRE_BALL,
            modifier: AbilityModifier::DamageBonus(0.15),
        },
    });

    // ─── Progression toward Frost Ray ────────────────────────────────
    out.push(stat(
        2001,
        "Intellect",
        "+5% damage per rank.",
        TalentStat::Damage,
        0.05,
        3,
        &[TalentId(2000)],
    ));
    out.push(stat(
        2002,
        "Arcane Focus",
        "+3% critical strike chance per rank.",
        TalentStat::CritChance,
        0.03,
        2,
        &[TalentId(2001)],
    ));

    // ─── Ring 2: Frost Ray + its support cluster ─────────────────────
    out.push(TalentNode {
        id: TalentId(2011),
        name: "Frost Ray",
        description: "Unlock the Frost Ray ability.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Mage,
        prerequisites: vec![TalentId(2002)],
        effect: TalentEffect::UnlockAbility {
            ability: crate::abilities::FROST_RAY,
        },
    });
    // ─── Frost Ray support cluster ───
    out.push(TalentNode {
        id: TalentId(2013),
        name: "Piercing Frost",
        description: "Frost Ray pierces +1 target.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Mage,
        prerequisites: vec![TalentId(2011)],
        effect: TalentEffect::AbilityMod {
            ability: crate::abilities::FROST_RAY,
            modifier: AbilityModifier::Pierce(1),
        },
    });
    out.push(TalentNode {
        id: TalentId(2014),
        name: "Glacial Edge",
        description: "Frost Ray deals +15% damage.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Mage,
        prerequisites: vec![TalentId(2011)],
        effect: TalentEffect::AbilityMod {
            ability: crate::abilities::FROST_RAY,
            modifier: AbilityModifier::DamageBonus(0.15),
        },
    });

    // ─── Progression toward Fire Wave ────────────────────────────────
    out.push(stat(
        2015,
        "Conduit",
        "+5% damage per rank.",
        TalentStat::Damage,
        0.05,
        2,
        &[TalentId(2011)],
    ));

    // ─── Ring 3: Fire Wave + its support cluster ─────────────────────
    out.push(TalentNode {
        id: TalentId(2020),
        name: "Fire Wave",
        description: "Unlock the Fire Wave ability.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Mage,
        prerequisites: vec![TalentId(2015)],
        effect: TalentEffect::UnlockAbility {
            ability: crate::abilities::FIRE_WAVE,
        },
    });
    out.push(TalentNode {
        id: TalentId(2016),
        name: "Wave Rider",
        description: "Fire Wave deals +15% damage.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Mage,
        prerequisites: vec![TalentId(2020)],
        effect: TalentEffect::AbilityMod {
            ability: crate::abilities::FIRE_WAVE,
            modifier: AbilityModifier::DamageBonus(0.15),
        },
    });

    // ─── Mid keystone ────────────────────────────────────────────────
    out.push(TalentNode {
        id: TalentId(2021),
        name: "Burning Crits",
        description: "Critical strikes apply Burn (3s DoT).",
        max_rank: 1,
        current_rank: 0,
        route: Route::Mage,
        prerequisites: vec![TalentId(2012)],
        effect: TalentEffect::Keystone {
            keystone: KeystoneId::BurningCrits,
        },
    });

    // ─── Ring 4 ────────────────────────────────────────────────────────
    out.push(TalentNode {
        id: TalentId(2030),
        name: "Beam Conduit",
        description: "Fireball becomes the Fireball Beam variant.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Mage,
        prerequisites: vec![TalentId(2020)],
        effect: TalentEffect::Keystone {
            keystone: KeystoneId::BeamConduit,
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
        route: Route::Mage,
        prerequisites: prerequisites.to_vec(),
        effect: TalentEffect::PercentBonus { stat, per_rank },
    }
}
