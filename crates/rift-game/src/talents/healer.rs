//! Healer route - battle priest and restoration lanes.

use crate::abilities::AbilityId;

use super::{
    node, stat_node, AbilityModifier, KeystoneId, PrerequisiteMode, Route, TalentEffect,
    TalentNode, TalentStat, TalentStatus,
};

const SMITE: AbilityId = AbilityId("smite");
const HOLY_NOVA: AbilityId = AbilityId("holy_nova");
const SAFEGUARD: AbilityId = AbilityId("safeguard");
const SANCTUARY_FIELD: AbilityId = AbilityId("sanctuary_field");

pub fn nodes() -> Vec<TalentNode> {
    use PrerequisiteMode::{All, Any};
    use TalentStat::*;
    use TalentStatus::*;

    vec![
        node(3000, "Mend", "Unlock Heal Target.", 1, Route::Healer, &[311], All, Ready, (0.0, 520.0), TalentEffect::UnlockAbility { ability: crate::abilities::HEAL_TARGET }),
        stat_node(3001, "Vitality", "+5% max HP per rank.", Route::Healer, MaxHp, 0.05, 3, &[3000], All, Ready, (-130.0, 650.0)),
        stat_node(3002, "Faith", "+5% healing/holy damage per rank.", Route::Healer, Damage, 0.05, 3, &[3000], All, Tuning, (130.0, 650.0)),
        node(3003, "Quick Mend", "Heal Target cooldown -0.5s.", 1, Route::Healer, &[3000], All, Ready, (0.0, 740.0), TalentEffect::AbilityMod { ability: crate::abilities::HEAL_TARGET, modifier: AbilityModifier::CooldownReduction(0.5) }),
        node(3010, "Smite", "Unlock Smite holy projectile/strike.", 1, Route::Healer, &[3002], All, NeedsAbility, (360.0, 820.0), TalentEffect::UnlockAbility { ability: SMITE }),
        node(3011, "Consecrated Force", "Smite +20% damage against damaged enemies.", 1, Route::Healer, &[3010], All, NeedsAbility, (700.0, 620.0), TalentEffect::AbilityMod { ability: SMITE, modifier: AbilityModifier::DamageBonus(0.20) }),
        node(3012, "Radiant Splash", "Smite splashes healing to you or lowest ally.", 1, Route::Healer, &[3010], All, NeedsSystem, (540.0, 900.0), TalentEffect::AbilityMod { ability: SMITE, modifier: AbilityModifier::DamageBonus(0.0) }),
        stat_node(3013, "Zeal", "+3% attack/cast speed per rank after healing.", Route::Healer, AttackSpeed, 0.03, 3, &[3012], All, NeedsSystem, (360.0, 1040.0)),
        node(3014, "Holy Nova", "Unlock point-blank holy AoE that damages enemies and heals allies.", 1, Route::Healer, &[3013], All, NeedsAbility, (360.0, 1220.0), TalentEffect::UnlockAbility { ability: HOLY_NOVA }),
        node(3015, "Blinding Light", "Holy Nova briefly blinds or disorients enemies.", 1, Route::Healer, &[3014], All, NeedsSystem, (540.0, 1320.0), TalentEffect::AbilityMod { ability: HOLY_NOVA, modifier: AbilityModifier::DamageBonus(0.0) }),
        node(3016, "Overflow", "Overhealing with Holy Nova becomes a short shield.", 1, Route::Healer, &[3014], All, NeedsSystem, (180.0, 1320.0), TalentEffect::AbilityMod { ability: HOLY_NOVA, modifier: AbilityModifier::DamageBonus(0.0) }),
        node(3017, "Battle Prayer", "Heals grant +10% damage for 4s.", 1, Route::Healer, &[3016], All, NeedsSystem, (360.0, 1480.0), TalentEffect::Keystone { keystone: KeystoneId::BattlePrayer }),
        stat_node(3060, "Tempered Zeal", "+3% holy damage per rank after healing.", Route::Healer, Damage, 0.03, 2, &[3017], All, NeedsSystem, (260.0, 1560.0)),
        node(3018, "Wrathful Benediction", "Your offensive holy spells also heal you, but direct heals cost longer cooldowns.", 1, Route::Healer, &[3060], All, NeedsSystem, (190.0, 1640.0), TalentEffect::Keystone { keystone: KeystoneId::Named("Wrathful Benediction") }),
        node(3019, "Martyr's Flame", "Taking damage charges your next heal or Smite.", 1, Route::Healer, &[3017], All, NeedsSystem, (530.0, 1640.0), TalentEffect::PassiveProc { description: "Taking damage charges your next heal or Smite.", chance: 1.0, per_rank: 0.0 }),
        node(3020, "Judgement Day", "Smite and Holy Nova deal bonus damage to enemies recently healed by your enemies or affected by shields.", 1, Route::Healer, &[3018], All, NeedsSystem, (190.0, 1820.0), TalentEffect::AbilityMod { ability: SMITE, modifier: AbilityModifier::DamageBonus(0.0) }),
        node(3030, "Regeneration", "Unlock Heal over Time.", 1, Route::Healer, &[3001], All, Ready, (-360.0, 820.0), TalentEffect::UnlockAbility { ability: crate::abilities::HEAL_OVER_TIME_TARGET }),
        node(3031, "Lingering Mend", "Heal over Time +15% effect.", 1, Route::Healer, &[3030], All, Ready, (-540.0, 750.0), TalentEffect::AbilityMod { ability: crate::abilities::HEAL_OVER_TIME_TARGET, modifier: AbilityModifier::DamageBonus(0.15) }),
        node(3032, "Steady Flow", "Heal over Time cooldown -0.5s.", 1, Route::Healer, &[3030], All, Ready, (-540.0, 900.0), TalentEffect::AbilityMod { ability: crate::abilities::HEAL_OVER_TIME_TARGET, modifier: AbilityModifier::CooldownReduction(0.5) }),
        node(3033, "Safeguard", "Unlock a targeted shield.", 1, Route::Healer, &[3031], All, NeedsAbility, (-360.0, 1040.0), TalentEffect::UnlockAbility { ability: SAFEGUARD }),
        node(3034, "Reinforced Ward", "Safeguard shield +20%.", 1, Route::Healer, &[3033], All, NeedsSystem, (-540.0, 1140.0), TalentEffect::AbilityMod { ability: SAFEGUARD, modifier: AbilityModifier::DamageBonus(0.20) }),
        node(3035, "Shared Shelter", "Safeguard grants a smaller shield to nearby allies.", 1, Route::Healer, &[3033], All, NeedsSystem, (-180.0, 1140.0), TalentEffect::AbilityMod { ability: SAFEGUARD, modifier: AbilityModifier::DamageBonus(0.0) }),
        node(3036, "Sanctuary Field", "Unlock ground zone that heals allies over time.", 1, Route::Healer, &[3035], All, NeedsAbility, (-360.0, 1320.0), TalentEffect::UnlockAbility { ability: SANCTUARY_FIELD }),
        node(3037, "Sanctuary", "Healed targets gain a small shield equal to 10% of the heal.", 1, Route::Healer, &[3036], All, NeedsSystem, (-360.0, 1480.0), TalentEffect::Keystone { keystone: KeystoneId::Sanctuary }),
        stat_node(3061, "Dawn Discipline", "+3% healing per rank on low-HP targets.", Route::Healer, MaxHp, 0.03, 2, &[3037], All, NeedsSystem, (-460.0, 1560.0)),
        node(3038, "Second Sunrise", "Once per floor, your heal prevents death on an ally or yourself.", 1, Route::Healer, &[3061], All, NeedsSystem, (-530.0, 1640.0), TalentEffect::Keystone { keystone: KeystoneId::Named("Second Sunrise") }),
        node(3039, "Warden's Circle", "Standing in your healing zone grants damage reduction.", 1, Route::Healer, &[3037], All, NeedsSystem, (-190.0, 1640.0), TalentEffect::PassiveProc { description: "Standing in your healing zone grants damage reduction.", chance: 1.0, per_rank: 0.0 }),
        node(3040, "Wellspring", "Sanctuary Field pulses faster on low-HP allies.", 1, Route::Healer, &[3038], All, NeedsSystem, (-530.0, 1820.0), TalentEffect::AbilityMod { ability: SANCTUARY_FIELD, modifier: AbilityModifier::CooldownReduction(0.0) }),
        node(3050, "Harmonic Prayer", "Healing after dealing damage, or dealing damage after healing, grants a small bonus.", 1, Route::Healer, &[3013, 3032], Any, NeedsSystem, (-220.0, 1400.0), TalentEffect::PassiveProc { description: "Alternating healing and damage grants a small bonus.", chance: 1.0, per_rank: 0.0 }),
        stat_node(3051, "Grace Under Fire", "+4% healing and +4% armor per rank.", Route::Healer, Defense, 0.04, 2, &[3050], All, Ready, (-40.0, 1600.0)),
    ]
}
