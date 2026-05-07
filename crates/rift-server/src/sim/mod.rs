//! Server-authoritative simulation: top-level orchestration.
//!
//! Submodules each own a slice of state (players, enemies,
//! projectiles, abilities, snapshots, floor lifecycle). This module
//! holds the [`Sim`] aggregate and the [`Sim::step`] loop that walks
//! the subsystems in order.
//!
//! Determinism: floor geometry comes from `rift_dungeon::Floor` —
//! the same generator the client runs — keyed by `(seed,
//! floor_index)`. We never replicate tiles or walls; clients
//! regenerate locally and trust the seed.

use std::collections::HashMap;

use glam::Vec3;
use hecs::Entity;
use rift_dungeon::Floor;
use rift_dungeon::FloorConfig;
use rift_net::{
    messages::{InputCmd, Snapshot, VoteChoice, WorldEvent},
    ClientId, NetId, NetTick,
};

pub mod ability;
pub mod channel;
pub mod debuff;
pub mod enemy;
pub mod floor;
pub mod loot;
pub mod player;
pub mod projectile;
pub mod shrine;
pub mod snapshot;
pub mod vote;

pub use player::ServerPlayer;
pub use projectile::ServerAoeZone;

/// Drop trailing `None`s from a sparse bag/stash so its `len()`
/// tracks the highest occupied slot + 1. Keeps wire payloads
/// minimal and prevents the bag from growing unbounded.
fn trim_trailing_none<T>(v: &mut Vec<Option<T>>) {
    while matches!(v.last(), Some(None)) {
        v.pop();
    }
}

/// Push `item` into the first empty slot of a sparse bag/stash,
/// or append to the end if every slot is occupied. Used by
/// pickups and unequip-into-bag flows where the caller doesn't
/// have an explicit destination index.
fn push_into_sparse<T>(v: &mut Vec<Option<T>>, item: T) {
    if let Some(slot) = v.iter_mut().find(|s| s.is_none()) {
        *slot = Some(item);
    } else {
        v.push(Some(item));
    }
}

/// Count of filled slots in a sparse bag/stash. Used by debug
/// logs that previously read `Vec::len()`.
fn count_filled<T>(v: &[Option<T>]) -> usize {
    v.iter().filter(|s| s.is_some()).count()
}

/// Maximum XZ distance (metres) between the picker and a ground
/// loot drop for a [`ClientMsg::PickUpLoot`] to succeed.
pub const PICKUP_RANGE: f32 = 2.0;

/// Top-level server simulation state. Owned by `Server`.
pub struct Sim {
    pub world: hecs::World,
    pub floor: Floor,
    pub floor_seed: u64,
    pub floor_index: u32,

    /// NetId allocators. Disjoint ranges so player / enemy /
    /// projectile / loot ids can never collide on the wire:
    /// - players:     `0x8000_0000..`     (high bit set)
    /// - enemies:     `0x0000_0001..0x2000_0000`
    /// - loot:        `0x2000_0000..0x4000_0000`
    /// - projectiles: `0x4000_0000..0x8000_0000`
    next_player_net_id: u32,
    next_enemy_net_id: u32,
    next_loot_net_id: u32,
    next_projectile_net_id: u32,
    /// NetId allocator for miscellaneous interactables (revive
    /// shrines and any future floor objects). Lives in
    /// `0x6000_0000..0x8000_0000` — disjoint from the
    /// projectile range that ends at `0x6000_0000` in practice
    /// (the projectile allocator wraps long before it ever
    /// gets there) and from the player range (`0x8000_0000+`).
    next_misc_net_id: u32,

    /// Most recent input from each client, coalesced. Drained by
    /// `player::apply_inputs` on every step.
    pending_inputs: HashMap<ClientId, InputCmd>,
    /// `client_id → Entity` lookup so disconnect / input dispatch
    /// is O(1).
    sessions: HashMap<ClientId, Entity>,

    /// Active server-driven AoE zones (e.g. Rain of Arrows).
    aoe_zones: Vec<ServerAoeZone>,
    /// Per-client ability cooldowns.
    cooldowns: ability::CooldownTable,

    /// World events generated this tick. Drained by the server main
    /// loop and broadcast on `Channel::Event` (reliable).
    pending_events: Vec<WorldEvent>,

    /// Authoritative rift-progress state for the current floor.
    /// Mutated by [`Self::step`] when enemies die; broadcast as
    /// `ServerMsg::RiftProgress` whenever `progress_dirty` is set.
    rift_progress: RiftProgress,
    /// `true` when `rift_progress` has changed since the last
    /// broadcast. Drained via [`Self::take_rift_progress_update`].
    progress_dirty: bool,
    /// Pending per-player XP / level updates produced by the
    /// most recent `step`. Drained by the server main loop and
    /// shipped as `ServerMsg::CharacterStats`.
    pending_stat_updates: Vec<StatsUpdate>,

    /// Player deaths queued during the most recent tick. The main
    /// loop drains this into `WorldEvent::Death` broadcasts so
    /// every client triggers the death animation, not just the
    /// owner. `(client_id, net_id)` so the broadcaster can also
    /// log + drop blood decals.
    pending_player_deaths: Vec<(ClientId, NetId)>,
    /// Counts down from [`HUB_RESPAWN_DELAY`] once the **whole
    /// party has wiped** on a non-hub floor (every connected
    /// player has `hp <= 0`). When it hits zero the main loop
    /// reads it via [`Sim::take_hub_respawn_request`] and drives
    /// `transition_floor(0)` so the dead party gets back to
    /// safety. `None` means "no wipe in progress".
    ///
    /// Single-player deaths no longer arm this — those players
    /// linger as ghosts (snapshot `DEAD` flag set, AI ignores
    /// them, can't deal damage) until the survivors either
    /// finish the floor, vote-exit, or die themselves.
    hub_respawn_timer: Option<f32>,

    /// Active rift-exit vote, if any. Opened by
    /// [`Self::request_exit_vote`] when 2+ players are
    /// connected; ticked down each step in
    /// [`Self::tick_exit_vote`]; cleared on resolution.
    /// Single-player exits short-circuit and never touch this.
    exit_vote: Option<vote::ExitVote>,
    /// Seconds remaining before another exit vote may be
    /// opened. Set to [`vote::VOTE_COOLDOWN`] on a fizzle;
    /// counts down to zero in [`Self::tick_exit_vote`].
    /// `0.0` when no recent fizzle (or after the cooldown
    /// has expired).
    exit_vote_cooldown: f32,
    /// Set whenever [`Self::exit_vote`] or
    /// [`Self::exit_vote_cooldown`] crosses a state boundary the
    /// HUD cares about (vote opened / cast / resolved /
    /// cooldown finished). Drained by
    /// [`Self::take_exit_vote_update`] which the main loop turns
    /// into a broadcast `ServerMsg::RiftExitVote`.
    exit_vote_dirty: bool,
}

/// Wall-clock seconds the dying player's avatar lingers in the
/// rift before the server force-loads them back to the hub. Long
/// enough for the client's death animation to play through.
pub const HUB_RESPAWN_DELAY: f32 = 3.5;

/// Seconds a player stays in the down-pose after dying before
/// rising as a ghost. The window is sized to let the death
/// animation breathe and to give teammates a beat to register
/// the loss before the avatar disappears (server filters ghost
/// rows out of remote snapshots once `is_ghost` flips).
pub const GHOST_RISE_DELAY: f32 = 3.5;

/// Outcome of [`Sim::request_exit_vote`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitVoteRequest {
    /// Solo player; caller must wipe ghost loot (none expected
    /// since the only player must be alive to initiate) and
    /// transition to the hub immediately.
    Pass,
    /// Multiplayer party; vote window opened, broadcast the
    /// fresh `RiftExitVote` snapshot via
    /// [`Sim::take_exit_vote_update`].
    Opened,
    /// Request rejected (cooldown, dead, in hub, vote already
    /// active). No state change; nothing to broadcast.
    Refused,
}

/// Server-authoritative rift state. One instance per floor —
/// reset by [`Sim::change_floor`].
#[derive(Clone, Copy, Debug)]
pub struct RiftProgress {
    /// Kills counted toward the boss spawn so far.
    pub progress: u32,
    /// Kills required before the boss appears.
    pub required: u32,
    pub boss_spawned: bool,
    pub boss_killed: bool,
    pub floor_complete: bool,
}

impl RiftProgress {
    fn for_floor(floor_index: u32) -> Self {
        // Hub has no progression. Otherwise scale linearly with
        // floor index — quick on early floors, longer on deeper
        // ones.
        let required = if floor_index == 0 {
            0
        } else {
            6 + floor_index * 3
        };
        Self {
            progress: 0,
            required,
            boss_spawned: false,
            boss_killed: false,
            floor_complete: false,
        }
    }
}

/// One queued XP / level update for a connected client. Built by
/// [`Sim::step`] when a player gains XP. Drained by the server
/// main loop and shipped as `ServerMsg::CharacterStats`.
#[derive(Clone, Copy, Debug)]
pub struct StatsUpdate {
    pub client_id: ClientId,
    pub level: u32,
    /// XP into the *current* level. What the HUD bar fills with.
    pub xp: u64,
    pub xp_to_next: u64,
    /// Cumulative lifetime XP. Persisted to the database so a
    /// reconnect can rebuild `(level, current_xp)` without the
    /// server having to re-do the level curve math itself.
    pub total_xp: u64,
}

impl Sim {
    pub fn new(floor_seed: u64, floor_index: u32) -> Self {
        let floor = floor::generate(floor_seed, floor_index);
        let mut sim = Self {
            world: hecs::World::new(),
            floor,
            floor_seed,
            floor_index,
            next_player_net_id: 1,
            next_enemy_net_id: 1,
            next_loot_net_id: 0x2000_0000,
            next_projectile_net_id: 0x4000_0000,
            next_misc_net_id: 0x6000_0000,
            pending_inputs: HashMap::new(),
            sessions: HashMap::new(),
            aoe_zones: Vec::new(),
            cooldowns: HashMap::new(),
            pending_events: Vec::new(),
            rift_progress: RiftProgress::for_floor(floor_index),
            progress_dirty: false,
            pending_stat_updates: Vec::new(),
            pending_player_deaths: Vec::new(),
            hub_respawn_timer: None,
            exit_vote: None,
            exit_vote_cooldown: 0.0,
            exit_vote_dirty: false,
        };
        enemy::spawn_for_floor(
            &mut sim.world,
            &sim.floor,
            sim.floor_index,
            &mut sim.next_enemy_net_id,
        );
        sim
    }

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
        enemy::despawn_all(&mut self.world);
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
        enemy::spawn_for_floor(
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

    /// Spawn (or look up the existing) player entity for a freshly-
    /// Helloed client. Returns the allocated `NetId`. Initial
    /// [`CharacterStats`] are baked into [`ServerPlayer::fresh`]
    /// from the hero config.
    pub fn spawn_player(
        &mut self,
        client_id: ClientId,
    ) -> NetId {
        if let Some(&existing) = self.sessions.get(&client_id) {
            if let Ok(p) = self.world.get::<&ServerPlayer>(existing) {
                return p.net_id;
            }
        }
        let net_id = NetId(self.next_player_net_id | 0x8000_0000);
        self.next_player_net_id = self.next_player_net_id.wrapping_add(1).max(1);
        let spawn = Vec3::new(self.floor.spawn_pos.x, 0.0, self.floor.spawn_pos.z);
        let entity = self
            .world
            .spawn((ServerPlayer::fresh(client_id, net_id, spawn),));
        self.sessions.insert(client_id, entity);
        log::info!("sim: spawned player {client_id:?} as {net_id:?} at {spawn:?}");
        net_id
    }

    pub fn despawn_player(&mut self, client_id: ClientId) {
        if let Some(entity) = self.sessions.remove(&client_id) {
            let _ = self.world.despawn(entity);
            log::info!("sim: despawned player {client_id:?}");
        }
        self.pending_inputs.remove(&client_id);
        self.cooldowns.remove(&client_id);
    }

    /// `true` if `client_id` is currently a ghost (risen-but-dead).
    /// Used by the message dispatch in `main.rs` to silently drop
    /// gameplay actions (cast, loot pickup, drop) for spectators.
    pub fn is_ghost(&self, client_id: ClientId) -> bool {
        let Some(&entity) = self.sessions.get(&client_id) else { return false };
        self.world
            .get::<&ServerPlayer>(entity)
            .map(|p| p.is_ghost)
            .unwrap_or(false)
    }

    /// Set the player's revive-shrine channel intent. `Some`
    /// requires alive + within [`SHRINE_INTERACT_RADIUS`] of
    /// the named shrine. `None` always succeeds (release F,
    /// walk out of range, etc.). Idempotent.
    pub fn set_shrine_channel(&mut self, client_id: ClientId, shrine: Option<NetId>) {
        use rift_net::messages::SHRINE_INTERACT_RADIUS;
        let Some(&entity) = self.sessions.get(&client_id) else { return };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else { return };
        match shrine {
            None => {
                p.channeling_shrine = None;
            }
            Some(id) => {
                if p.is_dead_or_ghosting() {
                    return;
                }
                drop(p);
                let Some((_, shrine_pos)) = shrine::find(&self.world, id) else {
                    return;
                };
                let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else { return };
                let dist_sq = (p.k.position - shrine_pos).length_squared();
                if dist_sq > SHRINE_INTERACT_RADIUS * SHRINE_INTERACT_RADIUS {
                    return;
                }
                p.channeling_shrine = Some(id);
            }
        }
    }

    /// Stash an input from a client — coalesced against any earlier
    /// input still pending for the same client this tick.
    pub fn ingest_input(&mut self, client_id: ClientId, cmd: InputCmd) {
        player::merge_pending(&mut self.pending_inputs, client_id, cmd);
    }

    /// Forward a `ClientMsg::CastAbility` to the ability dispatch.
    pub fn cast_ability(
        &mut self,
        client_id: ClientId,
        ability_id: u8,
        client_origin: [f32; 3],
        aim_dir: [f32; 2],
        placed_target: Option<[f32; 3]>,
        tick: NetTick,
    ) {
        ability::cast(
            &mut self.world,
            &self.sessions,
            &mut self.cooldowns,
            &mut self.aoe_zones,
            &mut self.pending_events,
            &mut self.next_projectile_net_id,
            client_id,
            ability_id,
            client_origin,
            aim_dir,
            placed_target,
            tick,
        );
    }

    /// Forward a `ClientMsg::EndChannel` request — cancels the
    /// caller's matching active channel (if any). Silently no-ops
    /// if the player isn't channeling that ability so a duplicate
    /// release packet doesn't error.
    pub fn end_channel(&mut self, client_id: ClientId, ability_id: u8) {
        let Some(&entity) = self.sessions.get(&client_id) else { return };
        channel::cancel(
            &mut self.world,
            entity,
            ability_id,
            &mut self.pending_events,
        );
    }

    /// Try to claim a ground-loot drop for `client_id`. Validates
    /// the picker is within [`PICKUP_RANGE`] of the loot row and
    /// has a free bag slot (cap [`rift_net::messages::INVENTORY_CAPACITY`]).
    ///
    /// Returns:
    /// - `Ok(item)` on success \u2014 loot entity is despawned, item is
    ///   already in the picker's `ServerPlayer.inventory`, caller
    ///   broadcasts `LootClaimed` and persists.
    /// - `Err(Some(reason))` when the request was understood but
    ///   refused (e.g. bag full); the caller forwards the reason
    ///   back to the picker so the UI can react. Loot entity is
    ///   left on the ground.
    /// - `Err(None)` for silent failures (missing session, missing
    ///   loot row, out-of-range) \u2014 these aren't worth notifying
    ///   the client about.
    pub fn try_pickup_loot(
        &mut self,
        client_id: ClientId,
        loot: NetId,
    ) -> Result<rift_game::loot::Item, Option<rift_net::messages::PickupRejectReason>> {
        let &player_entity = self.sessions.get(&client_id).ok_or(None)?;
        let player_pos = self
            .world
            .get::<&ServerPlayer>(player_entity)
            .map_err(|_| None)?
            .k
            .position;

        // Find the loot ECS entity by net id.
        let target = self
            .world
            .query::<&loot::ServerLoot>()
            .iter()
            .find(|(_, l)| l.net_id == loot)
            .map(|(e, l)| (e, l.position, l.item.clone()))
            .ok_or(None)?;
        let (loot_entity, loot_pos, item) = target;

        let dx = loot_pos.x - player_pos.x;
        let dz = loot_pos.z - player_pos.z;
        if dx * dx + dz * dz > PICKUP_RANGE * PICKUP_RANGE {
            return Err(None);
        }

        // Capacity check before we mutate anything: leave the
        // loot row alive so the player can pick it up after
        // freeing a slot.
        if let Ok(p) = self.world.get::<&ServerPlayer>(player_entity) {
            if count_filled(&p.inventory) >= rift_net::messages::INVENTORY_CAPACITY {
                return Err(Some(
                    rift_net::messages::PickupRejectReason::InventoryFull,
                ));
            }
        }

        let _ = self.world.despawn(loot_entity);
        // Push onto the picker's `ServerPlayer.inventory` so the
        // authoritative server-side bag stays in sync with what
        // the client mirrors. Long-term DB persistence is handled
        // by the caller (server main) which also has the
        // `PersistenceHandle`.
        if let Ok(mut p) = self.world.get::<&mut ServerPlayer>(player_entity) {
            push_into_sparse(&mut p.inventory, item.clone());
            log::debug!(
                "sim: inventory for {:?} now has {} item(s)",
                client_id,
                count_filled(&p.inventory)
            );
        }
        Ok(item)
    }

    /// Hydrate a freshly-spawned player's inventory from a
    /// pre-loaded list (typically the rows fetched by
    /// `PersistenceHandle::load_inventory_blocking`). Idempotent;
    /// replaces whatever was there. Called once during the
    /// `Hello` handshake right after `spawn_player`. `equipment`
    /// is the parallel set of pre-equipped items (rows whose
    /// persisted `equipped_slot` was non-null).
    pub fn set_player_inventory(
        &mut self,
        client_id: ClientId,
        items: Vec<Option<rift_game::loot::Item>>,
        equipment: rift_game::loot::Equipment,
    ) {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return;
        };
        if let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) {
            p.inventory = items;
            trim_trailing_none(&mut p.inventory);
            p.equipment = equipment;
            p.recompute_stats();
        }
    }

    /// Hydrate a freshly-spawned player's level + XP from the
    /// persisted `CharacterRecord`. Restores `Experience`
    /// directly (`current_xp` rolls inside one level, `total_xp`
    /// is the sum). Recomputes stats so the HP pool reflects
    /// the loaded level. Idempotent.
    pub fn set_player_experience(
        &mut self,
        client_id: ClientId,
        level: u32,
        total_xp: u64,
    ) {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return;
        };
        if let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) {
            p.experience.level = level.max(1);
            p.experience.total_xp = total_xp;
            // Derive `current_xp` (XP into the current level)
            // from `(total_xp, level)`. We don't persist
            // current_xp separately yet, so re-deriving keeps
            // the bar accurate after a reload. The XP curve
            // lives in `rift_game::experience` so server and
            // client agree byte-for-byte.
            let xp_for_levels =
                rift_game::experience::total_xp_for_level(p.experience.level);
            p.experience.current_xp = total_xp.saturating_sub(xp_for_levels);
            p.level = p.experience.level;
            p.recompute_stats();
            p.hp = p.hp_max;
        }
    }

    /// Read a player's authoritative XP / level snapshot for the
    /// initial `CharacterStats` reply pushed at Welcome time.
    pub fn player_stats_snapshot(
        &self,
        client_id: ClientId,
    ) -> Option<(u32, u64, u64)> {
        let &entity = self.sessions.get(&client_id)?;
        let p = self.world.get::<&ServerPlayer>(entity).ok()?;
        Some((
            p.experience.level,
            p.experience.current_xp,
            p.experience.xp_to_next_level(),
        ))
    }

    /// Replace the entire ability loadout for `client_id`. Used
    /// at hydrate time to restore the persisted bar after a
    /// fresh `Hello`. No-op when the client isn't connected.
    pub fn set_player_loadout(
        &mut self,
        client_id: ClientId,
        slots: [u8; 6],
    ) {
        let Some(&entity) = self.sessions.get(&client_id) else { return };
        if let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) {
            p.loadout = rift_game::loadout::Loadout::from_slots(slots);
        }
    }

    /// Snapshot of the authoritative ability loadout. Used by
    /// the session handler to push `ServerMsg::Loadout` to the
    /// owning client at Welcome time and after every accepted
    /// `SetLoadoutSlot`.
    pub fn player_loadout_snapshot(
        &self,
        client_id: ClientId,
    ) -> Option<[u8; 6]> {
        let &entity = self.sessions.get(&client_id)?;
        let p = self.world.get::<&ServerPlayer>(entity).ok()?;
        Some(p.loadout.slots)
    }

    /// Mutate one slot of the player's ability bar. Validates:
    /// - `slot_index` is in range *and* unlocked at the player's
    ///   current level (per `loadout::SLOT_UNLOCK_LEVELS`)
    /// - `ability_id` is either the empty-slot sentinel or a
    ///   player-castable ability whose own `unlock_level` the
    ///   player has reached.
    /// Returns the freshly-updated full loadout (so the caller
    /// can persist + reply in one go) or `None` if the request
    /// was rejected.
    pub fn set_player_loadout_slot(
        &mut self,
        client_id: ClientId,
        slot_index: u8,
        ability_id: u8,
    ) -> Option<[u8; 6]> {
        let slot_idx = slot_index as usize;
        if slot_idx >= rift_game::loadout::SLOT_COUNT {
            return None;
        }
        let &entity = self.sessions.get(&client_id)?;
        let mut p = self.world.get::<&mut ServerPlayer>(entity).ok()?;
        let player_level = p.experience.level;
        if !rift_game::loadout::is_slot_unlocked(slot_idx, player_level) {
            return None;
        }
        // Allow either the empty sentinel (clearing the slot) or
        // an unlocked player-castable ability.
        let allow_empty = ability_id == rift_game::loadout::EMPTY_SLOT;
        let allow_ability = rift_game::loadout::is_player_ability(ability_id)
            && rift_game::loadout::is_ability_unlocked(ability_id, player_level);
        if !allow_empty && !allow_ability {
            return None;
        }
        p.loadout.set_slot(slot_idx, ability_id);
        Some(p.loadout.slots)
    }

    /// Move the bag item at `inventory_index` into its canonical
    /// equipment slot. If the slot is already filled, the
    /// previously-equipped item is pushed back to the bag at the
    /// same index so the UI position stays stable.
    ///
    /// Returns `true` on success. `false` indicates a no-op: bad
    /// index, item has no compatible slot, or the player isn't
    /// connected.
    pub fn equip_from_bag(&mut self, client_id: ClientId, inventory_index: usize) -> bool {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
            return false;
        };
        let Some(item) = p.inventory.get_mut(inventory_index).and_then(|s| s.take()) else {
            return false;
        };
        let slot = p.equipment.default_slot(&item);
        if !rift_game::loot::Equipment::accepts(slot, &item) {
            // Item base has no equip slot we accept — put it back
            // and bail. (Currently every BaseItem has a real slot,
            // so this branch is defensive.)
            p.inventory[inventory_index] = Some(item);
            return false;
        }
        let displaced = p.equipment.set(slot, Some(item));
        if let Some(prev) = displaced {
            // Re-occupy the same bag slot so the UI position the
            // client just saw stays stable across the swap.
            p.inventory[inventory_index] = Some(prev);
        }
        trim_trailing_none(&mut p.inventory);
        p.recompute_stats();
        true
    }

    /// Move the item currently in `slot` back into the bag (at
    /// the end). Returns `true` if anything actually moved;
    /// `false` for an empty slot or a stale client byte.
    pub fn unequip_to_bag(
        &mut self,
        client_id: ClientId,
        slot: rift_game::loot::EquipSlot,
    ) -> bool {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
            return false;
        };
        let Some(item) = p.equipment.take(slot) else {
            return false;
        };
        push_into_sparse(&mut p.inventory, item);
        p.recompute_stats();
        true
    }

    /// Snapshot the player's bag + equipment as a flat list of
    /// rolled items tagged with their current slot byte (or
    /// `None` for the bag) and the bag position the row last
    /// occupied. Bag rows carry their `Vec` index; equipped
    /// rows carry the equip-slot byte (the value is unused on
    /// load but kept stable so manual SQL inspection reads
    /// sensibly). Used by the persistence layer to produce a
    /// `ResetCharacterInventory` payload after every equip /
    /// unequip / reorder event.
    pub fn dump_player_inventory(
        &self,
        client_id: ClientId,
    ) -> Vec<(Option<u8>, i32, rift_game::loot::Item)> {
        let mut out = Vec::new();
        let Some(&entity) = self.sessions.get(&client_id) else {
            return out;
        };
        let Ok(p) = self.world.get::<&ServerPlayer>(entity) else {
            return out;
        };
        for (idx, slot) in p.inventory.iter().enumerate() {
            if let Some(it) = slot {
                out.push((None, idx as i32, it.clone()));
            }
        }
        for (slot, it) in p.equipment.iter() {
            out.push((Some(slot.to_u8()), slot.to_u8() as i32, it.clone()));
        }
        out
    }

    /// Borrow the player's bag (read-only). Used by the server's
    /// dispatch path to encode `InventorySync` payloads. Returns
    /// the sparse vec verbatim so empty slots are preserved on
    /// the wire.
    pub fn player_inventory(&self, client_id: ClientId) -> Vec<Option<rift_game::loot::Item>> {
        self.sessions
            .get(&client_id)
            .and_then(|&e| self.world.get::<&ServerPlayer>(e).ok())
            .map(|p| p.inventory.clone())
            .unwrap_or_default()
    }

    /// Borrow the player's equipment as `(slot, item)` pairs for
    /// every filled slot. Used by the server's dispatch path to
    /// encode `EquipmentSync` payloads.
    pub fn player_equipment(
        &self,
        client_id: ClientId,
    ) -> Vec<(rift_game::loot::EquipSlot, rift_game::loot::Item)> {
        self.sessions
            .get(&client_id)
            .and_then(|&e| self.world.get::<&ServerPlayer>(e).ok())
            .map(|p| p.equipment.iter().map(|(s, i)| (s, i.clone())).collect())
            .unwrap_or_default()
    }

    /// Borrow the player's stash (read-only). Used by the
    /// server's dispatch path to encode `StashSync` payloads.
    /// Sparse like [`Self::player_inventory`].
    pub fn player_stash(&self, client_id: ClientId) -> Vec<Option<rift_game::loot::Item>> {
        self.sessions
            .get(&client_id)
            .and_then(|&e| self.world.get::<&ServerPlayer>(e).ok())
            .map(|p| p.stash.clone())
            .unwrap_or_default()
    }

    /// Hydrate a freshly-spawned player's stash from the
    /// pre-loaded list (typically the rows fetched by
    /// `PersistenceHandle::load_stash_blocking`). Idempotent.
    pub fn set_player_stash(
        &mut self,
        client_id: ClientId,
        items: Vec<Option<rift_game::loot::Item>>,
    ) {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return;
        };
        if let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) {
            p.stash = items;
            trim_trailing_none(&mut p.stash);
        }
    }

    /// Toggle the per-player "stash session is open" flag.
    /// Set to `true` on a successful `OpenStash`, `false` on
    /// `CloseStash` / disconnect / floor transition. Gates
    /// every deposit / withdraw so an out-of-band transfer
    /// from a far-away client is rejected at the server edge.
    pub fn set_stash_open(&mut self, client_id: ClientId, open: bool) {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return;
        };
        if let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) {
            p.stash_open = open;
        }
    }

    /// Whether `client_id`'s current session has the chest open.
    pub fn is_stash_open(&self, client_id: ClientId) -> bool {
        self.sessions
            .get(&client_id)
            .and_then(|&e| self.world.get::<&ServerPlayer>(e).ok())
            .map(|p| p.stash_open)
            .unwrap_or(false)
    }

    /// Move the bag item at `inventory_index` to the end of the
    /// stash. Returns `true` on success; `false` if the index is
    /// out of range.
    pub fn deposit_to_stash(
        &mut self,
        client_id: ClientId,
        inventory_index: usize,
    ) -> bool {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
            return false;
        };
        let Some(item) = p.inventory.get_mut(inventory_index).and_then(|s| s.take()) else {
            return false;
        };
        push_into_sparse(&mut p.stash, item);
        trim_trailing_none(&mut p.inventory);
        true
    }

    /// Move the stash item at `stash_index` to the end of the
    /// bag. Returns `true` on success; `false` if the index is
    /// out of range.
    pub fn withdraw_from_stash(
        &mut self,
        client_id: ClientId,
        stash_index: usize,
    ) -> bool {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
            return false;
        };
        let Some(item) = p.stash.get_mut(stash_index).and_then(|s| s.take()) else {
            return false;
        };
        push_into_sparse(&mut p.inventory, item);
        trim_trailing_none(&mut p.stash);
        true
    }

    /// Deposit the bag item at `inventory_index` into a specific
    /// `stash_index`. If the destination is already occupied the
    /// two items swap (the prior stash occupant goes back to the
    /// freed bag slot). Grows the stash with `None` placeholders
    /// when `stash_index` is past the current length, then trims
    /// trailing `None`s on both containers.
    pub fn deposit_to_stash_slot(
        &mut self,
        client_id: ClientId,
        inventory_index: usize,
        stash_index: usize,
    ) -> bool {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
            return false;
        };
        // Source must hold an item.
        let Some(item) = p.inventory.get_mut(inventory_index).and_then(|s| s.take()) else {
            return false;
        };
        // Grow stash if dest is past the end.
        if stash_index >= p.stash.len() {
            p.stash.resize_with(stash_index + 1, || None);
        }
        // Swap-or-place: if the dest slot is occupied, the prior
        // occupant returns to the freed bag slot.
        let displaced = p.stash[stash_index].take();
        p.stash[stash_index] = Some(item);
        if let Some(prev) = displaced {
            // Re-grow the bag if needed (in practice we just
            // emptied that index, so it always fits).
            if inventory_index >= p.inventory.len() {
                p.inventory.resize_with(inventory_index + 1, || None);
            }
            p.inventory[inventory_index] = Some(prev);
        }
        trim_trailing_none(&mut p.inventory);
        trim_trailing_none(&mut p.stash);
        true
    }

    /// Withdraw the stash item at `stash_index` into a specific
    /// `inventory_index`. Mirror of `deposit_to_stash_slot`.
    pub fn withdraw_from_stash_slot(
        &mut self,
        client_id: ClientId,
        stash_index: usize,
        inventory_index: usize,
    ) -> bool {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
            return false;
        };
        let Some(item) = p.stash.get_mut(stash_index).and_then(|s| s.take()) else {
            return false;
        };
        if inventory_index >= p.inventory.len() {
            p.inventory.resize_with(inventory_index + 1, || None);
        }
        let displaced = p.inventory[inventory_index].take();
        p.inventory[inventory_index] = Some(item);
        if let Some(prev) = displaced {
            if stash_index >= p.stash.len() {
                p.stash.resize_with(stash_index + 1, || None);
            }
            p.stash[stash_index] = Some(prev);
        }
        trim_trailing_none(&mut p.inventory);
        trim_trailing_none(&mut p.stash);
        true
    }

    /// Snapshot the player's stash as a flat list of rolled
    /// items. Used by the persistence layer to produce a
    /// `ResetCharacterStash` payload after every deposit /
    /// withdraw. `equipped_slot` is always `None` for stash
    /// rows so the persisted shape stays slim.
    pub fn dump_player_stash(
        &self,
        client_id: ClientId,
    ) -> Vec<Option<rift_game::loot::Item>> {
        self.sessions
            .get(&client_id)
            .and_then(|&e| self.world.get::<&ServerPlayer>(e).ok())
            .map(|p| p.stash.clone())
            .unwrap_or_default()
    }

    /// Swap two bag slots, used by the inventory UI's
    /// drag-and-drop reorder path. Either index may be empty
    /// (past the current bag length); the bag is grown with
    /// `None` placeholders to fit, then trimmed back to the
    /// last filled slot. Returns `true` on success.
    pub fn swap_inventory_slots(
        &mut self,
        client_id: ClientId,
        a: usize,
        b: usize,
    ) -> bool {
        if a == b {
            return false;
        }
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
            return false;
        };
        let max = a.max(b);
        if max >= p.inventory.len() {
            // Both indices off the end of the bag = no-op.
            if a >= p.inventory.len() && b >= p.inventory.len() {
                return false;
            }
            p.inventory.resize_with(max + 1, || None);
        }
        p.inventory.swap(a, b);
        trim_trailing_none(&mut p.inventory);
        true
    }

    /// Swap two stash slots, used by the inventory UI's
    /// drag-and-drop reorder path inside the stash panel.
    /// Either index may be empty (past the current stash
    /// length); the stash is grown with `None` placeholders to
    /// fit, then trimmed back to the last filled slot. Returns
    /// `true` on success.
    pub fn swap_stash_slots(
        &mut self,
        client_id: ClientId,
        a: usize,
        b: usize,
    ) -> bool {
        if a == b {
            return false;
        }
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
            return false;
        };
        let max = a.max(b);
        if max >= p.stash.len() {
            if a >= p.stash.len() && b >= p.stash.len() {
                return false;
            }
            p.stash.resize_with(max + 1, || None);
        }
        p.stash.swap(a, b);
        trim_trailing_none(&mut p.stash);
        true
    }

    /// Remove the bag item at `inventory_index` and return it,
    /// along with the player's current world position so the
    /// caller can spawn a `ServerLoot` entity at the player's
    /// feet. `None` if the index is out of range or the player
    /// isn't connected.
    pub fn pop_inventory_item(
        &mut self,
        client_id: ClientId,
        inventory_index: usize,
    ) -> Option<(rift_game::loot::Item, glam::Vec3)> {
        let &entity = self.sessions.get(&client_id)?;
        let mut p = self.world.get::<&mut ServerPlayer>(entity).ok()?;
        let item = p.inventory.get_mut(inventory_index).and_then(|s| s.take())?;
        trim_trailing_none(&mut p.inventory);
        Some((item, p.k.position))
    }

    /// Move whatever's currently in `slot` into the bag at
    /// `inventory_index`, swapping with whatever is already
    /// there (or growing the bag if the index is past the end).
    /// Returns `true` on success.
    pub fn unequip_to_bag_slot(
        &mut self,
        client_id: ClientId,
        slot: rift_game::loot::EquipSlot,
        inventory_index: usize,
    ) -> bool {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
            return false;
        };
        let Some(unequipped) = p.equipment.take(slot) else {
            return false;
        };
        // Grow the bag to fit the requested index. The displaced
        // item (if any) gets re-equipped if it's compatible with
        // the slot, otherwise it lands at the first free bag
        // slot (or the end).
        if inventory_index >= p.inventory.len() {
            p.inventory.resize_with(inventory_index + 1, || None);
            p.inventory[inventory_index] = Some(unequipped);
        } else {
            let displaced = std::mem::replace(
                &mut p.inventory[inventory_index],
                Some(unequipped),
            );
            if let Some(prev) = displaced {
                if rift_game::loot::Equipment::accepts(slot, &prev) {
                    p.equipment.set(slot, Some(prev));
                } else {
                    push_into_sparse(&mut p.inventory, prev);
                }
            }
        }
        trim_trailing_none(&mut p.inventory);
        p.recompute_stats();
        true
    }

    /// Drain world events generated this tick. Caller broadcasts on
    /// `Channel::Event`.
    pub fn drain_events(&mut self) -> Vec<WorldEvent> {
        std::mem::take(&mut self.pending_events)
    }

    /// Spawn a `ServerLoot` entity at `position` carrying `item`,
    /// allocate it a fresh loot net-id, and queue a
    /// `WorldEvent::LootDropped` so every observer's loot
    /// pillar appears. Used by player-initiated drop-to-ground
    /// from the inventory UI.
    pub fn spawn_dropped_loot(&mut self, item: rift_game::loot::Item, position: glam::Vec3) {
        use rift_net::messages::ItemBlob;
        let net_id = rift_net::NetId(self.next_loot_net_id);
        self.next_loot_net_id = self.next_loot_net_id.wrapping_add(1);
        if self.next_loot_net_id >= 0x4000_0000 {
            self.next_loot_net_id = 0x2000_0000;
        }
        let (base_id, rarity, ilvl, affixes) = item.to_wire();
        let blob = ItemBlob { base_id, rarity, ilvl, affixes };
        let loot = loot::ServerLoot {
            net_id,
            position,
            item,
        };
        let _ = self.world.spawn((loot,));
        self.pending_events.push(WorldEvent::LootDropped {
            loot: net_id,
            item: blob,
            position: position.to_array(),
        });
    }

    /// Advance the simulation by one fixed timestep. `tick` is the
    /// server's current monotonic tick counter — channel ticks
    /// stamp it into their `WorldEvent::ChannelTick` so clients can
    /// interpolate against snapshot timing.
    pub fn step(&mut self, dt: f32, tick: NetTick) {
        // 1. Players: ingest inputs, integrate motion.
        player::apply_inputs(&mut self.world, &self.sessions, &mut self.pending_inputs);
        player::integrate_motion(&mut self.world, &self.floor, dt);

        // 2. Enemies: AI tick (queues melee damage + ranged
        //    shot requests), then integrate motion and spawn
        //    any caster bolts the AI asked for.
        let player_targets = player::target_positions(&self.world);
        let damage_mult = FloorConfig::for_floor(self.floor_index).enemy_damage_mult;
        let ai_outcome = enemy::tick_ai(&mut self.world, &player_targets, damage_mult, dt);
        let melee_damage = ai_outcome.melee_damage;
        // Unified enemy ability cast pipeline. Every enemy
        // attack flows through this single stream:
        //   * `Start` events translate into `AbilityCast` wire
        //     events so clients can play the telegraph.
        //   * `Resolve` events run authoritative effects via
        //     [`ability::resolve_enemy_cast`] which reads the
        //     ability's registry entry and produces projectiles
        //     / damage / summons. Summons go through a local
        //     queue so net-id allocation stays owned by Sim.
        let mut summon_queue: Vec<(glam::Vec3, u8, f32)> = Vec::new();
        let mut melee_from_resolves: Vec<(hecs::Entity, f32)> = Vec::new();
        for cast in ai_outcome.casts {
            match cast {
                enemy::EnemyCast::Start {
                    owner,
                    ability_id,
                    origin,
                    target,
                    dir_x,
                    dir_y,
                } => {
                    self.pending_events.push(rift_net::messages::WorldEvent::AbilityCast {
                        caster: owner,
                        ability: ability_id as u16,
                        origin: origin.to_array(),
                        dir: [dir_x, dir_y],
                        target: Some(target.to_array()),
                        start_tick: tick,
                    });
                }
                enemy::EnemyCast::Resolve {
                    owner,
                    ability_id,
                    origin,
                    aim,
                    damage_mult,
                    param_a,
                } => {
                    ability::resolve_enemy_cast(
                        ability::EnemyCastResolve {
                            caster: owner,
                            origin,
                            aim,
                            ability_id,
                            damage_mult,
                            param_a,
                        },
                        &player_targets,
                        &mut self.world,
                        &mut self.next_projectile_net_id,
                        &mut melee_from_resolves,
                        &mut summon_queue,
                        &mut self.pending_events,
                        tick,
                    );
                }
            }
        }
        // Drain any summons queued during cast resolves into
        // real enemy entities. Net-ids come from the same
        // allocator the floor packs use so clients see them as
        // ordinary enemies.
        for (pos, role_byte, hp_mult) in &summon_queue {
            enemy::spawn_summon(
                &mut self.world,
                *pos,
                *role_byte,
                *hp_mult,
                self.floor_index,
                &mut self.next_enemy_net_id,
            );
        }
        // Merge the two damage queues (AI melee + cast
        // resolves) into one for the player-damage pass.
        let mut melee_damage = melee_damage;
        melee_damage.extend(melee_from_resolves);
        enemy::integrate_motion(&mut self.world, &self.floor, dt);

        // 3. Apply queued enemy → player melee damage. Players
        //    crossing 0 hp emit a `Death` event and queue a
        //    `(client_id, net_id)` entry for the main loop. The
        //    first death on a non-hub floor also arms the
        //    auto-respawn timer.
        apply_player_damage(
            &mut self.world,
            &mut self.pending_events,
            &mut self.pending_player_deaths,
            melee_damage,
        );
        self.check_party_wipe();

        // 4. Tick ability cooldowns.
        ability::tick_cooldowns(&mut self.cooldowns, dt);

        // 5. Snapshot enemies for collision queries, then run
        //    projectiles + AoE zones + channels against them.
        //    All damage paths share one `DeathCtx` so DoT and
        //    direct kills both run through `loot::finalise_kills`
        //    (which emits `Death`, rolls drops, and despawns).
        let enemies = enemy::snapshot_for_collision(&self.world);

        let mut kills: Vec<loot::KillInfo> = Vec::new();
        let mut ctx = loot::DeathCtx {
            events: &mut self.pending_events,
            next_loot_net_id: &mut self.next_loot_net_id,
            tick,
            floor_index: self.floor_index,
            kills: &mut kills,
        };
        // Unified projectile tick — handles both player→enemy
        // and enemy→player bolts (distinguished by `Team`).
        // Enemy-team hits are returned as `(player, damage)`
        // rows for the player-damage path below; the player-
        // team path runs through `DeathCtx` like before, so
        // event ordering stays consistent.
        let enemy_proj_damage = projectile::tick(
            &mut self.world,
            &self.floor,
            &enemies,
            &player_targets,
            &mut ctx,
            dt,
        );
        projectile::tick_aoe(
            &mut self.world,
            &mut self.aoe_zones,
            &enemies,
            &mut ctx,
            dt,
        );
        channel::tick(
            &mut self.world,
            &enemies,
            &mut ctx,
            tick,
            dt,
        );

        // 6. Tick debuff stacks: decay durations, fire DoT damage,
        //    drop expired entries. Runs last so DoT events ride
        //    out on this frame's snapshot.
        debuff::tick(&mut self.world, &mut ctx, dt);

        // Apply the enemy-projectile damage collected before the
        // `DeathCtx` scope. Done here, after `ctx` is dropped, so
        // the player-damage path can borrow `pending_events` /
        // `pending_player_deaths` without aliasing.
        if !enemy_proj_damage.is_empty() {
            apply_player_damage(
                &mut self.world,
                &mut self.pending_events,
                &mut self.pending_player_deaths,
                enemy_proj_damage,
            );
            self.check_party_wipe();
        }

        // 6b. Tick revive shrines after the DeathCtx scope ends so
        //     the borrow on `pending_events` is free. `shrine::tick`
        //     pushes `WorldEvent::PlayersRevived` directly into the
        //     event queue, so the broadcast picks it up this tick.
        shrine::tick(&mut self.world, &mut self.pending_events, dt);

        // 7. Death-fade: tick the death timer on dying enemies and
        //    despawn rows whose timer hit zero. Kept separate from
        //    the kill path so the corpse stays in snapshots long
        //    enough for the client to play its `Death` clip.
        enemy::tick_dying(&mut self.world, dt);

        // 8. Award XP + bump rift progress for every kill this
        //    tick. Boss kills end the floor; non-boss kills push
        //    the progress bar and may trigger the boss spawn.
        if !kills.is_empty() {
            self.process_kills(&kills);
        }

        // 9. Wipe-respawn countdown. `check_party_wipe` arms
        //    this only when every player on a non-hub floor is
        //    dead; the main loop reads it via
        //    [`Self::take_hub_respawn_request`] when it expires
        //    and force-loads everyone back to the hub.
        if let Some(t) = self.hub_respawn_timer.as_mut() {
            *t -= dt;
        }

        // 10. Per-player ghost-rise countdown. Each dead player
        //     ticks their own timer; when it hits 0 they flip
        //     `is_ghost = true` which (a) lets `apply_inputs`
        //     accept movement next tick and (b) makes the
        //     snapshot pipeline drop their row from every other
        //     viewer's outbound snapshot. We also emit a
        //     `PlayerGhosted` event so remote clients can play
        //     a poof VFX at the body's last position instead of
        //     watching the avatar pop out of existence.
        let mut risen: Vec<(NetId, [f32; 3])> = Vec::new();
        for (_e, p) in self.world.query_mut::<&mut player::ServerPlayer>() {
            if let Some(t) = p.ghost_rise_timer.as_mut() {
                *t -= dt;
                if *t <= 0.0 {
                    p.ghost_rise_timer = None;
                    p.is_ghost = true;
                    risen.push((p.net_id, p.k.position.to_array()));
                }
            }
        }
        for (entity, position) in risen {
            self.pending_events.push(WorldEvent::PlayerGhosted {
                entity,
                position,
            });
        }
    }

    /// Resolve every kill produced by the damage subsystems this
    /// tick. Walks the list once: bumps rift progress for normal
    /// kills, flips `floor_complete` when the boss dies, spawns
    /// the boss when progress hits required, and grants XP to
    /// every connected player. Sets `progress_dirty` whenever the
    /// rift state changes so the main loop broadcasts a fresh
    /// `RiftProgress` next iteration.
    fn process_kills(&mut self, kills: &[loot::KillInfo]) {

        let mut spawn_boss_now = false;
        for k in kills {
            if k.role == enemy::role::BOSS {
                if !self.rift_progress.boss_killed {
                    self.rift_progress.boss_killed = true;
                    self.rift_progress.floor_complete = true;
                    self.progress_dirty = true;
                    log::info!(
                        "sim: floor {} boss killed — floor complete",
                        self.floor_index
                    );
                }
            } else if !self.rift_progress.boss_spawned {
                if self.rift_progress.required > 0 {
                    let next = (self.rift_progress.progress + 1)
                        .min(self.rift_progress.required);
                    if next != self.rift_progress.progress {
                        self.rift_progress.progress = next;
                        self.progress_dirty = true;
                    }
                    if self.rift_progress.progress >= self.rift_progress.required {
                        spawn_boss_now = true;
                    }
                }
            }
        }

        // Grant XP to every connected player. Use their current
        // level for the kill-XP scaling so over-levelled players
        // get diminished returns.
        let monster_level = (self.floor_index as u32).max(1);
        let player_entities: Vec<(ClientId, Entity)> = self
            .sessions
            .iter()
            .map(|(c, e)| (*c, *e))
            .collect();
        for (cid, entity) in player_entities {
            let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
                continue;
            };
            // Skip dead players, ghosts, and players in the
            // down-pose waiting to rise. Awarding XP here would
            // also trigger a level-up heal that resurrects a
            // player who died on the same tick.
            if p.is_dead_or_ghosting() {
                continue;
            }
            let mut total = 0u64;
            for _ in kills.iter().filter(|k| k.role != enemy::role::BOSS) {
                total += rift_game::experience::Experience::xp_for_kill(
                    monster_level,
                    p.experience.level,
                );
            }
            // Boss kills are worth a fat lump of XP \u2014 5\u00d7 a
            // normal kill at the floor's monster level.
            for _ in kills.iter().filter(|k| k.role == enemy::role::BOSS) {
                total += rift_game::experience::Experience::xp_for_kill(
                    monster_level,
                    p.experience.level,
                ) * 5;
            }
            if total == 0 {
                continue;
            }
            p.grant_xp(total);
            self.pending_stat_updates.push(StatsUpdate {
                client_id: cid,
                level: p.experience.level,
                xp: p.experience.current_xp,
                xp_to_next: p.experience.xp_to_next_level(),
                total_xp: p.experience.total_xp,
            });
        }

        if spawn_boss_now {
            self.spawn_boss();
        }
    }

    /// Spawn the floor's boss in the BSP-derived `boss_room_center`.
    /// Higher HP, slower speed, role = `enemy::role::BOSS`.
    /// Idempotent against `rift_progress.boss_spawned`.
    fn spawn_boss(&mut self) {
        if self.rift_progress.boss_spawned {
            return;
        }
        let boss_pos = Vec3::new(
            self.floor.boss_room_center.x,
            0.0,
            self.floor.boss_room_center.z,
        );
        let cfg = FloorConfig::for_floor(self.floor_index);
        let hp = cfg.enemy_health * 8.0 + self.floor_index as f32 * 30.0;
        let speed = cfg.enemy_speed * 0.7;
        let net_id = NetId(self.next_enemy_net_id);
        self.next_enemy_net_id = self.next_enemy_net_id.wrapping_add(1).max(1);
        let enemy = enemy::ServerEnemy {
            net_id,
            role: enemy::role::BOSS,
            k: rift_game::kinematic::Kinematic {
                position: boss_pos,
                velocity: Vec3::ZERO,
                yaw: 0.0,
                aim_yaw: 0.0,
                locomotion: rift_game::kinematic::loco::IDLE,
                vy: 0.0,
                airborne: false,
                ..Default::default()
            },
            target_lock: None,
            speed,
            hp_max: hp,
            hp,
            attack_cooldown: 0.0,
            attack_anim_remaining: 0.0,
            dying_remaining: 0.0,
            ai_phase: enemy::AiPhase::default(),
        };
        self.world
            .spawn((enemy, debuff::DebuffStack::default(), enemy::BossState::new(self.floor_index)));
        self.rift_progress.boss_spawned = true;
        self.progress_dirty = true;
        log::info!(
            "sim: floor {} boss spawned at {:?} (hp={hp:.0})",
            self.floor_index, boss_pos
        );
    }

    /// Drain any per-player stat updates queued this tick.
    pub fn drain_stat_updates(&mut self) -> Vec<StatsUpdate> {
        std::mem::take(&mut self.pending_stat_updates)
    }

    /// Take the current rift-progress snapshot iff something
    /// changed since the last drain. Returns `None` when there's
    /// nothing to broadcast.
    pub fn take_rift_progress_update(&mut self) -> Option<RiftProgress> {
        if self.progress_dirty {
            self.progress_dirty = false;
            Some(self.rift_progress)
        } else {
            None
        }
    }

    /// Read the current rift progress (for use at Welcome time
    /// without consuming the dirty flag).
    pub fn rift_progress(&self) -> RiftProgress {
        self.rift_progress
    }

    /// Drain any queued player deaths produced by the latest
    /// tick. The main loop turns each entry into a broadcast
    /// `WorldEvent::Death` so every client (not just the dier)
    /// can play the death animation.
    pub fn drain_player_deaths(&mut self) -> Vec<(ClientId, NetId)> {
        std::mem::take(&mut self.pending_player_deaths)
    }

    /// Arm [`Self::hub_respawn_timer`] when every player on a
    /// non-hub floor has hit zero HP. Idempotent — safe to call
    /// from every damage-application site. Single deaths leave
    /// the survivor(s) playing on; only a full party wipe pulls
    /// everyone back to safety.
    fn check_party_wipe(&mut self) {
        if self.floor_index == 0 || self.hub_respawn_timer.is_some() {
            return;
        }
        let mut total = 0usize;
        let mut dead = 0usize;
        for (_e, p) in self.world.query::<&ServerPlayer>().iter() {
            total += 1;
            if p.hp <= 0.0 {
                dead += 1;
            }
        }
        if total > 0 && dead == total {
            log::info!(
                "sim: party wipe on floor {} ({} players); arming hub respawn",
                self.floor_index,
                total
            );
            self.hub_respawn_timer = Some(HUB_RESPAWN_DELAY);
        }
    }

    /// Wipe inventory **and** equipment of every dead player.
    /// Intended for the wipe-respawn path: called by the main
    /// loop right before [`Self::change_floor`] when
    /// [`Self::take_hub_respawn_request`] returns `true`. Stash
    /// is untouched. Returns the affected `client_id`s so the
    /// main loop can fan out fresh `InventorySync` +
    /// `EquipmentSync` and persist the new (empty) bag.
    pub fn wipe_dead_loot(&mut self) -> Vec<ClientId> {
        let mut affected: Vec<ClientId> = Vec::new();
        for (_e, p) in self.world.query_mut::<&mut ServerPlayer>() {
            if p.hp > 0.0 {
                continue;
            }
            p.inventory.clear();
            p.equipment = rift_game::loot::Equipment::new();
            p.recompute_stats();
            affected.push(p.client_id);
        }
        if !affected.is_empty() {
            log::info!(
                "sim: wiped loot for {} dead player(s) on rift exit",
                affected.len()
            );
        }
        affected
    }

    /// Outcome of [`Self::request_exit_vote`]: either an
    /// instant-pass (solo, must be exited immediately by the
    /// caller), an opened vote window, or a refusal (cooldown,
    /// already in hub, dead, etc.).
    ///
    /// See the `ExitVoteRequest` enum below for variants.

    /// Handle a [`rift_net::ClientMsg::RiftExitVoteStart`] from
    /// `client_id`. Solo players (one connected) get an instant
    /// `Pass` outcome — caller wipes dead-player loot and
    /// transitions to the hub. Multiplayer parties get a fresh
    /// vote window opened with the initiator auto-recorded as
    /// `Yes`; subsequent ticks resolve via [`Self::tick_exit_vote`].
    ///
    /// Silently rejected (returns `Refused`) if:
    /// - we're already in the hub,
    /// - the caster is in the down-pose (dead but not yet a
    ///   ghost — the rise timer hasn't elapsed),
    /// - a vote is already active,
    /// - the cooldown timer hasn't expired yet.
    ///
    /// Ghost initiators are refused: a ghost could otherwise
    /// gatekeep their living teammates inside the rift by
    /// repeatedly opening votes (or by being the lone holdout
    /// initiator on a vote whose other voters can't even see
    /// them). Ghosts also can't cast on an open vote (the roll
    /// is built from living players only). Party-wipe recovery
    /// is handled by the existing hub-respawn timer.
    pub fn request_exit_vote(
        &mut self,
        client_id: ClientId,
    ) -> ExitVoteRequest {
        if self.floor_index == 0 {
            return ExitVoteRequest::Refused;
        }
        if self.exit_vote.is_some() || self.exit_vote_cooldown > 0.0 {
            return ExitVoteRequest::Refused;
        }
        let Some(&entity) = self.sessions.get(&client_id) else {
            return ExitVoteRequest::Refused;
        };
        let initiator_alive = self
            .world
            .get::<&ServerPlayer>(entity)
            .map(|p| p.hp > 0.0 && !p.is_ghost)
            .unwrap_or(false);
        // Down-pose (dead pre-rise) and ghosts are both refused.
        // Only living teammates can open or cast on a vote.
        if !initiator_alive {
            return ExitVoteRequest::Refused;
        }
        // Build the living-voter roll up front so we know whether
        // we're solo or party. Ghosts are never voters.
        let mut roll: HashMap<NetId, VoteChoice> = HashMap::new();
        let mut initiator_net_id: Option<NetId> = None;
        for (_e, p) in self.world.query::<&ServerPlayer>().iter() {
            if p.hp <= 0.0 {
                continue;
            }
            roll.insert(p.net_id, VoteChoice::Pending);
            if p.client_id == client_id {
                initiator_net_id = Some(p.net_id);
            }
        }
        // Solo: alive caller is the only living player.
        if roll.len() <= 1 {
            log::info!("vote: solo exit by {:?} instant pass", client_id);
            return ExitVoteRequest::Pass;
        }
        // Multiplayer: stamp the initiator as Yes immediately.
        if let Some(nid) = initiator_net_id {
            roll.insert(nid, VoteChoice::Yes);
        }
        log::info!(
            "vote: opened by {:?} ({} living voters)",
            client_id,
            roll.len()
        );
        self.exit_vote = Some(vote::ExitVote {
            kind: rift_net::messages::VoteKind::Exit,
            time_remaining: vote::VOTE_DURATION,
            votes: roll,
        });
        self.exit_vote_dirty = true;
        ExitVoteRequest::Opened
    }

    /// Handle a [`rift_net::ClientMsg::RequestEnterRift`] received
    /// while currently on a rift floor. Solo parties bypass this
    /// path and fall through to instant transition. Multiplayer
    /// parties open a 15s ready-check vote so one player pressing
    /// F at the exit portal doesn't yank everyone else into the
    /// next floor unprepared. Same shape + lifetime as
    /// [`Self::request_exit_vote`]; only `kind` and the
    /// resolution path differ (see [`Self::tick_exit_vote`]).
    pub fn request_descend_vote(
        &mut self,
        client_id: ClientId,
    ) -> ExitVoteRequest {
        if self.floor_index == 0 {
            // Hub \u2192 first floor is always instant. Caller falls
            // through to the transition path.
            return ExitVoteRequest::Refused;
        }
        if self.exit_vote.is_some() || self.exit_vote_cooldown > 0.0 {
            return ExitVoteRequest::Refused;
        }
        let Some(&entity) = self.sessions.get(&client_id) else {
            return ExitVoteRequest::Refused;
        };
        let initiator_alive = self
            .world
            .get::<&ServerPlayer>(entity)
            .map(|p| p.hp > 0.0 && !p.is_ghost)
            .unwrap_or(false);
        if !initiator_alive {
            return ExitVoteRequest::Refused;
        }
        let mut roll: HashMap<NetId, VoteChoice> = HashMap::new();
        let mut initiator_net_id: Option<NetId> = None;
        for (_e, p) in self.world.query::<&ServerPlayer>().iter() {
            if p.hp <= 0.0 {
                continue;
            }
            roll.insert(p.net_id, VoteChoice::Pending);
            if p.client_id == client_id {
                initiator_net_id = Some(p.net_id);
            }
        }
        if roll.len() <= 1 {
            log::info!("vote: solo descend by {:?} instant pass", client_id);
            return ExitVoteRequest::Pass;
        }
        if let Some(nid) = initiator_net_id {
            roll.insert(nid, VoteChoice::Yes);
        }
        log::info!(
            "vote: descend opened by {:?} ({} living voters)",
            client_id,
            roll.len()
        );
        self.exit_vote = Some(vote::ExitVote {
            kind: rift_net::messages::VoteKind::Descend,
            time_remaining: vote::VOTE_DURATION,
            votes: roll,
        });
        self.exit_vote_dirty = true;
        ExitVoteRequest::Opened
    }

    /// Handle a [`rift_net::ClientMsg::RiftExitVoteCast`] from
    /// `client_id`. Silently no-ops when no vote is active, the
    /// caster isn't on the voter roll, or the caster has already
    /// voted. Sets the dirty flag so the main loop broadcasts a
    /// fresh `RiftExitVote` next iteration.
    pub fn cast_exit_vote(&mut self, client_id: ClientId, yes: bool) {
        let Some(vote) = self.exit_vote.as_mut() else { return };
        let Some(&entity) = self.sessions.get(&client_id) else { return };
        let Some(net_id) = self
            .world
            .get::<&ServerPlayer>(entity)
            .ok()
            .map(|p| p.net_id)
        else {
            return;
        };
        let Some(slot) = vote.votes.get_mut(&net_id) else { return };
        if !matches!(slot, VoteChoice::Pending) {
            // No changing your mind.
            return;
        }
        *slot = if yes { VoteChoice::Yes } else { VoteChoice::No };
        log::info!(
            "vote: {:?} cast {}",
            client_id,
            if yes { "YES" } else { "NO" }
        );
        self.exit_vote_dirty = true;
    }

    /// Per-tick: decrement the active vote's deadline / cooldown
    /// and resolve once outcome is known. Returns the resolution
    /// so the main loop can wipe dead-player loot + transition to
    /// the hub on a `Pass`.
    pub fn tick_exit_vote(&mut self, dt: f32) -> vote::TickOutcome {
        // Cooldown countdown (independent of any active vote).
        if self.exit_vote_cooldown > 0.0 {
            let prev = self.exit_vote_cooldown;
            self.exit_vote_cooldown = (prev - dt).max(0.0);
            // Mark dirty when we cross integer-second boundaries
            // so the HUD ring animates smoothly. Cheap: at most
            // one extra broadcast per second.
            if prev.ceil() != self.exit_vote_cooldown.ceil() {
                self.exit_vote_dirty = true;
            }
        }
        let Some(vote) = self.exit_vote.as_mut() else {
            return vote::TickOutcome::Idle;
        };
        let prev_remaining = vote.time_remaining;
        vote.time_remaining = (prev_remaining - dt).max(0.0);
        if prev_remaining.ceil() != vote.time_remaining.ceil() {
            // Tick boundary: HUD countdown ring updates.
            self.exit_vote_dirty = true;
        }
        match vote::resolve(vote) {
            vote::TickOutcome::Idle => vote::TickOutcome::Idle,
            vote::TickOutcome::Passed(kind) => {
                log::info!("vote: passed unanimously ({:?})", kind);
                self.exit_vote = None;
                self.exit_vote_cooldown = 0.0;
                self.exit_vote_dirty = true;
                vote::TickOutcome::Passed(kind)
            }
            vote::TickOutcome::Fizzled => {
                log::info!(
                    "vote: fizzled (no/timeout) — {}s cooldown",
                    vote::VOTE_COOLDOWN as u32
                );
                self.exit_vote = None;
                self.exit_vote_cooldown = vote::VOTE_COOLDOWN;
                self.exit_vote_dirty = true;
                vote::TickOutcome::Fizzled
            }
        }
    }

    /// Drain the dirty flag and produce a wire-shape
    /// [`VoteState`] reflecting the current sim state. The main
    /// loop ships this as `ServerMsg::RiftExitVote` whenever it
    /// returns `Some`.
    pub fn take_exit_vote_update(&mut self) -> Option<rift_net::messages::VoteState> {
        if !self.exit_vote_dirty {
            return None;
        }
        self.exit_vote_dirty = false;
        Some(vote::build_state(
            self.exit_vote.as_ref(),
            self.exit_vote_cooldown,
        ))
    }

    /// `true` once the post-death countdown has elapsed. Consumes
    /// the request — callers are expected to immediately drive
    /// `change_floor(0)`. Returns `false` while the timer is
    /// still running, or when no death is pending.
    pub fn take_hub_respawn_request(&mut self) -> bool {
        match self.hub_respawn_timer {
            Some(t) if t <= 0.0 => {
                self.hub_respawn_timer = None;
                true
            }
            _ => false,
        }
    }

    /// Build the snapshot for one receiving client.
    pub fn build_snapshot(&self, tick: NetTick, ack_for: ClientId) -> Snapshot {
        snapshot::build(&self.world, tick, ack_for)
    }
}

/// Apply queued enemy → player melee damage and emit one `Damage`
/// event per applied hit. Deaths transition the player into the
/// "dead" snapshot flag and queue a `(client_id, net_id)` entry
/// for the caller to broadcast as `WorldEvent::Death`.
fn apply_player_damage(
    world: &mut hecs::World,
    events: &mut Vec<WorldEvent>,
    deaths: &mut Vec<(ClientId, NetId)>,
    pending: Vec<(Entity, f32)>,
) {
    for (player_entity, amount) in pending {
        if let Ok(mut p) = world.get::<&mut ServerPlayer>(player_entity) {
            // Already dead: ignore further hits so the death
            // event only fires once and we don't spam damage
            // numbers on a corpse.
            if p.hp <= 0.0 {
                continue;
            }
            let was_alive = p.hp > 0.0;
            p.hp = (p.hp - amount).max(0.0);
            let pos = p.k.position;
            let net_id = p.net_id;
            let client_id = p.client_id;
            let died = was_alive && p.hp <= 0.0;
            if died {
                p.is_ghost = false;
                p.ghost_rise_timer = Some(GHOST_RISE_DELAY);
            }
            drop(p);
            events.push(WorldEvent::Damage {
                target: net_id,
                amount,
                crit: false,
                position: pos.to_array(),
            });
            if died {
                events.push(WorldEvent::Death {
                    entity: net_id,
                    killer: None,
                });
                deaths.push((client_id, net_id));
            }
        }
    }
}
