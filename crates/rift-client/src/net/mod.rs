//! Client-side network session.
//!
//! Owns the renet client, the inbound `ServerMsg` dispatch, and the
//! per-tick state every other module reads from. The actual work
//! (snapshot reconciliation, ECS world sync, outbound commands)
//! lives in submodules — this file is the transport + dispatch +
//! drain surface.
//!
//! Activated via `--connect <addr>` on the command line. When
//! omitted, the game runs single-player exactly as before and
//! none of these modules are touched.

mod commands;
mod snapshot;
mod world_sync;

pub use snapshot::{PendingFloor, RemoteEntity, RemoteProfile};

use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    time::Duration,
};

use glam::Vec3;
use rift_dungeon::{Floor, FloorConfig};
use rift_engine::Input;
use rift_net::{
    decode, encode,
    messages::InputCmd,
    open_client, renet, Channel, ClientHandle, ClientId, ClientMsg, NetId, NetSettings, NetTick,
    ServerMsg, PROTOCOL_VERSION,
};
use rift_game::kinematic::Kinematic;

use snapshot::RemoteInterp;

/// Per-projectile dead-reckoned visual state. Snapshots arrive
/// at 20 Hz but projectiles fly at 25+ m/s — way too jumpy to
/// drive directly. We snap to the snapshot whenever its
/// position changes (`anchor_pos`/`anchor_vel`/`anchor_seen`)
/// and otherwise extrapolate the rendered position forward at
/// `anchor_vel * frame_dt` between snapshots so the trail and
/// fireball glide.
#[derive(Clone, Copy, Debug)]
pub struct ProjectileRender {
    pub render_pos: Vec3,
    pub anchor_pos: Vec3,
    pub anchor_vel: Vec3,
    pub yaw: f32,
    /// Resolved detonation VFX, stored at spawn so the despawn
    /// path doesn't need to re-look-up the ability. Distinct
    /// per ability — fireball ⇒ orange burst, caster bolt ⇒
    /// violet burst.
    pub impact: rift_game::abilities::VfxKind,
    /// Uniform render scale (from `ShapeVisuals::Projectile.scale`).
    pub scale: f32,
}

#[derive(Clone, Debug)]
pub struct ClientProfile {
    pub account_name: String,
    pub character_name: String,
    pub class_id: String,
    pub gender: rift_net::messages::Gender,
}

/// Active networking session for a connected client. One per
/// running game when `--connect` is in use. Owned by the binary
/// and ticked before each frame's `update`.
pub struct NetClient {
    handle: ClientHandle,
    /// Whether we've already sent the initial `Hello`. We delay the
    /// send until renet reports the underlying netcode handshake is
    /// complete; otherwise the message is queued in renet's buffer
    /// and lost if the connection fails.
    hello_sent: bool,
    /// Highest server tick we've seen, sent back as `Ack` so the
    /// server can prune older snapshots from the per-client history.
    pub(super) last_server_tick: NetTick,
    /// Cached identity for the `Hello` payload.
    profile: Option<ClientProfile>,
    /// Server-assigned net id for our own player. Populated by the
    /// `Welcome` message; until then we don't know which row in a
    /// snapshot is ours.
    pub(super) our_net_id: Option<NetId>,
    /// Latest known state of every replicated entity, keyed by net
    /// id. Rebuilt from each snapshot.
    pub remote: HashMap<NetId, RemoteEntity>,
    /// Per-remote interpolation buffer. Keeps the previous + current
    /// snapshot sample so `sync_avatars` can blend between them on
    /// every render frame, smoothing 20 Hz snapshot delivery into
    /// fluid 60+ Hz visual motion. Indexed by net id so it survives
    /// snapshots that briefly omit an entity (avoids full reset).
    pub(super) interp: HashMap<NetId, RemoteInterp>,
    /// Cosmetic identity for every remote player we've been told
    /// about. Populated by `PlayerJoined`; entries are removed on
    /// `PlayerLeft`. Consumed by `sync_avatars` when it spawns the
    /// avatar entity for a never-before-seen `net_id`.
    pub(super) profiles: HashMap<NetId, RemoteProfile>,
    /// Outbound input sequence. Bumped each time we send an input.
    pub(super) input_seq: u32,
    /// Wall-clock accumulator for input rate-limiting. We send
    /// inputs at ~60 Hz regardless of frame rate. Also doubles as
    /// "time since the last predict step" so the visual position
    /// in [`world_sync::sync_local_player`] can extrapolate the
    /// XZ position by `predicted.velocity * accumulator` and
    /// stay smooth at >60 fps render rates.
    pub(super) input_accumulator: Duration,
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
    pub(super) enemy_entities: HashMap<NetId, hecs::Entity>,
    /// Renderer object index per replicated server-spawned
    /// projectile. Lightweight — no ECS entity, no animation,
    /// just a position-driven mesh.
    pub(super) projectile_objects: HashMap<NetId, usize>,
    /// VFX trail emitter per active projectile. Re-anchored to
    /// the projectile's position every frame so the embers /
    /// smoke streak along the flight path. Despawned when the
    /// projectile vanishes from snapshots, at which point we
    /// also spawn a one-shot detonation at its last known
    /// position.
    pub(super) projectile_trails: HashMap<NetId, rift_engine::renderer::vfx::EffectId>,
    /// Client-side dead-reckoned render position per projectile.
    /// Snapshots arrive at `SNAPSHOT_HZ` (20 Hz) which is far
    /// too slow for fast straight-line projectiles to look
    /// smooth — so we extrapolate from the last known position
    /// using the snapshot's velocity each frame, snapping back
    /// whenever a fresh snapshot lands. Doubles as the spawn
    /// anchor for the detonation when the projectile despawns.
    pub(super) projectile_render: HashMap<NetId, ProjectileRender>,
    /// Last known world-space position of every replicated entity
    /// the server has ever told us about. Survives across snapshots
    /// (the snapshot drops a row the moment an enemy dies, but the
    /// reliable `Death` event may still need that position to drop
    /// a blood decal). Updated whenever we ingest a snapshot row.
    pub last_positions: HashMap<NetId, Vec3>,
    /// Reliable world events received this tick. Drained by the
    /// binary each frame so it can spawn floating combat text /
    /// hit reactions / death animations off them.
    pending_events: VecDeque<rift_net::messages::WorldEvent>,
    /// Loot pickup confirmations received this tick (from
    /// `ServerMsg::LootClaimed`). Drained by the binary so it can
    /// tear down the loot-pillar visual and — if the picker is
    /// us — add the rolled item to the local inventory.
    pending_loot_claims: VecDeque<(NetId, ClientId)>,
    /// Loot pickup rejections received this tick (from
    /// `ServerMsg::PickupRejected`). Drained by the binary so it
    /// can show a warning toast ("Inventory full") and discard
    /// any local prediction tied to the rejected request.
    pending_pickup_rejections:
        VecDeque<(NetId, rift_net::messages::PickupRejectReason)>,
    /// Full inventory replication for the local player. Sent by
    /// the server on session start (after `Welcome`) and any time
    /// the bag is rewritten authoritatively. Drained by the
    /// binary so it can replace `GameState::mp_inventory` whole.
    pending_inventory_sync: Option<Vec<Option<rift_net::messages::ItemBlob>>>,
    /// Full equipment replication for the local player. Sent
    /// alongside / immediately after `pending_inventory_sync`,
    /// and again after every server-applied equip / unequip.
    /// `None` means "no fresh sync this frame"; the binary
    /// applies whole-cloth replacements only.
    pending_equipment_sync: Option<Vec<(u8, rift_net::messages::ItemBlob)>>,
    /// Full stash replication for the local player. Sent by the
    /// server in reply to `OpenStash` and after every server-
    /// applied deposit / withdraw. Drained by the binary so it
    /// can replace `LootClientState::stash_items` whole.
    pending_stash_sync: Option<Vec<Option<rift_net::messages::ItemBlob>>>,
    /// Latest authoritative XP / level snapshot for the local
    /// character, drained by the binary once per frame and
    /// pushed into `PlayerState::experience`.
    pending_character_stats: Option<(u32, u64, u64)>,
    /// Latest authoritative ability-loadout snapshot (six wire
    /// ids). Drained once per frame and pushed into
    /// `PlayerState::loadout`, which re-materializes the runtime
    /// `AbilitySlot`.
    pending_loadout: Option<[u8; 6]>,
    /// Latest authoritative rift-progress snapshot. Same
    /// shape as `ServerMsg::RiftProgress` (progress, required,
    /// boss_spawned, boss_killed, floor_complete). Drained by
    /// the binary into `RiftState`.
    pending_rift_progress: Option<(u32, u32, bool, bool, bool)>,
    /// Latest authoritative rift exit vote snapshot. Replaced
    /// wholesale on every `ServerMsg::RiftExitVote`. The binary
    /// drains it into `GameState` so the HUD can render the
    /// vote panel + cooldown.
    pending_exit_vote: Option<rift_net::messages::VoteState>,
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
    pub(super) predicted: Kinematic,
    /// Whether `predicted` has been seeded from a server snapshot.
    /// Until the first snapshot lands we don't know our authoritative
    /// position and shouldn't override the SP player transform.
    pub(super) predicted_ready: bool,
    /// `true` once the server has flagged our player row with
    /// `entity_flags::DEAD`. Drives input gating: we stop sending
    /// movement/ability commands and freeze local prediction so
    /// the death animation isn't fighting input replay. Cleared
    /// when the server respawns us via `LoadFloor`.
    pub(super) local_dead: bool,
    /// `true` once the server has flagged our player row with
    /// `entity_flags::GHOST` — we've risen from the down-pose
    /// and are spectating. Movement input is re-enabled (the
    /// flag is checked alongside `local_dead`); abilities + loot
    /// pickup remain server-rejected. Cleared on `LoadFloor`
    /// (heal_all clears `is_ghost` server-side).
    pub(super) local_ghost: bool,
    /// History of inputs we've sent but the server hasn't yet
    /// acked. Each entry is `(seq, dt, cmd)`. On every snapshot we
    /// drop entries with `seq <= ack_seq` and replay the rest on
    /// top of the authoritative position.
    pub(super) input_history: VecDeque<(u32, f32, InputCmd)>,
    /// Floor we're predicting against. Regenerated when a `Welcome`
    /// or future `LoadFloor` arrives, using the same seed the
    /// server uses so collision results match exactly.
    pub(super) predict_floor: Option<Floor>,
    /// Smooth correction error: the offset we still need to bleed
    /// off from a recent server correction. Decays exponentially
    /// each frame so big snaps don't visibly teleport the camera.
    pub(super) correction_error: Vec3,
    /// Latest aim direction (XZ, world-space) the binary has handed
    /// us via `set_aim`. Shipped on the next outbound `InputCmd` so
    /// the server can replicate it to remote observers and keep
    /// their spine-twist visual in sync with where this client is
    /// actually pointing the cursor. `[0, 0]` means "no aim known
    /// yet" — the server falls back to body yaw in that case.
    pub(super) pending_aim: [f32; 2],
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
    pub(super) roster_request: Option<String>,
    /// Whether we've already shipped a `RequestRoster` for the
    /// current `roster_request`. Reset whenever a fresh account
    /// name is queued.
    pub(super) roster_request_sent: bool,
    /// Latest roster reply from the server. `None` means "never
    /// asked" or "asked but no reply yet". Drained by the binary
    /// once it's been forwarded into the character-select UI.
    pub(super) roster: Option<Vec<rift_net::messages::RosterEntry>>,
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
            projectile_trails: HashMap::new(),
            projectile_render: HashMap::new(),
            last_positions: HashMap::new(),
            pending_events: VecDeque::new(),
            pending_loot_claims: VecDeque::new(),
            pending_pickup_rejections: VecDeque::new(),
            pending_inventory_sync: None,
            pending_equipment_sync: None,
            pending_stash_sync: None,
            pending_character_stats: None,
            pending_loadout: None,
            pending_rift_progress: None,
            pending_exit_vote: None,
            our_client_id: None,
            predicted: Kinematic::default(),
            predicted_ready: false,
            local_dead: false,
            local_ghost: false,
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
        // catches up with the predicted state over ~100 ms.
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
                // Server respawns the player on the new floor at
                // full HP, so reset the death gate. Otherwise a
                // hub-respawn after a death would keep input
                // suppressed indefinitely.
                self.local_dead = false;
                self.local_ghost = false;
                self.input_history.clear();
                // Discard any in-flight snapshots that predate the
                // transition tick — they describe the *old* floor.
                self.last_server_tick = tick;
                // Drop interp buffers so freshly-spawned remote
                // avatars on the new floor don't interpolate from
                // their stale old-floor positions.
                self.interp.clear();
                // Stale `last_positions` entries reference NetIds
                // from the previous floor; drop them so the next
                // floor's snapshot path starts from a clean map.
                self.last_positions.clear();
                // Drop the snapshot mirror too. Otherwise the last
                // pre-respawn snapshot (which still has hp=0 / DEAD
                // for the player who died) survives the LoadFloor
                // handler and is read by `sync_local_player` next
                // frame — clobbering the freshly-spawned local
                // entity's full HP back to 0 and re-triggering the
                // death FSM, which would re-trigger the death
                // animation and strand input gated permanently.
                // Clearing here means `sync_local_player`
                // is a no-op until the first post-respawn snapshot
                // populates `self.remote` with valid data.
                self.remote.clear();
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
                // VFX trail emitters get recycled by
                // `VfxSystem::clear_all` during the floor reset
                // (state.rs::reset_for_regeneration), so we just
                // forget our id handles here.
                self.projectile_trails.clear();
                self.projectile_render.clear();
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
            ServerMsg::PickupRejected { loot, reason } => {
                log::warn!("net: PickupRejected loot={loot:?} reason={reason:?}");
                self.pending_pickup_rejections.push_back((loot, reason));
            }
            ServerMsg::InventorySync { items } => {
                log::info!("net: InventorySync {} item(s)", items.len());
                self.pending_inventory_sync = Some(items);
            }
            ServerMsg::EquipmentSync { slots } => {
                log::info!("net: EquipmentSync {} slot(s)", slots.len());
                self.pending_equipment_sync = Some(slots);
            }
            ServerMsg::StashSync { items } => {
                log::info!("net: StashSync {} item(s)", items.len());
                self.pending_stash_sync = Some(items);
            }
            ServerMsg::CharacterStats { level, xp, xp_to_next } => {
                log::debug!(
                    "net: CharacterStats level={level} xp={xp}/{xp_to_next}"
                );
                self.pending_character_stats = Some((level, xp, xp_to_next));
            }
            ServerMsg::Loadout { slots } => {
                log::debug!("net: Loadout {slots:?}");
                self.pending_loadout = Some(slots);
            }
            ServerMsg::RiftProgress {
                progress,
                required,
                boss_spawned,
                boss_killed,
                floor_complete,
            } => {
                log::debug!(
                    "net: RiftProgress {progress}/{required} boss_spawned={boss_spawned} boss_killed={boss_killed} complete={floor_complete}"
                );
                self.pending_rift_progress = Some((
                    progress,
                    required,
                    boss_spawned,
                    boss_killed,
                    floor_complete,
                ));
            }
            ServerMsg::RiftExitVote(state) => {
                log::debug!(
                    "net: RiftExitVote active={} t={:.1}s cd={:.1}s voters={}",
                    state.active,
                    state.time_remaining,
                    state.cooldown_remaining,
                    state.voters.len()
                );
                self.pending_exit_vote = Some(state);
            }
            ServerMsg::Kick { reason } => {
                log::warn!("net: kicked: {reason}");
            }
        }
    }

    /// Wire-level send. Encodes `msg` and ships it on `ch`. No-ops
    /// when the connection isn't live so callers don't have to gate
    /// every send on `is_connected`.
    pub(super) fn send(&mut self, ch: Channel, msg: &ClientMsg) {
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

    /// Drain reliable world events received since the last call.
    /// The binary consumes these to spawn floating damage numbers,
    /// trigger hit-react animations, and play death clips.
    pub fn drain_events(&mut self) -> VecDeque<rift_net::messages::WorldEvent> {
        std::mem::take(&mut self.pending_events)
    }

    /// Drain `(loot_net_id, claimed_by_client_id)` pairs received
    /// via [`ServerMsg::LootClaimed`] since the last call. The
    /// binary forwards each one into `GameState::resolve_loot_claim`
    /// to tear down the visual; whether we add the item to our
    /// inventory is decided by comparing `claimed_by` to
    /// [`Self::our_client_id`].
    pub fn drain_loot_claims(&mut self) -> VecDeque<(NetId, ClientId)> {
        std::mem::take(&mut self.pending_loot_claims)
    }

    /// Drain `(loot_net_id, reason)` pairs received via
    /// [`ServerMsg::PickupRejected`] since the last call. The
    /// binary surfaces a warning to the player for each one.
    pub fn drain_pickup_rejections(
        &mut self,
    ) -> VecDeque<(NetId, rift_net::messages::PickupRejectReason)> {
        std::mem::take(&mut self.pending_pickup_rejections)
    }

    /// Take the most recent `InventorySync` if one has arrived
    /// since the last call. The binary applies it whole-cloth to
    /// `GameState::mp_inventory`, so a partial drain isn't
    /// meaningful — we surface a single `Option`.
    pub fn drain_inventory_sync(
        &mut self,
    ) -> Option<Vec<Option<rift_net::messages::ItemBlob>>> {
        self.pending_inventory_sync.take()
    }

    /// Take the most recent `EquipmentSync` if one has arrived
    /// since the last call. Mirrors [`Self::drain_inventory_sync`]
    /// in shape: the binary rebuilds [`LootClientState::equipment`]
    /// from the returned slot list whole-cloth. `None` means no
    /// fresh sync.
    pub fn drain_equipment_sync(
        &mut self,
    ) -> Option<Vec<(u8, rift_net::messages::ItemBlob)>> {
        self.pending_equipment_sync.take()
    }

    /// Take the most recent `StashSync` if one has arrived
    /// since the last call. Mirrors [`Self::drain_inventory_sync`]
    /// in shape.
    pub fn drain_stash_sync(
        &mut self,
    ) -> Option<Vec<Option<rift_net::messages::ItemBlob>>> {
        self.pending_stash_sync.take()
    }

    /// Take the most recent `CharacterStats` reply if one has
    /// arrived since the last call. Tuple is
    /// `(level, xp_into_level, xp_to_next)`. The binary writes
    /// these into `PlayerState::experience` and re-runs
    /// `recompute_stats` so the HUD bar / spawned HP pool match
    /// the new level.
    pub fn drain_character_stats(&mut self) -> Option<(u32, u64, u64)> {
        self.pending_character_stats.take()
    }

    /// Take the most recent `Loadout` reply if one has arrived
    /// since the last call. The binary writes the six wire ids
    /// into `PlayerState::loadout` and re-materializes the
    /// runtime `AbilitySlot` so the HUD bar reflects the
    /// authoritative bar.
    pub fn drain_loadout(&mut self) -> Option<[u8; 6]> {
        self.pending_loadout.take()
    }

    /// Take the most recent `RiftProgress` reply if one has
    /// arrived since the last call. Tuple matches the wire
    /// shape: `(progress, required, boss_spawned, boss_killed,
    /// floor_complete)`. The binary mirrors these into
    /// `RiftState`.
    pub fn drain_rift_progress(
        &mut self,
    ) -> Option<(u32, u32, bool, bool, bool)> {
        self.pending_rift_progress.take()
    }

    /// Take the most recent `RiftExitVote` snapshot if one has
    /// arrived since the last call. The binary mirrors it into
    /// `GameState::exit_vote` so the HUD vote panel can render.
    pub fn drain_exit_vote(&mut self) -> Option<rift_net::messages::VoteState> {
        self.pending_exit_vote.take()
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

    /// `true` once the latest snapshot flagged our player row with
    /// `entity_flags::DEAD`. The binary uses this to drive the
    /// death fade overlay and skip click-to-cast / interaction
    /// while the death animation plays.
    pub fn is_local_dead(&self) -> bool {
        self.local_dead
    }

    /// `true` once the local player has risen as a ghost
    /// (`entity_flags::GHOST` on our snapshot row). Implies
    /// `is_local_dead()` is also `true` — ghosts are still
    /// dead from the HP point of view; the flag just unlocks
    /// movement input and switches the avatar to spectator
    /// mode. Cleared when the server respawns us.
    pub fn is_local_ghost(&self) -> bool {
        self.local_ghost
    }

    /// Diagnostic disconnect reason, if renet has decided we're done.
    pub fn disconnect_reason(&self) -> Option<renet::DisconnectReason> {
        self.handle.client.disconnect_reason()
    }
}
