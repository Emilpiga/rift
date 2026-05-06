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

mod sim;
use sim::Sim;

/// How often we kick off an opportunistic auto-save for every
/// connected character. Save is fire-and-forget on the server
/// loop; the persistence worker drains writes asynchronously and
/// never blocks gameplay.
const AUTO_SAVE_INTERVAL: Duration = Duration::from_secs(60);

/// Per-connected-client state the server tracks. Mostly placeholder
/// for Phase 1 — we just need a way to know "this client has
/// said Hello, here's the name they claimed".
struct ClientSession {
    client_id: ClientId,
    /// `None` until the client's `Hello` has been processed.
    character_name: Option<String>,
    /// Account display name supplied with `Hello`. Used to
    /// resolve / create the persistent `accounts` row that owns
    /// this session's character.
    account_name: Option<String>,
    /// Profile fields (set on Hello). `None` until welcomed.
    class_id: Option<String>,
    gender: Option<Gender>,
    /// Net id assigned to this player by the simulation. `None` until
    /// the player has been spawned (post-Hello).
    net_id: Option<NetId>,
    /// Authoritative persisted state for this character. `None` if
    /// persistence is disabled (no `--database-url`) or if the load
    /// failed and we fell back to in-memory only.
    record: Option<CharacterRecord>,
}

/// Top-level server state. One instance per running server binary.
struct Server {
    handle: ServerHandle,
    /// Authoritative simulation clock.
    tick: NetTick,
    /// Active sessions, keyed by renet client id.
    sessions: Vec<ClientSession>,
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
            sessions: Vec::new(),
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
        // applied input seq into `ack_seq`.
        let snap_period = Duration::from_secs_f32(1.0 / SNAPSHOT_HZ as f32);
        self.snapshot_accumulator += frame_dt;
        if self.snapshot_accumulator >= snap_period {
            self.snapshot_accumulator = Duration::ZERO;
            self.broadcast_snapshot();
        }

        // Periodic auto-save. Fire-and-forget per session — the
        // persistence worker drains writes off-thread so this loop
        // never blocks on a slow database.
        self.auto_save_accumulator += frame_dt;
        if self.auto_save_accumulator >= AUTO_SAVE_INTERVAL {
            self.auto_save_accumulator = Duration::ZERO;
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
                self.sessions.push(ClientSession {
                    client_id: ClientId(client_id.raw()),
                    character_name: None,
                    account_name: None,
                    class_id: None,
                    gender: None,
                    net_id: None,
                    record: None,
                });
            }
            renet::ServerEvent::ClientDisconnected { client_id, reason } => {
                log::info!("client disconnected: {client_id} ({reason:?})");
                let cid = ClientId(client_id.raw());
                // Capture the net id + the persisted record before
                // we tear down session state so we can broadcast
                // `PlayerLeft` *and* fire one final save.
                let (left_net_id, final_record) = self
                    .sessions
                    .iter()
                    .find(|s| s.client_id == cid)
                    .map(|s| (s.net_id, s.record.clone()))
                    .unwrap_or((None, None));
                if let (Some(handle), Some(record)) = (&self.persistence, final_record) {
                    if !handle.save(record) {
                        log::warn!("persistence: final-save dropped for {cid:?}");
                    }
                }
                self.sessions.retain(|s| s.client_id != cid);
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
            } => {
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
                // Resolve the persistent character row before we
                // welcome the player. We block here on purpose:
                // the load happens once per session and we need
                // level/xp before the world spawn. Falls back to
                // an in-memory record if persistence is disabled
                // or the database is unreachable, so dev iteration
                // without `docker compose up` still works.
                let record = self.load_character_record(
                    &account_name,
                    &character_name,
                    &class_id,
                    gender,
                );
                if let Some(s) = self.sessions.iter_mut().find(|s| s.client_id == from) {
                    s.account_name = Some(account_name.clone());
                    s.character_name = Some(character_name.clone());
                    s.class_id = Some(class_id.clone());
                    s.gender = Some(gender);
                    s.record = Some(record);
                }
                // Spawn the player into the simulation now that
                // we know who they are. The returned net id is
                // what the client uses to recognize itself in
                // subsequent snapshots.
                let net_id = self.sim.spawn_player(from);
                if let Some(s) = self.sessions.iter_mut().find(|s| s.client_id == from) {
                    s.net_id = Some(net_id);
                }
                // Hydrate the freshly-spawned player's inventory
                // from the database. Skipped when persistence is
                // disabled (dev mode) or the load fails — the
                // empty bag is still a valid game state.
                if let Some(handle) = &self.persistence {
                    if let Some(s) = self.sessions.iter().find(|s| s.client_id == from) {
                        if let Some(rec) = &s.record {
                            match handle.load_inventory_blocking(rec.id) {
                                Ok(rows) => {
                                    let items: Vec<rift_game::loot::Item> = rows
                                        .into_iter()
                                        .filter_map(|r| {
                                            rift_game::loot::Item::from_persisted(
                                                &r.base_id,
                                                r.rarity as u8,
                                                r.ilvl as u16,
                                                &r.affixes,
                                            )
                                        })
                                        .collect();
                                    log::info!(
                                        "persistence: loaded {} item(s) for {from:?}",
                                        items.len()
                                    );
                                    self.sim.set_player_inventory(from, items);
                                }
                                Err(e) => log::warn!(
                                    "persistence: load_inventory failed for {from:?}: {e}"
                                ),
                            }
                        }
                    }
                }
                let welcome = ServerMsg::Welcome {
                    your_client_id: from,
                    your_net_id: net_id,
                    floor_seed: self.sim.floor_seed,
                    floor_index: self.sim.floor_index,
                    tick: self.tick,
                };
                self.send_to(from, Channel::Control, &welcome);

                // Catch the newcomer up on every already-connected
                // player, then announce them to everyone.
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
            ClientMsg::Input(cmd) => {
                self.sim.ingest_input(from, cmd);
            }
            ClientMsg::CastAbility {
                ability_id,
                origin: _,
                aim_dir,
                placed_target,
            } => {
                self.sim.cast_ability(
                    from,
                    ability_id,
                    aim_dir,
                    placed_target,
                    self.tick,
                );
            }
            ClientMsg::EndChannel { ability_id } => {
                self.sim.end_channel(from, ability_id);
            }
            ClientMsg::PickUpLoot { net_id } => {
                // Validate range + remove the loot row inside the
                // sim. On success, broadcast `LootClaimed` so every
                // client (including the picker) tears down its
                // visual; the picker also gets the rolled `Item`
                // appended to their persisted inventory.
                if let Some(item) = self.sim.try_pickup_loot(from, net_id) {
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
                    // Persist the pickup. We look up the picker's
                    // `character_id` via the cached
                    // `CharacterRecord` in their session and queue
                    // a fire-and-forget INSERT — the
                    // server-authoritative bag has already been
                    // updated by `try_pickup_loot`, so a
                    // dropped/late write is recoverable on the
                    // next session by re-rolling.
                    if let Some(handle) = &self.persistence {
                        if let Some(rec) = self
                            .sessions
                            .iter()
                            .find(|s| s.client_id == from)
                            .and_then(|s| s.record.as_ref())
                        {
                            let (base_id, rarity, ilvl, affixes) = item.to_persisted();
                            let persisted = PersistedItem {
                                base_id,
                                rarity: rarity as i16,
                                ilvl: ilvl as i32,
                                affixes,
                            };
                            if !handle.append_inventory_item(rec.id, persisted) {
                                log::warn!(
                                    "persistence: append_inventory_item dropped for {from:?}"
                                );
                            }
                        }
                    }
                }
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
                self.send_to(
                    from,
                    Channel::Control,
                    &ServerMsg::Roster { entries },
                );
            }
        }
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
        for s in &self.sessions {
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
    let mut bind: SocketAddr = "0.0.0.0:34000".parse().unwrap();
    let mut public: Option<SocketAddr> = None;
    let mut database_url: Option<String> =
        Some("postgres://rift:rift@127.0.0.1:55432/rift".to_string());
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
                     [--database-url postgres://rift:rift@127.0.0.1:55432/rift] [--no-db]"
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
