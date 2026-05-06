//! Headless Rift Crawler server.
//!
//! Phase 1 scope: open a UDP/netcode endpoint, accept up to 4
//! clients, tick a hecs `World` at 30 Hz, and round-trip the
//! `Hello`/`Welcome` handshake. No floor generation, no AI, no
//! damage — those land in later phases when we're ready to share
//! simulation code with the client.
//!
//! ## CLI
//!
//! ```text
//! rift-server [--bind 0.0.0.0:34000] [--public 127.0.0.1:34000]
//! ```
//!
//! `--bind` is the socket address the OS opens. `--public` is what
//! we tell the netcode connect tokens — for local dev they're the
//! same; behind a Cloudflare/NAT relay they differ.

use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};

use anyhow::Result;
use rift_net::{
    decode, encode, open_server, renet, Channel, ClientId, ClientMsg, Gender, NetId, NetSettings,
    NetTick, ServerHandle, ServerMsg, MAX_CLIENTS, PROTOCOL_VERSION, SNAPSHOT_HZ, TICK_HZ,
};
use rift_net::messages::RosterEntry;
use rift_persistence::{CharacterRecord, PersistedItem, PersistenceHandle, Uuid};

mod session;
mod sim;

use session::{ClientSession, SessionManager};
use sim::Sim;

/// How often we kick off an opportunistic auto-save for every
/// connected character. Save is fire-and-forget on the server
/// loop; the persistence worker drains writes asynchronously and
/// never blocks gameplay.
const AUTO_SAVE_INTERVAL: Duration = Duration::from_secs(60);

/// Top-level server state. One instance per running server binary.
struct Server {
    handle: ServerHandle,
    /// Authoritative simulation clock.
    tick: NetTick,
    /// Active sessions, keyed by renet client id.
    sessions: SessionManager,
    /// Last instant we ticked simulation; used to compute fixed-step dt.
    last_tick: Instant,
    /// Authoritative world simulation.
    sim: Sim,
    /// Carries the leftover wall-clock between fixed-step ticks so
    /// we don't drift when frame_dt isn't a clean multiple of the
    /// tick period.
    tick_accumulator: Duration,
    /// Same idea for snapshot broadcasts (decoupled from sim rate).
    snapshot_accumulator: Duration,
    /// Persistence worker handle. `None` when the server is started
    /// without `--database-url`, in which case all characters are
    /// purely in-memory — useful for offline iteration / tests.
    persistence: Option<PersistenceHandle>,
    /// Wall-clock since the last opportunistic auto-save tick.
    auto_save_accumulator: Duration,
}

impl Server {
    fn new(
        bind: SocketAddr,
        public: SocketAddr,
        persistence: Option<PersistenceHandle>,
    ) -> Result<Self> {
        let handle = open_server(bind, public, MAX_CLIENTS, &NetSettings::default())?;
        // Phase 3: hard-code the floor seed and start everyone on the
        // hub (index 0). Floor transitions / lobby control land in
        // Phase 6.
        let sim = Sim::new(42, 0);
        Ok(Self {
            handle,
            tick: NetTick::default(),
            sessions: SessionManager::new(),
            last_tick: Instant::now(),
            sim,
            tick_accumulator: Duration::ZERO,
            snapshot_accumulator: Duration::ZERO,
            persistence,
            auto_save_accumulator: Duration::ZERO,
        })
    }

    /// One pass of the main loop: poll netcode, drain client messages,
    /// run a sim tick if it's time, send pending traffic.
    fn step(&mut self) -> Result<()> {
        let now = Instant::now();
        let frame_dt = now - self.last_tick;
        self.last_tick = now;

        // Drive the renet/netcode layer. Must happen before we
        // consume messages or send any.
        if let Err(e) = self.handle.transport.update(frame_dt, &mut self.handle.server) {
            log::error!("transport update: {e:?}");
        }
        // RenetServer also needs its own per-frame `update` to
        // advance reliability/resend timers.
        self.handle.server.update(frame_dt);

        // Pump connect/disconnect events.
        while let Some(event) = self.handle.server.get_event() {
            self.handle_server_event(event);
        }

        // Drain inbound messages from every connected client.
        let connected: Vec<u64> = self
            .handle
            .server
            .clients_id()
            .iter()
            .map(|id| id.raw())
            .collect();
        for raw_id in connected {
            // Snapshot channel carries unreliable client → server
            // input commands. We decode the same `ClientMsg` enum
            // as the other channels and dispatch identically.
            while let Some(bytes) = self
                .handle
                .server
                .receive_message(renet::ClientId::from_raw(raw_id), Channel::Snapshot as u8)
            {
                match decode::<ClientMsg>(&bytes) {
                    Ok(msg) => self.handle_client_msg(ClientId(raw_id), msg),
                    Err(e) => log::warn!("decode snapshot from {raw_id}: {e}"),
                }
            }

            while let Some(bytes) = self
                .handle
                .server
                .receive_message(renet::ClientId::from_raw(raw_id), Channel::Control as u8)
            {
                match decode::<ClientMsg>(&bytes) {
                    Ok(msg) => self.handle_client_msg(ClientId(raw_id), msg),
                    Err(e) => log::warn!("decode control from {raw_id}: {e}"),
                }
            }
            while let Some(bytes) = self
                .handle
                .server
                .receive_message(renet::ClientId::from_raw(raw_id), Channel::Event as u8)
            {
                match decode::<ClientMsg>(&bytes) {
                    Ok(msg) => self.handle_client_msg(ClientId(raw_id), msg),
                    Err(e) => log::warn!("decode event from {raw_id}: {e}"),
                }
            }
        }

        // Fixed-step simulation tick. Drain any leftover time from
        // last frame plus this frame's slice; if we slept a long
        // time we may run multiple ticks back-to-back.
        let tick_period = Duration::from_secs_f32(1.0 / TICK_HZ as f32);
        self.tick_accumulator += frame_dt;
        while self.tick_accumulator >= tick_period {
            self.tick_accumulator -= tick_period;
            self.simulate_one_tick(tick_period.as_secs_f32());
        }

        // Snapshot broadcast on its own clock. Every connected
        // client gets their own snapshot so we can stamp their last
        // applied input seq into `ack_seq`. Subtract the period
        // (rather than zeroing) so we don't drift when frame_dt
        // isn't a clean multiple of `snap_period` — mirrors how
        // `tick_accumulator` handles the simulation step.
        let snap_period = Duration::from_secs_f32(1.0 / SNAPSHOT_HZ as f32);
        self.snapshot_accumulator += frame_dt;
        if self.snapshot_accumulator >= snap_period {
            self.snapshot_accumulator -= snap_period;
            self.broadcast_snapshot();
        }

        // Periodic auto-save. Fire-and-forget per session — the
        // persistence worker drains writes off-thread so this loop
        // never blocks on a slow database.
        self.auto_save_accumulator += frame_dt;
        if self.auto_save_accumulator >= AUTO_SAVE_INTERVAL {
            self.auto_save_accumulator -= AUTO_SAVE_INTERVAL;
            self.auto_save_all();
        }

        // Flush outbound traffic. Must happen after we've enqueued any
        // server-originated messages this frame.
        self.handle.transport.send_packets(&mut self.handle.server);
        Ok(())
    }

    fn handle_server_event(&mut self, event: renet::ServerEvent) {
        match event {
            renet::ServerEvent::ClientConnected { client_id } => {
                log::info!("client connected: {client_id}");
                let cid = ClientId(client_id.raw());
                self.sessions.insert(ClientSession::new(cid));
            }
            renet::ServerEvent::ClientDisconnected { client_id, reason } => {
                log::info!("client disconnected: {client_id} ({reason:?})");
                let cid = ClientId(client_id.raw());
                // Pull the session out so we can fire one final
                // save and broadcast `PlayerLeft` with the net id
                // it owned. After this point the session row is
                // gone, so any late ticks for `cid` are no-ops.
                let removed = self.sessions.remove(cid);
                let (left_net_id, final_record) = removed
                    .as_ref()
                    .map(|s| (s.net_id, s.record.clone()))
                    .unwrap_or((None, None));
                if let (Some(handle), Some(record)) = (&self.persistence, final_record) {
                    if !handle.save(record) {
                        log::warn!("persistence: final-save dropped for {cid:?}");
                    }
                }
                self.sim.despawn_player(cid);
                if let Some(net_id) = left_net_id {
                    self.broadcast(Channel::Control, &ServerMsg::PlayerLeft { net_id });
                }
            }
        }
    }

    fn handle_client_msg(&mut self, from: ClientId, msg: ClientMsg) {
        match msg {
            ClientMsg::Hello {
                protocol_version,
                account_name,
                character_name,
                class_id,
                gender,
            } => self.handle_hello(
                from,
                protocol_version,
                account_name,
                character_name,
                class_id,
                gender,
            ),
            ClientMsg::Input(cmd) => self.sim.ingest_input(from, cmd),
            ClientMsg::CastAbility {
                ability_id,
                origin,
                aim_dir,
                placed_target,
            } => {
                self.sim
                    .cast_ability(from, ability_id, origin, aim_dir, placed_target, self.tick);
            }
            ClientMsg::EndChannel { ability_id } => {
                self.sim.end_channel(from, ability_id);
            }
            ClientMsg::PickUpLoot { net_id } => self.handle_pick_up_loot(from, net_id),
            ClientMsg::EquipItem { inventory_index } => {
                self.handle_equip_item(from, inventory_index as usize);
            }
            ClientMsg::UnequipItem { slot } => {
                self.handle_unequip_item(from, slot);
            }
            ClientMsg::OpenStash => self.handle_open_stash(from),
            ClientMsg::CloseStash => self.handle_close_stash(from),
            ClientMsg::DepositToStash { inventory_index } => {
                self.handle_deposit_to_stash(from, inventory_index as usize);
            }
            ClientMsg::WithdrawFromStash { stash_index } => {
                self.handle_withdraw_from_stash(from, stash_index as usize);
            }
            ClientMsg::SwapInventorySlots { a, b } => {
                self.handle_swap_inventory_slots(from, a as usize, b as usize);
            }
            ClientMsg::SwapStashSlots { a, b } => {
                self.handle_swap_stash_slots(from, a as usize, b as usize);
            }
            ClientMsg::DropInventoryItem { inventory_index } => {
                self.handle_drop_inventory_item(from, inventory_index as usize);
            }
            ClientMsg::UnequipToBagSlot { slot, inventory_index } => {
                self.handle_unequip_to_bag_slot(from, slot, inventory_index as usize);
            }
            ClientMsg::Ack { .. } => { /* phase 4 */ }
            ClientMsg::Goodbye => {
                log::info!("Goodbye from {from:?}");
            }
            ClientMsg::RequestEnterRift => {
                // Accept iff currently in the hub. Once a session
                // is in a rift this same message bumps to the next
                // floor (boss-kill auto-advance lives in Phase 7).
                let new_index = if self.sim.floor_index == 0 {
                    1
                } else {
                    self.sim.floor_index + 1
                };
                self.transition_floor(new_index);
            }
            ClientMsg::RequestReturnToHub => {
                if self.sim.floor_index != 0 {
                    self.transition_floor(0);
                }
            }
            ClientMsg::RequestRoster { account_name } => {
                log::info!("RequestRoster from {from:?}: account={account_name:?}");
                let entries = self.lookup_roster(&account_name);
                self.send_to(from, Channel::Control, &ServerMsg::Roster { entries });
            }
        }
    }

    /// Handle a fresh client's `Hello`: validate protocol, hydrate
    /// their persisted character + inventory, spawn into the sim,
    /// and broadcast the join. The heaviest of the dispatch arms;
    /// kept as its own method so [`Self::handle_client_msg`] reads
    /// as a flat dispatch table.
    fn handle_hello(
        &mut self,
        from: ClientId,
        protocol_version: u16,
        account_name: String,
        character_name: String,
        class_id: String,
        gender: Gender,
    ) {
        if protocol_version != PROTOCOL_VERSION {
            let reason = format!(
                "protocol version mismatch: server={PROTOCOL_VERSION} client={protocol_version}"
            );
            log::warn!("rejecting {from:?}: {reason}");
            self.send_to(from, Channel::Control, &ServerMsg::Reject { reason });
            return;
        }
        log::info!(
            "Hello from {from:?}: account={account_name:?} name={character_name:?} class={class_id:?} gender={gender:?}"
        );

        // Phased login: load → spawn → hydrate → reply → announce.
        // Each step is a small named method so the dispatch is
        // legible and individual stages are unit-testable in
        // isolation later if we want.
        self.load_player_state(from, &account_name, &character_name, &class_id, gender);
        let net_id = self.spawn_player_session(from, &class_id);
        let loaded_bag = self.hydrate_player_state(from);
        self.send_initial_packets(from, net_id, &loaded_bag);
        self.announce_join(from, net_id, character_name, class_id, gender);
    }

    /// Resolve the persisted character row (or fall back to an
    /// in-memory record) and stash all profile fields on the
    /// session. Mutating `account_name` etc. happens here so the
    /// rest of the login flow can read them off the session
    /// without having to thread parameters around.
    fn load_player_state(
        &mut self,
        from: ClientId,
        account_name: &str,
        character_name: &str,
        class_id: &str,
        gender: Gender,
    ) {
        // Block here on purpose: the load happens once per session
        // and we need level/xp before the world spawn. Falls back
        // to an in-memory record if persistence is disabled or
        // unreachable, so dev iteration without `docker compose up`
        // still works.
        let record =
            self.load_character_record(account_name, character_name, class_id, gender);
        if let Some(s) = self.sessions.get_mut(from) {
            s.account_name = Some(account_name.to_string());
            s.character_name = Some(character_name.to_string());
            s.class_id = Some(class_id.to_string());
            s.gender = Some(gender);
            s.record = Some(record);
        }
    }

    /// Spawn the player into the simulation and stash the net id
    /// on their session. The net id is what the client uses to
    /// recognize itself in subsequent snapshots.
    fn spawn_player_session(&mut self, from: ClientId, class_id: &str) -> NetId {
        let class = rift_game::classes::class_from_str(class_id);
        let net_id = self.sim.spawn_player(from, class);
        if let Some(s) = self.sessions.get_mut(from) {
            s.net_id = Some(net_id);
        }
        net_id
    }

    /// Push the persisted XP/level and inventory back into the
    /// freshly-spawned sim entity. Returns the loaded bag so the
    /// caller can replicate it to the picker without re-querying
    /// the simulation.
    fn hydrate_player_state(&mut self, from: ClientId) -> Vec<Option<rift_game::loot::Item>> {
        // XP / level: rebuild `current_xp` from `(total_xp, level)`
        // and recompute stats so the player isn't reset to level 1
        // on every login.
        if let Some(rec) = self.sessions.get(from).and_then(|s| s.record.as_ref()) {
            let level = rec.level.max(1) as u32;
            let xp = rec.xp.max(0) as u64;
            self.sim.set_player_experience(from, level, xp);
        }

        // Inventory: skipped when persistence is disabled (dev
        // mode) or the load fails — the empty bag is still a
        // valid game state.
        let mut loaded_items: Vec<Option<rift_game::loot::Item>> = Vec::new();
        let mut loaded_equipment = rift_game::loot::Equipment::new();
        let Some(handle) = &self.persistence else {
            return loaded_items;
        };
        let Some(rec_id) = self.sessions.record_id(from) else {
            return loaded_items;
        };
        match handle.load_inventory_blocking(rec_id) {
            Ok(rows) => {
                // Each persisted row decodes either into the bag
                // (at its persisted `slot_index`) or directly into
                // an equipment slot, depending on `equipped_slot`.
                // A row whose slot byte is unknown (mismatched
                // build) falls through to the bag's first free
                // slot so the player never silently loses the item.
                for r in rows {
                    let Some(item) = rift_game::loot::Item::from_persisted(
                        &r.base_id,
                        r.rarity as u8,
                        r.ilvl as u16,
                        &r.affixes,
                    ) else {
                        continue;
                    };
                    match r.equipped_slot {
                        Some(b) => match rift_game::loot::EquipSlot::from_u8(b as u8) {
                            Some(slot)
                                if rift_game::loot::Equipment::accepts(slot, &item) =>
                            {
                                loaded_equipment.set(slot, Some(item));
                            }
                            _ => place_at_slot_index(&mut loaded_items, r.slot_index, item),
                        },
                        None => place_at_slot_index(&mut loaded_items, r.slot_index, item),
                    }
                }
                let bag_filled = loaded_items.iter().filter(|s| s.is_some()).count();
                log::info!(
                    "persistence: loaded {} bag item(s) + {} equipped for {from:?}",
                    bag_filled,
                    loaded_equipment.count()
                );
                self.sim.set_player_inventory(
                    from,
                    loaded_items.clone(),
                    loaded_equipment,
                );
            }
            Err(e) => log::warn!("persistence: load_inventory failed for {from:?}: {e}"),
        }
        // Hydrate the per-character stash. Eager-load it at
        // Hello time so the chest interaction path stays
        // synchronous — the actual `StashSync` packet is held
        // back until the player opens the chest, which keeps
        // login lean.
        match handle.load_stash_blocking(rec_id) {
            Ok(rows) => {
                let mut stash_items: Vec<Option<rift_game::loot::Item>> = Vec::new();
                for r in rows {
                    let Some(item) = rift_game::loot::Item::from_persisted(
                        &r.base_id,
                        r.rarity as u8,
                        r.ilvl as u16,
                        &r.affixes,
                    ) else {
                        continue;
                    };
                    place_at_slot_index(&mut stash_items, r.slot_index, item);
                }
                let stash_filled = stash_items.iter().filter(|s| s.is_some()).count();
                log::info!(
                    "persistence: loaded {} stash item(s) for {from:?}",
                    stash_filled,
                );
                self.sim.set_player_stash(from, stash_items);
            }
            Err(e) => log::warn!("persistence: load_stash failed for {from:?}: {e}"),
        }
        loaded_items
    }

    /// Send the just-welcomed client every "here's your initial
    /// state" packet they need before snapshots start landing:
    /// `Welcome`, the bag + equipment mirrors, the XP bar, and
    /// the rift-progress meter.
    fn send_initial_packets(
        &mut self,
        from: ClientId,
        net_id: NetId,
        loaded_bag: &[Option<rift_game::loot::Item>],
    ) {
        let welcome = ServerMsg::Welcome {
            your_client_id: from,
            your_net_id: net_id,
            floor_seed: self.sim.floor_seed,
            floor_index: self.sim.floor_index,
            tick: self.tick,
        };
        self.send_to(from, Channel::Control, &welcome);

        // Replicate the persisted bag to the picker so their
        // local mirror matches the server bag the moment they
        // enter the world. Sent on the reliable Control channel
        // so it can't be dropped. Sent unconditionally so an
        // empty bag definitively clears any stale UI state.
        let blobs: Vec<Option<rift_net::messages::ItemBlob>> = loaded_bag
            .iter()
            .map(|s| s.as_ref().map(item_to_blob))
            .collect();
        self.send_to(
            from,
            Channel::Control,
            &ServerMsg::InventorySync { items: blobs },
        );

        // Replicate the equipped set even when empty: the client
        // uses an empty `EquipmentSync` as a definitive "you have
        // nothing equipped" signal, which lets it clear any
        // stale UI state from a previous session on the same
        // process (rare but cheap to be correct about).
        let equip_pairs = self.sim.player_equipment(from);
        let equip_blobs: Vec<(u8, rift_net::messages::ItemBlob)> = equip_pairs
            .iter()
            .map(|(slot, it)| (slot.to_u8(), item_to_blob(it)))
            .collect();
        self.send_to(
            from,
            Channel::Control,
            &ServerMsg::EquipmentSync { slots: equip_blobs },
        );

        // Initial XP / level snapshot so the HUD bar is correct
        // before the first kill.
        if let Some((level, xp, xp_to_next)) = self.sim.player_stats_snapshot(from) {
            self.send_to(
                from,
                Channel::Control,
                &ServerMsg::CharacterStats { level, xp, xp_to_next },
            );
        }
        // Initial rift-progress snapshot (current floor's bar
        // state, including any kills already racked up by
        // already-connected players).
        let rp = self.sim.rift_progress();
        self.send_to(
            from,
            Channel::Control,
            &ServerMsg::RiftProgress {
                progress: rp.progress,
                required: rp.required,
                boss_spawned: rp.boss_spawned,
                boss_killed: rp.boss_killed,
                floor_complete: rp.floor_complete,
            },
        );
    }

    /// Catch the newcomer up on every already-connected player,
    /// then broadcast their own `PlayerJoined` to the room.
    fn announce_join(
        &mut self,
        from: ClientId,
        net_id: NetId,
        character_name: String,
        class_id: String,
        gender: Gender,
    ) {
        let already_here: Vec<ServerMsg> = self
            .sessions
            .iter()
            .filter(|s| s.client_id != from)
            .filter_map(|s| {
                Some(ServerMsg::PlayerJoined {
                    net_id: s.net_id?,
                    client_id: s.client_id,
                    character_name: s.character_name.clone()?,
                    class_id: s.class_id.clone()?,
                    gender: s.gender?,
                })
            })
            .collect();
        for msg in already_here {
            self.send_to(from, Channel::Control, &msg);
        }
        let joined = ServerMsg::PlayerJoined {
            net_id,
            client_id: from,
            character_name,
            class_id,
            gender,
        };
        self.broadcast(Channel::Control, &joined);
    }

    /// Validate a loot pickup, broadcast `LootClaimed`, and queue
    /// the persistent inventory append.
    fn handle_pick_up_loot(&mut self, from: ClientId, net_id: NetId) {
        let item = match self.sim.try_pickup_loot(from, net_id) {
            Ok(item) => item,
            Err(Some(reason)) => {
                log::info!(
                    "loot pickup rejected for {from:?}: {reason:?}"
                );
                self.send_to(
                    from,
                    Channel::Control,
                    &ServerMsg::PickupRejected { loot: net_id, reason },
                );
                return;
            }
            Err(None) => return,
        };
        log::info!(
            "loot picked: {} (item-level {}) by {from:?}",
            item.display_name(),
            item.ilvl,
        );
        self.broadcast(
            Channel::Control,
            &ServerMsg::LootClaimed {
                loot: net_id,
                claimed_by: from,
            },
        );
        // Push a fresh `InventorySync` to the picker so their
        // local mirror lands in the *exact* slot the
        // authoritative bag chose (`push_into_sparse` fills the
        // first hole, which the client can't reproduce on its
        // own without re-implementing the same logic). Every
        // other inventory mutation does this; pickup needs it
        // too or drop-then-pickup leaves the UI desynced until
        // the next equip / swap forces a sync.
        self.broadcast_inventory_state(from);
        // Persist the pickup. We look up the picker's
        // `character_id` via the cached `CharacterRecord` in
        // their session and queue a fire-and-forget INSERT — the
        // server-authoritative bag has already been updated by
        // `try_pickup_loot`, so a dropped/late write is
        // recoverable on the next session by re-rolling.
        if let Some(handle) = &self.persistence {
            if let Some(rec) = self.sessions.get(from).and_then(|s| s.record.as_ref()) {
                let (base_id, rarity, ilvl, affixes) = item.to_persisted();
                let persisted = PersistedItem {
                    base_id,
                    rarity: rarity as i16,
                    ilvl: ilvl as i32,
                    affixes,
                    equipped_slot: None,
                    // The append SQL computes the next free
                    // bag-position via `MAX(slot_index)+1`, so
                    // the field's value here is ignored.
                    slot_index: 0,
                };
                if !handle.append_inventory_item(rec.id, persisted) {
                    log::warn!("persistence: append_inventory_item dropped for {from:?}");
                }
            }
        }
    }

    /// Move the picker's bag item at `inventory_index` into its
    /// canonical equipment slot, broadcast the resulting
    /// inventory + equipment state to the picker, and queue a
    /// `ResetCharacterInventory` so the persisted snapshot
    /// matches the in-memory swap.
    fn handle_equip_item(&mut self, from: ClientId, inventory_index: usize) {
        if !self.sim.equip_from_bag(from, inventory_index) {
            log::debug!(
                "equip: rejected equip request from {from:?} idx={inventory_index}"
            );
            return;
        }
        self.broadcast_inventory_state(from);
        self.persist_inventory_state(from);
    }

    /// Move whatever's currently in `slot` back into the bag.
    /// No-op for unknown slot bytes or empty slots.
    fn handle_unequip_item(&mut self, from: ClientId, slot_byte: u8) {
        let Some(slot) = rift_game::loot::EquipSlot::from_u8(slot_byte) else {
            log::debug!("unequip: unknown slot byte {slot_byte} from {from:?}");
            return;
        };
        if !self.sim.unequip_to_bag(from, slot) {
            return;
        }
        self.broadcast_inventory_state(from);
        self.persist_inventory_state(from);
    }

    /// Send the picker fresh `InventorySync` + `EquipmentSync`
    /// frames reflecting the current authoritative state.
    fn broadcast_inventory_state(&mut self, to: ClientId) {
        let bag = self.sim.player_inventory(to);
        let equip = self.sim.player_equipment(to);
        let inv_blobs: Vec<Option<rift_net::messages::ItemBlob>> =
            bag.iter().map(|s| s.as_ref().map(item_to_blob)).collect();
        let equip_blobs: Vec<(u8, rift_net::messages::ItemBlob)> = equip
            .iter()
            .map(|(s, it)| (s.to_u8(), item_to_blob(it)))
            .collect();
        self.send_to(
            to,
            Channel::Control,
            &ServerMsg::InventorySync { items: inv_blobs },
        );
        self.send_to(
            to,
            Channel::Control,
            &ServerMsg::EquipmentSync { slots: equip_blobs },
        );
    }

    /// Snapshot the picker's bag + equipment into a flat
    /// `Vec<PersistedItem>` and queue a `ResetCharacterInventory`
    /// so the database row set matches the post-swap layout.
    fn persist_inventory_state(&mut self, from: ClientId) {
        let Some(handle) = &self.persistence else {
            return;
        };
        let Some(rec_id) = self.sessions.record_id(from) else {
            return;
        };
        let dump = self.sim.dump_player_inventory(from);
        let rows: Vec<PersistedItem> = dump
            .into_iter()
            .map(|(slot, slot_index, item)| {
                let (base_id, rarity, ilvl, affixes) = item.to_persisted();
                PersistedItem {
                    base_id,
                    rarity: rarity as i16,
                    ilvl: ilvl as i32,
                    affixes,
                    equipped_slot: slot.map(|b| b as i16),
                    slot_index,
                }
            })
            .collect();
        if !handle.reset_character_inventory(rec_id, rows) {
            log::warn!("persistence: reset_character_inventory dropped for {from:?}");
        }
    }

    /// Begin a stash session for `from`: validate the player is
    /// in the hub, mark the per-player `stash_open` flag, and
    /// reply with the current stash contents. Range validation
    /// is left to the client because the chest position is a
    /// purely visual hub asset \u2014 in the hub the player is
    /// effectively at the chest the moment they open the panel.
    fn handle_open_stash(&mut self, from: ClientId) {
        if self.sim.floor_index != 0 {
            log::debug!("stash: open rejected for {from:?} — not in hub");
            return;
        }
        self.sim.set_stash_open(from, true);
        self.send_stash_state(from);
    }

    /// End the stash session for `from`. Subsequent
    /// deposit / withdraw requests are rejected until a fresh
    /// `OpenStash` arrives.
    fn handle_close_stash(&mut self, from: ClientId) {
        self.sim.set_stash_open(from, false);
    }

    /// Move the bag item at `inventory_index` into the stash and
    /// re-broadcast both inventories. Persists both tables.
    fn handle_deposit_to_stash(&mut self, from: ClientId, inventory_index: usize) {
        if !self.sim.is_stash_open(from) {
            log::debug!("stash: deposit rejected for {from:?} — stash not open");
            return;
        }
        if !self.sim.deposit_to_stash(from, inventory_index) {
            log::debug!(
                "stash: deposit rejected for {from:?} idx={inventory_index}"
            );
            return;
        }
        self.broadcast_inventory_state(from);
        self.send_stash_state(from);
        self.persist_inventory_state(from);
        self.persist_stash_state(from);
    }

    /// Move the stash item at `stash_index` back into the bag
    /// and re-broadcast both inventories. Persists both tables.
    fn handle_withdraw_from_stash(&mut self, from: ClientId, stash_index: usize) {
        if !self.sim.is_stash_open(from) {
            log::debug!("stash: withdraw rejected for {from:?} — stash not open");
            return;
        }
        if !self.sim.withdraw_from_stash(from, stash_index) {
            log::debug!(
                "stash: withdraw rejected for {from:?} idx={stash_index}"
            );
            return;
        }
        self.broadcast_inventory_state(from);
        self.send_stash_state(from);
        self.persist_inventory_state(from);
        self.persist_stash_state(from);
    }

    /// Send the picker a fresh `StashSync` reflecting the
    /// current authoritative stash contents.
    fn send_stash_state(&mut self, to: ClientId) {
        let stash = self.sim.player_stash(to);
        let blobs: Vec<Option<rift_net::messages::ItemBlob>> =
            stash.iter().map(|s| s.as_ref().map(item_to_blob)).collect();
        self.send_to(
            to,
            Channel::Control,
            &ServerMsg::StashSync { items: blobs },
        );
    }

    /// Snapshot the picker's stash into a flat
    /// `Vec<PersistedItem>` and queue a `ResetCharacterStash`
    /// so the database row set matches the post-transfer layout.
    fn persist_stash_state(&mut self, from: ClientId) {
        let Some(handle) = &self.persistence else {
            return;
        };
        let Some(rec_id) = self.sessions.record_id(from) else {
            return;
        };
        let stash = self.sim.dump_player_stash(from);
        let rows: Vec<PersistedItem> = stash
            .into_iter()
            .enumerate()
            .filter_map(|(slot_index, opt)| {
                let item = opt?;
                let (base_id, rarity, ilvl, affixes) = item.to_persisted();
                Some(PersistedItem {
                    base_id,
                    rarity: rarity as i16,
                    ilvl: ilvl as i32,
                    affixes,
                    equipped_slot: None,
                    slot_index: slot_index as i32,
                })
            })
            .collect();
        if !handle.reset_character_stash(rec_id, rows) {
            log::warn!("persistence: reset_character_stash dropped for {from:?}");
        }
    }

    /// Reorder the bag: swap two slots. Either may be empty;
    /// see [`Sim::swap_inventory_slots`].
    fn handle_swap_inventory_slots(&mut self, from: ClientId, a: usize, b: usize) {
        if !self.sim.swap_inventory_slots(from, a, b) {
            log::debug!("inv: swap rejected for {from:?} a={a} b={b}");
            return;
        }
        self.broadcast_inventory_state(from);
        self.persist_inventory_state(from);
    }

    /// Reorder the stash: swap two slots. Either may be empty.
    /// Requires an open stash session. Persists the stash on
    /// success.
    fn handle_swap_stash_slots(&mut self, from: ClientId, a: usize, b: usize) {
        if !self.sim.is_stash_open(from) {
            log::debug!("stash: swap rejected for {from:?} — stash not open");
            return;
        }
        if !self.sim.swap_stash_slots(from, a, b) {
            log::debug!("stash: swap rejected for {from:?} a={a} b={b}");
            return;
        }
        self.send_stash_state(from);
        self.persist_stash_state(from);
    }

    /// Drop the bag item at `inventory_index` onto the ground at
    /// the picker's current position. Removes the row, spawns a
    /// fresh `ServerLoot`, and queues `WorldEvent::LootDropped`
    /// so every observer's loot pillar appears.
    fn handle_drop_inventory_item(&mut self, from: ClientId, inventory_index: usize) {
        let Some((item, pos)) = self.sim.pop_inventory_item(from, inventory_index) else {
            log::debug!(
                "inv: drop rejected for {from:?} idx={inventory_index}"
            );
            return;
        };
        log::info!(
            "inv: {from:?} dropped {} at {pos:?}",
            item.display_name(),
        );
        self.sim.spawn_dropped_loot(item, pos);
        self.broadcast_inventory_state(from);
        self.persist_inventory_state(from);
    }

    /// Move whatever's currently in `slot` into the bag at
    /// `inventory_index` (drag-drop counterpart to
    /// `handle_unequip_item`).
    fn handle_unequip_to_bag_slot(
        &mut self,
        from: ClientId,
        slot_byte: u8,
        inventory_index: usize,
    ) {
        let Some(slot) = rift_game::loot::EquipSlot::from_u8(slot_byte) else {
            log::debug!("unequip: unknown slot byte {slot_byte} from {from:?}");
            return;
        };
        if !self.sim.unequip_to_bag_slot(from, slot, inventory_index) {
            return;
        }
        self.broadcast_inventory_state(from);
        self.persist_inventory_state(from);
    }

    /// Switch the simulation onto a new floor and tell every client
    /// to do the same. Reliable-ordered on `Channel::Control` so
    /// late snapshots from the previous floor can't corrupt the
    /// transition: clients drop any snapshot whose tick predates the
    /// `LoadFloor.tick` they accepted.
    fn transition_floor(&mut self, new_index: u32) {
        let spawn = self.sim.change_floor(new_index);
        let msg = ServerMsg::LoadFloor {
            seed: self.sim.floor_seed,
            index: new_index,
            is_hub: new_index == 0,
            spawn_pos: spawn.to_array(),
            tick: self.tick,
        };
        self.broadcast(Channel::Control, &msg);
    }

    fn send_to(&mut self, to: ClientId, ch: Channel, msg: &ServerMsg) {
        let bytes = match encode(msg) {
            Ok(b) => b,
            Err(e) => {
                log::error!("encode {msg:?}: {e}");
                return;
            }
        };
        self.handle.server.send_message(
            renet::ClientId::from_raw(to.0),
            ch as u8,
            bytes,
        );
    }

    /// Send a message to every currently-connected client.
    fn broadcast(&mut self, ch: Channel, msg: &ServerMsg) {
        let bytes = match encode(msg) {
            Ok(b) => b,
            Err(e) => {
                log::error!("encode {msg:?}: {e}");
                return;
            }
        };
        self.handle
            .server
            .broadcast_message(ch as u8, bytes);
    }

    fn simulate_one_tick(&mut self, dt: f32) {
        self.tick = self.tick.next();
        self.sim.step(dt, self.tick);
        // Broadcast any world events the tick produced (damage,
        // deaths, ability casts) reliably so clients can drive HUD
        // and one-shot animations without waiting for the next
        // snapshot.
        let events = self.sim.drain_events();
        for ev in events {
            self.broadcast(Channel::Event, &ServerMsg::Event(ev));
        }
        // Per-player XP / level updates: targeted send to each
        // owner, since other players never see another character's
        // bar.
        let stat_updates = self.sim.drain_stat_updates();
        for u in stat_updates {
            // Persist the new level / total XP so we don't lose
            // progress on disconnect. Save is fire-and-forget;
            // the worker UPSERTs by character id.
            self.persist_xp_for(u.client_id, u.level, u.total_xp);
            self.send_to(
                u.client_id,
                Channel::Control,
                &ServerMsg::CharacterStats {
                    level: u.level,
                    xp: u.xp,
                    xp_to_next: u.xp_to_next,
                },
            );
        }
        // Rift-progress changes: broadcast so every client's HUD
        // sees the same bar / boss state.
        if let Some(rp) = self.sim.take_rift_progress_update() {
            self.broadcast(
                Channel::Control,
                &ServerMsg::RiftProgress {
                    progress: rp.progress,
                    required: rp.required,
                    boss_spawned: rp.boss_spawned,
                    boss_killed: rp.boss_killed,
                    floor_complete: rp.floor_complete,
                },
            );
        }

        // Drain any player deaths queued this tick. The
        // `WorldEvent::Death` for each one already went out via
        // `drain_events` above, so we just need the log. The
        // server-side respawn-to-hub fires once
        // `take_hub_respawn_request` returns true (see below).
        for (cid, net_id) in self.sim.drain_player_deaths() {
            log::info!("player died: {cid:?} ({net_id:?})");
        }
        // Auto-respawn: when the post-death countdown elapses the
        // sim asks us to load the hub. We force it regardless of
        // current floor so a death always pulls the party back to
        // safety.
        if self.sim.take_hub_respawn_request() {
            log::info!("respawning party to hub after death");
            self.transition_floor(0);
        }
    }

    /// Persist the latest XP / level snapshot for the supplied
    /// client. Fire-and-forget; failure logs but doesn't block
    /// gameplay. Mutates the in-memory `record` so subsequent
    /// `Save` calls (e.g. on disconnect) carry the latest values.
    /// Total XP is provided by the sim alongside the level so the
    /// XP curve never has to be recomputed here.
    fn persist_xp_for(&mut self, client_id: ClientId, level: u32, total_xp: u64) {
        let Some(s) = self.sessions.get_mut(client_id) else {
            return;
        };
        let Some(rec) = s.record.as_mut() else { return };
        rec.level = level as i32;
        rec.xp = total_xp.min(i32::MAX as u64) as i32;
        if let Some(handle) = &self.persistence {
            let _ = handle.save(rec.clone());
        }
    }

    /// Resolve the persistent record for a session's `Hello`. If
    /// persistence is disabled (no DB), or the worker fails the
    /// query, we synthesize a fresh record so the player can still
    /// play — their progress just won't survive a restart. The
    /// fallback record uses a random UUID so subsequent saves on
    /// the same name don't collide with a real DB row by accident.
    fn load_character_record(
        &self,
        account_name: &str,
        character_name: &str,
        class_id: &str,
        gender: Gender,
    ) -> CharacterRecord {
        let gender_id = gender as i16;
        if let Some(handle) = &self.persistence {
            match handle.load_or_create_blocking(
                account_name.to_string(),
                character_name.to_string(),
                class_id.to_string(),
                gender_id,
            ) {
                Ok(rec) => {
                    log::info!(
                        "persistence: loaded {} on account {} (level={}, xp={})",
                        rec.name,
                        account_name,
                        rec.level,
                        rec.xp,
                    );
                    return rec;
                }
                Err(e) => {
                    log::warn!(
                        "persistence: load_or_create failed for account={account_name:?} name={character_name:?}: {e}; using in-memory record"
                    );
                }
            }
        }
        // Fallback: ephemeral record. `id` is a fresh UUID so the
        // periodic `Save` UPDATE simply targets zero rows — that's
        // a no-op, not an error.
        CharacterRecord {
            id: Uuid::new_v4(),
            account_id: Uuid::new_v4(),
            name: character_name.to_string(),
            class_id: class_id.to_string(),
            gender: gender_id,
            level: 1,
            xp: 0,
        }
    }

    /// Resolve `account_name` to its character roster. Falls back
    /// to an empty list when persistence is disabled or the DB
    /// query fails — the client is then free to create a fresh
    /// character, which load_character_record will persist on the
    /// next Hello.
    fn lookup_roster(&self, account_name: &str) -> Vec<RosterEntry> {
        let Some(handle) = &self.persistence else { return Vec::new() };
        match handle.list_account_characters_blocking(account_name.to_string()) {
            Ok((_account_id, records)) => records
                .into_iter()
                .map(|r| RosterEntry {
                    character_name: r.name,
                    class_id: r.class_id,
                    gender: gender_from_i16(r.gender),
                    level: r.level.max(0) as u32,
                })
                .collect(),
            Err(e) => {
                log::warn!(
                    "persistence: list_account_characters failed for {account_name:?}: {e}; returning empty roster"
                );
                Vec::new()
            }
        }
    }

    /// Fire a fire-and-forget save for every session that has a
    /// persisted record attached. Called from the periodic
    /// auto-save tick. Cheap when no characters are connected or
    /// when persistence is disabled.
    fn auto_save_all(&self) {
        let Some(handle) = &self.persistence else { return };
        let mut count = 0usize;
        for s in self.sessions.iter() {
            if let Some(rec) = &s.record {
                if handle.save(rec.clone()) {
                    count += 1;
                }
            }
        }
        if count > 0 {
            log::debug!("persistence: auto-save queued for {count} character(s)");
        }
    }

    /// Build and broadcast a per-client snapshot. Each client gets
    /// their own copy because `ack_seq` is per-client.
    fn broadcast_snapshot(&mut self) {
        let connected: Vec<u64> = self
            .handle
            .server
            .clients_id()
            .iter()
            .map(|id| id.raw())
            .collect();
        for raw in connected {
            let cid = ClientId(raw);
            let snap = self.sim.build_snapshot(self.tick, cid);
            self.send_to(cid, Channel::Snapshot, &ServerMsg::Snapshot(snap));
        }
    }
}

/// Decode a stored gender column back to the wire enum. Unknown
/// codes (forward-compat with future variants) fall back to
/// Female so we never panic on a malformed row.
fn gender_from_i16(g: i16) -> Gender {
    match g {
        x if x == Gender::Male as i16 => Gender::Male,
        _ => Gender::Female,
    }
}

/// Convert an authoritative `rift_game::loot::Item` into the
/// `ItemBlob` shape that ships over the wire. Centralised so all
/// the `InventorySync` / `EquipmentSync` builders agree on the
/// field layout — bumping `Item::to_wire` only needs to be
/// reflected here.
fn item_to_blob(item: &rift_game::loot::Item) -> rift_net::messages::ItemBlob {
    let (base_id, rarity, ilvl, affixes) = item.to_wire();
    rift_net::messages::ItemBlob {
        base_id,
        rarity,
        ilvl,
        affixes,
    }
}

/// Insert `item` at `slot_index` in a sparse bag/stash, growing
/// the vector with `None` placeholders as needed. If the target
/// slot is somehow already occupied (corrupted data, duplicate
/// `slot_index`), the new item lands at the next free slot
/// instead so nothing gets lost.
fn place_at_slot_index(
    bag: &mut Vec<Option<rift_game::loot::Item>>,
    slot_index: i32,
    item: rift_game::loot::Item,
) {
    let idx = slot_index.max(0) as usize;
    if idx >= bag.len() {
        bag.resize_with(idx + 1, || None);
    }
    if bag[idx].is_some() {
        // Fall back: walk forward to the first hole, else append.
        if let Some(slot) = bag.iter_mut().find(|s| s.is_none()) {
            *slot = Some(item);
        } else {
            bag.push(Some(item));
        }
    } else {
        bag[idx] = Some(item);
    }
}

/// Parsed command-line configuration.
struct Args {
    bind: SocketAddr,
    public: SocketAddr,
    /// Postgres connection string. `None` disables persistence
    /// entirely (everything stays in memory). Pass `--no-db` to
    /// force-disable; otherwise we default to a local docker
    /// compose URL.
    database_url: Option<String>,
}

fn parse_args() -> Args {
    // Tiny ad-hoc argv parser. Not worth pulling clap until we have
    // more than a handful of flags.
    //
    // Defaults are sourced from environment variables first so the
    // same binary works in dev (no env, all defaults), in docker
    // compose (env injected by the compose file), and on PaaS
    // hosts that prefer env-only configuration.
    //
    //   PORT             — overrides the bind port (Railway / Fly /
    //                      Heroku idiom). Bound on `0.0.0.0`.
    //   RIFT_BIND        — full bind socket address (overrides PORT).
    //   RIFT_PUBLIC      — advertised public address for connect
    //                      tokens. Required when the server is
    //                      behind NAT / a load balancer.
    //   DATABASE_URL     — Postgres connection string. Empty string
    //                      or `disabled` skips persistence (same as
    //                      `--no-db`).
    let mut bind: SocketAddr = match std::env::var("RIFT_BIND") {
        Ok(v) if !v.is_empty() => v.parse().expect("invalid RIFT_BIND"),
        _ => match std::env::var("PORT") {
            Ok(p) if !p.is_empty() => format!("0.0.0.0:{p}")
                .parse()
                .expect("invalid PORT"),
            _ => "0.0.0.0:34000".parse().unwrap(),
        },
    };
    let mut public: Option<SocketAddr> = match std::env::var("RIFT_PUBLIC") {
        Ok(v) if !v.is_empty() => Some(v.parse().expect("invalid RIFT_PUBLIC")),
        _ => None,
    };
    let mut database_url: Option<String> = match std::env::var("DATABASE_URL") {
        Ok(v) if !v.is_empty() && v != "disabled" => Some(v),
        Ok(_) => None,
        Err(_) => Some("postgres://rift:rift@127.0.0.1:55432/rift".to_string()),
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--bind" => {
                if let Some(v) = iter.next() {
                    bind = v.parse().expect("invalid --bind address");
                }
            }
            "--public" => {
                if let Some(v) = iter.next() {
                    public = Some(v.parse().expect("invalid --public address"));
                }
            }
            "--database-url" => {
                if let Some(v) = iter.next() {
                    database_url = Some(v);
                }
            }
            "--no-db" => {
                database_url = None;
            }
            "--help" | "-h" => {
                eprintln!(
                    "rift-server [--bind 0.0.0.0:34000] [--public 127.0.0.1:34000] \
                     [--database-url postgres://rift:rift@127.0.0.1:55432/rift] [--no-db]\n\
                     \n\
                     Env vars (used as defaults if the matching flag is omitted):\n  \
                     PORT, RIFT_BIND, RIFT_PUBLIC, DATABASE_URL"
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    let public = public.unwrap_or(bind);
    Args { bind, public, database_url }
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args = parse_args();

    // Bring up persistence first so any DB problem aborts the
    // boot before we open the network socket. `--no-db` skips it
    // entirely for offline iteration.
    let persistence = match args.database_url.as_deref() {
        Some(url) => {
            log::info!("persistence: connecting to {url}");
            match rift_persistence::spawn(url.to_string()) {
                Ok(handle) => {
                    log::info!("persistence: ready");
                    Some(handle)
                }
                Err(e) => {
                    log::error!(
                        "persistence: failed to initialise ({e}); continuing without DB. \
                         Pass --no-db to silence this message."
                    );
                    None
                }
            }
        }
        None => {
            log::info!("persistence: disabled (--no-db)");
            None
        }
    };

    let mut server = Server::new(args.bind, args.public, persistence)?;    log::info!("rift-server ready on {} (public {})", args.bind, args.public);

    // Tight wall-clock loop. Renet/netcode are non-blocking; we
    // sleep just enough to pace the network update at ~60 Hz so we
    // don't hog a core when idle. The simulation runs at TICK_HZ
    // independently, gated inside `step`.
    let net_period = Duration::from_secs_f32(1.0 / 60.0);
    loop {
        let start = Instant::now();
        if let Err(e) = server.step() {
            log::error!("step: {e:?}");
        }
        let elapsed = start.elapsed();
        if elapsed < net_period {
            std::thread::sleep(net_period - elapsed);
        }
    }
}
