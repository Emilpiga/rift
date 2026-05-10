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
    messages::{InputCmd, Snapshot, WorldEvent},
    ClientId, NetId, NetTick,
};

pub mod ability;
pub mod channel;
pub mod combat_ctx;
pub mod effect;
pub mod enemies;
pub mod floor;
pub mod loot;
pub mod meters;
pub mod player;
pub mod projectile;
pub mod shrine;
pub mod snapshot;
pub mod vote;
pub mod procs;
pub mod transforms;
pub mod stash_ops;
pub mod inventory_ops;
pub mod voting_ops;
pub mod loot_ops;
pub mod damage;
pub mod step;
pub mod player_ops;
pub mod ability_ops;
pub mod floor_ops;

pub use player::ServerPlayer;
pub use player::StashTab;
pub use projectile::ServerAoeZone;

/// Drop trailing `None`s from a sparse bag/stash so its `len()`
/// tracks the highest occupied slot + 1. Keeps wire payloads
/// minimal and prevents the bag from growing unbounded.
pub(super) fn trim_trailing_none<T>(v: &mut Vec<Option<T>>) {
    while matches!(v.last(), Some(None)) {
        v.pop();
    }
}

/// Push `item` into the first empty slot of a sparse bag/stash,
/// or append to the end if every slot is occupied. Used by
/// pickups and unequip-into-bag flows where the caller doesn't
/// have an explicit destination index.
pub(super) fn push_into_sparse<T>(v: &mut Vec<Option<T>>, item: T) {
    if let Some(slot) = v.iter_mut().find(|s| s.is_none()) {
        *slot = Some(item);
    } else {
        v.push(Some(item));
    }
}

/// Count of filled slots in a sparse bag/stash. Used by debug
/// logs that previously read `Vec::len()`.
pub(super) fn count_filled<T>(v: &[Option<T>]) -> usize {
    v.iter().filter(|s| s.is_some()).count()
}

/// Per-rarity base salvage yield, lightly scaled by ilvl. The
/// curve `1 + ilvl/20` keeps early salvage meaningful (a level-1
/// Common still mints 1 shard) while letting deep-floor drops be
/// noticeably more valuable (an ilvl-40 Rare mints 24 shards).
pub fn salvage_yield(rarity: rift_game::loot::Rarity, ilvl: u32) -> u32 {
    rift_game::loot::salvage_yield(rarity, ilvl)
}

/// Maximum XZ distance (metres) between the picker and a ground
/// loot drop for a [`ClientMsg::PickUpLoot`] to succeed.
pub const PICKUP_RANGE: f32 = 2.0;

/// Server ticks a player-dropped item stays restricted to its
/// originating party snapshot. After the window closes, eligibility
/// is lifted and any peer in the Sim can pick it up. Sized to span
/// a full post-run gathering — long enough to portal back to town
/// and pass loot around, short enough that a returning stranger
/// can't farm a friend's freshly-dropped gear.
///
/// Encoded in ticks (server ticks at [`rift_net::TICK_HZ`] = 30 Hz)
/// so the gate evaluates in pure tick math without a wall-clock
/// dependency. 15 minutes × 60 s × 30 Hz = 27 000 ticks.
pub const SHARE_WINDOW_TICKS: u32 = 15 * 60 * 30;

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

    /// Per-client cumulative combat meters for this run.
    /// Reset by the server main loop on instance entry; ticked
    /// every step (`elapsed += dt`); broadcast ~1 Hz as
    /// `ServerMsg::MeterSnapshot`.
    pub meters: meters::Meters,

    /// Most recent server tick observed by [`Self::step`]. Cached
    /// here so non-step paths (loot-drop, pickup-gate, persistence
    /// hooks) can evaluate tick-relative deadlines like the loot
    /// share window without threading the live tick through
    /// every call site.
    current_tick: NetTick,
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
    /// `true` when this update represents at least one level
    /// transition this tick (XP gain crossed one or more
    /// thresholds). The server uses this to drive a SYSTEM
    /// chat line to the levelled-up player without having to
    /// remember the previous level itself.
    pub levelled_up: bool,
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
            meters: meters::Meters::default(),
            current_tick: NetTick(0),
        };
        enemies::spawn_for_floor(
            &mut sim.world,
            &sim.floor,
            sim.floor_index,
            &mut sim.next_enemy_net_id,
        );
        sim
    }


    /// Drain world events generated this tick. Caller broadcasts on
    /// `Channel::Event`.
    pub fn drain_events(&mut self) -> Vec<WorldEvent> {
        std::mem::take(&mut self.pending_events)
    }

    /// `true` when this Sim represents the hub world (floor
    /// index 0). Used by the inventory-drop handler to refuse
    /// drop requests in town — without this gate, players could
    /// dump endgame gear in the hub for under-level alts to
    /// scoop up, defeating the level-requirement system.
    pub fn is_hub(&self) -> bool {
        self.floor_index == 0
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
            // Anchored items survive every wipe path. Filter
            // bag + equipment in place so the player keeps the
            // chase drops they earned, while everything else
            // (regular legendaries included) is lost as usual.
            //
            // **Unstable wins over Anchored.** A legendary
            // anchored item picked up inside the rift is still
            // unstable until extraction; dying before extracting
            // shatters it just like every other unstable drop.
            // The "death shatters unstable loot" contract is
            // absolute \u2014 anchored only protects items that
            // already cleared a previous extraction.
            let mut kept_inventory: Vec<Option<rift_game::loot::Item>> = Vec::new();
            for slot in p.inventory.drain(..) {
                if let Some(it) = slot {
                    if it.anchored && !it.unstable {
                        kept_inventory.push(Some(it));
                    }
                }
            }
            p.inventory = kept_inventory;
            // Equipment: same anchored-survives rule, but
            // *keep* anchored items in their slots. Dropping
            // them into the bag would force the player to re-
            // equip on respawn for no gameplay reason. We walk
            // every slot, take the item, and either put it
            // back (anchored) or let it fall into the void
            // (everything else).
            for slot in rift_game::loot::EquipSlot::ALL {
                if let Some(it) = p.equipment.take(slot) {
                    if it.anchored && !it.unstable {
                        p.equipment.set(slot, Some(it));
                    }
                }
            }
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

    /// Stabilise every living player's bag + equipment,
    /// flipping every `unstable` item to stable. This is the
    /// "loot purified by extraction" gate — called on the
    /// successful extract-vote path *before* the players are
    /// returned to the hub so [`Server::move_client_to_hub`]'s
    /// defensive in-line strip sees nothing left to strip.
    /// Dead players are deliberately skipped: their unstable
    /// items shatter via [`Self::wipe_dead_loot`] regardless
    /// of whether the party voted to extract — a corpse never
    /// benefits from extraction.
    ///
    /// Returns the list of clients touched so the main loop
    /// can fan out a fresh `InventorySync` (the tooltip line
    /// changes from "⚠ Unstable" to clean) and persist the
    /// now-stable bag once they're back on the hub sim.
    pub fn stabilize_inventory(&mut self) -> Vec<ClientId> {
        let mut affected: Vec<ClientId> = Vec::new();
        for (_e, p) in self.world.query_mut::<&mut ServerPlayer>() {
            if p.hp <= 0.0 {
                continue;
            }
            let mut touched = false;
            for slot in p.inventory.iter_mut() {
                if let Some(it) = slot {
                    if it.unstable {
                        it.unstable = false;
                        touched = true;
                    }
                }
            }
            for slot in rift_game::loot::EquipSlot::ALL {
                if let Some(mut it) = p.equipment.take(slot) {
                    if it.unstable {
                        it.unstable = false;
                        touched = true;
                    }
                    p.equipment.set(slot, Some(it));
                }
            }
            if touched {
                affected.push(p.client_id);
            }
        }
        if !affected.is_empty() {
            log::info!(
                "sim: stabilised unstable loot for {} extracting player(s)",
                affected.len()
            );
        }
        affected
    }

    /// Remove every unstable item from every player's bag and
    /// equipment in this Sim. The "unsafe exit" counterpart to
    /// [`Self::stabilize_inventory`]. Currently unused — the
    /// hub-return path strips per-player inline against the
    /// extracted [`ServerPlayer`] in
    /// [`Server::move_client_to_hub`] — but kept around for
    /// future eviction / disconnect-grace callers that need
    /// to act on every player in the Sim at once.
    ///
    /// Returns clients that actually lost something so the
    /// caller can fan out a fresh `InventorySync` and persist
    /// the now-shrunken bag.
    #[allow(dead_code)]
    pub fn strip_unstable_loot(&mut self) -> Vec<ClientId> {
        let mut affected: Vec<ClientId> = Vec::new();
        for (_e, p) in self.world.query_mut::<&mut ServerPlayer>() {
            let before_bag = p.inventory.iter().filter(|s| s.is_some()).count();
            let mut kept: Vec<Option<rift_game::loot::Item>> = Vec::new();
            for slot in p.inventory.drain(..) {
                match slot {
                    Some(it) if it.unstable => {} // shatter
                    Some(it) => kept.push(Some(it)),
                    None => kept.push(None),
                }
            }
            // Trim trailing empties to mirror the sparse-Vec
            // invariant the rest of the inventory code relies on.
            while matches!(kept.last(), Some(None)) {
                kept.pop();
            }
            p.inventory = kept;
            let after_bag = p.inventory.iter().filter(|s| s.is_some()).count();

            let mut equip_lost = 0usize;
            for slot in rift_game::loot::EquipSlot::ALL {
                if let Some(it) = p.equipment.take(slot) {
                    if it.unstable {
                        equip_lost += 1;
                    } else {
                        p.equipment.set(slot, Some(it));
                    }
                }
            }
            if before_bag != after_bag || equip_lost > 0 {
                p.recompute_stats();
                affected.push(p.client_id);
            }
        }
        if !affected.is_empty() {
            log::info!(
                "sim: shattered unstable loot for {} player(s) on rift exit",
                affected.len()
            );
        }
        affected
    }

    /// Build the snapshot for one receiving client.
    pub fn build_snapshot(&self, tick: NetTick, ack_for: ClientId) -> Snapshot {
        snapshot::build(&self.world, tick, ack_for)
    }

    /// Build a per-instance meter broadcast. Caller scopes the
    /// send to the instance's members.
    pub fn build_meter_snapshot(&self) -> rift_net::messages::ServerMsg {
        self.meters.build_snapshot(&self.world)
    }
}


#[cfg(test)]
mod equip_tests;

#[cfg(test)]
mod provenance_tests;

#[cfg(test)]
impl Sim {
    /// Test-only helper: stuff `item` into `client_id`'s bag and
    /// immediately pop it back out, returning the item plus the
    /// player's current world position. Mirrors the shape of the
    /// `pop_inventory_item` → `spawn_player_drop` handoff so
    /// share-window tests can exercise the drop path without
    /// going through the full inventory plumbing.
    pub(crate) fn pop_inventory_item_from_seed(
        &mut self,
        client_id: rift_net::ids::ClientId,
        item: rift_game::loot::Item,
    ) -> (rift_game::loot::Item, glam::Vec3) {
        let entity = *self.sessions.get(&client_id).expect("session registered");
        {
            let mut p = self.world.get::<&mut ServerPlayer>(entity).unwrap();
            p.inventory.push(Some(item));
        }
        self.pop_inventory_item(client_id, 0)
            .expect("just-pushed item must pop")
    }
}
