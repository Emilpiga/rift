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
    collections::HashMap,
    net::{SocketAddr, ToSocketAddrs},
    time::{Duration, Instant},
};

use anyhow::Result;
use rift_net::{
    decode, encode, open_server, renet, Channel, ClientId, ClientMsg, NetSettings, NetTick,
    ServerHandle, ServerMsg, MAX_CLIENTS, SNAPSHOT_HZ, TICK_HZ,
};
use rift_persistence::PersistenceHandle;

mod chat;
mod handlers;
mod session;
mod sim;

use chat::ChatHistory;
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
    /// Authoritative simulation for the global hub. All players
    /// land here on connect; they leave by stepping into the
    /// rift portal (which moves them into [`Self::rift`]).
    hub: Sim,
    /// Authoritative simulation for the active rift instance.
    /// In the future this becomes a `Vec<Sim>` so multiple
    /// 4-player parties can run their own rifts in parallel; for
    /// now there's exactly one. Kept always-present (rather than
    /// `Option<Sim>`) so the per-tick step loop doesn't have to
    /// branch on emptiness — when nobody's in the rift, the
    /// world simply has no player entities and the AI / projectile
    /// systems run over empty queries.
    rift: Sim,
    /// Maps each connected client to which sim they currently
    /// inhabit: `0` = hub, `1+` = rift instance index. Updated
    /// on Hello (always 0) and on portal transitions.
    client_floor: HashMap<ClientId, u32>,
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
    /// Server-global chat history. New connections replay
    /// recent GLOBAL + SYSTEM lines from this on accept.
    chat: ChatHistory,
    /// Last-seen value of `RiftProgress::boss_killed` so the
    /// boss-slain SYSTEM line fires once per actual kill
    /// instead of once per progress snapshot.
    prev_boss_killed: bool,
}

impl Server {
    fn new(
        bind: SocketAddr,
        public: SocketAddr,
        persistence: Option<PersistenceHandle>,
    ) -> Result<Self> {
        let handle = open_server(bind, public, MAX_CLIENTS, &NetSettings::default())?;
        // Two simulations always run side by side: the global
        // hub (floor 0) and the active rift instance (floor 1
        // for now; future runs may rotate the seed/index per
        // descend). New connections land in the hub and only
        // step into the rift once they walk through the portal.
        let hub = Sim::new(42, 0);
        let rift = Sim::new(42, 1);
        Ok(Self {
            handle,
            tick: NetTick::default(),
            sessions: SessionManager::new(),
            last_tick: Instant::now(),
            hub,
            rift,
            client_floor: HashMap::new(),
            tick_accumulator: Duration::ZERO,
            snapshot_accumulator: Duration::ZERO,
            persistence,
            auto_save_accumulator: Duration::ZERO,
            chat: ChatHistory::default(),
            prev_boss_killed: false,
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
                // Capture the leaver's character name (if they
                // ever finished the Hello handshake) so we can
                // emit a "X has left" system line *after* the
                // session row is gone but before the
                // broadcast loops reuse the borrow.
                let leaver_name = removed
                    .as_ref()
                    .and_then(|s| s.character_name.clone());
                if let (Some(handle), Some(record)) = (&self.persistence, final_record) {
                    if !handle.save(record) {
                        log::warn!("persistence: final-save dropped for {cid:?}");
                    }
                }
                self.hub.despawn_player(cid);
                self.rift.despawn_player(cid);
                self.client_floor.remove(&cid);
                if let Some(net_id) = left_net_id {
                    self.broadcast(Channel::Control, &ServerMsg::PlayerLeft { net_id });
                }
                if let Some(name) = leaver_name {
                    self.emit_system_global(&format!("{name} has left."));
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
            ClientMsg::Input(cmd) => self.sim_for_client_mut(from).ingest_input(from, cmd),
            ClientMsg::CastAbility {
                ability_id,
                origin,
                aim_dir,
                placed_target,
                target_net_id,
            } => {
                let tick = self.tick;
                let sim = self.sim_for_client_mut(from);
                if sim.is_ghost(from) {
                    return;
                }
                sim.cast_ability(
                    from,
                    ability_id,
                    origin,
                    aim_dir,
                    placed_target,
                    target_net_id,
                    tick,
                );
            }
            ClientMsg::EndChannel { ability_id } => {
                self.sim_for_client_mut(from).end_channel(from, ability_id);
            }
            ClientMsg::PickUpLoot { net_id } => {
                if self.sim_for_client(from).is_ghost(from) {
                    return;
                }
                self.handle_pick_up_loot(from, net_id)
            }
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
            ClientMsg::DepositToStashSlot { inventory_index, stash_index } => {
                self.handle_deposit_to_stash_slot(
                    from,
                    inventory_index as usize,
                    stash_index as usize,
                );
            }
            ClientMsg::WithdrawFromStash { stash_index } => {
                self.handle_withdraw_from_stash(from, stash_index as usize);
            }
            ClientMsg::WithdrawFromStashSlot { stash_index, inventory_index } => {
                self.handle_withdraw_from_stash_slot(
                    from,
                    stash_index as usize,
                    inventory_index as usize,
                );
            }
            ClientMsg::SwapInventorySlots { a, b } => {
                self.handle_swap_inventory_slots(from, a as usize, b as usize);
            }
            ClientMsg::SwapStashSlots { a, b } => {
                self.handle_swap_stash_slots(from, a as usize, b as usize);
            }
            ClientMsg::DropInventoryItem { inventory_index } => {
                if self.sim_for_client(from).is_ghost(from) {
                    return;
                }
                self.handle_drop_inventory_item(from, inventory_index as usize);
            }
            ClientMsg::UnequipToBagSlot { slot, inventory_index } => {
                self.handle_unequip_to_bag_slot(from, slot, inventory_index as usize);
            }
            ClientMsg::SetLoadoutSlot { slot_index, ability_id } => {
                self.handle_set_loadout_slot(from, slot_index, ability_id);
            }
            ClientMsg::Ack { .. } => { /* phase 4 */ }
            ClientMsg::Goodbye => {
                log::info!("Goodbye from {from:?}");
            }
            ClientMsg::RequestEnterRift => {
                use crate::sim::ExitVoteRequest;
                let current = self.floor_for_client(from);
                // Hub → rift: move only this client into the
                // active rift instance. Other hub players keep
                // their current scene; the rift sim continues
                // ticking regardless of who else is in it.
                if current == 0 {
                    self.move_client_to_rift(from);
                    return;
                }
                // In-rift → next floor: open a descend ready
                // check unless the party is solo. Solo players
                // bypass the vote and transition immediately.
                // (Multi-floor descends happen on the rift sim;
                // future per-party rifts will keep this scoped
                // to the requester's instance.)
                match self.rift.request_descend_vote(from) {
                    ExitVoteRequest::Pass => {
                        log::info!("vote: solo {from:?} descending");
                        let next_index = self.rift.floor_index + 1;
                        self.advance_rift_floor(next_index);
                    }
                    ExitVoteRequest::Opened => { /* broadcast via take_exit_vote_update */ }
                    ExitVoteRequest::Refused => {
                        log::debug!("vote: refused descend from {from:?}");
                    }
                }
            }
            ClientMsg::RequestReturnToHub => {
                if self.floor_for_client(from) != 0 {
                    self.move_client_to_hub(from);
                }
            }
            ClientMsg::RequestRoster { account_name } => {
                log::info!("RequestRoster from {from:?}: account={account_name:?}");
                let entries = self.lookup_roster(&account_name);
                self.send_to(from, Channel::Control, &ServerMsg::Roster { entries });
            }
            ClientMsg::RiftExitVoteStart => {
                use crate::sim::ExitVoteRequest;
                if self.floor_for_client(from) == 0 {
                    log::debug!("vote: refused start from hub player {from:?}");
                    return;
                }
                match self.rift.request_exit_vote(from) {
                    ExitVoteRequest::Pass => {
                        log::info!("vote: solo {from:?} exiting rift");
                        let wiped = self.rift.wipe_dead_loot();
                        self.return_all_rift_to_hub();
                        for cid in wiped {
                            self.broadcast_inventory_state(cid);
                            self.persist_inventory_state(cid);
                        }
                    }
                    ExitVoteRequest::Opened => { /* broadcast via take_exit_vote_update */ }
                    ExitVoteRequest::Refused => {
                        log::debug!("vote: refused start from {from:?}");
                    }
                }
            }
            ClientMsg::RiftExitVoteCast { yes } => {
                self.rift.cast_exit_vote(from, yes);
            }
            ClientMsg::SetShrineChannel { shrine } => {
                self.sim_for_client_mut(from).set_shrine_channel(from, shrine);
            }
            ClientMsg::ChatSend { channel, target, text } => {
                self.handle_chat_send(from, channel, target, text);
            }
        }
    }

    // ── Per-message handlers live in `handlers/` siblings. ──
    // Login / hydrate flow:    handlers/session.rs
    // Bag / equip / stash:     handlers/inventory.rs
    // Persistence reads/saves: handlers/persistence.rs

    /// Look up which floor a given client is currently on.
    /// Defaults to 0 (hub) for any client we haven't tracked
    /// yet — the same default used at Hello time.
    pub(crate) fn floor_for_client(&self, cid: ClientId) -> u32 {
        self.client_floor.get(&cid).copied().unwrap_or(0)
    }

    /// Resolve which Sim a given client lives in. Hub for floor
    /// 0, rift for everything else. Used by every per-client
    /// gameplay handler so the same dispatch table works
    /// regardless of where the player currently is.
    pub(crate) fn sim_for_client(&self, cid: ClientId) -> &Sim {
        if self.floor_for_client(cid) == 0 { &self.hub } else { &self.rift }
    }

    pub(crate) fn sim_for_client_mut(&mut self, cid: ClientId) -> &mut Sim {
        if self.floor_for_client(cid) == 0 { &mut self.hub } else { &mut self.rift }
    }

    /// All currently-connected clients sitting on the given
    /// floor. Used to scope event / progress / vote broadcasts
    /// so a hub player never sees rift-only payloads (and vice
    /// versa once we add hub-only broadcasts).
    fn clients_on_floor(&self, floor_index: u32) -> Vec<ClientId> {
        self.client_floor
            .iter()
            .filter_map(|(cid, &f)| if f == floor_index { Some(*cid) } else { None })
            .collect()
    }

    /// Send a message to every client currently on `floor_index`.
    /// Implemented as repeated unicast rather than `broadcast`
    /// because renet has no per-channel multicast and we want
    /// hub clients to *not* see rift-scoped traffic.
    fn broadcast_to_floor(&mut self, floor_index: u32, ch: Channel, msg: &ServerMsg) {
        let recipients = self.clients_on_floor(floor_index);
        for cid in recipients {
            self.send_to(cid, ch, msg);
        }
    }

    /// Move `cid` from the hub into the active rift instance and
    /// hand them a `LoadFloor` so their client rebuilds the
    /// scene. No-op if the client is somehow not in the hub.
    fn move_client_to_rift(&mut self, cid: ClientId) {
        let Some((player, effects)) = self.hub.extract_player(cid) else {
            log::warn!("move_client_to_rift: {cid:?} has no hub entity");
            return;
        };
        let _net_id = self.rift.inject_player(cid, player, effects);
        self.client_floor.insert(cid, self.rift.floor_index);
        let spawn = self.rift.floor.spawn_pos;
        let load = ServerMsg::LoadFloor {
            seed: self.rift.floor_seed,
            index: self.rift.floor_index,
            is_hub: false,
            spawn_pos: [spawn.x, 0.0, spawn.z],
            tick: self.tick,
        };
        self.send_to(cid, Channel::Control, &load);
        // Replay the rift's current progress meter so this
        // late-joiner's HUD lines up with the floor state.
        let rp = self.rift.rift_progress();
        self.send_to(
            cid,
            Channel::Control,
            &ServerMsg::RiftProgress {
                progress: rp.progress,
                required: rp.required,
                boss_spawned: rp.boss_spawned,
                boss_killed: rp.boss_killed,
                floor_complete: rp.floor_complete,
            },
        );
        // FLOOR system ping so existing rift inhabitants see
        // who joined them.
        let floor_idx = self.rift.floor_index;
        let name = self
            .sessions
            .get(cid)
            .and_then(|s| s.character_name.clone())
            .unwrap_or_else(|| "A player".to_string());
        self.emit_system_floor(
            floor_idx,
            &format!("{name} entered floor {floor_idx}."),
        );
    }

    /// Move `cid` from the rift back into the hub.
    fn move_client_to_hub(&mut self, cid: ClientId) {
        let Some((player, effects)) = self.rift.extract_player(cid) else {
            log::warn!("move_client_to_hub: {cid:?} has no rift entity");
            return;
        };
        let _net_id = self.hub.inject_player(cid, player, effects);
        self.client_floor.insert(cid, 0);
        let spawn = self.hub.floor.spawn_pos;
        let load = ServerMsg::LoadFloor {
            seed: self.hub.floor_seed,
            index: 0,
            is_hub: true,
            spawn_pos: [spawn.x, 0.0, spawn.z],
            tick: self.tick,
        };
        self.send_to(cid, Channel::Control, &load);
    }

    /// Move every client currently in the rift back to the hub
    /// and reset the rift instance for the next descent. Used
    /// by the exit-vote success path and the wipe-respawn timer.
    fn return_all_rift_to_hub(&mut self) {
        let rift_clients = self.clients_on_floor(self.rift.floor_index);
        for cid in &rift_clients {
            self.move_client_to_hub(*cid);
        }
        // Reset the rift sim so the next entry meets a fresh
        // floor (enemies despawned, vote / progress cleared).
        // Future: re-roll the seed here for replay variety.
        let same_idx = self.rift.floor_index.max(1);
        self.rift.change_floor(same_idx);
        // Same-floor reset still wipes the boss-kill state.
        self.prev_boss_killed = false;
    }

    /// Server-driven floor advance inside the rift. Moves every
    /// rift client onto the new floor index in lockstep — the
    /// "shared rift" model where descend votes affect everyone
    /// who's currently in the instance.
    fn advance_rift_floor(&mut self, new_index: u32) {
        // Re-map every client currently in the rift to the new
        // floor index *before* broadcasting — `client_floor`
        // is what `broadcast_to_floor` filters on, so without
        // this update the LoadFloor packet would target the
        // already-vacated old index and nobody would rebuild.
        let old_index = self.rift.floor_index;
        let movers = self.clients_on_floor(old_index);
        let spawn = self.rift.change_floor(new_index);
        // Boss-kill rising-edge tracker is per-floor — reset
        // on advance so the next boss is announced fresh.
        self.prev_boss_killed = false;
        for cid in &movers {
            self.client_floor.insert(*cid, new_index);
        }
        let msg = ServerMsg::LoadFloor {
            seed: self.rift.floor_seed,
            index: new_index,
            is_hub: false,
            spawn_pos: spawn.to_array(),
            tick: self.tick,
        };
        for cid in &movers {
            self.send_to(*cid, Channel::Control, &msg);
        }
        // Single SYSTEM line on the new floor — descend votes
        // move the whole party in lockstep, so we only need
        // one announce, not one per mover.
        if !movers.is_empty() {
            self.emit_system_floor(
                new_index,
                &format!("Party descended to floor {new_index}."),
            );
        }
    }

    pub(crate) fn send_to(&mut self, to: ClientId, ch: Channel, msg: &ServerMsg) {
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
        // Step both sims in lockstep. Order doesn't matter — they
        // share no entities and gameplay never crosses the floor
        // boundary mid-tick.
        self.hub.step(dt, self.tick);
        self.rift.step(dt, self.tick);

        // World events: each sim emits its own queue. Route every
        // event to clients on that sim's floor only — a hub
        // player has no business seeing a damage tick from the
        // rift (and vice versa).
        let hub_events = self.hub.drain_events();
        for ev in hub_events {
            self.broadcast_to_floor(0, Channel::Event, &ServerMsg::Event(ev));
        }
        let rift_floor = self.rift.floor_index;
        let rift_events = self.rift.drain_events();
        for ev in rift_events {
            self.broadcast_to_floor(rift_floor, Channel::Event, &ServerMsg::Event(ev));
        }

        // Per-player XP / level updates: targeted send to each
        // owner. Drained from both sims so XP earned in either
        // location persists.
        for u in self.hub.drain_stat_updates().into_iter()
            .chain(self.rift.drain_stat_updates().into_iter())
        {
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
            // SYSTEM-to-self chat ping on level transitions so
            // the player gets visible feedback in the
            // scrollback even if their HUD level pip is
            // off-screen.
            if u.levelled_up {
                self.emit_system_to(
                    u.client_id,
                    &format!("You reached level {}!", u.level),
                );
            }
        }

        // Rift-progress changes: only the rift sim has a
        // meaningful progress bar. Scoped to clients currently
        // in the rift.
        if let Some(rp) = self.rift.take_rift_progress_update() {
            // Detect boss-kill rising edge so we can fire one
            // GLOBAL system announcement per kill instead of
            // one per progress snapshot.
            let boss_just_killed = rp.boss_killed && !self.prev_boss_killed;
            self.prev_boss_killed = rp.boss_killed;
            self.broadcast_to_floor(
                rift_floor,
                Channel::Control,
                &ServerMsg::RiftProgress {
                    progress: rp.progress,
                    required: rp.required,
                    boss_spawned: rp.boss_spawned,
                    boss_killed: rp.boss_killed,
                    floor_complete: rp.floor_complete,
                },
            );
            if boss_just_killed {
                let floor = self.rift.floor_index;
                self.emit_system_global(&format!(
                    "The boss of floor {floor} has been slain!"
                ));
            }
        }

        // Death log (rift only — hub has no enemies).
        for (cid, net_id) in self.rift.drain_player_deaths() {
            log::info!("player died: {cid:?} ({net_id:?})");
            // Look up the dead player's name + which floor
            // they died on so the FLOOR system message lands
            // with the right audience.
            let name = self
                .sessions
                .get(cid)
                .and_then(|s| s.character_name.clone())
                .unwrap_or_else(|| "A player".to_string());
            let floor = self.floor_for_client(cid);
            self.emit_system_floor(floor, &format!("{name} was slain."));
        }
        // Hub sim drain too, just in case (no-op today but
        // keeps the queue from accumulating across reconnects).
        for _ in self.hub.drain_player_deaths() {}

        // Wipe-respawn: rift sim arms the timer when every
        // player on the rift floor is dead. Pull the whole
        // party back to the hub and wipe their loot.
        if self.rift.take_hub_respawn_request() {
            log::info!("respawning party to hub after wipe");
            let wiped = self.rift.wipe_dead_loot();
            self.return_all_rift_to_hub();
            for cid in wiped {
                self.broadcast_inventory_state(cid);
                self.persist_inventory_state(cid);
            }
        }
        // Hub sim's hub-respawn-request shouldn't happen, but
        // drain it defensively to avoid a stuck flag.
        let _ = self.hub.take_hub_respawn_request();

        // Rift exit vote: tick + resolve. Only the rift has a vote.
        // Capture the recipient set *before* resolving the
        // outcome — a passing Exit vote moves every voter back
        // to the hub, so by the time `take_exit_vote_update`
        // hands us the "vote cleared" state, those clients are
        // no longer on the rift floor and a `broadcast_to_floor`
        // would target an empty audience. The voters' HUDs
        // would then keep their stale vote panel visible
        // forever. Send the post-resolve vote update to whoever
        // was *in the vote*, regardless of where they're
        // standing now.
        let vote_recipients = self.clients_on_floor(rift_floor);
        let outcome = self.rift.tick_exit_vote(dt);
        match outcome {
            crate::sim::vote::TickOutcome::Passed(
                rift_net::messages::VoteKind::Exit,
            ) => {
                log::info!("vote: party voted to leave rift");
                let wiped = self.rift.wipe_dead_loot();
                self.return_all_rift_to_hub();
                for cid in wiped {
                    self.broadcast_inventory_state(cid);
                    self.persist_inventory_state(cid);
                }
            }
            crate::sim::vote::TickOutcome::Passed(
                rift_net::messages::VoteKind::Descend,
            ) => {
                log::info!("vote: party voted to descend");
                let next_index = self.rift.floor_index + 1;
                self.advance_rift_floor(next_index);
            }
            _ => {}
        }
        if let Some(state) = self.rift.take_exit_vote_update() {
            let msg = ServerMsg::RiftExitVote(state);
            for cid in &vote_recipients {
                self.send_to(*cid, Channel::Control, &msg);
            }
        }
    }

    /// Build and broadcast a per-client snapshot. Each client gets
    /// their own copy because `ack_seq` is per-client *and* we
    /// route the build through whichever Sim they currently
    /// inhabit so they never see entities from the other floor.
    fn broadcast_snapshot(&mut self) {
        let connected: Vec<u64> = self
            .handle
            .server
            .clients_id()
            .iter()
            .map(|id| id.raw())
            .collect();
        let tick = self.tick;
        for raw in connected {
            let cid = ClientId(raw);
            let snap = self.sim_for_client_mut(cid).build_snapshot(tick, cid);
            self.send_to(cid, Channel::Snapshot, &ServerMsg::Snapshot(snap));
        }
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
    // Resolve to a SocketAddr but accept hostnames (e.g. Fly.io's
    // `fly-global-services:34000`) in addition to literal IPs.
    fn resolve_bind(s: &str) -> SocketAddr {
        s.to_socket_addrs()
            .unwrap_or_else(|e| panic!("invalid bind address {s:?}: {e}"))
            .next()
            .unwrap_or_else(|| panic!("bind address {s:?} resolved to no addrs"))
    }
    let mut bind: SocketAddr = match std::env::var("RIFT_BIND") {
        Ok(v) if !v.is_empty() => resolve_bind(&v),
        _ => match std::env::var("PORT") {
            Ok(p) if !p.is_empty() => resolve_bind(&format!("0.0.0.0:{p}")),
            _ => resolve_bind("0.0.0.0:34000"),
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
                    bind = resolve_bind(&v);
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
