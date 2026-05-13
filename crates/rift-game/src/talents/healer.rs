//! Healer route — heals, buffs, support utility.
//!
//! See `TALENT_TREE.md` §8.3.
//!
//! Reserved `TalentId` range: **3000 .. 3999**.

use super::hub::CONNECTOR_HEALER_TAIL;
use super::{AbilityModifier, KeystoneId, Route, TalentEffect, TalentId, TalentNode, TalentStat};

pub fn nodes() -> Vec<TalentNode> {
    let mut out = Vec::new();

    // ─── Ring 1: Mend + its support cluster ──────────────────────────
    out.push(TalentNode {
        id: TalentId(3000),
        name: "Mend",
        description: "Unlock the Heal Target ability.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Healer,
        prerequisites: vec![CONNECTOR_HEALER_TAIL],
        effect: TalentEffect::UnlockAbility {
            ability: crate::abilities::HEAL_TARGET,
        },
    });
    // "Empowered Healing": §8.3 lists "+20% heal effectiveness".
    // No heal-amount `AbilityModifier` variant exists yet, so
    // this ships as +20 % "damage" (which the healing pipeline
    // applies as the heal magnitude scalar — they share the
    // same number axis). Replace with a dedicated `HealBonus`
    // modifier when the enum grows one.
    out.push(TalentNode {
        id: TalentId(3011),
        name: "Empowered Healing",
        description: "Heal Target restores +20% more health.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Healer,
        prerequisites: vec![TalentId(3000)],
        effect: TalentEffect::AbilityMod {
            ability: crate::abilities::HEAL_TARGET,
            modifier: AbilityModifier::DamageBonus(0.20),
        },
    });
    out.push(TalentNode {
        id: TalentId(3012),
        name: "Quick Mend",
        description: "Heal Target cooldown reduced by 0.5s.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Healer,
        prerequisites: vec![TalentId(3000)],
        effect: TalentEffect::AbilityMod {
            ability: crate::abilities::HEAL_TARGET,
            modifier: AbilityModifier::CooldownReduction(0.5),
        },
    });

    // ─── Progression toward Regeneration ─────────────────────────────
    out.push(stat(
        3001,
        "Vitality",
        "+5% maximum health per rank.",
        TalentStat::MaxHp,
        0.05,
        3,
        &[TalentId(3000)],
    ));
    out.push(stat(
        3002,
        "Faith",
        "+5% damage per rank (boosts heal magnitude).",
        TalentStat::Damage,
        0.05,
        2,
        &[TalentId(3001)],
    ));

    // ─── Ring 2: Regeneration + its support cluster ──────────────────
    out.push(TalentNode {
        id: TalentId(3010),
        name: "Regeneration",
        description: "Unlock the Heal over Time ability.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Healer,
        prerequisites: vec![TalentId(3002)],
        effect: TalentEffect::UnlockAbility {
            ability: crate::abilities::HEAL_OVER_TIME_TARGET,
        },
    });
    out.push(TalentNode {
        id: TalentId(3013),
        name: "Lingering Mend",
        description: "Heal over Time deals +15% effect.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Healer,
        prerequisites: vec![TalentId(3010)],
        effect: TalentEffect::AbilityMod {
            ability: crate::abilities::HEAL_OVER_TIME_TARGET,
            modifier: AbilityModifier::DamageBonus(0.15),
        },
    });
    out.push(TalentNode {
        id: TalentId(3014),
        name: "Steady Flow",
        description: "Heal over Time cooldown reduced by 0.5s.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Healer,
        prerequisites: vec![TalentId(3010)],
        effect: TalentEffect::AbilityMod {
            ability: crate::abilities::HEAL_OVER_TIME_TARGET,
            modifier: AbilityModifier::CooldownReduction(0.5),
        },
    });

    // ─── Progression toward Battle Prayer ────────────────────────────
    out.push(stat(
        3015,
        "Devotion",
        "+5% maximum health per rank.",
        TalentStat::MaxHp,
        0.05,
        2,
        &[TalentId(3010)],
    ));

    // ─── Ring 3 ────────────────────────────────────────────────────────
    out.push(TalentNode {
        id: TalentId(3020),
        name: "Battle Prayer",
        description: "Your heals also grant +10% damage for 4s.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Healer,
        prerequisites: vec![TalentId(3015)],
        effect: TalentEffect::Keystone {
            keystone: KeystoneId::BattlePrayer,
        },
    });

    // ─── Ring 4 ────────────────────────────────────────────────────────
    out.push(TalentNode {
        id: TalentId(3030),
        name: "Sanctuary",
        description: "Healed targets gain a small shield (10% of heal).",
        max_rank: 1,
        current_rank: 0,
        route: Route::Healer,
        prerequisites: vec![TalentId(3020)],
        effect: TalentEffect::Keystone {
            keystone: KeystoneId::Sanctuary,
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
        route: Route::Healer,
        prerequisites: prerequisites.to_vec(),
        effect: TalentEffect::PercentBonus { stat, per_rank },
    }
}
