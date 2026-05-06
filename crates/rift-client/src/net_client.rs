//! Client-side network plumbing.
//!
//! Phase 2 scope: open a client endpoint, run the renet update +
//! send/receive loop each frame, ship a `Hello` once connected, and
//! log incoming `ServerMsg`s. Nothing here mutates `GameState`'s
//! simulation yet — that's Phase 3's job.
//!
//! Activated via `--connect <addr>` on the command line. When
//! omitted, the game runs single-player exactly as before and this
//! module isn't touched.

use std::{collections::{HashMap, VecDeque}, net::SocketAddr, time::{Duration, Instant}};

use glam::{Quat, Vec3};
use rift_dungeon::{Floor, FloorConfig};
use rift_engine::ecs::components::{
    AnimationSet, LocalPlayer, NetControlled, Player, PlayerAction, Renderable, RemotePlayer,
    Transform, Velocity,
};
use rift_engine::animation::Animator;
use rift_engine::{Input, Renderer};
use rift_net::{
    decode, encode,
    messages::{button_bits, EntityKind, Gender, InputCmd, Snapshot},
    open_client, renet, Channel, ClientHandle, ClientId, ClientMsg, NetId, NetSettings, NetTick,
    ServerMsg, PROTOCOL_VERSION,
};
use rift_game::kinematic::Kinematic;
use winit::keyboard::KeyCode;

use rift_game::character::Gender as GameGender;
use crate::game::character_spawn::{spawn_character_entity, AnimLibraryCache, CharacterSpawn};
use crate::game::floor::spawn_remote_enemy_entity;
use crate::game::monster_assets::MonsterCache;

/// Snapshot of one remote-controlled entity, derived from the
/// latest server snapshot. Drives this frame's per-remote ECS
/// Transform/Velocity write.
#[derive(Clone, Debug)]
pub struct RemoteEntity {
    pub net_id: NetId,
    pub kind: EntityKind,
    pub position: Vec3,
    pub yaw: f32,
    pub velocity: Vec3,
    pub health_pct: f32,
    pub airborne: bool,
    /// Latest full-body action id from the snapshot
    /// (`rift_game::kinematic::action`). Drives the dodge-roll
    /// animation on remote avatars.
    pub action: u8,
}

/// Two-sample interpolation buffer for one remote entity. The
/// renderer always reads `prev → curr` blended by an alpha derived
/// from wall-clock time, with a one-snapshot-period render delay so
/// `curr` is always in the future relative to what we display. This
/// turns 20 Hz snapshot delivery into 60+ Hz visually smooth motion.
#[derive(Clone, Debug)]
struct InterpSample {
    position: Vec3,
    yaw: f32,
    /// Aim yaw (spine-twist / cursor direction). Interpolated with
    /// the rest so spell beams and aim cones don't jitter at the
    /// snapshot rate.
    aim_yaw: f32,
}

#[derive(Clone, Debug)]
struct RemoteInterp {
    prev: InterpSample,
    curr: InterpSample,
    /// Wall-clock time at which `curr` arrived. Display alpha ramps
    /// from 0 to 1 over the next snapshot period after this.
    curr_arrival: Instant,
}

/// A floor transition the server has told us about and we haven't
/// yet applied to the visual world. Drained once per frame by the
/// binary, which reruns the equivalent of the SP `perform_transition`
/// path with the server-provided seed/index. Until drained, snapshot
/// processing is paused (see `apply_snapshot`) so we don't try to
/// place avatars in an old-floor world.
#[derive(Clone, Debug)]
pub struct PendingFloor {
    pub seed: u64,
    pub index: u32,
    pub is_hub: bool,
    pub spawn_pos: Vec3,
    pub tick: NetTick,
}

/// Cosmetic identity for a remote player. Received via
/// `ServerMsg::PlayerJoined` and consumed when we spawn that
/// player's avatar entity into the local world.
#[derive(Clone, Debug)]
pub struct RemoteProfile {
    pub net_id: NetId,
    pub client_id: ClientId,
    pub character_name: String,
    pub class_id: String,
    pub gender: Gender,
}

/// Active networking session for a connected client. One per running
/// game when `--connect` is in use. Owned by the binary and ticked
/// before each frame's `update`.
pub struct NetClient {
    handle: ClientHandle,
    /// Whether we've already sent the initial `Hello`. We delay the
    /// send until renet reports the underlying netcode handshake is
    /// complete; otherwise the message is queued in renet's buffer
    /// and lost if the connection fails.
    hello_sent: bool,
    /// Highest server tick we've seen, sent back as `Ack` so the
    /// server can prune older snapshots from the per-client history.
    last_server_tick: NetTick,
    /// Cached identity for the `Hello` payload.
    profile: Option<ClientProfile>,
    /// Server-assigned net id for our own player. Populated by the
    /// `Welcome` message; until then we don't know which row in a
    /// snapshot is ours.
    our_net_id: Option<NetId>,
    /// Latest known state of every replicated entity, keyed by net
    /// id. Rebuilt from each snapshot.
    pub remote: HashMap<NetId, RemoteEntity>,
    /// Per-remote interpolation buffer. Keeps the previous + current
    /// snapshot sample so `sync_avatars` can blend between them on
    /// every render frame, smoothing 20 Hz snapshot delivery into
    /// fluid 60+ Hz visual motion. Indexed by net id so it survives
    /// snapshots that briefly omit an entity (avoids full reset).
    interp: HashMap<NetId, RemoteInterp>,
    /// Cosmetic identity for every remote player we've been told
    /// about. Populated by `PlayerJoined`; entries are removed on
    /// `PlayerLeft`. Consumed by `sync_avatars` when it spawns the
    /// avatar entity for a never-before-seen `net_id`.
    profiles: HashMap<NetId, RemoteProfile>,
    /// Outbound input sequence. Bumped each time we send an input.
    input_seq: u32,
    /// Wall-clock accumulator for input rate-limiting. We send
    /// inputs at ~60 Hz regardless of frame rate.
    input_accumulator: Duration,
    /// ECS entity per replicated remote player. Populated lazily by
    /// `sync_avatars` once both the profile (from `PlayerJoined`)
    /// and the kinematic state (from a snapshot) are available.
    /// Cleared on `PlayerLeft`.
    pub avatar_entities: HashMap<NetId, hecs::Entity>,
    /// ECS entity per replicated server-driven enemy. Spawned
    /// lazily by `sync_enemies` the first frame a fresh enemy
    /// `NetId` shows up in a snapshot, despawned when the server
    /// stops shipping it (death / floor change). Skinned mesh
    /// comes from the shared `MonsterCache` on `FloorManager`.
    enemy_entities: HashMap<NetId, hecs::Entity>,
    /// Renderer object index per replicated server-spawned
    /// projectile. Lightweight — no ECS entity, no animation,
    /// just a position-driven mesh.
    projectile_objects: HashMap<NetId, usize>,
    /// Last known world-space position of every replicated entity
    /// the server has ever told us about. Survives across snapshots
    /// (the snapshot drops a row the moment an enemy dies, but the
    /// reliable `Death` event may still need that position to drop
    /// a blood decal). Updated whenever we ingest a snapshot row.
    pub last_positions: HashMap<NetId, Vec3>,
    /// Reliable world events received this tick. Drained by the
    /// binary each frame so it can spawn floating combat text /
    /// hit reactions / death animations off them.
    pending_events: std::collections::VecDeque<rift_net::messages::WorldEvent>,
    /// Loot pickup confirmations received this tick (from
    /// `ServerMsg::LootClaimed`). Drained by the binary so it can
    /// tear down the loot-pillar visual and — if the picker is
    /// us — add the rolled item to the local inventory.
    pending_loot_claims: std::collections::VecDeque<(NetId, ClientId)>,
    /// Our authoritative `ClientId` once `Welcome` lands. Used to
    /// answer "was this loot claimed by us?" without re-walking
    /// the renet handle.
    our_client_id: Option<ClientId>,
    /// Locally-predicted state for our own player. Updated every
    /// frame by replaying unacknowledged inputs from the latest
    /// snapshot's authoritative position. Driven into the local
    /// `Player` ECS entity's `Transform` by `sync_local_player`,
    /// so the camera + animation pipeline see the predicted
    /// position with zero added latency.
    predicted: Kinematic,
    /// Whether `predicted` has been seeded from a server snapshot.
    /// Until the first snapshot lands we don't know our authoritative
    /// position and shouldn't override the SP player transform.
    predicted_ready: bool,
    /// History of inputs we've sent but the server hasn't yet
    /// acked. Each entry is `(seq, dt, cmd)`. On every snapshot we
    /// drop entries with `seq <= ack_seq` and replay the rest on
    /// top of the authoritative position.
    input_history: VecDeque<(u32, f32, InputCmd)>,
    /// Floor we're predicting against. Regenerated when a `Welcome`
    /// or future `LoadFloor` arrives, using the same seed the
    /// server uses so collision results match exactly.
    predict_floor: Option<Floor>,
    /// Smooth correction error: the offset we still need to bleed
    /// off from a recent server correction. Decays exponentially
    /// each frame so big snaps don't visibly teleport the camera.
    correction_error: Vec3,
    /// Latest aim direction (XZ, world-space) the binary has handed
    /// us via `set_aim`. Shipped on the next outbound `InputCmd` so
    /// the server can replicate it to remote observers and keep
    /// their spine-twist visual in sync with where this client is
    /// actually pointing the cursor. `[0, 0]` means "no aim known
    /// yet" — the server falls back to body yaw in that case.
    pending_aim: [f32; 2],
    /// Server-pushed floor transition awaiting the binary's next
    /// frame to actually rebuild visuals. Set by the `LoadFloor`
    /// handler; drained by `take_pending_floor`.
    pending_floor: Option<PendingFloor>,
    /// Current floor index, mirrored from `Welcome` and from each
    /// applied `LoadFloor`. Used to discard stale snapshots and
    /// PlayerJoined that might otherwise leak across transitions.
    floor_index: u32,
    /// The canonical rift seed shipped in `Welcome`. Used by the
    /// binary to seed `GameState::net_floor_seed` so SP rift
    /// regeneration mixes in the same per-floor offset the server
    /// does. Stays stable for the lifetime of the connection.
    rift_seed: u64,
    /// Account name we plan to lookup the roster for. Set by
    /// `request_roster` from the account-entry screen. Cleared
    /// once the request has been flushed onto the wire so we
    /// don't spam the server every frame.
    roster_request: Option<String>,
    /// Whether we've already shipped a `RequestRoster` for the
    /// current `roster_request`. Reset whenever a fresh account
    /// name is queued.
    roster_request_sent: bool,
    /// Latest roster reply from the server. `None` means "never
    /// asked" or "asked but no reply yet". Drained by the binary
    /// once it's been forwarded into the character-select UI.
    roster: Option<Vec<rift_net::messages::RosterEntry>>,
}

#[derive(Clone, Debug)]
pub struct ClientProfile {
    pub account_name: String,
    pub character_name: String,
    pub class_id: String,
    pub gender: rift_net::messages::Gender,
}

impl NetClient {
    pub fn connect(server: SocketAddr) -> anyhow::Result<Self> {
        // Pick a stable-but-process-unique client id. For Phase 1 we
        // use the OS PID; once auth lands this becomes the player's
        // account id.
        let client_id = ClientId(std::process::id() as u64);
        let handle = open_client(server, client_id, &NetSettings::default())?;
        Ok(Self {
            handle,
            hello_sent: false,
            last_server_tick: NetTick::default(),
            profile: None,
            our_net_id: None,
            remote: HashMap::new(),
            interp: HashMap::new(),
            profiles: HashMap::new(),
            input_seq: 0,
            input_accumulator: Duration::ZERO,
            avatar_entities: HashMap::new(),
            enemy_entities: HashMap::new(),
            projectile_objects: HashMap::new(),
            last_positions: HashMap::new(),
            pending_events: std::collections::VecDeque::new(),
            pending_loot_claims: std::collections::VecDeque::new(),
            our_client_id: None,
            predicted: Kinematic::default(),
            predicted_ready: false,
            input_history: VecDeque::new(),
            predict_floor: None,
            correction_error: Vec3::ZERO,
            pending_aim: [0.0, 0.0],
            pending_floor: None,
            floor_index: 0,
            rift_seed: 0,
            roster_request: None,
            roster_request_sent: false,
            roster: None,
        })
    }

    /// Pump network state. Call once per frame, before the renderer's
    /// `update`. `dt` is wall-clock since the last call. `input` is
    /// the engine's current input snapshot, which we sample for the
    /// outbound `InputCmd`. Pass `None` while UI states (character
    /// select, menus) shouldn't drive the avatar.
    pub fn step(&mut self, dt: Duration, input: Option<&Input>) {
        // Drive netcode + renet timers.
        if let Err(e) = self
            .handle
            .transport
            .update(dt, &mut self.handle.client)
        {
            log::warn!("net: transport update: {e:?}");
        }
        self.handle.client.update(dt);

        // Once the underlying handshake completes, send Hello exactly
        // once. After that the connection is fully usable for
        // game traffic.
        if !self.hello_sent
            && self.handle.client.is_connected()
            && self.profile.is_some()
        {
            self.send_hello();
            self.hello_sent = true;
        }

        // Forward any pending roster request the moment renet
        // reports the connection is live. We don't wait for
        // Hello — the roster lookup is a pre-Hello step so the
        // user can pick which character to log in as.
        if !self.roster_request_sent
            && self.handle.client.is_connected()
            && self.roster_request.is_some()
        {
            if let Some(account_name) = self.roster_request.clone() {
                self.send(
                    Channel::Control,
                    &ClientMsg::RequestRoster { account_name: account_name.clone() },
                );
                log::info!("net: requested roster for account {account_name:?}");
                self.roster_request_sent = true;
            }
        }

        // Drain inbound messages from every channel. The renet
        // client API is per-channel so we ask each one in turn.
        for ch in [Channel::Snapshot, Channel::Event, Channel::Control] {
            while let Some(bytes) = self.handle.client.receive_message(ch as u8) {
                match decode::<ServerMsg>(&bytes) {
                    Ok(msg) => self.handle_server_msg(msg),
                    Err(e) => log::warn!("net: decode {ch:?}: {e}"),
                }
            }
        }

        // Send input at ~60 Hz once welcomed. Coalesce — if multiple
        // frames pass between sends, only the latest input ships.
        // Each accepted send: build the command, predict it locally
        // against the latest authoritative state, push to history
        // for replay-on-reconcile, and ship.
        if self.our_net_id.is_some() {
            self.input_accumulator += dt;
            let input_period = Duration::from_secs_f32(1.0 / 60.0);
            if self.input_accumulator >= input_period {
                let send_dt = self.input_accumulator.as_secs_f32();
                self.input_accumulator = Duration::ZERO;
                if let Some(inp) = input {
                    self.send_input(inp, send_dt);
                }
            }
        }

        // Decay smooth-correction error toward zero so the visual
        // catches up with the predicted state over ~100 ms.
        let decay = (-dt.as_secs_f32() / 0.10).exp();
        self.correction_error *= decay;

        // Flush.
        if let Err(e) = self
            .handle
            .transport
            .send_packets(&mut self.handle.client)
        {
            log::warn!("net: transport send: {e:?}");
        }
    }

    /// Set (or replace, before Hello has been sent) the cosmetic
    /// profile this client advertises to the server. Called by the
    /// binary once character-select has finished. Calling this
    /// after Hello has already been sent is a no-op — the server
    /// uses the first Hello as authoritative for the session.
    pub fn set_profile(&mut self, profile: ClientProfile) {
        if self.hello_sent {
            log::warn!("net: set_profile called after Hello — ignored");
            return;
        }
        self.profile = Some(profile);
    }

    /// Queue a roster lookup for `account_name`. The actual
    /// `RequestRoster` is sent once renet reports the connection
    /// is live (see `step`). Calling this with a different name
    /// than is already pending re-arms the send so we always
    /// look up whatever the most recent account-entry confirmed.
    pub fn request_roster(&mut self, account_name: String) {
        let same_pending = self.roster_request.as_deref() == Some(account_name.as_str());
        if !same_pending {
            self.roster_request_sent = false;
        }
        self.roster_request = Some(account_name);
    }

    /// Drain the most recent roster reply, if any. Returns `None`
    /// while we're still waiting for the server. The caller takes
    /// ownership; subsequent calls return `None` until a fresh
    /// roster lands.
    pub fn take_roster(&mut self) -> Option<Vec<rift_net::messages::RosterEntry>> {
        self.roster.take()
    }

    fn send_hello(&mut self) {
        let Some(profile) = self.profile.clone() else {
            return;
        };
        let msg = ClientMsg::Hello {
            protocol_version: PROTOCOL_VERSION,
            account_name: profile.account_name.clone(),
            character_name: profile.character_name.clone(),
            class_id: profile.class_id.clone(),
            gender: profile.gender,
        };
        self.send(Channel::Control, &msg);
        log::info!(
            "net: sent Hello as {:?} on account {:?} ({:?})",
            profile.character_name,
            profile.account_name,
            profile.gender,
        );
    }

    fn handle_server_msg(&mut self, msg: ServerMsg) {
        match msg {
            ServerMsg::Welcome {
                your_client_id,
                your_net_id,
                floor_seed,
                floor_index,
                tick,
            } => {
                self.last_server_tick = tick;
                self.our_net_id = Some(your_net_id);
                self.our_client_id = Some(your_client_id);
                self.floor_index = floor_index;
                self.rift_seed = floor_seed;
                // Mirror the server's floor pick so prediction
                // collides against the same tile grid. Index 0 is
                // the safe hub, anything else is rift content with
                // the same seed-mixing rule used in `rift_server::sim`.
                self.predict_floor = Some(if floor_index == 0 {
                    Floor::hub()
                } else {
                    let mixed = floor_seed + floor_index as u64 * 7;
                    Floor::generate(FloorConfig::for_floor(floor_index), mixed)
                });
                log::info!(
                    "net: Welcome client_id={your_client_id:?} net_id={your_net_id:?} \
                     floor=({floor_seed}, {floor_index}) tick={tick:?}"
                );
            }
            ServerMsg::Reject { reason } => {
                log::error!("net: Reject from server: {reason}");
            }
            ServerMsg::Roster { entries } => {
                log::info!("net: Roster received ({} entries)", entries.len());
                self.roster = Some(entries);
            }
            ServerMsg::Snapshot(snap) => {
                self.apply_snapshot(snap);
            }
            ServerMsg::PlayerJoined {
                net_id,
                client_id,
                character_name,
                class_id,
                gender,
            } => {
                if Some(net_id) == self.our_net_id {
                    // PlayerJoined for ourselves — server is just
                    // catching us up, no avatar to spawn locally.
                    return;
                }
                log::info!(
                    "net: PlayerJoined net_id={net_id:?} client={client_id:?} name={character_name:?} class={class_id:?} gender={gender:?}"
                );
                self.profiles.insert(
                    net_id,
                    RemoteProfile {
                        net_id,
                        client_id,
                        character_name,
                        class_id,
                        gender,
                    },
                );
            }
            ServerMsg::PlayerLeft { net_id } => {
                log::info!("net: PlayerLeft net_id={net_id:?}");
                self.profiles.remove(&net_id);
                // Despawning the entity itself happens in
                // `sync_avatars` next frame so we do it under the
                // caller's `&mut World`.
            }
            ServerMsg::Event(ev) => {
                self.pending_events.push_back(ev);
            }
            ServerMsg::LoadFloor {
                seed,
                index,
                is_hub,
                spawn_pos,
                tick,
            } => {
                log::info!(
                    "net: LoadFloor seed={seed} index={index} is_hub={is_hub} \
                     spawn={spawn_pos:?} tick={tick:?}"
                );
                let spawn = Vec3::from_array(spawn_pos);
                self.floor_index = index;
                self.rift_seed = seed;
                // Rebuild the prediction floor so client-side
                // movement collides against the new layout.
                self.predict_floor = Some(if is_hub {
                    Floor::hub()
                } else {
                    let mixed = seed + index as u64 * 7;
                    Floor::generate(FloorConfig::for_floor(index), mixed)
                });
                // Reset prediction state. The server has snapped
                // every player to `spawn_pos`; matching that here
                // avoids the next snapshot triggering a giant
                // correction error.
                self.predicted.position = spawn;
                self.predicted.velocity = Vec3::ZERO;
                self.predicted.vy = 0.0;
                self.predicted.airborne = false;
                self.predicted.yaw = 0.0;
                self.predicted_ready = true;
                self.correction_error = Vec3::ZERO;
                self.input_history.clear();
                // Discard any in-flight snapshots that predate the
                // transition tick — they describe the *old* floor.
                self.last_server_tick = tick;
                // Drop interp buffers so freshly-spawned remote
                // avatars on the new floor don't interpolate from
                // their stale old-floor positions.
                self.interp.clear();
                // The binary will wipe the world via
                // `apply_net_transition` next frame, invalidating
                // every Entity handle we held. Drop the maps so
                // `sync_avatars` / `sync_enemies` re-spawn cleanly
                // off the next snapshot rather than trying to
                // write through stale entity ids.
                self.avatar_entities.clear();
                self.enemy_entities.clear();
                // Projectile renderer slots from the previous floor
                // are about to be invalidated by the world wipe;
                // drop them so the next floor's projectile rows
                // get fresh slot allocations.
                self.projectile_objects.clear();
                // Hand the transition off to the binary. It runs
                // the equivalent SP regenerate path next frame.
                self.pending_floor = Some(PendingFloor {
                    seed,
                    index,
                    is_hub,
                    spawn_pos: spawn,
                    tick,
                });
            }
            ServerMsg::LootClaimed { loot, claimed_by } => {
                log::debug!("net: LootClaimed loot={loot:?} by={claimed_by:?}");
                self.pending_loot_claims.push_back((loot, claimed_by));
            }
            ServerMsg::Kick { reason } => {
                log::warn!("net: kicked: {reason}");
            }
        }
    }

    /// Rebuild `remote` from the latest server snapshot, and
    /// reconcile our local prediction with the authoritative
    /// position the server reports for our own player.
    fn apply_snapshot(&mut self, snap: Snapshot) {
        // Drop stale snapshots — UDP can deliver out of order.
        if snap.tick.diff(self.last_server_tick) <= 0 {
            return;
        }
        self.last_server_tick = snap.tick;

        // Find our own row before draining `remote` so we have
        // both the authoritative state and the new ack seq in hand.
        let our_id = self.our_net_id;
        let our_auth = our_id.and_then(|nid| {
            snap.entities
                .iter()
                .find(|e| e.net_id == nid)
                .map(|e| (Vec3::from_array(e.position), Vec3::from_array(e.velocity), e.yaw))
        });

        self.remote.clear();
        let now = Instant::now();
        let our_id_for_interp = self.our_net_id;
        for e in snap.entities {
            let net_id = e.net_id;
            let position = Vec3::from_array(e.position);
            let yaw = e.yaw;
            let action = match &e.kind {
                EntityKind::Player { action, .. } => *action,
                _ => 0,
            };
            let aim_yaw = match &e.kind {
                EntityKind::Player { aim_yaw, .. } => *aim_yaw,
                _ => yaw,
            };
            self.remote.insert(
                net_id,
                RemoteEntity {
                    net_id,
                    kind: e.kind,
                    position,
                    yaw,
                    velocity: Vec3::from_array(e.velocity),
                    health_pct: e.health_pct,
                    airborne: e.flags & rift_net::messages::entity_flags::AIRBORNE != 0,
                    action,
                },
            );
            self.last_positions.insert(net_id, position);
            // Skip our own row — we own the local player's transform
            // through prediction, not through the interp buffer.
            if Some(net_id) == our_id_for_interp {
                continue;
            }
            let new_sample = InterpSample { position, yaw, aim_yaw };
            self.interp
                .entry(net_id)
                .and_modify(|b| {
                    // Shift current → previous, slot the new one in
                    // as current. `curr_arrival = now` resets the
                    // ramp so the next frame starts at alpha=0.
                    b.prev = b.curr.clone();
                    b.curr = new_sample.clone();
                    b.curr_arrival = now;
                })
                .or_insert_with(|| RemoteInterp {
                    prev: new_sample.clone(),
                    curr: new_sample,
                    curr_arrival: now,
                });
        }

        // Reconcile: drop acked inputs from history, snap to the
        // server's authoritative state, then replay everything
        // still in flight to recover the predicted "now".
        if let Some((auth_pos, auth_vel, auth_yaw)) = our_auth {
            self.input_history
                .retain(|(seq, _, _)| seq.wrapping_sub(snap.ack_seq) as i32 > 0);

            let prev = self.predicted.position;
            self.predicted.position = auth_pos;
            self.predicted.velocity = auth_vel;
            self.predicted.yaw = auth_yaw;

            if let Some(floor) = self.predict_floor.as_ref() {
                let history: Vec<(u32, f32, InputCmd)> =
                    self.input_history.iter().cloned().collect();
                for (_, dt, cmd) in history {
                    rift_game::kinematic::apply_input(&mut self.predicted, cmd.move_dir, cmd.aim_dir, cmd.buttons);
                    rift_game::kinematic::integrate(&mut self.predicted, floor, dt);
                }
            }

            // Roll the visible position correction into
            // `correction_error` so we bleed it off smoothly
            // instead of teleporting on each snapshot.
            if self.predicted_ready {
                self.correction_error += prev - self.predicted.position;
                // Cap the smoothing budget so a real teleport
                // (death respawn, floor transition) snaps cleanly.
                if self.correction_error.length() > 1.5 {
                    self.correction_error = Vec3::ZERO;
                }
            } else {
                self.predicted_ready = true;
            }
        }

        let enemy_count = self
            .remote
            .values()
            .filter(|re| matches!(re.kind, EntityKind::Enemy { .. }))
            .count();
        log::debug!(
            "net: snapshot tick={:?} entities={} (enemies={}) ack_seq={} pending_inputs={}",
            snap.tick,
            self.remote.len(),
            enemy_count,
            snap.ack_seq,
            self.input_history.len(),
        );
    }

    /// Build and ship a single `InputCmd` from the engine's current
    /// input state. Also predicts the command locally against
    /// `predicted` and stashes it in `input_history` so the next
    /// snapshot can replay-on-top during reconciliation.
    fn send_input(&mut self, input: &Input, dt: f32) {
        self.input_seq = self.input_seq.wrapping_add(1);

        // WASD → camera-relative move axis, matching the SP
        // `player_input_system`. We rotate the raw axis by the
        // active camera yaw before sending so the wire payload is
        // already in world space — the server doesn't know about
        // cameras and shouldn't have to.
        let mut buttons: u16 = 0;
        let mut dx = 0.0f32;
        let mut dz = 0.0f32;
        if input.is_key_held(KeyCode::KeyW) {
            dz -= 1.0;
            buttons |= button_bits::MOVE_FORWARD;
        }
        if input.is_key_held(KeyCode::KeyS) {
            dz += 1.0;
            buttons |= button_bits::MOVE_BACK;
        }
        if input.is_key_held(KeyCode::KeyA) {
            dx -= 1.0;
            buttons |= button_bits::MOVE_LEFT;
        }
        if input.is_key_held(KeyCode::KeyD) {
            dx += 1.0;
            buttons |= button_bits::MOVE_RIGHT;
        }
        // Edge-detected jump request: send the JUMP bit on the
        // frame Space transitions from up→down. The server treats
        // this as a request and only acts on it when feet are
        // planted (matching the SP `player_action_pre_system`
        // check), so a held key won't auto-bunny-hop.
        if input.key_just_pressed(KeyCode::Space) {
            buttons |= button_bits::JUMP;
        }

        // Rotate the raw input axis by camera yaw so "W" means
        // "forward from where the camera is looking", same as SP.
        let cam_yaw = input.camera_yaw();
        let world = Quat::from_rotation_y(cam_yaw) * Vec3::new(dx, 0.0, dz);
        let mut x = world.x;
        let mut z = world.z;
        let len2 = x * x + z * z;
        if len2 > 1.0 {
            let inv = 1.0 / len2.sqrt();
            x *= inv;
            z *= inv;
        }

        let cmd = InputCmd {
            seq: self.input_seq,
            tick_estimate: self.last_server_tick,
            move_dir: [x, z],
            aim_dir: self.pending_aim,
            buttons,
            cast_target: None,
        };

        // Predict locally so the local avatar moves immediately,
        // and stash for replay-on-reconcile.
        if self.predicted_ready {
            if let Some(floor) = self.predict_floor.as_ref() {
                rift_game::kinematic::apply_input(&mut self.predicted, cmd.move_dir, cmd.aim_dir, cmd.buttons);
                rift_game::kinematic::integrate(&mut self.predicted, floor, dt);
            }
        }
        // Bound history at 2 seconds of input so it can't grow
        // unbounded if the server stops acking for any reason.
        if self.input_history.len() >= 128 {
            self.input_history.pop_front();
        }
        self.input_history.push_back((self.input_seq, dt, cmd));

        self.send(Channel::Snapshot, &ClientMsg::Input(cmd));
    }

    fn send(&mut self, ch: Channel, msg: &ClientMsg) {
        if !self.handle.client.is_connected() {
            return;
        }
        let bytes = match encode(msg) {
            Ok(b) => b,
            Err(e) => {
                log::error!("net: encode: {e}");
                return;
            }
        };
        self.handle.client.send_message(ch as u8, bytes);
    }

    pub fn is_connected(&self) -> bool {
        self.handle.client.is_connected()
    }

    /// Update the aim direction shipped on the next outbound input.
    /// Call once per frame from the binary after `GameState::update`
    /// has computed the cursor → world aim, so the value travels to
    /// the server promptly. Pass `Vec3::ZERO` to clear (server then
    /// falls back to body yaw for the spine-twist on remotes).
    pub fn set_aim(&mut self, aim: Vec3) {
        // Drop the y component — aim is a horizontal direction on
        // the wire. Renormalise so a zero-length input cleanly
        // reads as "no aim" on the server side.
        let len = (aim.x * aim.x + aim.z * aim.z).sqrt();
        if len > 1.0e-4 {
            self.pending_aim = [aim.x / len, aim.z / len];
        } else {
            self.pending_aim = [0.0, 0.0];
        }
    }

    /// Take the queued floor transition (if any) so the binary can
    /// rebuild visuals to match. Returns `None` once consumed; the
    /// next `LoadFloor` from the server queues another.
    pub fn take_pending_floor(&mut self) -> Option<PendingFloor> {
        self.pending_floor.take()
    }

    /// Currently-active floor index (0 = hub) according to the most
    /// recent `Welcome` / `LoadFloor` from the server.
    pub fn floor_index(&self) -> u32 {
        self.floor_index
    }

    /// The canonical rift seed last announced by the server. Used by
    /// the binary to seed `GameState::net_floor_seed` so dungeon
    /// generation matches everyone else's. Stable across the
    /// connection unless the server explicitly broadcasts a fresh
    /// `LoadFloor` with a new seed.
    pub fn rift_seed(&self) -> u64 {
        self.rift_seed
    }

    /// Ask the server to advance to the next floor (or, if currently
    /// in the hub, enter the rift). Server is the authority on
    /// whether the request is honoured; if accepted, every client
    /// receives a reliable `LoadFloor`.
    pub fn request_enter_rift(&mut self) {
        log::info!("net: -> RequestEnterRift");
        self.send(Channel::Control, &ClientMsg::RequestEnterRift);
    }

    /// Ask the server to teleport the session back to the hub.
    pub fn request_return_to_hub(&mut self) {
        log::info!("net: -> RequestReturnToHub");
        self.send(Channel::Control, &ClientMsg::RequestReturnToHub);
    }

    /// Ask the server to fire an ability. Server is the authority on
    /// cooldown / range / damage; on success it spawns projectiles
    /// (replicated via snapshots) and emits `WorldEvent`s reliably.
    /// `aim_dir` should be the XZ-plane unit direction.
    pub fn request_cast(
        &mut self,
        ability_id: u8,
        origin: Vec3,
        aim_dir: Vec3,
        placed_target: Option<Vec3>,
    ) {
        let aim = Vec3::new(aim_dir.x, 0.0, aim_dir.z).normalize_or_zero();
        // Locally-predicted side-effects of the cast on the
        // shared `Kinematic` state. Today only Evasive Roll has a
        // movement effect that the prediction loop has to mirror;
        // every other ability either spawns server-side projectiles
        // (no kinematic side-effect on the caster) or runs a
        // channel that the server drives via separate messages.
        if ability_id == rift_game::abilities::id::EVASIVE_ROLL {
            rift_game::kinematic::start_roll(&mut self.predicted, aim);
        }
        let msg = ClientMsg::CastAbility {
            ability_id,
            origin: origin.to_array(),
            aim_dir: [aim.x, aim.z],
            placed_target: placed_target.map(|v| v.to_array()),
        };
        self.send(Channel::Event, &msg);
    }

    /// Tell the server to end the current channel for `ability_id`.
    /// Sent on button release / movement-cancel during a
    /// hold-to-channel ability. Server silently ignores if the
    /// caller isn't actually channeling that ability so duplicate
    /// release packets are safe.
    pub fn request_end_channel(&mut self, ability_id: u8) {
        let msg = ClientMsg::EndChannel { ability_id };
        self.send(Channel::Event, &msg);
    }

    /// Ask the server to claim a ground-loot drop on our behalf.
    /// Server validates range and broadcasts [`ServerMsg::LootClaimed`]
    /// on success; clients tear down their visuals on receipt.
    pub fn request_pickup_loot(&mut self, loot: NetId) {
        log::debug!("net: -> PickUpLoot {loot:?}");
        self.send(Channel::Control, &ClientMsg::PickUpLoot { net_id: loot });
    }

    /// Drain reliable world events received since the last call.
    /// The binary consumes these to spawn floating damage numbers,
    /// trigger hit-react animations, and play death clips.
    pub fn drain_events(
        &mut self,
    ) -> std::collections::VecDeque<rift_net::messages::WorldEvent> {
        std::mem::take(&mut self.pending_events)
    }

    /// Drain `(loot_net_id, claimed_by_client_id)` pairs received
    /// via [`ServerMsg::LootClaimed`] since the last call. The
    /// binary forwards each one into `GameState::resolve_loot_claim`
    /// to tear down the visual; whether we add the item to our
    /// inventory is decided by comparing `claimed_by` to
    /// [`our_client_id`].
    pub fn drain_loot_claims(
        &mut self,
    ) -> std::collections::VecDeque<(NetId, ClientId)> {
        std::mem::take(&mut self.pending_loot_claims)
    }

    /// Our authoritative `ClientId` once `Welcome` has arrived.
    pub fn our_client_id(&self) -> Option<ClientId> {
        self.our_client_id
    }

    /// Our authoritative `NetId` once `Welcome` has arrived. Used by
    /// the binary to skip own-cast events that we already played
    /// locally for input responsiveness.
    pub fn our_net_id(&self) -> Option<NetId> {
        self.our_net_id
    }

    /// Diagnostic disconnect reason, if renet has decided we're done.
    pub fn disconnect_reason(&self) -> Option<renet::DisconnectReason> {
        self.handle.client.disconnect_reason()
    }

    /// Reconcile renderer state with the latest snapshot. Spawns a
    /// placeholder avatar mesh for every remote `NetId` we haven't
    /// seen before, updates each avatar's transform, and collapses
    /// the model matrix of any avatar whose entity is no longer in
    /// the snapshot.
    ///
    /// Local player (`our_net_id`) is intentionally skipped — it's
    /// still owned by the SP `GameState` rendering path. Phase 4.2
    /// will reconcile it with the server-authoritative position.
    /// Reconcile remote-player ECS state with the latest snapshot.
    ///
    /// For every remote `NetId` in `remote` that has a known
    /// `RemoteProfile` and isn't yet spawned, we instantiate a full
    /// skinned character entity (sharing `anim_cache` with the local
    /// player path) and tag it as `RemotePlayer + NetControlled`.
    /// Then for every spawned remote, we drive Transform / Velocity
    /// / aim from the snapshot row so `locomotion_anim_system`,
    /// `skinning_system`, and `render_sync_system` (all of which
    /// run inside `GameState::update`) treat the remote like any
    /// other animated character.
    ///
    /// Local player (`our_net_id`) is intentionally skipped — its
    /// avatar lives in the SP-spawned entity that `sync_local_player`
    /// already drives.
    pub fn sync_avatars(
        &mut self,
        world: &mut hecs::World,
        renderer: &mut Renderer,
        anim_cache: &mut AnimLibraryCache,
    ) {
        let Some(our_net_id) = self.our_net_id else {
            // Wait for Welcome before spawning any avatars: we need
            // to know which net id is ours so we don't render an
            // avatar on top of the local player.
            return;
        };

        // ─── Despawn vanished remotes ────────────────────────────
        // Three cases drop a remote: explicit `PlayerLeft` (no
        // longer in `profiles`), a snapshot that omits the net id
        // (no longer in `remote`), or a world reset (e.g. a floor
        // regeneration `*world = World::new()` from
        // `floor_mgr.generate`) that invalidated our cached entity
        // id. The last case is tricky because hecs reuses entity
        // ids across `World::new()` resets, so `world.contains` may
        // return true for a completely unrelated new entity. We
        // verify by checking the entity still carries our
        // `RemotePlayer { net_id }` tag with the expected id.
        let stale: Vec<NetId> = self
            .avatar_entities
            .iter()
            .filter(|(nid, entity)| {
                if !self.remote.contains_key(nid) || !self.profiles.contains_key(nid) {
                    return true;
                }
                match world.get::<&RemotePlayer>(**entity) {
                    Ok(rp) => rp.net_id != nid.0,
                    Err(_) => true,
                }
            })
            .map(|(nid, _)| *nid)
            .collect();
        for net_id in stale {
            if let Some(entity) = self.avatar_entities.remove(&net_id) {
                // Hide the renderer slot before despawning so we
                // don't leak a frame of the old pose. Skip if the
                // entity is already dead or got reused (world reset).
                if world.get::<&RemotePlayer>(entity).map(|rp| rp.net_id == net_id.0).unwrap_or(false) {
                    if let Ok(r) = world.get::<&Renderable>(entity) {
                        let idx = r.object_index;
                        if idx < renderer.objects.len() {
                            renderer.objects[idx].model_matrix = glam::Mat4::ZERO;
                        }
                    }
                    let _ = world.despawn(entity);
                }
                log::info!("net: despawned remote avatar {net_id:?}");
            }
        }

        // ─── Spawn newcomers ─────────────────────────────────────
        // Collect first to avoid holding an immutable borrow on
        // `self.remote` during `spawn_character_entity`'s mutable
        // world+renderer borrows.
        let to_spawn: Vec<(NetId, RemoteProfile, Vec3)> = self
            .remote
            .iter()
            .filter(|(nid, _)| **nid != our_net_id)
            .filter(|(nid, _)| !self.avatar_entities.contains_key(nid))
            .filter_map(|(nid, re)| {
                self.profiles
                    .get(nid)
                    .cloned()
                    .map(|p| (*nid, p, re.position))
            })
            .collect();

        for (net_id, profile, position) in to_spawn {
            let cfg = CharacterSpawn {
                position,
                gender: gender_to_game(profile.gender),
                // Speed/HP placeholders: server is authoritative for
                // both, but the components need *some* value for the
                // SP systems we share with locals.
                move_speed: rift_game::kinematic::PLAYER_SPEED,
                max_hp: 100.0,
            };
            let entity = match spawn_character_entity(world, renderer, anim_cache, cfg) {
                Ok(e) => e,
                Err(e) => {
                    log::warn!("net: failed to spawn remote avatar {net_id:?}: {e:?}");
                    continue;
                }
            };
            // Mark as remote + net-controlled so SP systems\n            // (player_input, movement, collision) leave the\n            // entity alone \u2014 we own its kinematics.
            world
                .insert(
                    entity,
                    (
                        RemotePlayer { net_id: net_id.0 },
                        NetControlled,
                    ),
                )
                .ok();
            self.avatar_entities.insert(net_id, entity);
            log::info!(
                "net: spawned remote avatar {net_id:?} as {:?} ({:?})",
                profile.character_name,
                profile.gender,
            );
        }

        // ─── Drive remote kinematics from snapshot ───────────────
        // Position + yaw come from the per-remote interp buffer:
        // we render `prev → curr` blended by an alpha derived from
        // the time since `curr` arrived, with one snapshot period
        // of intentional lag so we always have a sample to
        // interpolate towards. Velocity is the latest known value
        // (not interpolated) so the animation tier picker can react
        // immediately when the remote starts/stops moving.
        let snapshot_period =
            Duration::from_secs_f32(1.0 / rift_net::SNAPSHOT_HZ as f32);
        let now = Instant::now();
        for (&net_id, &entity) in &self.avatar_entities {
            let Some(re) = self.remote.get(&net_id) else {
                continue;
            };
            let (display_pos, display_yaw, display_aim_yaw) = match self.interp.get(&net_id) {
                Some(b) => {
                    let elapsed = now
                        .saturating_duration_since(b.curr_arrival)
                        .as_secs_f32();
                    let period = snapshot_period.as_secs_f32().max(1e-4);
                    let alpha = (elapsed / period).clamp(0.0, 1.0);
                    let pos = b.prev.position.lerp(b.curr.position, alpha);
                    // Shortest-path yaw lerp via Quat slerp so a
                    // wraparound from +π → -π doesn't spin the avatar.
                    let q_prev = Quat::from_rotation_y(b.prev.yaw);
                    let q_curr = Quat::from_rotation_y(b.curr.yaw);
                    let q = q_prev.slerp(q_curr, alpha);
                    let (yaw, _, _) = q.to_euler(glam::EulerRot::YXZ);
                    let qa_prev = Quat::from_rotation_y(b.prev.aim_yaw);
                    let qa_curr = Quat::from_rotation_y(b.curr.aim_yaw);
                    let qa = qa_prev.slerp(qa_curr, alpha);
                    let (aim_yaw, _, _) = qa.to_euler(glam::EulerRot::YXZ);
                    (pos, yaw, aim_yaw)
                }
                None => {
                    let aim_yaw = match re.kind {
                        EntityKind::Player { aim_yaw, .. } => aim_yaw,
                        _ => re.yaw,
                    };
                    (re.position, re.yaw, aim_yaw)
                }
            };
            if let Ok(mut t) = world.get::<&mut Transform>(entity) {
                t.position = display_pos;
                t.rotation = Quat::from_rotation_y(display_yaw);
            }
            // Velocity drives `locomotion_anim_system`'s
            // Idle/Walk/Jog/Sprint pick. Server already sends
            // world-space horizontal velocity. Take the latest
            // value (not interpolated) so the animation tier
            // changes the same frame movement starts/stops.
            if let Ok(mut v) = world.get::<&mut Velocity>(entity) {
                v.linear = re.velocity;
            }
            // Aim direction (for spine twist + remote channel
            // beams). Slerped above so it tracks at render rate
            // instead of jumping at the snapshot rate.
            if matches!(re.kind, EntityKind::Player { .. }) {
                if let Ok(mut p) = world.get::<&mut Player>(entity) {
                    p.aim_dir = Vec3::new(
                        display_aim_yaw.sin(),
                        0.0,
                        display_aim_yaw.cos(),
                    );
                }
            }
            // Jump: when the snapshot says the remote is airborne,
            // tag its `Player.action = JumpAir` and cross-fade to
            // the air clip. `locomotion_anim_system` early-returns
            // when `action != None`, so the air pose stays put for
            // as long as the snapshot reports airborne. On
            // touchdown we snap back to None and locomotion takes
            // over the next frame.
            let was_airborne = world
                .get::<&Player>(entity)
                .map(|p| matches!(p.action, PlayerAction::JumpAir))
                .unwrap_or(false);
            if re.airborne != was_airborne {
                if re.airborne {
                    if let Ok(mut p) = world.get::<&mut Player>(entity) {
                        p.action = PlayerAction::JumpAir;
                        p.action_timer = 0.0;
                    }
                    let clip = world
                        .get::<&AnimationSet>(entity)
                        .ok()
                        .and_then(|s| s.find_any(&["Jump", "Jump_Loop", "Jump_Air"]));
                    if let Some(clip) = clip {
                        if let Ok(mut anim) = world.get::<&mut Animator>(entity) {
                            anim.cross_fade(clip, true, 0.10);
                            anim.speed = 1.0;
                        }
                    }
                } else if let Ok(mut p) = world.get::<&mut Player>(entity) {
                    p.action = PlayerAction::None;
                    p.action_timer = 0.0;
                }
            }

            // Dodge-roll: drive the roll clip on the remote avatar
            // while the snapshot reports an active roll action.
            // Mirrors what `set_player_action` does on the local
            // path — sets `Player.action = Roll` so the SP
            // locomotion picker steps aside, then cross-fades the
            // roll clip. Cleared as soon as the snapshot flips back
            // to `NONE` (server's roll timer expired).
            let snap_rolling =
                re.action == rift_game::kinematic::action::ROLL;
            let was_rolling = world
                .get::<&Player>(entity)
                .map(|p| matches!(p.action, PlayerAction::Roll))
                .unwrap_or(false);
            if snap_rolling && !was_rolling {
                if let Ok(mut p) = world.get::<&mut Player>(entity) {
                    p.action = PlayerAction::Roll;
                    p.action_timer = rift_game::kinematic::ROLL_DURATION;
                    p.aim_dir = Vec3::new(re.yaw.sin(), 0.0, re.yaw.cos());
                }
                let clip = world
                    .get::<&AnimationSet>(entity)
                    .ok()
                    .and_then(|s| s.find_any(&[
                        "Roll", "Roll_Forward", "Dodge_Roll", "Dodge",
                    ]));
                if let Some(clip) = clip {
                    if let Ok(mut anim) = world.get::<&mut Animator>(entity) {
                        anim.cross_fade(clip, false, 0.08);
                        anim.speed = 1.0;
                    }
                }
            } else if !snap_rolling && was_rolling {
                if let Ok(mut p) = world.get::<&mut Player>(entity) {
                    p.action = PlayerAction::None;
                    p.action_timer = 0.0;
                }
            }
        }
        // Drop interp buffers for entities that have despawned so
        // the map doesn't grow unbounded across long sessions.
        self.interp
            .retain(|nid, _| self.avatar_entities.contains_key(nid));
    }

    /// Reconcile server-replicated enemy entities with the latest
    /// snapshot. Spawns a skinned monster ECS entity for any new
    /// `EntityKind::Enemy` row, drives its `Transform` / `Velocity`
    /// / `Health` from the snapshot, and despawns any previously
    /// known enemy that's no longer in the snapshot (server-side
    /// death or floor change).
    ///
    /// The enemy entity intentionally does NOT carry the SP
    /// `Enemy` / `AiAgent` / `Collider` components — server is
    /// authoritative for movement, hits, and death. We add
    /// `NetControlled` so any future SP gate that filters by it
    /// short-circuits cleanly.
    pub fn sync_enemies(
        &mut self,
        world: &mut hecs::World,
        renderer: &mut Renderer,
        monsters: &mut MonsterCache,
    ) {
        if self.our_net_id.is_none() {
            return;
        }

        // ── Despawn vanished enemies ────────────────────────────
        let stale: Vec<NetId> = self
            .enemy_entities
            .iter()
            .filter(|(nid, _)| !self.remote.contains_key(nid))
            .map(|(nid, _)| *nid)
            .collect();
        for net_id in stale {
            if let Some(entity) = self.enemy_entities.remove(&net_id) {
                if let Ok(r) = world.get::<&Renderable>(entity) {
                    let idx = r.object_index;
                    if idx < renderer.objects.len() {
                        renderer.objects[idx].model_matrix = glam::Mat4::ZERO;
                    }
                }
                let _ = world.despawn(entity);
            }
        }

        // ── Spawn newcomers ─────────────────────────────────────
        // Cap at a few spawns per frame: each spawn does a
        // synchronous GPU mesh upload + texture bind, and a fresh
        // floor can have hundreds of enemies. Doing them all in a
        // single frame stalls the renderer for seconds. Remaining
        // enemies stream in over the next handful of frames as
        // their NetIds keep showing up in snapshots.
        const MAX_SPAWNS_PER_FRAME: usize = 8;
        let to_spawn: Vec<(NetId, u8, Vec3, f32)> = self
            .remote
            .iter()
            .filter(|(nid, _)| !self.enemy_entities.contains_key(nid))
            .filter_map(|(nid, re)| match re.kind {
                EntityKind::Enemy { role, .. } => Some((*nid, role, re.position, re.health_pct)),
                _ => None,
            })
            .take(MAX_SPAWNS_PER_FRAME)
            .collect();
        if !to_spawn.is_empty() {
            log::info!(
                "net: sync_enemies spawning {} of {} enemy rows in `remote`",
                to_spawn.len(),
                self.remote.values().filter(|re| matches!(re.kind, EntityKind::Enemy { .. })).count(),
            );
        }
        for (net_id, role_byte, position, hp_pct) in to_spawn {
            let role = match role_byte_to_monster_role(role_byte) {
                Some(r) => r,
                None => continue,
            };
            // We don't know hp_max from the wire (only health_pct).
            // Pick a sane default so HUD bar math works; the actual
            // current value is overwritten from health_pct each
            // frame anyway.
            let hp_max = 100.0_f32;
            let hp = hp_max * hp_pct;
            match spawn_remote_enemy_entity(
                world, renderer, monsters, role, position, hp_max,
            ) {
                Ok(entity) => {
                    if let Ok(mut h) = world.get::<&mut rift_engine::ecs::components::Health>(entity) {
                        h.current = hp;
                    }
                    self.enemy_entities.insert(net_id, entity);
                    log::info!(
                        "net: spawned remote enemy {net_id:?} role={role:?} at {position:?}"
                    );
                }
                Err(e) => {
                    log::warn!(
                        "net: failed to spawn remote enemy {net_id:?} role={role:?}: {e:?}"
                    );
                }
            }
        }

        // ── Drive remote-enemy kinematics from snapshot ─────────
        let snapshot_period =
            Duration::from_secs_f32(1.0 / rift_net::SNAPSHOT_HZ as f32);
        let now = Instant::now();
        for (&net_id, &entity) in &self.enemy_entities {
            let Some(re) = self.remote.get(&net_id) else {
                continue;
            };
            let (display_pos, display_yaw) = match self.interp.get(&net_id) {
                Some(b) => {
                    let elapsed = now
                        .saturating_duration_since(b.curr_arrival)
                        .as_secs_f32();
                    let period = snapshot_period.as_secs_f32().max(1e-4);
                    let alpha = (elapsed / period).clamp(0.0, 1.0);
                    let pos = b.prev.position.lerp(b.curr.position, alpha);
                    let q_prev = Quat::from_rotation_y(b.prev.yaw);
                    let q_curr = Quat::from_rotation_y(b.curr.yaw);
                    let q = q_prev.slerp(q_curr, alpha);
                    let (yaw, _, _) = q.to_euler(glam::EulerRot::YXZ);
                    (pos, yaw)
                }
                None => (re.position, re.yaw),
            };
            if let Ok(mut t) = world.get::<&mut Transform>(entity) {
                t.position = display_pos;
                t.rotation = Quat::from_rotation_y(display_yaw);
            }
            if let Ok(mut v) = world.get::<&mut Velocity>(entity) {
                v.linear = re.velocity;
            }
            if let Ok(mut h) = world.get::<&mut rift_engine::ecs::components::Health>(entity) {
                // Treat health_pct as the canonical source of truth
                // for current/max ratio. Keep `max` stable from
                // spawn so HUD bars don't jitter when the server's
                // hp_max disagrees with our placeholder.
                h.current = h.max * re.health_pct;
            }
            // Surface the server's anim byte by writing into
            // EnemyAnim.attacking — the SP animation tier picker
            // for skinned enemies reads it to swap to the attack
            // clip. WALK / IDLE are picked by the locomotion
            // animation system off `Velocity` (already set above).
            if let EntityKind::Enemy { anim, debuffs, .. } = re.kind {
                if let Ok(mut ea) = world.get::<&mut rift_engine::ecs::components::EnemyAnim>(entity) {
                    ea.attacking = anim == 2; // server::sim::enemy_anim::ATTACK
                }
                // Surface the active-debuff bitmask. HUD reads it
                // to paint indicator pips above the enemy. Insert
                // the component on first sight; thereafter just
                // refresh the mask.
                let has_debuffs = world
                    .get::<&rift_engine::ecs::components::Debuffs>(entity)
                    .is_ok();
                if has_debuffs {
                    if let Ok(mut d) = world.get::<&mut rift_engine::ecs::components::Debuffs>(entity) {
                        d.mask = debuffs;
                    }
                } else {
                    let _ = world.insert_one(
                        entity,
                        rift_engine::ecs::components::Debuffs { mask: debuffs },
                    );
                }
            }
        }
    }

    /// Reconcile renderer projectile slots with the latest snapshot.
    /// For every replicated projectile NetId we don't yet have a
    /// renderer slot for, allocate a fireball mesh; for every NetId
    /// we have but isn't in the snapshot any more, hide its slot.
    /// Position + yaw come straight from the snapshot — projectiles
    /// move fast enough that interpolation doesn't buy much.
    pub fn sync_projectiles(&mut self, renderer: &mut Renderer) {
        if self.our_net_id.is_none() {
            return;
        }

        // Despawn vanished projectiles (zero out the model matrix
        // so the renderer skips them).
        let stale: Vec<NetId> = self
            .projectile_objects
            .iter()
            .filter(|(nid, _)| !self.remote.contains_key(nid))
            .map(|(nid, _)| *nid)
            .collect();
        for net_id in stale {
            if let Some(idx) = self.projectile_objects.remove(&net_id) {
                if idx < renderer.objects.len() {
                    renderer.objects[idx].model_matrix = glam::Mat4::ZERO;
                }
            }
        }

        // Spawn newcomers. Use a small fireball mesh for the visual.
        let to_spawn: Vec<(NetId, Vec3)> = self
            .remote
            .iter()
            .filter(|(nid, _)| !self.projectile_objects.contains_key(nid))
            .filter_map(|(nid, re)| match re.kind {
                EntityKind::Projectile { .. } => Some((*nid, re.position)),
                _ => None,
            })
            .collect();
        for (net_id, _pos) in to_spawn {
            let mesh = rift_engine::renderer::mesh::Mesh::fireball();
            if renderer.add_mesh(&mesh, glam::Mat4::ZERO).is_ok() {
                let idx = renderer.objects.len() - 1;
                self.projectile_objects.insert(net_id, idx);
            }
        }

        // Drive transforms. Use snapshot position directly — fast
        // small projectiles don't need interp, and we'd need a
        // bespoke buffer because `interp` is keyed for entities we
        // also drive via ECS Transform.
        for (&net_id, &idx) in &self.projectile_objects {
            let Some(re) = self.remote.get(&net_id) else {
                continue;
            };
            if idx >= renderer.objects.len() {
                continue;
            }
            let scale = glam::Vec3::splat(0.6);
            renderer.objects[idx].model_matrix = glam::Mat4::from_translation(re.position)
                * glam::Mat4::from_rotation_y(re.yaw)
                * glam::Mat4::from_scale(scale);
        }
    }

    /// Drive the local SP `Player` entity's `Transform` from our
    /// predicted state, plus the residual smooth-correction error.
    /// Called from the binary BEFORE `GameState::update` so SP's
    /// `camera_follow_system` and `render_sync_system` (both run
    /// inside `update`) see the predicted position. We also zero
    /// the player's `Velocity` so `movement_system` becomes a
    /// no-op for the local player — we own kinematics now.
    ///
    /// Y is intentionally preserved from whatever the SP path
    /// last wrote so we don't fight any vertical animation/bob/
    /// foot-placement logic the engine owns. The server only
    /// cares about XZ collision anyway.
    ///
    /// SP code keeps owning `Player.action`, animations,
    /// abilities, equipment, etc.
    pub fn sync_local_player(&self, world: &mut hecs::World) {
        if !self.predicted_ready {
            return;
        }
        // Visible position bleeds the residual error away over
        // time so corrections aren't visually abrupt.
        let visible = self.predicted.position + self.correction_error;
        let yaw = self.predicted.yaw;
        // Lazy-attach `NetControlled` to the local player so
        // `movement_system` and `collision_system` skip its
        // horizontal integration — we own that via the prediction
        // loop. Done in a second pass since hecs disallows
        // structural changes during a query.
        let mut needs_marker: Vec<hecs::Entity> = Vec::new();
        for (entity, (transform, _player, _local, marker)) in world.query_mut::<(
            &mut Transform,
            &Player,
            &LocalPlayer,
            Option<&NetControlled>,
        )>() {
            // Override XZ from the predicted state. Y is left
            // alone so SP-owned jump physics (`movement_system`'s
            // gravity branch, which still runs for net players)
            // can keep playing locally.
            transform.position.x = visible.x;
            transform.position.z = visible.z;
            transform.rotation = Quat::from_rotation_y(yaw);
            if marker.is_none() {
                needs_marker.push(entity);
            }
        }
        for e in needs_marker {
            let _ = world.insert_one(e, NetControlled);
        }

        // Server-authoritative HP: the local player's snapshot row
        // carries `health_pct`. Mirror it onto the SP `Health`
        // component so the HUD HP bar reflects damage taken from
        // server-side enemy hits without us locally subtracting.
        if let Some(our_id) = self.our_net_id {
            if let Some(re) = self.remote.get(&our_id) {
                let target_pct = re.health_pct;
                for (_e, (_p, _l, h)) in world
                    .query_mut::<(&Player, &LocalPlayer, &mut rift_engine::ecs::components::Health)>()
                {
                    h.current = h.max * target_pct;
                }
            }
        }
    }
}

/// Bridge between the wire enum (`rift_net`) and the in-game
/// enum (`rift_game::character`). Done here, not in either crate,
/// so neither has to depend on the other.
fn gender_to_game(g: Gender) -> GameGender {
    match g {
        Gender::Male => GameGender::Male,
        Gender::Female => GameGender::Female,
    }
}

/// Map the wire role byte (`rift_server::sim::role::*`) to the
/// client's `MonsterRole`. Unknown values are dropped — a future
/// new role won't crash an old client; it just won't render.
fn role_byte_to_monster_role(r: u8) -> Option<rift_game::monsters::MonsterRole> {
    use rift_game::monsters::MonsterRole;
    Some(match r {
        0 => MonsterRole::Brute,
        1 => MonsterRole::Stalker,
        2 => MonsterRole::Caster,
        3 => MonsterRole::Elite,
        4 => MonsterRole::Boss,
        _ => return None,
    })
}