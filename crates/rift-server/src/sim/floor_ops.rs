//! Floor-lifecycle methods on [`Sim`]: floor transitions.
//! Split out of `sim/mod.rs`. Pure `impl Sim` block — every
//! method is defined on `Sim` and migrated here verbatim.

use glam::Vec3;

use super::{
    ability, channel, enemies, floor, loot, player, projectile, shrine, RiftProgress, Sim,
};

impl Sim {
    /// Switch to a different floor. Wipes all combat state and
    /// snaps every connected player to the new spawn position.
    /// Returns the spawn the server seated everyone at so the
    /// caller can put it in the broadcast `LoadFloor`.
    pub fn change_floor(&mut self, new_index: u32) -> Vec3 {
        self.floor_index = new_index;
        self.floor = floor::generate(self.floor_seed, new_index);
        let spawn = Vec3::new(self.floor.spawn_pos.x, 0.0, self.floor.spawn_pos.z);
        player::snap_all_to(&mut self.world, spawn);
        // HP restore policy depends on destination:
        //   - Hub (index 0): full heal + clear ghost state.
        //     Triggered by manual exit-vote, party-wipe respawn,
        //     login. Team is back in the safe zone, everyone
        //     starts fresh.
        //   - Deeper rift floor: heal LIVING players only;
        //     ghosts follow along still in spectator mode
        //     instead of being resurrected by the floor change.
        if new_index == 0 {
            player::heal_all(&mut self.world);
        } else {
            player::heal_living(&mut self.world);
        }
        enemies::despawn_all(&mut self.world);
        projectile::despawn_all(&mut self.world);
        loot::despawn_all(&mut self.world);
        shrine::despawn_all(&mut self.world);
        self.aoe_zones.clear();
        channel::clear_all(&mut self.world);
        ability::clear_cooldowns(&mut self.cooldowns);
        self.pending_inputs.clear();
        // Drop any in-flight WorldEvents (Damage, Death,
        // AbilityCast, ...) queued earlier this tick. Their
        // NetIds reference entities we just despawned, so
        // letting them ship to the new floor would surface
        // ghost damage numbers / death sounds against ids the
        // client never saw alive.
        self.pending_events.clear();
        enemies::spawn_for_floor(
            &mut self.world,
            &self.floor,
            self.floor_index,
            &mut self.next_enemy_net_id,
        );
        // Roll a (rare) revive shrine on rift floors >= 2.
        // `maybe_spawn` no-ops on hub / floor 1.
        shrine::maybe_spawn(
            &mut self.world,
            &self.floor,
            self.floor_seed,
            self.floor_index,
            &mut self.next_misc_net_id,
        );
        self.rift_progress = RiftProgress::for_floor(new_index);
        self.progress_dirty = true;
        // Wipe any in-flight death/respawn bookkeeping — the new
        // floor starts everyone alive and the timer should not
        // carry over.
        self.pending_player_deaths.clear();
        self.hub_respawn_timer = None;
        // Vote state is per-floor: a transition cancels any
        // in-flight vote and clears the cooldown so a fresh
        // descent doesn't carry baggage from the previous one.
        if self.exit_vote.is_some() || self.exit_vote_cooldown > 0.0 {
            self.exit_vote = None;
            self.exit_vote_cooldown = 0.0;
            self.exit_vote_dirty = true;
        }
        log::info!(
            "sim: changed to floor {new_index} (seed={}) at spawn {spawn:?}",
            self.floor_seed
        );
        spawn
    }

}
