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
pub mod debuff;
pub mod enemy;
pub mod floor;
pub mod loot;
pub mod player;
pub mod projectile;
pub mod snapshot;

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
    /// Counts down from [`HUB_RESPAWN_DELAY`] once any player has
    /// died on a non-hub floor. When it hits zero the main loop
    /// reads it via [`Sim::take_hub_respawn_request`] and drives
    /// `transition_floor(0)` so the dead player(s) get back to
    /// safety. `None` means “no respawn pending”.
    hub_respawn_timer: Option<f32>,
}

/// Wall-clock seconds the dying player's avatar lingers in the
/// rift before the server force-loads them back to the hub. Long
/// enough for the client's death animation to play through.
pub const HUB_RESPAWN_DELAY: f32 = 3.5;

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
        // Restore HP for everyone on the new floor. Floors only
        // change after a successful boss kill, a manual return to
        // hub, or a death-triggered respawn — in all three cases
        // the player should arrive at full HP.
        player::heal_all(&mut self.world);
        enemy::despawn_all(&mut self.world);
        projectile::despawn_all(&mut self.world);
        loot::despawn_all(&mut self.world);
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
        self.rift_progress = RiftProgress::for_floor(new_index);
        self.progress_dirty = true;
        // Wipe any in-flight death/respawn bookkeeping — the new
        // floor starts everyone alive and the timer should not
        // carry over.
        self.pending_player_deaths.clear();
        self.hub_respawn_timer = None;
        log::info!(
            "sim: changed to floor {new_index} (seed={}) at spawn {spawn:?}",
            self.floor_seed
        );
        spawn
    }

    /// Spawn (or look up the existing) player entity for a freshly-
    /// Helloed client. Returns the allocated `NetId`. `class_id`
    /// drives the initial [`CharacterStats`] snapshot baked into
    /// [`ServerPlayer::fresh`].
    pub fn spawn_player(
        &mut self,
        client_id: ClientId,
        class: rift_game::classes::ClassId,
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
            .spawn((ServerPlayer::fresh(client_id, net_id, spawn, class),));
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

        // 2. Enemies: AI tick (queues melee damage), then
        //    integrate motion.
        let player_targets = player::target_positions(&self.world);
        let melee_damage = enemy::tick_ai(&mut self.world, &player_targets, dt);
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
        if !self.pending_player_deaths.is_empty()
            && self.floor_index != 0
            && self.hub_respawn_timer.is_none()
        {
            self.hub_respawn_timer = Some(HUB_RESPAWN_DELAY);
        }

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
        projectile::tick(&mut self.world, &self.floor, &enemies, &mut ctx, dt);
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

        // 9. Death-respawn countdown. Once any player has died on
        //    a non-hub floor we tick this down; the main loop
        //    reads it via [`Self::take_hub_respawn_request`] when
        //    it expires and force-loads everyone back to the hub.
        if let Some(t) = self.hub_respawn_timer.as_mut() {
            *t -= dt;
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
            speed,
            hp_max: hp,
            hp,
            attack_cooldown: 0.0,
            attack_anim_remaining: 0.0,
            dying_remaining: 0.0,
        };
        self.world
            .spawn((enemy, debuff::DebuffStack::default()));
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
