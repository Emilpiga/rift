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
    /// Cached per-ability gear modifiers (extra projectiles,
    /// cooldown scalar, damage scalar, transforms, procs, …).
    /// Recomputed alongside [`cached_stats`] from the current
    /// `Equipment` so the HUD ability tooltip can reflect
    /// legendary effects (e.g. Cleavebreaker's `+2 projectiles
    /// to Fireball Volley`) without rebuilding the aggregate
    /// every frame.
    cached_ability_mods: rift_game::loot::ability_mods::AbilityMods,
    /// Last server-reported essence pool fraction (0..=1) for
    /// the local player. Mirrored from the snapshot's
    /// `resource_pct` each frame in `world_sync`. The HUD reads
    /// this directly; the canonical scalar is server-side.
    pub resource_pct: f32,
    /// HUD-only summon timers synthesized from owned minion
    /// snapshot rows. Rendered beside regular buffs so summons
    /// advertise remaining duration without becoming mechanical
    /// status effects on the player.
    pub summon_effects: Vec<rift_engine::ecs::components::ActiveEffect>,
    /// Last server-reported salvage currency balance. Mirrored
    /// from [`rift_net::ServerMsg::ShardsSync`] in `main.rs`.
    /// The HUD reads this for the shard counter; the canonical
    /// value is the server's `ServerPlayer.shards` (persisted
    /// in the `characters.shards` column).
    pub shards: u32,
    /// Auto-alternating Punch state. `true` ⇒ next Punch swing
    /// is a Jab; `false` ⇒ next is a Cross. Flipped on every
    /// successful Punch cast. Reset to `true` (Jab) when more
    /// than `PUNCH_RESET_AFTER` seconds have elapsed since the
    /// last Punch so that the next opening swing of a fresh
    /// combat is always a Jab. Cosmetic-only — the server
    /// doesn't care which clip the client picked.
    pub punch_jab_next: bool,
    /// Wall-clock time of the last Punch cast, used to decide
    /// whether to reset [`Self::punch_jab_next`] to Jab on the
    /// next swing.
    pub last_punch_at: Option<std::time::Instant>,
}

/// After this much idle time between Punch swings, the next
/// Punch resets to Jab so that the opening hit of a fresh combat
/// is always a Jab.
pub const PUNCH_RESET_AFTER: std::time::Duration = std::time::Duration::from_millis(1000);

impl PlayerState {
    pub fn new() -> Self {
        Self::with_profile(Gender::Female, String::new(), Loadout::default_hero())
    }

    pub fn with_profile(gender: Gender, name: String, loadout: Loadout) -> Self {
        let config = HERO.clone();
        let attributes = Attributes::for_class(config.primary_attribute);

        let talents = talents::fresh_character_tree();
        let abilities = materialize_unlocked(&loadout, &talents);

        let experience = Experience::new();
        let default_equipment = rift_game::loot::Equipment::default();
        let cached_stats = CharacterStats::compute(
            &attributes,
            experience.level,
            &default_equipment.active_affix_sum(),
            &talents.stat_modifiers(),
        );
        let cached_ability_mods = default_equipment.ability_mods();

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
            cached_ability_mods,
            resource_pct: 1.0,
            summon_effects: Vec::new(),
            shards: 0,
            punch_jab_next: true,
            last_punch_at: None,
        }
    }

    /// Swap the ability in `slot_index` for the one with `wire_id`.
    /// Re-materializes the runtime `AbilitySlot` so cooldowns
    /// reset for the swapped slot. No-op when `slot_index` is
    /// out of range.
    pub fn set_loadout_slot(
        &mut self,
        slot_index: usize,
        wire_id: rift_game::abilities::AbilityWireId,
    ) {
        self.loadout.set_slot(slot_index, wire_id);
        self.refresh_abilities();
    }

    /// Rebuild the runtime action bar from the persisted loadout,
    /// hiding abilities whose `UnlockAbility` talent is no longer
    /// invested. The loadout itself is left untouched so relearning
    /// the talent can reveal the remembered slot again.
    pub fn refresh_abilities(&mut self) {
        self.abilities = materialize_unlocked(&self.loadout, &self.talents);
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
        self.cached_ability_mods = equipment.ability_mods();
    }

    /// Borrow the cached resolved stats. O(1) — no recomputation.
    /// Call [`recompute_stats`] when the underlying inputs
    /// change.
    pub fn stats(&self) -> &CharacterStats {
        &self.cached_stats
    }

    /// Borrow the cached per-ability gear modifiers. O(1).
    /// HUD ability tooltips read this so legendary effects
    /// (extra projectiles, cooldown reductions, transforms…)
    /// show through on the displayed numbers.
    pub fn ability_mods(&self) -> &rift_game::loot::ability_mods::AbilityMods {
        &self.cached_ability_mods
    }

    /// Convenience for spawn paths that only want max HP.
    pub fn max_hp(&self) -> f32 {
        self.cached_stats.max_hp
    }
}

fn materialize_unlocked(loadout: &Loadout, talents: &TalentTree) -> AbilitySlot {
    let mut bar = AbilitySlot::new();
    for (index, &wire_id) in loadout.slots.iter().enumerate() {
        if !rift_game::loadout::is_ability_unlocked(wire_id, talents) {
            continue;
        }
        if let Some(ability) = rift_game::abilities::lookup(wire_id) {
            bar.slots[index] = Some(rift_game::abilities::AbilityState::new(ability.clone()));
        }
    }
    bar
}
