//! Slim, client-side player profile.
//!
//! The server is authoritative for damage / XP / loot, so this
//! struct only carries data the local rendering + UX paths read:
//! ability cooldowns (HUD ability bar), experience level (HUD XP
//! bar), gender / config (skinned avatar spawn), talents (visual
//! ability tweaks). Lifted out of `state.rs` so the gameplay
//! sub-systems (`loot_system`, `portal_system`, `stash_system`)
//! can borrow it without pulling in the full `GameState` header.

use rift_game::abilities::AbilitySlot;
use rift_game::attributes::Attributes;
use rift_game::character::Gender;
use rift_game::experience::Experience;
use rift_game::hero::{HeroConfig, HERO};
use rift_game::loadout::Loadout;
use rift_game::stats::CharacterStats;
use rift_game::talents::{self, TalentTree};

pub struct PlayerState {
    pub gender: Gender,
    pub name: String,
    pub config: HeroConfig,
    pub attributes: Attributes,
    pub experience: Experience,
    /// The player's chosen six-ability action bar. Source of
    /// truth for [`Self::abilities`]; mutate via
    /// [`Self::set_loadout_slot`] which keeps the runtime
    /// `AbilitySlot` in sync.
    pub loadout: Loadout,
    pub abilities: AbilitySlot,
    pub talents: TalentTree,
    /// Cached resolved character sheet. Recomputed only when the
    /// inputs change (equipment sync, attribute respec, level
    /// up). The HUD reads this every frame instead of redoing
    /// the affix sum + multiplier math per frame.
    cached_stats: CharacterStats,
}

impl PlayerState {
    pub fn new() -> Self {
        Self::with_profile(Gender::Female, String::new(), Loadout::default_hero())
    }

    pub fn with_profile(gender: Gender, name: String, loadout: Loadout) -> Self {
        let config = HERO.clone();
        let attributes = Attributes::for_class(config.primary_attribute);

        let abilities = loadout.materialize();
        let talents = talents::hunter_tree();

        let experience = Experience::new();
        let cached_stats = CharacterStats::compute(
            &attributes,
            experience.level,
            &rift_game::loot::Equipment::default().active_affix_sum(),
            &talents.stat_modifiers(),
        );

        Self {
            gender,
            name,
            config,
            attributes,
            experience,
            loadout,
            abilities,
            talents,
            cached_stats,
        }
    }

    /// Swap the ability in `slot_index` for the one with `wire_id`.
    /// Re-materializes the runtime `AbilitySlot` so cooldowns
    /// reset for the swapped slot. No-op when `slot_index` is
    /// out of range.
    pub fn set_loadout_slot(&mut self, slot_index: usize, wire_id: u8) {
        self.loadout.set_slot(slot_index, wire_id);
        self.abilities = self.loadout.materialize();
    }

    /// Recompute the cached character sheet from the supplied
    /// equipment plus current attributes / level. Call after any
    /// `EquipmentSync`, attribute change, or level up.
    pub fn recompute_stats(&mut self, equipment: &rift_game::loot::Equipment) {
        self.cached_stats = CharacterStats::compute(
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
