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
mod instance;
mod party;
mod session;
mod sim;

use chat::ChatHistory;
use instance::{InstanceManager, RiftInstanceId};
use party::PartyManager;
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
    /// rift portal (which moves them into one of the
    /// [`Self::instances`]).
    hub: Sim,
    /// Active rift instances, keyed by id. Each instance owns
    /// its own [`Sim`] + envelope of metadata (private vs.
    /// matchmade, owning party, capacity, start floor). The
    /// map is empty when no rift run is in progress.
    instances: InstanceManager,
    /// Maps each connected client to which sim they currently
    /// inhabit: `Some(id)` = rift instance, `None` (no entry) =
    /// hub. Updated on Hello (always hub) and on portal
    /// transitions. The matching `client_floor` map below
    /// mirrors the player's display floor (always `0` when
    /// hub-side, `instance.sim.floor_index` when in a rift) so
    /// chat / event scoping logic that pre-dates the instance
    /// model keeps working.
    client_instance: HashMap<ClientId, RiftInstanceId>,
    client_floor: HashMap<ClientId, u32>,
    /// Party state. Solo players have no row here. See
    /// [`PartyManager`] for the invariant set.
    parties: PartyManager,
    /// Carries the leftover wall-clock between fixed-step ticks so
    /// we don't drift when frame_dt isn't a clean multiple of the
    /// tick period.
    tick_accumulator: Duration,
    /// Same idea for snapshot broadcasts (decoupled from sim rate).
    snapshot_accumulator: Duration,
    /// Drives 1 Hz `ServerMsg::MeterSnapshot` broadcasts so the
    /// HUD damage-meter panel updates without spamming the
    /// control channel every tick.
    meter_accumulator: Duration,
    /// Persistence worker handle. `None` when the server is started
    /// without `--database-url`, in which case all characters are
    /// purely in-memory — useful for offline iteration / tests.
    persistence: Option<PersistenceHandle>,
    /// Wall-clock since the last opportunistic auto-save tick.
    auto_save_accumulator: Duration,
    /// Server-global chat history. New connections replay
    /// recent GLOBAL + SYSTEM lines from this on accept.
    chat: ChatHistory,
    /// Pending portal proposals awaiting opt-in from the
    /// non-proposer party members. Drained per-tick by the
    /// portal handler. Empty when nobody is mid-modal.
    pending_portal_proposals: HashMap<ClientId, crate::handlers::portal::PendingProposal>,
}

impl Server {
    fn new(
        bind: SocketAddr,
        public: SocketAddr,
        persistence: Option<PersistenceHandle>,
    ) -> Result<Self> {
        let handle = open_server(bind, public, MAX_CLIENTS, &NetSettings::default())?;
        // Hub sim is always present. Rift instances are
        // created on-demand when a player walks through the
        // portal modal (see `handlers/portal.rs`).
        let hub = Sim::new(42, 0);
        Ok(Self {
            handle,
            tick: NetTick::default(),
            sessions: SessionManager::new(),
            last_tick: Instant::now(),
            hub,
            instances: InstanceManager::new(),
            client_instance: HashMap::new(),
            client_floor: HashMap::new(),
            parties: PartyManager::new(),
            tick_accumulator: Duration::ZERO,
            snapshot_accumulator: Duration::ZERO,
            meter_accumulator: Duration::ZERO,
            persistence,
            auto_save_accumulator: Duration::ZERO,
            chat: ChatHistory::default(),
            pending_portal_proposals: HashMap::new(),
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

        // Combat-meter broadcast at 1 Hz. The HUD doesn't need
        // sub-second precision and these messages can grow with
        // party size, so the cheap rate keeps bandwidth flat.
        const METER_PERIOD: Duration = Duration::from_secs(1);
        self.meter_accumulator += frame_dt;
        if self.meter_accumulator >= METER_PERIOD {
            self.meter_accumulator -= METER_PERIOD;
            self.broadcast_meters();
        }

        // Periodic auto-save. Fire-and-forget per session — the
        // persistence worker drains writes off-thread so this loop
        // never blocks on a slow database.
        self.auto_save_accumulator += frame_dt;
        if self.auto_save_accumulator >= AUTO_SAVE_INTERVAL {
            self.auto_save_accumulator -= AUTO_SAVE_INTERVAL;
            self.auto_save_all();
        }

        // Time-out expired party invites and stale portal
        // proposals. Both are cheap O(n) walks over tiny
        // collections, so we just hit them every frame rather
        // than carrying a separate accumulator.
        self.parties.evict_expired_invites(now);
        self.tick_portal_proposals(now);

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
                if let Some(instance_id) = self.client_instance.remove(&cid) {
                    if let Some(inst) = self.instances.get_mut(instance_id) {
                        inst.sim.despawn_player(cid);
                    }
                    // If this disconnect emptied the instance,
                    // dissolve it so it doesn't tick forever.
                    if self.clients_in_instance(instance_id).is_empty() {
                        self.instances.dissolve(instance_id);
                    }
                }
                self.client_floor.remove(&cid);
                // Drop them from any party they belonged to —
                // disconnect mirrors a /leave for the rest of
                // the party's UI. Snapshot the roster before
                // the remove so the singleton-collapse path
                // can still notify the orphaned lone member.
                let pre_members: Vec<ClientId> = self
                    .parties
                    .party_of(cid)
                    .map(|p| p.members.clone())
                    .unwrap_or_default();
                let removed_party = self.parties.leave(cid);
                self.broadcast_party_after_remove(cid, removed_party, &pre_members);
                // Tear down any portal proposals that involve
                // this client. Two paths:
                //  * They were the proposer — cancel the
                //    proposal and close every awaiting
                //    member's modal.
                //  * They were an awaiting confirmer — drop
                //    them from `awaiting`. If that empties
                //    the set, resolve the proposal now
                //    rather than waiting 30 s.
                self.cancel_portal_proposal_for(cid);
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
            ClientMsg::DepositToStash { inventory_index, tab_index } => {
                self.handle_deposit_to_stash(
                    from,
                    inventory_index as usize,
                    tab_index as usize,
                );
            }
            ClientMsg::DepositToStashSlot { inventory_index, tab_index, stash_index } => {
                self.handle_deposit_to_stash_slot(
                    from,
                    inventory_index as usize,
                    tab_index as usize,
                    stash_index as usize,
                );
            }
            ClientMsg::WithdrawFromStash { tab_index, stash_index } => {
                self.handle_withdraw_from_stash(
                    from,
                    tab_index as usize,
                    stash_index as usize,
                );
            }
            ClientMsg::WithdrawFromStashSlot { tab_index, stash_index, inventory_index } => {
                self.handle_withdraw_from_stash_slot(
                    from,
                    tab_index as usize,
                    stash_index as usize,
                    inventory_index as usize,
                );
            }
            ClientMsg::SwapInventorySlots { a, b } => {
                self.handle_swap_inventory_slots(from, a as usize, b as usize);
            }
            ClientMsg::SwapStashSlots { tab_index, a, b } => {
                self.handle_swap_stash_slots(
                    from,
                    tab_index as usize,
                    a as usize,
                    b as usize,
                );
            }
            ClientMsg::BuyStashTab => {
                self.handle_buy_stash_tab(from);
            }
            ClientMsg::RenameStashTab { tab_index, name } => {
                self.handle_rename_stash_tab(from, tab_index as usize, &name);
            }
            ClientMsg::RecolorStashTab { tab_index, color } => {
                self.handle_recolor_stash_tab(from, tab_index as usize, color);
            }
            ClientMsg::DropInventoryItem { inventory_index } => {
                if self.sim_for_client(from).is_ghost(from) {
                    return;
                }
                self.handle_drop_inventory_item(from, inventory_index as usize);
            }
            ClientMsg::SalvageInventoryItem { inventory_index } => {
                if self.sim_for_client(from).is_ghost(from) {
                    return;
                }
                self.handle_salvage_inventory_item(from, inventory_index as usize);
            }
            ClientMsg::SalvageInventoryBulk { rarity_max } => {
                if self.sim_for_client(from).is_ghost(from) {
                    return;
                }
                self.handle_salvage_inventory_bulk(from, rarity_max);
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
                // Legacy / shorthand: behave as if the player
                // chose Solo at floor 1 in the portal modal.
                // Real entries arrive via `ProposeRiftEntry`.
                if self.instance_for_client(from).is_none() {
                    self.handle_propose_rift_entry(
                        from,
                        1,
                        rift_net::messages::party_mode::SOLO,
                    );
                    return;
                }
                use crate::sim::ExitVoteRequest;
                let Some(instance_id) = self.instance_for_client(from) else {
                    return;
                };
                let req = self
                    .instances
                    .get_mut(instance_id)
                    .map(|inst| inst.sim.request_descend_vote(from));
                match req {
                    Some(ExitVoteRequest::Pass) => {
                        log::info!("vote: solo {from:?} descending");
                        let next_index = self
                            .instances
                            .get(instance_id)
                            .map(|inst| inst.sim.floor_index + 1)
                            .unwrap_or(1);
                        self.advance_instance_floor(instance_id, next_index);
                    }
                    Some(ExitVoteRequest::Opened) => {}
                    Some(ExitVoteRequest::Refused) => {
                        log::debug!("vote: refused descend from {from:?}");
                    }
                    None => {}
                }
            }
            ClientMsg::RequestReturnToHub => {
                if self.instance_for_client(from).is_some() {
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
                let Some(instance_id) = self.instance_for_client(from) else {
                    log::debug!("vote: refused start from hub player {from:?}");
                    return;
                };
                let req = self
                    .instances
                    .get_mut(instance_id)
                    .map(|inst| inst.sim.request_exit_vote(from));
                match req {
                    Some(ExitVoteRequest::Pass) => {
                        log::info!("vote: solo {from:?} exiting rift");
                        let wiped = self
                            .instances
                            .get_mut(instance_id)
                            .map(|inst| inst.sim.wipe_dead_loot())
                            .unwrap_or_default();
                        self.return_all_to_hub(instance_id);
                        for cid in wiped {
                            self.broadcast_inventory_state(cid);
                            self.persist_inventory_state(cid);
                        }
                    }
                    Some(ExitVoteRequest::Opened) => {}
                    Some(ExitVoteRequest::Refused) => {
                        log::debug!("vote: refused start from {from:?}");
                    }
                    None => {}
                }
            }
            ClientMsg::RiftExitVoteCast { yes } => {
                if let Some(instance_id) = self.instance_for_client(from) {
                    if let Some(inst) = self.instances.get_mut(instance_id) {
                        inst.sim.cast_exit_vote(from, yes);
                    }
                }
            }
            ClientMsg::SetShrineChannel { shrine } => {
                self.sim_for_client_mut(from).set_shrine_channel(from, shrine);
            }
            ClientMsg::ChatSend { channel, target, text } => {
                self.handle_chat_send(from, channel, target, text);
            }
            ClientMsg::ProposeRiftEntry { start_floor, mode } => {
                self.handle_propose_rift_entry(from, start_floor, mode);
            }
            ClientMsg::PortalConfirm { accept } => {
                self.handle_portal_confirm(from, accept);
            }
            ClientMsg::PartyInvite { name } => {
                self.handle_party_invite(from, name);
            }
            ClientMsg::PartyAccept { from: which } => {
                self.handle_party_accept(from, which);
            }
            ClientMsg::PartyDecline { from: which } => {
                self.handle_party_decline(from, which);
            }
            ClientMsg::PartyLeave => {
                self.handle_party_leave(from);
            }
            ClientMsg::PartyKick { name } => {
                self.handle_party_kick(from, name);
            }
            ClientMsg::PartyPromote { name } => {
                self.handle_party_promote(from, name);
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

    /// Resolve which Sim a given client lives in. Hub when
    /// they're not in any rift instance; otherwise the sim of
    /// their current instance.
    pub(crate) fn sim_for_client(&self, cid: ClientId) -> &Sim {
        match self.client_instance.get(&cid) {
            Some(id) => self
                .instances
                .get(*id)
                .map(|i| &i.sim)
                .unwrap_or(&self.hub),
            None => &self.hub,
        }
    }

    pub(crate) fn sim_for_client_mut(&mut self, cid: ClientId) -> &mut Sim {
        match self.client_instance.get(&cid).copied() {
            Some(id) => match self.instances.get_mut(id) {
                Some(inst) => &mut inst.sim,
                None => &mut self.hub,
            },
            None => &mut self.hub,
        }
    }

    /// `Some(id)` when `cid` is currently inside a rift
    /// instance. `None` for hub players. Mirrors the gating
    /// path the chat router and party / portal handlers use to
    /// scope their work.
    pub(crate) fn instance_for_client(&self, cid: ClientId) -> Option<RiftInstanceId> {
        self.client_instance.get(&cid).copied()
    }

    /// All currently-connected clients sitting in `instance`.
    /// Used to scope event / progress / vote broadcasts to
    /// just the instance's audience.
    pub(crate) fn clients_in_instance(&self, instance: RiftInstanceId) -> Vec<ClientId> {
        self.client_instance
            .iter()
            .filter_map(|(cid, &id)| if id == instance { Some(*cid) } else { None })
            .collect()
    }

    /// All currently-connected clients on the hub (i.e. not
    /// in any rift instance). Used by `chat_channel::HUB` and
    /// hub-scoped event broadcasts.
    pub(crate) fn clients_on_hub(&self) -> Vec<ClientId> {
        self.sessions
            .iter()
            .map(|s| s.client_id)
            .filter(|cid| !self.client_instance.contains_key(cid))
            .collect()
    }

    /// All currently-connected clients sharing the same
    /// world-scope as `cid`: every other player in their rift
    /// instance, or every other hub player when `cid` is
    /// hub-side. Used by the FLOOR chat channel and by
    /// per-floor system pings.
    pub(crate) fn clients_in_world_with(&self, cid: ClientId) -> Vec<ClientId> {
        match self.client_instance.get(&cid) {
            Some(id) => self.clients_in_instance(*id),
            None => self.clients_on_hub(),
        }
    }

    /// Send a message to every client currently in `instance`.
    /// Implemented as repeated unicast since renet has no
    /// per-channel multicast and we want each rift instance's
    /// traffic isolated from every other instance.
    pub(crate) fn broadcast_to_instance(
        &mut self,
        instance: RiftInstanceId,
        ch: Channel,
        msg: &ServerMsg,
    ) {
        let recipients = self.clients_in_instance(instance);
        for cid in recipients {
            self.send_to(cid, ch, msg);
        }
    }

    /// Send a message to every client currently on the hub
    /// (i.e. not in any rift instance).
    pub(crate) fn broadcast_to_hub(&mut self, ch: Channel, msg: &ServerMsg) {
        let recipients = self.clients_on_hub();
        for cid in recipients {
            self.send_to(cid, ch, msg);
        }
    }

    /// Move `cid` from the hub into `instance` and hand them a
    /// `LoadFloor` so their client rebuilds the scene. No-op
    /// if the client is somehow not in the hub or the
    /// instance has been dropped underneath us.
    pub(crate) fn move_client_to_instance(
        &mut self,
        cid: ClientId,
        instance: RiftInstanceId,
    ) {
        let Some((player, effects)) = self.hub.extract_player(cid) else {
            log::warn!("move_client_to_instance: {cid:?} has no hub entity");
            return;
        };
        let Some(inst) = self.instances.get_mut(instance) else {
            log::warn!("move_client_to_instance: instance {instance:?} gone");
            // Re-inject the player back into the hub so they
            // don't disappear from every snapshot.
            let _ = self.hub.inject_player(cid, player, effects);
            return;
        };
        let _net_id = inst.sim.inject_player(cid, player, effects);
        let floor_idx = inst.sim.floor_index;
        let seed = inst.sim.floor_seed;
        let spawn = inst.sim.floor.spawn_pos;
        let rp = inst.sim.rift_progress();
        self.client_instance.insert(cid, instance);
        self.client_floor.insert(cid, floor_idx);
        let load = ServerMsg::LoadFloor {
            seed,
            index: floor_idx,
            is_hub: false,
            spawn_pos: [spawn.x, 0.0, spawn.z],
            tick: self.tick,
        };
        self.send_to(cid, Channel::Control, &load);
        // Replay the rift's current progress meter so this
        // late-joiner's HUD lines up with the floor state.
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
        // FLOOR system ping so existing instance inhabitants see
        // who joined them.
        let name = self
            .sessions
            .get(cid)
            .and_then(|s| s.character_name.clone())
            .unwrap_or_else(|| "A player".to_string());
        self.emit_system_floor(
            floor_idx,
            &format!("{name} entered floor {floor_idx}."),
        );
        // Cross-world equipment-visual rendezvous: tell
        // `cid` what the existing instance members are
        // wearing, and the existing members what `cid` is
        // wearing. Without this, a player crossing into a
        // rift would see other party members undressed
        // until those members changed equipment.
        self.catch_up_peer_equipment_visuals(cid);
        self.broadcast_peer_equipment_visuals(cid);
    }

    /// Move `cid` from whatever rift instance they're in back
    /// to the hub. No-op if `cid` is hub-side already. After
    /// the move, if the instance has no remaining members it
    /// is dissolved.
    pub(crate) fn move_client_to_hub(&mut self, cid: ClientId) {
        let Some(instance_id) = self.client_instance.remove(&cid) else {
            return;
        };
        let mut maybe_dissolve = false;
        if let Some(inst) = self.instances.get_mut(instance_id) {
            if let Some((player, effects)) = inst.sim.extract_player(cid) {
                let _net_id = self.hub.inject_player(cid, player, effects);
            }
            if self.clients_in_instance(instance_id).is_empty() {
                maybe_dissolve = true;
            }
        }
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
        // Same cross-world rendezvous as `move_client_to_instance`,
        // mirrored for the hub side.
        self.catch_up_peer_equipment_visuals(cid);
        self.broadcast_peer_equipment_visuals(cid);
        if maybe_dissolve {
            self.instances.dissolve(instance_id);
        }
    }

    /// Move every client currently in `instance` back to the
    /// hub and dissolve the instance. Used by the exit-vote
    /// success path and the wipe-respawn timer.
    pub(crate) fn return_all_to_hub(&mut self, instance: RiftInstanceId) {
        let movers = self.clients_in_instance(instance);
        for cid in &movers {
            self.move_client_to_hub(*cid);
        }
        // Belt-and-suspenders: if no movers existed (race
        // between disconnect + vote) but the instance is
        // still around, drop it explicitly so the map doesn't
        // leak.
        if self.instances.get(instance).is_some() {
            self.instances.dissolve(instance);
        }
    }

    /// Server-driven floor advance inside `instance`. Moves
    /// every client in the instance onto the new floor index
    /// in lockstep — the "shared instance" model where descend
    /// votes affect everyone who's currently in it.
    pub(crate) fn advance_instance_floor(
        &mut self,
        instance: RiftInstanceId,
        new_index: u32,
    ) {
        let Some(inst) = self.instances.get_mut(instance) else {
            log::warn!("advance_instance_floor: instance {instance:?} gone");
            return;
        };
        let movers = self
            .client_instance
            .iter()
            .filter_map(|(cid, id)| if *id == instance { Some(*cid) } else { None })
            .collect::<Vec<_>>();
        let spawn = inst.sim.change_floor(new_index);
        // Boss-kill rising-edge tracker is per-floor — reset
        // on advance so the next boss is announced fresh.
        inst.prev_boss_killed = false;
        let seed = inst.sim.floor_seed;
        for cid in &movers {
            self.client_floor.insert(*cid, new_index);
        }
        let msg = ServerMsg::LoadFloor {
            seed,
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
        // Drop sends to clients that have already been evicted
        // by the netcode layer. The handler graph holds onto
        // `ClientId`s in queued broadcasts (party UI, portal
        // proposals, chat lines, …) and a disconnect that lands
        // mid-frame can leave one of those queues pointing at a
        // gone client. Without this guard `renet::send_message`
        // logs `Tried to send a message to invalid client …`
        // every time, which floods the server console on every
        // user disconnect. We still log at trace so a real bug
        // (e.g. sending to an id we never accepted) is
        // discoverable when needed.
        let raw = renet::ClientId::from_raw(to.0);
        if !self.handle.server.is_connected(raw) {
            log::trace!(
                "send_to: skipping {:?} — client no longer connected",
                to
            );
            return;
        }
        let bytes = match encode(msg) {
            Ok(b) => b,
            Err(e) => {
                log::error!("encode {msg:?}: {e}");
                return;
            }
        };
        self.handle.server.send_message(raw, ch as u8, bytes);
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
        // Step the hub plus every active instance. Order
        // doesn't matter — they share no entities and gameplay
        // never crosses sim boundaries mid-tick.
        self.hub.step(dt, self.tick);
        let instance_ids: Vec<RiftInstanceId> =
            self.instances.iter().map(|(id, _)| *id).collect();
        for id in &instance_ids {
            if let Some(inst) = self.instances.get_mut(*id) {
                inst.sim.step(dt, self.tick);
            }
        }

        // World events: each sim emits its own queue. Hub
        // events fan out to hub players only; each instance's
        // events fan out to its own audience.
        let hub_events = self.hub.drain_events();
        for ev in hub_events {
            self.broadcast_to_hub(Channel::Event, &ServerMsg::Event(ev));
        }
        for id in &instance_ids {
            let evs = self
                .instances
                .get_mut(*id)
                .map(|inst| inst.sim.drain_events())
                .unwrap_or_default();
            for ev in evs {
                self.broadcast_to_instance(*id, Channel::Event, &ServerMsg::Event(ev));
            }
        }

        // Per-player XP / level updates: targeted send to each
        // owner. Drained from every sim so XP earned anywhere
        // persists.
        let mut stat_updates = self.hub.drain_stat_updates();
        for id in &instance_ids {
            if let Some(inst) = self.instances.get_mut(*id) {
                stat_updates.extend(inst.sim.drain_stat_updates());
            }
        }
        for u in stat_updates {
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

        // Rift-progress changes: per-instance, scoped to that
        // instance's audience. Boss-kill rising edge fires the
        // GLOBAL announcement and bumps every member's
        // `deepest_cleared_floor` once per actual kill.
        for id in &instance_ids {
            let Some(inst) = self.instances.get_mut(*id) else { continue };
            let Some(rp) = inst.sim.take_rift_progress_update() else { continue };
            let boss_just_killed = rp.boss_killed && !inst.prev_boss_killed;
            inst.prev_boss_killed = rp.boss_killed;
            let floor = inst.sim.floor_index;
            self.broadcast_to_instance(
                *id,
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
                self.emit_system_global(&format!(
                    "The boss of floor {floor} has been slain!"
                ));
                // Bump deepest_cleared_floor for every member
                // currently in the instance.
                let members = self.clients_in_instance(*id);
                for cid in members {
                    self.bump_deepest_cleared_floor(cid, floor);
                }
            }
        }

        // Death log (rift instances only — hub has no enemies).
        for id in &instance_ids {
            let deaths = self
                .instances
                .get_mut(*id)
                .map(|inst| inst.sim.drain_player_deaths())
                .unwrap_or_default();
            for (cid, net_id) in deaths {
                log::info!("player died: {cid:?} ({net_id:?})");
                let name = self
                    .sessions
                    .get(cid)
                    .and_then(|s| s.character_name.clone())
                    .unwrap_or_else(|| "A player".to_string());
                let floor = self.floor_for_client(cid);
                self.emit_system_floor(floor, &format!("{name} was slain."));
            }
        }
        // Hub sim drain too, just in case (no-op today but
        // keeps the queue from accumulating across reconnects).
        for _ in self.hub.drain_player_deaths() {}

        // Wipe-respawn: each instance arms its own timer when
        // every player on its current floor is dead. Pull the
        // affected party back to the hub and wipe their loot.
        for id in &instance_ids {
            let armed = self
                .instances
                .get_mut(*id)
                .map(|inst| inst.sim.take_hub_respawn_request())
                .unwrap_or(false);
            if armed {
                log::info!("respawning party to hub after wipe in {id:?}");
                let wiped = self
                    .instances
                    .get_mut(*id)
                    .map(|inst| inst.sim.wipe_dead_loot())
                    .unwrap_or_default();
                self.return_all_to_hub(*id);
                for cid in wiped {
                    self.broadcast_inventory_state(cid);
                    self.persist_inventory_state(cid);
                }
            }
        }
        let _ = self.hub.take_hub_respawn_request();

        // Rift exit votes: tick + resolve, per instance. Capture
        // the recipient set *before* resolving the outcome — a
        // passing Exit vote moves every voter back to the hub,
        // so by the time `take_exit_vote_update` hands us the
        // "vote cleared" state, those clients are no longer in
        // the instance. Send the post-resolve update to whoever
        // was *in the vote*, regardless of where they're
        // standing now.
        for id in &instance_ids {
            let voters = self.clients_in_instance(*id);
            let outcome = self
                .instances
                .get_mut(*id)
                .map(|inst| inst.sim.tick_exit_vote(dt));
            // Drain the wire-shape vote update *before* the
            // resolve actions below run. A passing Exit vote
            // calls `return_all_to_hub`, which dissolves the
            // (now-empty) instance \u2014 after that, the sim is
            // gone and `take_exit_vote_update` would be a
            // no-op, so the cleared `VoteState` (active=false,
            // cooldown=0) would never reach the clients and
            // their HUD vote panel would stick on screen back
            // in the hub.
            let update = self
                .instances
                .get_mut(*id)
                .and_then(|inst| inst.sim.take_exit_vote_update());
            match outcome {
                Some(crate::sim::vote::TickOutcome::Passed(
                    rift_net::messages::VoteKind::Exit,
                )) => {
                    log::info!("vote: party voted to leave instance {id:?}");
                    let wiped = self
                        .instances
                        .get_mut(*id)
                        .map(|inst| inst.sim.wipe_dead_loot())
                        .unwrap_or_default();
                    self.return_all_to_hub(*id);
                    for cid in wiped {
                        self.broadcast_inventory_state(cid);
                        self.persist_inventory_state(cid);
                    }
                }
                Some(crate::sim::vote::TickOutcome::Passed(
                    rift_net::messages::VoteKind::Descend,
                )) => {
                    log::info!("vote: party voted to descend in {id:?}");
                    let next_index = self
                        .instances
                        .get(*id)
                        .map(|inst| inst.sim.floor_index + 1)
                        .unwrap_or(1);
                    self.advance_instance_floor(*id, next_index);
                }
                _ => {}
            }
            if let Some(state) = update {
                let msg = ServerMsg::RiftExitVote(state);
                for cid in &voters {
                    self.send_to(*cid, Channel::Control, &msg);
                }
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

    /// Push a `MeterSnapshot` to every member of every active
    /// rift instance. Hub players don't get one (no fight to
    /// score). Driven by the 1 Hz `meter_accumulator` in the
    /// main tick loop.
    fn broadcast_meters(&mut self) {
        let instance_ids: Vec<_> = self.instances.iter().map(|(id, _)| *id).collect();
        for id in instance_ids {
            let members = self.clients_in_instance(id);
            if members.is_empty() {
                continue;
            }
            let Some(inst) = self.instances.get(id) else {
                continue;
            };
            let snap = inst.sim.build_meter_snapshot();
            for cid in members {
                self.send_to(cid, Channel::Control, &snap);
            }
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
