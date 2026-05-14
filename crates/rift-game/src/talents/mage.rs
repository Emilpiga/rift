//! Mage route - pyromancer and cryomancer lanes.

use crate::abilities::AbilityId;

use super::{
    node, stat_node, AbilityModifier, KeystoneId, PrerequisiteMode, Route, TalentEffect,
    TalentNode, TalentStat, TalentStatus,
};

const ICE_LANCE: AbilityId = AbilityId("ice_lance");

pub fn nodes() -> Vec<TalentNode> {
    use PrerequisiteMode::{All, Any};
    use TalentStat::*;
    use TalentStatus::*;

    vec![
        node(2000, "Fireball", "Unlock Fireball.", 1, Route::Mage, &[211], All, Ready, (0.0, -520.0), TalentEffect::UnlockAbility { ability: crate::abilities::FIRE_BALL }),
        stat_node(2001, "Intellect", "+5% spell damage per rank.", Route::Mage, Damage, 0.05, 3, &[2000], All, Ready, (-130.0, -650.0)),
        stat_node(2002, "Arcane Focus", "+3% crit chance per rank.", Route::Mage, CritChance, 0.03, 2, &[2000], All, Ready, (130.0, -650.0)),
        stat_node(2003, "Quick Casting", "+2% cooldown reduction per rank.", Route::Mage, CooldownReduction, 0.02, 3, &[2000], All, Ready, (0.0, -740.0)),
        node(2010, "Fireball Volley", "Fireball fires +2 projectiles.", 1, Route::Mage, &[2000], All, Ready, (-320.0, -560.0), TalentEffect::AbilityMod { ability: crate::abilities::FIRE_BALL, modifier: AbilityModifier::ExtraProjectiles(2) }),
        node(2011, "Kindling", "Fireball +15% damage.", 1, Route::Mage, &[2000], All, Ready, (-520.0, -780.0), TalentEffect::AbilityMod { ability: crate::abilities::FIRE_BALL, modifier: AbilityModifier::DamageBonus(0.15) }),
        node(2012, "Ignite", "Spell crits have a chance to apply Burn.", 1, Route::Mage, &[2011], All, NeedsSystem, (-620.0, -920.0), TalentEffect::PassiveProc { description: "Spell crits have a chance to apply Burn.", chance: 0.2, per_rank: 0.0 }),
        node(2013, "Fire Wave", "Unlock Fire Wave.", 1, Route::Mage, &[2012], All, Ready, (-500.0, -1080.0), TalentEffect::UnlockAbility { ability: crate::abilities::FIRE_WAVE }),
        node(2014, "Wave Rider", "Fire Wave +15% damage.", 1, Route::Mage, &[2013], All, Ready, (-670.0, -1160.0), TalentEffect::AbilityMod { ability: crate::abilities::FIRE_WAVE, modifier: AbilityModifier::DamageBonus(0.15) }),
        node(2015, "Backdraft", "Fire Wave pulls burning enemies slightly inward.", 1, Route::Mage, &[2013], All, NeedsSystem, (-330.0, -1160.0), TalentEffect::AbilityMod { ability: crate::abilities::FIRE_WAVE, modifier: AbilityModifier::DamageBonus(0.0) }),
        node(2016, "Combustion", "Burning enemies explode on death for area fire damage.", 1, Route::Mage, &[2015], All, NeedsSystem, (-500.0, -1280.0), TalentEffect::PassiveProc { description: "Burning enemies explode on death for area fire damage.", chance: 1.0, per_rank: 0.0 }),
        node(2017, "Burning Crits", "Crits apply Burn.", 1, Route::Mage, &[2016], All, NeedsSystem, (-500.0, -1460.0), TalentEffect::Keystone { keystone: KeystoneId::BurningCrits }),
        stat_node(2060, "Cinder Theory", "+3% fire damage per rank against burning enemies.", Route::Mage, Damage, 0.03, 2, &[2017], All, NeedsSystem, (-620.0, -1540.0)),
        node(2018, "Inferno Heart", "Burning enemies take ramping damage, but your non-fire damage is reduced.", 1, Route::Mage, &[2060], All, NeedsSystem, (-670.0, -1620.0), TalentEffect::Keystone { keystone: KeystoneId::Named("Inferno Heart") }),
        stat_node(2063, "Banked Ember", "+2% cooldown reduction per rank after a fire crit.", Route::Mage, CooldownReduction, 0.02, 2, &[2017], All, NeedsSystem, (-380.0, -1540.0)),
        node(2019, "Phoenix Spark", "Once per floor, lethal damage detonates nearby enemies and restores some HP.", 1, Route::Mage, &[2063], All, NeedsSystem, (-330.0, -1620.0), TalentEffect::PassiveProc { description: "Once per floor cheat death with a fire detonation and partial heal.", chance: 1.0, per_rank: 0.0 }),
        node(2030, "Frost Ray", "Unlock Frost Ray.", 1, Route::Mage, &[2002], All, Ready, (360.0, -820.0), TalentEffect::UnlockAbility { ability: crate::abilities::FROST_RAY }),
        node(2031, "Piercing Frost", "Frost Ray pierces +1 target.", 1, Route::Mage, &[2030], All, Ready, (540.0, -890.0), TalentEffect::AbilityMod { ability: crate::abilities::FROST_RAY, modifier: AbilityModifier::Pierce(1) }),
        node(2032, "Glacial Edge", "Frost Ray +15% damage.", 1, Route::Mage, &[2030], All, Ready, (700.0, -620.0), TalentEffect::AbilityMod { ability: crate::abilities::FROST_RAY, modifier: AbilityModifier::DamageBonus(0.15) }),
        node(2033, "Chill", "Frost hits slow enemies. Starts lightweight; data should leave room for Brittle and Freeze later.", 1, Route::Mage, &[2030], All, NeedsSystem, (360.0, -980.0), TalentEffect::PassiveProc { description: "Frost hits apply a lightweight slow/debuff.", chance: 1.0, per_rank: 0.0 }),
        node(2034, "Ice Lance", "Unlock Ice Lance single-target projectile.", 1, Route::Mage, &[2033], All, NeedsAbility, (360.0, -1140.0), TalentEffect::UnlockAbility { ability: ICE_LANCE }),
        node(2035, "Splinter", "Ice Lance splits on chilled targets.", 1, Route::Mage, &[2034], All, NeedsSystem, (540.0, -1230.0), TalentEffect::AbilityMod { ability: ICE_LANCE, modifier: AbilityModifier::ExtraProjectiles(1) }),
        node(2036, "Deep Freeze", "Ice Lance can freeze brittle enemies.", 1, Route::Mage, &[2034], All, NeedsSystem, (180.0, -1230.0), TalentEffect::AbilityMod { ability: ICE_LANCE, modifier: AbilityModifier::DamageBonus(0.0) }),
        node(2037, "Brittle", "Repeated cold hits apply Brittle; next heavy hit shatters.", 1, Route::Mage, &[2036], All, NeedsSystem, (360.0, -1380.0), TalentEffect::PassiveProc { description: "Repeated cold hits apply Brittle; next heavy hit shatters.", chance: 1.0, per_rank: 0.0 }),
        node(2038, "Absolute Zero", "Frozen enemies take greatly increased damage.", 1, Route::Mage, &[2037], All, NeedsSystem, (360.0, -1540.0), TalentEffect::Keystone { keystone: KeystoneId::Named("Absolute Zero") }),
        stat_node(2061, "Fracture Study", "+3% crit damage per rank against chilled enemies.", Route::Mage, CritDamage, 0.03, 2, &[2038], All, NeedsSystem, (240.0, -1620.0)),
        node(2039, "Shatterstorm", "Shattering an enemy launches ice shards at nearby enemies.", 1, Route::Mage, &[2061], All, NeedsSystem, (190.0, -1700.0), TalentEffect::Keystone { keystone: KeystoneId::Named("Shatterstorm") }),
        stat_node(2062, "Crystal Lens", "+2% projectile speed per rank for cold spells.", Route::Mage, ProjectileSpeed, 0.02, 2, &[2038], All, NeedsSystem, (480.0, -1620.0)),
        node(2040, "Beam Conduit", "Fireball can become a beam-style spell if no Fireball projectile modifiers are active.", 1, Route::Mage, &[2062], All, NeedsSystem, (530.0, -1700.0), TalentEffect::Keystone { keystone: KeystoneId::BeamConduit }),
        node(2050, "Thermal Shock", "Fire damage against chilled enemies, or frost damage against burning enemies, deals bonus damage.", 1, Route::Mage, &[2016, 2037], Any, NeedsSystem, (0.0, -1380.0), TalentEffect::PassiveProc { description: "Opposed elemental hits deal bonus damage.", chance: 1.0, per_rank: 0.0 }),
        stat_node(2051, "Elemental Savant", "+4% fire and frost damage per rank.", Route::Mage, Damage, 0.04, 2, &[2050], All, Ready, (0.0, -1540.0)),
        node(2052, "Unstable Elements", "Alternating fire and frost spells grants stacking damage, but repeating the same element consumes the stacks.", 1, Route::Mage, &[2051], All, NeedsSystem, (0.0, -1720.0), TalentEffect::Keystone { keystone: KeystoneId::Named("Unstable Elements") }),
    ]
}
