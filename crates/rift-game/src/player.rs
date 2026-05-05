use rift_engine::combat::{
    Ability, AbilitySlot, Attributes, AttributeScaling, ClassConfig, ClassId, Experience,
    LevelUpReward, TalentTree,
};
use rift_engine::loot::inventory::PlayerStats;

use crate::character::Gender;
use crate::{abilities, classes, talents};

/// All player combat state: class, attributes, XP, abilities, talents.
pub struct PlayerState {
    pub class: ClassId,
    pub gender: Gender,
    pub name: String,
    pub config: ClassConfig,
    pub attributes: Attributes,
    pub attribute_scaling: AttributeScaling,
    pub experience: Experience,
    pub abilities: AbilitySlot,
    pub talents: TalentTree,
}

impl PlayerState {
    pub fn new(class: ClassId) -> Self {
        Self::with_profile(class, Gender::Female, String::new())
    }

    /// Build a player state for a freshly-loaded character profile.
    pub fn with_profile(class: ClassId, gender: Gender, name: String) -> Self {
        let config = classes::config_for(class);
        let attributes = Attributes::for_class(config.primary_attribute);
        let attribute_scaling = AttributeScaling::new(config.primary_attribute);

        // Class roster is owned by the game crate. Engine just runs
        // whatever Ability data lands in these slots.
        let mut ability_slots = AbilitySlot::new();
        let roster: [Ability; 6] = match class {
            classes::HUNTER => abilities::hunter_roster(),
            // New classes get their own rosters here.
            _ => abilities::hunter_roster(),
        };
        for (i, ab) in roster.into_iter().enumerate() {
            ability_slots.set(i, ab);
        }

        let talents = match class {
            classes::HUNTER => talents::hunter_tree(),
            _ => talents::hunter_tree(),
        };

        Self {
            class,
            gender,
            name,
            config,
            attributes,
            attribute_scaling,
            experience: Experience::new(),
            abilities: ability_slots,
            talents,
        }
    }

    /// Compute final damage for a weapon attack given equipment stats.
    pub fn compute_attack_damage(&self, equip_stats: &PlayerStats) -> f32 {
        let talent_bonuses = self.talents.compute_bonuses();
        let base = self.config.base_damage + equip_stats.flat_damage;
        let attr_bonus = self.attribute_scaling.damage_bonus(&self.attributes);
        let talent_bonus = talent_bonuses.damage_pct;
        let equip_pct = equip_stats.percent_damage;
        base * (1.0 + attr_bonus + talent_bonus + equip_pct)
    }

    /// Grant XP for a kill. Returns level-up rewards (if any).
    pub fn grant_kill_xp(&mut self, monster_level: u32) -> Vec<LevelUpReward> {
        let xp = Experience::xp_for_kill(monster_level, self.experience.level);
        let rewards = self.experience.grant_xp(xp);
        for reward in &rewards {
            self.attributes.unspent_points += reward.attribute_points;
            self.talents.unspent_points += reward.talent_points;
        }
        rewards
    }

    /// Player max HP based on level + class.
    pub fn max_hp(&self) -> f32 {
        self.config.base_hp + self.config.hp_per_level * self.experience.level as f32
    }
}
