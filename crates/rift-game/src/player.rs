use rift_engine::combat::{
    Ability, AbilitySlot, Attributes, AttributeScaling, Class, ClassConfig, Experience,
    LevelUpReward, TalentTree,
};
use rift_engine::loot::inventory::PlayerStats;

/// All player combat state: class, attributes, XP, abilities, talents.
pub struct PlayerState {
    pub class: Class,
    pub config: ClassConfig,
    pub attributes: Attributes,
    pub attribute_scaling: AttributeScaling,
    pub experience: Experience,
    pub abilities: AbilitySlot,
    pub talents: TalentTree,
}

impl PlayerState {
    pub fn new(class: Class) -> Self {
        let config = class.config();
        let attributes = Attributes::for_class(config.primary_attribute);
        let attribute_scaling = AttributeScaling::new(config.primary_attribute);

        let mut abilities = AbilitySlot::new();
        abilities.set(0, Ability::steady_shot());
        abilities.set(1, Ability::multi_shot());
        abilities.set(2, Ability::evasive_roll());
        abilities.set(3, Ability::rapid_fire());
        abilities.set(4, Ability::mark_for_death());
        abilities.set(5, Ability::rain_of_arrows());

        Self {
            class,
            config,
            attributes,
            attribute_scaling,
            experience: Experience::new(),
            abilities,
            talents: TalentTree::hunter(),
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
