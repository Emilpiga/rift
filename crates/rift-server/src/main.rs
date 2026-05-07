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
    net::{SocketAddr, ToSocketAddrs},
    time::{Duration, Instant},
};

use anyhow::Result;
use rift_net::{
    decode, encode, open_server, renet, Channel, ClientId, ClientMsg, NetSettings, NetTick,
    ServerHandle, ServerMsg, MAX_CLIENTS, SNAPSHOT_HZ, TICK_HZ,
};
use rift_persistence::PersistenceHandle;

mod handlers;
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

    // ── Per-message handlers live in `handlers/` siblings. ──
    // Login / hydrate flow:    handlers/session.rs
    // Bag / equip / stash:     handlers/inventory.rs
    // Persistence reads/saves: handlers/persistence.rs

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
