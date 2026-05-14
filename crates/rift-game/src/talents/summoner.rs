//! Summoner route - void summons and necromancy.

use crate::abilities::AbilityId;

use super::{
    node, stat_node, AbilityModifier, KeystoneId, PrerequisiteMode, Route, TalentEffect,
    TalentNode, TalentStat, TalentStatus,
};

const VOID_FAMILIAR: AbilityId = AbilityId("void_familiar");
const RIFTLING_SWARM: AbilityId = AbilityId("riftling_swarm");
const VOID_GATE: AbilityId = AbilityId("void_gate");
const RAISE_HUSK: AbilityId = AbilityId("raise_husk");
const CORPSE_BURST: AbilityId = AbilityId("corpse_burst");

pub fn nodes() -> Vec<TalentNode> {
    use PrerequisiteMode::{All, Any};
    use TalentStat::*;
    use TalentStatus::*;

    vec![
        node(4000, "Void Familiar", "Unlock a small void familiar summon.", 1, Route::Summoner, &[411], All, NeedsAbility, (-520.0, 0.0), TalentEffect::UnlockAbility { ability: VOID_FAMILIAR }),
        stat_node(4001, "Pet Mastery", "+5% minion damage per rank.", Route::Summoner, Damage, 0.05, 3, &[4000], All, NeedsSystem, (-650.0, -130.0)),
        stat_node(4002, "Binder's Focus", "+3% minion health per rank.", Route::Summoner, MaxHp, 0.03, 3, &[4000], All, NeedsSystem, (-650.0, 130.0)),
        node(4003, "Unstable Sympathy", "Your crits briefly empower your active minions.", 1, Route::Summoner, &[4000], All, NeedsSystem, (-780.0, 0.0), TalentEffect::PassiveProc { description: "Your crits briefly empower your active minions.", chance: 1.0, per_rank: 0.0 }),
        node(4010, "Riftling Swarm", "Unlock multiple short-lived void riftlings. First slice should ship this as a true multi-minion ability.", 1, Route::Summoner, &[4001], All, FirstSliceWip, (-820.0, -360.0), TalentEffect::UnlockAbility { ability: RIFTLING_SWARM }),
        node(4011, "Many Mouths", "Riftling Swarm summons +1 riftling.", 1, Route::Summoner, &[4010], All, NeedsSystem, (-980.0, -500.0), TalentEffect::AbilityMod { ability: RIFTLING_SWARM, modifier: AbilityModifier::ExtraProjectiles(1) }),
        node(4012, "Hungering Riftlings", "Riftlings deal bonus damage to low-HP enemies.", 1, Route::Summoner, &[4010], All, NeedsSystem, (-980.0, -260.0), TalentEffect::AbilityMod { ability: RIFTLING_SWARM, modifier: AbilityModifier::DamageBonus(0.0) }),
        node(4013, "Void Gate", "Unlock a portal that periodically spawns void entities.", 1, Route::Summoner, &[4011], All, NeedsAbility, (-1160.0, -500.0), TalentEffect::UnlockAbility { ability: VOID_GATE }),
        node(4014, "Wide Aperture", "Void Gate spawns faster but lasts slightly shorter.", 1, Route::Summoner, &[4013], All, NeedsSystem, (-1340.0, -600.0), TalentEffect::AbilityMod { ability: VOID_GATE, modifier: AbilityModifier::CooldownReduction(0.0) }),
        node(4015, "Collapse", "Void Gate explodes when it expires.", 1, Route::Summoner, &[4013], All, NeedsSystem, (-1340.0, -400.0), TalentEffect::AbilityMod { ability: VOID_GATE, modifier: AbilityModifier::DamageBonus(0.0) }),
        node(4016, "Bonded", "Your minions inherit your crit chance.", 1, Route::Summoner, &[4015], All, NeedsSystem, (-1520.0, -400.0), TalentEffect::Keystone { keystone: KeystoneId::Bonded }),
        stat_node(4060, "Thin Veil", "+3% minion health per rank while a void summon is active.", Route::Summoner, MaxHp, 0.03, 2, &[4016], All, NeedsSystem, (-1620.0, -500.0)),
        node(4017, "Beyond the Veil", "You can maintain one extra summon, but your own direct damage is reduced.", 1, Route::Summoner, &[4060], All, NeedsSystem, (-1700.0, -550.0), TalentEffect::Keystone { keystone: KeystoneId::Named("Beyond the Veil") }),
        node(4018, "Event Horizon", "Minion hits have a small chance to pull enemies toward their target.", 1, Route::Summoner, &[4016], All, NeedsSystem, (-1700.0, -250.0), TalentEffect::PassiveProc { description: "Minion hits have a small chance to pull enemies toward their target.", chance: 0.1, per_rank: 0.0 }),
        stat_node(4061, "Gatekeeper's Tax", "+2% cooldown reduction per rank for summon abilities.", Route::Summoner, CooldownReduction, 0.02, 2, &[4017], All, NeedsSystem, (-1800.0, -620.0)),
        node(4019, "Rift Sovereign", "Void Gate becomes permanent until recast, but reserves part of your max HP.", 1, Route::Summoner, &[4061], All, NeedsSystem, (-1880.0, -680.0), TalentEffect::Keystone { keystone: KeystoneId::Named("Rift Sovereign") }),
        node(4020, "Starved Threshold", "When a void minion expires, it briefly weakens nearby enemies.", 1, Route::Summoner, &[4017], All, NeedsSystem, (-1880.0, -430.0), TalentEffect::PassiveProc { description: "When a void minion expires, it briefly weakens nearby enemies.", chance: 1.0, per_rank: 0.0 }),
        node(4030, "Raise Husk", "Raise a temporary corpse husk from a slain enemy.", 1, Route::Summoner, &[4002], All, NeedsAbility, (-820.0, 360.0), TalentEffect::UnlockAbility { ability: RAISE_HUSK }),
        node(4031, "Bone Memory", "Raised husks inherit a small part of the slain enemy's damage.", 1, Route::Summoner, &[4030], All, NeedsSystem, (-980.0, 260.0), TalentEffect::AbilityMod { ability: RAISE_HUSK, modifier: AbilityModifier::DamageBonus(0.0) }),
        node(4032, "Grave Pace", "Raised husks move faster and decay slower.", 1, Route::Summoner, &[4030], All, NeedsSystem, (-980.0, 500.0), TalentEffect::AbilityMod { ability: RAISE_HUSK, modifier: AbilityModifier::CooldownReduction(0.0) }),
        node(4033, "Corpse Burst", "Detonate a corpse or husk for area damage.", 1, Route::Summoner, &[4031], All, NeedsAbility, (-1160.0, 260.0), TalentEffect::UnlockAbility { ability: CORPSE_BURST }),
        node(4034, "Black Powder", "Corpse Burst radius +20%.", 1, Route::Summoner, &[4033], All, NeedsSystem, (-1340.0, 160.0), TalentEffect::AbilityMod { ability: CORPSE_BURST, modifier: AbilityModifier::DamageBonus(0.0) }),
        node(4035, "Bone Shrapnel", "Corpse Burst fires fragments at nearby enemies.", 1, Route::Summoner, &[4033], All, NeedsSystem, (-1340.0, 360.0), TalentEffect::AbilityMod { ability: CORPSE_BURST, modifier: AbilityModifier::ExtraProjectiles(3) }),
        node(4036, "Death Tax", "Enemies killed by minions have a chance to leave a usable corpse.", 1, Route::Summoner, &[4035], All, NeedsSystem, (-1520.0, 360.0), TalentEffect::PassiveProc { description: "Enemies killed by minions have a chance to leave a usable corpse.", chance: 0.25, per_rank: 0.0 }),
        node(4037, "Necromancer", "Slain enemies have a chance to rise as minions.", 1, Route::Summoner, &[4036], All, NeedsSystem, (-1700.0, 360.0), TalentEffect::Keystone { keystone: KeystoneId::Necromancer }),
        stat_node(4062, "Bone Ledger", "+3% minion damage per rank while a raised minion is active.", Route::Summoner, Damage, 0.03, 2, &[4037], All, NeedsSystem, (-1800.0, 280.0)),
        node(4038, "Army of the Hollow", "Raised minions last much longer, but individual minions deal less damage.", 1, Route::Summoner, &[4062], All, NeedsSystem, (-1880.0, 220.0), TalentEffect::Keystone { keystone: KeystoneId::Named("Army of the Hollow") }),
        node(4039, "Last Rites", "Consuming a corpse heals your minions and damages nearby enemies.", 1, Route::Summoner, &[4037], All, NeedsSystem, (-1880.0, 500.0), TalentEffect::PassiveProc { description: "Consuming a corpse heals your minions and damages nearby enemies.", chance: 1.0, per_rank: 0.0 }),
        stat_node(4063, "Hollow Cadence", "+2% attack speed per rank for raised minions.", Route::Summoner, AttackSpeed, 0.02, 2, &[4038], All, NeedsSystem, (-1980.0, 280.0)),
        node(4040, "Black Communion", "When a raised minion dies, nearby raised minions gain attack speed briefly.", 1, Route::Summoner, &[4063], All, NeedsSystem, (-2060.0, 220.0), TalentEffect::PassiveProc { description: "When a raised minion dies, nearby raised minions gain attack speed briefly.", chance: 1.0, per_rank: 0.0 }),
        node(4050, "Empty Choir", "When a void minion dies near a corpse, it can raise a weak husk.", 1, Route::Summoner, &[4015, 4036], Any, NeedsSystem, (-1540.0, 0.0), TalentEffect::PassiveProc { description: "When a void minion dies near a corpse, it can raise a weak husk.", chance: 1.0, per_rank: 0.0 }),
        stat_node(4051, "Pact Mathematics", "+4% minion damage and +4% minion health per rank.", Route::Summoner, Damage, 0.04, 2, &[4050], All, NeedsSystem, (-1720.0, 0.0)),
    ]
}
