//! Cross-route synergy nodes from the finished talent tree blueprint.

use super::{node, PrerequisiteMode, Route, TalentEffect, TalentNode, TalentStatus};

pub fn nodes() -> Vec<TalentNode> {
    use PrerequisiteMode::All;
    use TalentStatus::*;

    vec![
        node(6100, "Flame Reaver", "Melee hits against burning enemies create a small fire cleave; Fire Wave empowers your next melee commit.", 1, Route::Synergy, &[1014, 2012], All, NeedsSystem, (1700.0, -1250.0), TalentEffect::Synergy { description: "Melee hits against burning enemies create a small fire cleave; Fire Wave empowers your next melee commit." }),
        node(6101, "Frostbreaker", "Staggering a chilled or brittle enemy triggers a small shatter burst.", 1, Route::Synergy, &[1032, 2033], All, NeedsSystem, (1560.0, 90.0), TalentEffect::Synergy { description: "Staggering a chilled or brittle enemy triggers a small shatter burst." }),
        node(6200, "Cold Star Pact", "Void minions deal bonus damage to chilled enemies and can extend Chill on hit.", 1, Route::Synergy, &[2033, 4010], All, FirstSliceWip, (-1100.0, -1600.0), TalentEffect::Synergy { description: "Void minions deal bonus damage to chilled enemies and can extend Chill on hit." }),
        node(6201, "Rift Combustion", "Burning enemies killed near a void summon collapse into a small unstable rift.", 1, Route::Synergy, &[2016, 4013], All, NeedsSystem, (-1020.0, -1160.0), TalentEffect::Synergy { description: "Burning enemies killed near a void summon collapse into a small unstable rift." }),
        node(6300, "Grave Mercy", "Healing a raised minion splashes a smaller heal to you; overhealing minions briefly shields them.", 1, Route::Synergy, &[3031, 4030], All, NeedsSystem, (-1020.0, 1040.0), TalentEffect::Synergy { description: "Healing a raised minion splashes a smaller heal to you; overhealing minions briefly shields them." }),
        node(6301, "Last Blessing", "When a raised minion expires, it releases a small heal or damage pulse based on nearby allies/enemies.", 1, Route::Synergy, &[3037, 4037], All, NeedsSystem, (-1560.0, 1120.0), TalentEffect::Synergy { description: "When a raised minion expires, it releases a small heal or damage pulse based on nearby allies/enemies." }),
        node(6400, "Consecrated Steel", "Melee commits inside your healing effects gain armor and deal minor holy splash damage.", 1, Route::Synergy, &[1034, 3030], All, NeedsSystem, (1320.0, 1180.0), TalentEffect::Synergy { description: "Melee commits inside your healing effects gain armor and deal minor holy splash damage." }),
        node(6401, "Frontline Prayer", "Shielding or healing yourself briefly empowers the next Shield Bash, Ground Slam, or Melee Attack.", 1, Route::Synergy, &[1031, 3003], All, NeedsSystem, (1260.0, 760.0), TalentEffect::Synergy { description: "Shielding or healing yourself briefly empowers the next Shield Bash, Ground Slam, or Melee Attack." }),
        node(6500, "Bloodbound Legion", "Your low-HP melee bonuses also empower nearby minions at reduced value.", 1, Route::Synergy, &[1017, 4003], All, NeedsSystem, (-1500.0, -920.0), TalentEffect::Synergy { description: "Your low-HP melee bonuses also empower nearby minions at reduced value." }),
        node(6600, "Radiant Frost", "Frost hits against enemies recently damaged by holy spells can create a small protective shield on you.", 1, Route::Synergy, &[2030, 3010], All, NeedsSystem, (2600.0, 0.0), TalentEffect::Synergy { description: "Frost hits against enemies recently damaged by holy spells can create a small protective shield on you." }),
    ]
}
