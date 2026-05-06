//! Slim, client-side player profile.
//!
//! The server is authoritative for damage / XP / loot, so this
//! struct only carries data the local rendering + UX paths read:
//! ability cooldowns (HUD ability bar), experience level (HUD XP
//! bar), gender / config (skinned avatar spawn), talents (visual
//! ability tweaks). Lifted out of `state.rs` so the gameplay
//! sub-systems (`loot_system`, `portal_system`, `stash_system`)
//! can borrow it without pulling in the full `GameState` header.

use rift_game::abilities::{self, Ability, AbilitySlot};
use rift_game::attributes::Attributes;
use rift_game::character::Gender;
use rift_game::classes::{self, ClassConfig, ClassId};
use rift_game::experience::Experience;
use rift_game::stats::CharacterStats;
use rift_game::talents::{self, TalentTree};

pub struct PlayerState {
    pub class: ClassId,
    pub gender: Gender,
    pub name: String,
    pub config: ClassConfig,
    pub attributes: Attributes,
    pub experience: Experience,
    pub abilities: AbilitySlot,
    pub talents: TalentTree,
    /// Cached resolved character sheet. Recomputed only when the
    /// inputs change (equipment sync, attribute respec, level
    /// up). The HUD reads this every frame instead of redoing
    /// the affix sum + multiplier math per frame.
    cached_stats: CharacterStats,
}

impl PlayerState {
    pub fn new(class: ClassId) -> Self {
        Self::with_profile(class, Gender::Female, String::new())
    }

    pub fn with_profile(class: ClassId, gender: Gender, name: String) -> Self {
        let config = classes::config_for(class);
        let attributes = Attributes::for_class(config.primary_attribute);

        let mut ability_slots = AbilitySlot::new();
        let roster: [Ability; 6] = match class {
            classes::HUNTER => abilities::hunter_roster(),
            _ => abilities::hunter_roster(),
        };
        for (i, ab) in roster.into_iter().enumerate() {
            ability_slots.set(i, ab);
        }

        let talents = match class {
            classes::HUNTER => talents::hunter_tree(),
            _ => talents::hunter_tree(),
        };

        let experience = Experience::new();
        let cached_stats = CharacterStats::compute(
            &config,
            &attributes,
            experience.level,
            &rift_game::loot::Equipment::default().active_affix_sum(),
            &talents.stat_modifiers(),
        );

        Self {
            class,
            gender,
            name,
            config,
            attributes,
            experience,
            abilities: ability_slots,
            talents,
            cached_stats,
        }
    }

    /// Recompute the cached character sheet from the supplied
    /// equipment plus current attributes / level. Call after any
    /// `EquipmentSync`, attribute change, or level up.
    pub fn recompute_stats(&mut self, equipment: &rift_game::loot::Equipment) {
        self.cached_stats = CharacterStats::compute(
            &self.config,
            &self.attributes,
            self.experience.level,
            &equipment.active_affix_sum(),
            &self.talents.stat_modifiers(),
        );
    }

    /// Borrow the cached resolved stats. O(1) — no recomputation.
    /// Call [`recompute_stats`] when the underlying inputs
    /// change.
    pub fn stats(&self) -> &CharacterStats {
        &self.cached_stats
    }

    /// Convenience for spawn paths that only want max HP.
    pub fn max_hp(&self) -> f32 {
        self.cached_stats.max_hp
    }
}
