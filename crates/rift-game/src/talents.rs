//! Hunter talent tree — pure content. The engine owns `TalentTree`,
//! `TalentNode`, `TalentEffect`, and the bonus aggregation; this module
//! just declares the Hunter's tree layout. Other classes get their own
//! module here.

use rift_engine::combat::talent::{
    AbilityModifier, TalentEffect, TalentId, TalentNode, TalentStat, TalentTree,
};

use crate::abilities;

pub fn hunter_tree() -> TalentTree {
    let nodes = vec![
        // ─── Tier 0 (baseline) ──────────────────
        TalentNode {
            id: TalentId(0),
            name: "Sharp Eyes",
            description: "+2% crit chance per rank.",
            max_rank: 3,
            current_rank: 0,
            tier: 0,
            prerequisites: vec![],
            effect: TalentEffect::PercentBonus {
                stat: TalentStat::CritChance,
                per_rank: 0.02,
            },
        },
        TalentNode {
            id: TalentId(1),
            name: "Steady Aim",
            description: "+4% damage per rank.",
            max_rank: 3,
            current_rank: 0,
            tier: 0,
            prerequisites: vec![],
            effect: TalentEffect::PercentBonus {
                stat: TalentStat::Damage,
                per_rank: 0.04,
            },
        },
        TalentNode {
            id: TalentId(2),
            name: "Fleet Footed",
            description: "+3% movement speed per rank.",
            max_rank: 3,
            current_rank: 0,
            tier: 0,
            prerequisites: vec![],
            effect: TalentEffect::PercentBonus {
                stat: TalentStat::MoveSpeed,
                per_rank: 0.03,
            },
        },
        // ─── Tier 1 (5 points required) ──────────
        TalentNode {
            id: TalentId(3),
            name: "Deadly Draw",
            description: "+15% crit damage per rank.",
            max_rank: 3,
            current_rank: 0,
            tier: 1,
            prerequisites: vec![TalentId(0)],
            effect: TalentEffect::PercentBonus {
                stat: TalentStat::CritDamage,
                per_rank: 0.15,
            },
        },
        TalentNode {
            id: TalentId(4),
            name: "Quick Fingers",
            description: "+5% attack speed per rank.",
            max_rank: 3,
            current_rank: 0,
            tier: 1,
            prerequisites: vec![TalentId(1)],
            effect: TalentEffect::PercentBonus {
                stat: TalentStat::AttackSpeed,
                per_rank: 0.05,
            },
        },
        TalentNode {
            id: TalentId(5),
            name: "Long Range",
            description: "+10% projectile range per rank.",
            max_rank: 2,
            current_rank: 0,
            tier: 1,
            prerequisites: vec![TalentId(1)],
            effect: TalentEffect::PercentBonus {
                stat: TalentStat::Range,
                per_rank: 0.10,
            },
        },
        // ─── Tier 2 (10 points required) ─────────
        TalentNode {
            id: TalentId(6),
            name: "Multi-Shot Mastery",
            description: "Multi-Shot fires +1 arrow per rank.",
            max_rank: 2,
            current_rank: 0,
            tier: 2,
            prerequisites: vec![TalentId(4)],
            effect: TalentEffect::AbilityMod {
                ability: abilities::MULTI_SHOT,
                modifier: AbilityModifier::ExtraProjectiles(1),
            },
        },
        TalentNode {
            id: TalentId(7),
            name: "Piercing Arrows",
            description: "Arrows pierce through 1 additional target per rank.",
            max_rank: 2,
            current_rank: 0,
            tier: 2,
            prerequisites: vec![TalentId(3)],
            effect: TalentEffect::AbilityMod {
                ability: abilities::STEADY_SHOT,
                modifier: AbilityModifier::Pierce(1),
            },
        },
        TalentNode {
            id: TalentId(8),
            name: "Thick Skin",
            description: "+5% defense per rank.",
            max_rank: 3,
            current_rank: 0,
            tier: 2,
            prerequisites: vec![TalentId(2)],
            effect: TalentEffect::PercentBonus {
                stat: TalentStat::Defense,
                per_rank: 0.05,
            },
        },
        // ─── Tier 3 (15 points required) ─────────
        TalentNode {
            id: TalentId(9),
            name: "Rapid Fire Mastery",
            description: "Rapid Fire cooldown reduced by 2s per rank.",
            max_rank: 2,
            current_rank: 0,
            tier: 3,
            prerequisites: vec![TalentId(4), TalentId(6)],
            effect: TalentEffect::AbilityMod {
                ability: abilities::RAPID_FIRE,
                modifier: AbilityModifier::CooldownReduction(2.0),
            },
        },
        TalentNode {
            id: TalentId(10),
            name: "Death Mark",
            description: "Mark for Death also chains to 1 nearby enemy per rank.",
            max_rank: 2,
            current_rank: 0,
            tier: 3,
            prerequisites: vec![TalentId(7)],
            effect: TalentEffect::AbilityMod {
                ability: abilities::MARK_FOR_DEATH,
                modifier: AbilityModifier::Chain(1),
            },
        },
        // ─── Tier 4 (20 points required) ─────────
        TalentNode {
            id: TalentId(11),
            name: "Arrow Storm",
            description: "Rain of Arrows fires +4 arrows and deals +20% damage per rank.",
            max_rank: 1,
            current_rank: 0,
            tier: 4,
            prerequisites: vec![TalentId(9)],
            effect: TalentEffect::AbilityMod {
                ability: abilities::RAIN_OF_ARROWS,
                modifier: AbilityModifier::DamageBonus(0.2),
            },
        },
    ];

    TalentTree {
        nodes,
        unspent_points: 0,
        total_spent: 0,
    }
}
