//! Client-side network session.
//!
//! Owns the renet client, the inbound `ServerMsg` dispatch, and the
//! per-tick state every other module reads from. The actual work
//! (snapshot reconciliation, ECS world sync, outbound commands)
//! lives in submodules — this file is the transport + dispatch +
//! drain surface.
//!
//! The client always runs networked — there is no offline mode.
//! The connect address is resolved at startup (`--connect`,
//! `RIFT_SERVER`, or the compile-time default); a missing address
//! is a hard error before this module is ever constructed.

mod commands;
mod snapshot;
mod world_sync;

pub use snapshot::{PendingFloor, RemoteEntity, RemoteProfile};
pub use world_sync::wire_gender_to_game;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::SocketAddr,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use glam::Vec3;
use rift_dungeon::{Floor, FloorConfig};
use rift_engine::Input;
use rift_game::kinematic::Kinematic;
use rift_net::{
    decode, encode, messages::InputCmd, open_client, renet, Channel, ClientHandle, ClientId,
    ClientMsg, NetId, NetSettings, NetTick, ServerMsg, Snapshot, PROTOCOL_VERSION,
};

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

#[derive(Clone, Copy, Debug)]
pub(super) struct EnemyDeathCue {
    pub poof_spawned: bool,
}

impl EnemyDeathCue {
    pub fn new() -> Self {
        Self {
            poof_spawned: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ClientProfile {
    pub account_name: String,
    pub character_name: String,
    pub class_id: String,
    pub gender: rift_net::messages::Gender,
}

/// One inbound chat line awaiting drain into the chat HUD.
/// Mirrors [`ServerMsg::Chat`] but flattened for the binary's
/// drain loop. `sender == None` flags a system event.
#[derive(Clone, Debug)]
pub struct PendingChat {
    pub channel: u8,
    pub sender: Option<String>,
    pub target: Option<String>,
    pub text: String,
}

/// Latest portal-modal prompt the server has asked us to show.
/// Mirrors [`rift_net::messages::ServerMsg::PortalPrompt`] but
/// owned by the binary so the drain step can move it into
/// `GameState.party` without cloning.
#[derive(Clone, Debug)]
pub struct PendingPortalPrompt {
    pub proposer: String,
    pub start_floor: u32,
    pub mode: u8,
    pub seconds_remaining: u32,
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
    /// `true` once we've sent the auth-only `Hello` for this
    /// connection. Gates the auto-send in `step` so we don't
    /// spam the wire while waiting for `Authenticated`.
    auth_sent: bool,
    /// Auth signer used to mint the opaque ticket for `Hello`.
    /// `Hello`. `None` until the binary plumbs one in via
    /// [`Self::set_signer`]; `send_hello` is a no-op until
    /// then so a misconfigured client can't ship a placeholder
    /// credential by accident.
    signer: Option<crate::auth::Signer>,
    /// `true` once we've sent `EnterWorld` for the current
    /// `profile`. Until then we're still in the character-select
    /// stage post-auth and the avatar isn't spawned server-side.
    enter_world_sent: bool,
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
    latest_snapshot: Option<Snapshot>,
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
    /// Per-remote-player previous `Kinematic::action` byte
    /// observed in the last applied snapshot. Used by the
    /// melee-swing mirror in `world_sync` to detect step-to-
    /// step transitions in the `ATTACK_A..ATTACK_D` combo
    /// range so each new swing triggers a fresh
    /// `SpellCast::play_oneshot` on the upper-body layer
    /// (rather than just a level-edge from none\u2192attack).
    pub(super) prev_action_byte: HashMap<NetId, u8>,
    /// ECS entity per replicated server-driven enemy. Spawned
    /// lazily by `sync_enemies` the first frame a fresh enemy
    /// `NetId` shows up in a snapshot, despawned when the server
    /// stops shipping it (death / floor change). Skinned mesh
    /// comes from the shared `MonsterCache` on `FloorManager`.
    pub(super) enemy_entities: HashMap<NetId, hecs::Entity>,
    /// ECS entity per replicated friendly minion. Uses monster
    /// assets like enemies, but remains separate so UI/targeting
    /// never treats summons as hostile.
    pub(super) minion_entities: HashMap<NetId, hecs::Entity>,
    pub(super) minion_visual_positions: HashMap<NetId, Vec3>,
    pub(super) minion_hover_time: f32,
    /// Explicit client-side death/despawn cue per enemy. Reliable
    /// death events and DEAD snapshots create the entry; the same
    /// visual-removal operation spawns the soul-return poof and
    /// frees/despawns the mesh, then this state suppresses any
    /// server corpse rows that continue arriving during the fade
    /// window.
    pub(super) enemy_death_cues: HashMap<NetId, EnemyDeathCue>,
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
    /// Per-projectile travel-loop audio emitter. Looking up
    /// the ability's `audio_for` recipe at spawn and attaching
    /// a looping `SpatialTrack` that we re-anchor to the
    /// projectile's render position every frame, so the loop
    /// follows the flight path. Despawned in the same stale-
    /// row pass that despawns the trail VFX.
    pub(super) projectile_audio: HashMap<NetId, rift_audio::EmitterId>,
    /// Wire id of the ability that spawned each live
    /// projectile. Used by the despawn pass to look up the
    /// impact sound (the ability lookup is cheap but it's
    /// trivially memoised here so the despawn doesn't need
    /// to chase through `Ability::lookup` for every stale
    /// row).
    pub(super) projectile_ability: HashMap<NetId, u8>,
    /// Client-side dead-reckoned render position per projectile.
    /// Snapshots arrive at `SNAPSHOT_HZ` (20 Hz) which is far
    /// too slow for fast straight-line projectiles to look
    /// smooth — so we extrapolate from the last known position
    /// using the snapshot's velocity each frame, snapping back
    /// whenever a fresh snapshot lands. Doubles as the spawn
    /// anchor for the detonation when the projectile despawns.
    pub(super) projectile_render: HashMap<NetId, ProjectileRender>,
    /// Projectile ids whose authoritative impact event has already
    /// spawned its burst. When their snapshot row disappears later,
    /// the despawn fallback only tears down mesh/trail/audio.
    pub(super) projectile_impacts: HashSet<NetId>,
    /// Last known world-space position of every replicated entity
    /// the server has ever told us about. Survives across snapshots
    /// (the snapshot drops a row the moment an enemy dies, but the
    /// reliable `Death` event may still need that position to drop
    /// a blood decal). Updated whenever we ingest a snapshot row.
    pub last_positions: HashMap<NetId, Vec3>,
    /// Last known world-space velocity of every replicated entity.
    /// Parallels `last_positions` and is updated from the same
    /// snapshot row. Primary consumer is the death-event handler:
    /// the moment an enemy dies, its velocity vector encodes the
    /// impact / impulse direction that the layered blood decal
    /// system uses to orient the corpse pool, spray fan, and
    /// wall arc. Survives row culls so a delayed Death packet
    /// can still recover direction even after the snapshot row
    /// has vanished.
    pub last_velocities: HashMap<NetId, Vec3>,
    /// Enemy NetIds the server has confirmed dead via reliable
    /// `WorldEvent::Death`. This is the authoritative death marker;
    /// `enemy_death_cues` records whether the local visual removal
    /// has already spawned the soul poof. While a NetId is present in
    /// this set, server corpse rows are ignored so the dead mesh does
    /// not respawn during the server's death window.
    pub dead_net_ids: std::collections::HashSet<NetId>,
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
    pending_pickup_rejections: VecDeque<(NetId, rift_net::messages::PickupRejectReason)>,
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
    /// Per-peer visible-equipment lists waiting to be applied to
    /// remote avatars. Each entry is `(client_id, base_ids)` —
    /// see `ServerMsg::PeerEquipmentVisuals`. The latest entry
    /// for a given client wins; older entries for the same
    /// client are coalesced on push so we never apply stale
    /// state. Drained by the binary once per frame.
    pub(super) pending_peer_equipment_visuals: VecDeque<(ClientId, Vec<u16>)>,
    /// Latest known visible-equipment base ids per peer client.
    /// Populated on receive and consulted on remote-avatar spawn
    /// so a freshly-spawned avatar gets dressed with whatever the
    /// server last told us about that player, not just whatever
    /// changes happen *after* the spawn.
    pub(super) peer_visuals_mirror: std::collections::HashMap<ClientId, Vec<u16>>,
    /// Full stash replication for the local player. Sent by the
    /// server in reply to `OpenStash` and after every server-
    /// applied deposit / withdraw / tab edit. Drained by the
    /// binary so it can replace `LootClientState::stash_tabs`
    /// whole.
    pending_stash_sync: Option<Vec<rift_net::messages::StashTabBlob>>,
    /// Shared hub stash-chest visual state. Drained by the
    /// binary and applied to the prop renderer; no private stash
    /// contents are included.
    pending_stash_chest_open: Option<bool>,
    /// Latest authoritative XP / level snapshot for the local
    /// character, drained by the binary once per frame and
    /// pushed into `PlayerState::experience`.
    pending_character_stats: Option<(u32, u64, u64)>,
    /// Latest authoritative ability-loadout snapshot (six wire
    /// ids). Drained once per frame and pushed into
    /// `PlayerState::loadout`, which re-materializes the runtime
    /// `AbilitySlot`.
    pending_loadout: Option<[u8; 6]>,
    /// Latest authoritative talent-tree snapshot. Flat
    /// `(talent_id, rank)` pairs plus the unspent-point count.
    /// Drained by the binary once per frame and applied onto
    /// `PlayerState::talents`.
    pending_talents: Option<(Vec<(u16, u8)>, u32)>,
    /// Latest authoritative shard balance from
    /// [`ServerMsg::ShardsSync`]. Drained by the binary once
    /// per frame and mirrored into `PlayerState::shards`.
    pending_shards: Option<u32>,
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
    /// Inbound chat messages awaiting drain into the
    /// `GameState.chat` UI buffer. Filled by
    /// `handle_server_msg` on every `ServerMsg::Chat`,
    /// drained by the binary into the chat scrollback once
    /// per frame.
    pub(super) pending_chats: VecDeque<PendingChat>,
    /// Latest authoritative party snapshot. Replaced wholesale
    /// on every `ServerMsg::PartyState`. The binary drains it
    /// into `GameState.party` once per frame so the party-
    /// frames widget can re-render. `None` here means "no
    /// update this frame" (not "solo" — a solo client
    /// receives a `PartyState` with empty members).
    pub(super) pending_party_state: Option<rift_net::messages::ServerMsg>,
    /// Toast queue for incoming party invites. Drained into
    /// `GameState.party` which surfaces the prompt + tracks
    /// the most-recent inviter for `/accept` shorthand.
    pub(super) pending_party_invites: VecDeque<String>,
    /// Soft-error toasts for refused party actions. Drained
    /// into the system chat channel.
    pub(super) pending_party_errors: VecDeque<String>,
    /// Latest portal-prompt modal request from the server.
    /// `Some` opens the modal, `None` closes it (set by
    /// `PortalPromptClosed`).
    pub(super) pending_portal_prompt: Option<Option<PendingPortalPrompt>>,
    /// Latest authoritative `deepest_cleared_floor` value for
    /// our own character. Drives the start-floor picker's
    /// upper bound in the portal modal.
    pub(super) pending_deepest_floor: Option<u32>,
    /// Latest combat-meter snapshot for our current rift
    /// instance. Replaced wholesale on every
    /// `ServerMsg::MeterSnapshot` (~1 Hz). Drained by the
    /// binary into the HUD's `MeterUi`.
    pending_meters: Option<(f32, Vec<rift_net::messages::MeterEntry>)>,
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
    /// Local player's current authoritative `move_speed`, mirrored
    /// from `PlayerState::stats().move_speed` whenever the game
    /// loop calls `set_predicted_move_speed`. Used by the input
    /// prediction path so Boots/MoveSpeed affixes feel responsive
    /// locally instead of waiting for the next server snapshot.
    pub(super) predicted_move_speed: f32,
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
    /// Visual Y position for the local player, smoothed toward
    /// the kinematic Y each frame. The kinematic snaps instantly
    /// to the per-tile floor elevation (so collision and
    /// projectile arcs use the authoritative height), but the
    /// avatar mesh + camera are driven by `visual_y` so stepping
    /// onto a raised dais glides up over a few frames instead
    /// of teleporting. `None` until first prediction frame so we
    /// don't lerp from origin.
    pub(super) visual_y: Option<f32>,
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
    /// `true` once the account-entry screen (or the startup
    /// auth resolver, in dev mode) has confirmed an identity
    /// for this connection — i.e. the player has said "I
    /// want to log in now". The signer mints the credential
    /// at `Hello` time, so the string identity used to live
    /// here is no longer needed; this is just a gate that
    /// lets `send_hello` know it's time to fire.
    pub(super) auth_armed: bool,
    /// Per-frame accumulator for the handshake diagnostic log
    /// (rate-limited to ~1 Hz so a stuck client doesn't spam
    /// the terminal but still surfaces *why* it's stuck).
    pub(super) handshake_diag_accum: f32,
    /// Latest roster reply from the server (carried inside
    /// `ServerMsg::Authenticated`). `None` means "never
    /// authenticated" or "authenticated but reply not yet
    /// forwarded". Drained by the binary once it's been
    /// forwarded into the character-select UI.
    pub(super) roster: Option<Vec<rift_net::messages::RosterEntry>>,
    /// Server-canonical display name returned alongside the
    /// roster in `ServerMsg::Authenticated`. Currently unused
    /// by the binary; reserved for the post-auth "logged in
    /// as …" UI that lands with the Steam path.
    #[allow(dead_code)]
    pub(super) display_name: Option<String>,
    /// Fatal rejection reason from the server (e.g. protocol
    /// version mismatch). Set on receipt of [`ServerMsg::Reject`];
    /// the binary checks this every frame and exits cleanly with
    /// the message instead of letting the user wonder why the
    /// connection silently hangs.
    pub(super) fatal_reject_reason: Option<String>,
}

impl NetClient {
    pub fn connect(server: SocketAddr) -> anyhow::Result<Self> {
        static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);
        let local_connection_id = NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed);
        let client_id = ClientId(((std::process::id() as u64) << 32) ^ local_connection_id);
        let handle = open_client(server, client_id, &NetSettings::default())?;
        Ok(Self {
            handle,
            auth_sent: false,
            signer: None,
            enter_world_sent: false,
            last_server_tick: NetTick::default(),
            profile: None,
            our_net_id: None,
            remote: HashMap::new(),
            latest_snapshot: None,
            interp: HashMap::new(),
            profiles: HashMap::new(),
            input_seq: 0,
            input_accumulator: Duration::ZERO,
            avatar_entities: HashMap::new(),
            prev_action_byte: HashMap::new(),
            enemy_entities: HashMap::new(),
            minion_entities: HashMap::new(),
            minion_visual_positions: HashMap::new(),
            minion_hover_time: 0.0,
            enemy_death_cues: HashMap::new(),
            projectile_objects: HashMap::new(),
            projectile_trails: HashMap::new(),
            projectile_audio: HashMap::new(),
            projectile_ability: HashMap::new(),
            projectile_render: HashMap::new(),
            projectile_impacts: HashSet::new(),
            last_positions: HashMap::new(),
            last_velocities: HashMap::new(),
            dead_net_ids: std::collections::HashSet::new(),
            pending_events: VecDeque::new(),
            pending_loot_claims: VecDeque::new(),
            pending_pickup_rejections: VecDeque::new(),
            pending_inventory_sync: None,
            pending_equipment_sync: None,
            pending_peer_equipment_visuals: VecDeque::new(),
            peer_visuals_mirror: HashMap::new(),
            pending_stash_sync: None,
            pending_stash_chest_open: None,
            pending_character_stats: None,
            pending_loadout: None,
            pending_talents: None,
            pending_shards: None,
            pending_rift_progress: None,
            pending_exit_vote: None,
            pending_chats: VecDeque::new(),
            pending_party_state: None,
            pending_party_invites: VecDeque::new(),
            pending_party_errors: VecDeque::new(),
            pending_portal_prompt: None,
            pending_deepest_floor: None,
            pending_meters: None,
            our_client_id: None,
            predicted: Kinematic::default(),
            predicted_move_speed: rift_game::hero::HERO.base_move_speed,
            predicted_ready: false,
            local_dead: false,
            local_ghost: false,
            input_history: VecDeque::new(),
            predict_floor: None,
            correction_error: Vec3::ZERO,
            visual_y: None,
            pending_aim: [0.0, 0.0],
            pending_floor: None,
            floor_index: 0,
            rift_seed: 0,
            auth_armed: false,
            handshake_diag_accum: 0.0,
            roster: None,
            display_name: None,
            fatal_reject_reason: None,
        })
    }

    /// Mark a projectile as having already played its reliable
    /// authoritative impact cue. Returns true when the projectile
    /// currently has a local render row whose later disappearance
    /// should skip the old despawn-driven impact fallback.
    pub fn note_projectile_impact(&mut self, projectile: NetId) -> bool {
        let tracked = self.projectile_render.contains_key(&projectile)
            || self.projectile_objects.contains_key(&projectile);
        if tracked {
            self.projectile_impacts.insert(projectile);
        }
        tracked
    }

    /// Pump network state. Call once per frame, before the renderer's
    /// `update`. `dt` is wall-clock since the last call. `input` is
    /// the engine's current input snapshot, which we sample for the
    /// outbound `InputCmd`. Pass `None` while UI states (character
    /// select, menus) shouldn't drive the avatar.
    pub fn step(&mut self, dt: Duration, input: Option<&Input>) {
        // Drive netcode + renet timers.
        if let Err(e) = self.handle.transport.update(dt, &mut self.handle.client) {
            log::warn!("net: transport update: {e:?}");
        }
        self.handle.client.update(dt);

        // Two-phase handshake:
        //   1. As soon as renet reports the connection is live
        //      and we have an account identity queued, ship the
        //      auth-only `Hello`. Server replies with
        //      `Authenticated` (carrying the roster) or
        //      `Reject`.
        //   2. After the player picks a character (which calls
        //      `set_profile`), ship `EnterWorld`. Server replies
        //      with `Welcome` and the avatar spawns.
        if !self.auth_sent
            && self.handle.client.is_connected()
            && self.auth_armed
            && self.signer.is_some()
        {
            self.send_hello();
            self.auth_sent = true;
        }

        // Diagnostic: once per second, log the handshake gate
        // state if we haven't shipped Hello yet. Helps an
        // operator tell at a glance whether the bottleneck is
        // renet connectivity, missing arm signal, or missing
        // signer.
        if !self.auth_sent {
            self.handshake_diag_accum += dt.as_secs_f32();
            if self.handshake_diag_accum >= 1.0 {
                self.handshake_diag_accum = 0.0;
                log::debug!(
                    "net: waiting to send Hello — connected={}, auth_armed={}, signer={}",
                    self.handle.client.is_connected(),
                    self.auth_armed,
                    self.signer.is_some(),
                );
            }
        }
        if !self.enter_world_sent
            && self.auth_sent
            && self.handle.client.is_connected()
            && self.profile.is_some()
        {
            self.send_enter_world();
            self.enter_world_sent = true;
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
        if let Err(e) = self.handle.transport.send_packets(&mut self.handle.client) {
            log::warn!("net: transport send: {e:?}");
        }
    }

    /// Set (or replace, before Hello has been sent) the cosmetic
    /// profile this client advertises to the server. Called by the
    /// binary once character-select has finished. Calling this
    /// after Hello has already been sent is a no-op — the server
    /// uses the first Hello as authoritative for the session.
    pub fn set_profile(&mut self, profile: ClientProfile) {
        if self.enter_world_sent {
            log::warn!("net: set_profile called after EnterWorld — ignored");
            return;
        }
        self.profile = Some(profile);
    }

    /// Install the auth signer the net client should use for
    /// the next `Hello`. Idempotent: calling again before
    /// `Hello` has been sent overwrites the previous signer
    /// (lets the binary swap dev identities mid-startup if it
    /// ever needs to). After `Hello` has been sent it logs and
    /// drops the new signer — the server will only accept the
    /// first `Hello` per connection anyway.
    pub fn set_signer(&mut self, signer: crate::auth::Signer) {
        if self.auth_sent {
            log::warn!("net: set_signer called after Hello — ignored");
            return;
        }
        self.signer = Some(signer);
    }

    /// Our character name, if `set_profile` has run. Used by the
    /// chat HUD to tell our own whisper echoes apart from
    /// inbound whispers (so `/r` only fills with names we
    /// genuinely received DMs from).
    pub fn character_name(&self) -> Option<&str> {
        self.profile.as_ref().map(|p| p.character_name.as_str())
    }

    /// Local player's authored gender from the active profile.
    /// `None` until `set_profile` has run. Used by the loot
    /// pipeline to pick the matching gendered mesh variant for
    /// dropped items so the bind-pose silhouette on the ground
    /// matches what the avatar would equip.
    pub fn local_gender(&self) -> Option<rift_net::messages::Gender> {
        self.profile.as_ref().map(|p| p.gender)
    }

    fn send_hello(&mut self) {
        if !self.auth_armed {
            return;
        }
        // Mint a fresh credential from the installed signer.
        // For the dev issuer this is an HMAC of the typed/randomised
        // identity; the server's `auth::resolve` re-verifies before
        // letting us into the account row. Without a signer we
        // refuse to ship a placeholder — the server has no key
        // to compare against and would just reject us anyway,
        // and shipping zero-signed traffic risks looking like a
        // probe.
        let Some(signer) = self.signer.as_ref() else {
            log::error!(
                "net: cannot send Hello — no auth signer installed (RIFT_DEV_AUTH_KEY \
                 unset and the client was not built with --features steam-auth). \
                 Holding back the handshake."
            );
            return;
        };
        let auth_ticket = signer.mint();
        let issuer = signer.identity_hint();
        let ticket_len = auth_ticket.len();
        let msg = ClientMsg::Hello {
            protocol_version: PROTOCOL_VERSION,
            auth_ticket,
        };
        self.send(Channel::Control, &msg);
        log::debug!("net: sent Hello (issuer={issuer}, ticket_len={ticket_len})");
    }

    fn send_enter_world(&mut self) {
        let Some(profile) = self.profile.clone() else {
            return;
        };
        let msg = ClientMsg::EnterWorld {
            character_name: profile.character_name.clone(),
            class_id: profile.class_id.clone(),
            gender: profile.gender,
        };
        self.send(Channel::Control, &msg);
        log::info!(
            "net: sent EnterWorld as {:?} ({:?})",
            profile.character_name,
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
                self.latest_snapshot = None;
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
                self.fatal_reject_reason = Some(reason);
            }
            ServerMsg::Authenticated {
                your_client_id,
                display_name,
                roster,
            } => {
                log::info!(
                    "net: Authenticated as {display_name:?} client_id={your_client_id:?} roster={} entries",
                    roster.len()
                );
                self.our_client_id = Some(your_client_id);
                self.display_name = Some(display_name);
                self.roster = Some(roster);
            }
            ServerMsg::Snapshot(snap) => {
                self.apply_snapshot(snap);
            }
            ServerMsg::SnapshotDelta(delta) => {
                self.apply_snapshot_delta(delta);
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
                self.latest_snapshot = None;
                // Drop interp buffers so freshly-spawned remote
                // avatars on the new floor don't interpolate from
                // their stale old-floor positions.
                self.interp.clear();
                // Stale `last_positions` entries reference NetIds
                // from the previous floor; drop them so the next
                // floor's snapshot path starts from a clean map.
                self.last_positions.clear();
                self.last_velocities.clear();
                // Same reason for `dead_net_ids` — a floor change
                // recycles the entire id range, so any leftover
                // "this id died" markers would mis-fire on a
                // freshly spawned enemy that happens to reuse the
                // same NetId.
                self.dead_net_ids.clear();
                self.enemy_death_cues.clear();
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
                self.minion_entities.clear();
                self.minion_visual_positions.clear();
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
                self.projectile_impacts.clear();
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
            ServerMsg::PeerEquipmentVisuals {
                client_id,
                base_ids,
            } => {
                log::debug!(
                    "net: PeerEquipmentVisuals client={client_id:?} {} item(s)",
                    base_ids.len(),
                );
                // Mirror the latest list for this peer so a
                // future avatar spawn can pick it up even if
                // the message arrives before the avatar exists.
                self.peer_visuals_mirror.insert(client_id, base_ids.clone());
                // Coalesce: drop any stale pending entry for the
                // same peer so we only apply the freshest list.
                self.pending_peer_equipment_visuals
                    .retain(|(cid, _)| *cid != client_id);
                self.pending_peer_equipment_visuals
                    .push_back((client_id, base_ids));
            }
            ServerMsg::StashSync { tabs } => {
                log::info!("net: StashSync {} tab(s)", tabs.len());
                self.pending_stash_sync = Some(tabs);
            }
            ServerMsg::StashChestState { open } => {
                self.pending_stash_chest_open = Some(open);
            }
            ServerMsg::CharacterStats {
                level,
                xp,
                xp_to_next,
            } => {
                log::debug!("net: CharacterStats level={level} xp={xp}/{xp_to_next}");
                self.pending_character_stats = Some((level, xp, xp_to_next));
            }
            ServerMsg::Loadout { slots } => {
                log::debug!("net: Loadout {slots:?}");
                self.pending_loadout = Some(slots);
            }
            ServerMsg::TalentsSync { invested, unspent } => {
                log::debug!(
                    "net: TalentsSync invested={} unspent={unspent}",
                    invested.len()
                );
                self.pending_talents = Some((invested, unspent));
            }
            ServerMsg::ShardsSync { amount } => {
                log::debug!("net: ShardsSync amount={amount}");
                self.pending_shards = Some(amount);
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
            ServerMsg::Chat {
                channel,
                sender,
                target,
                text,
            } => {
                self.pending_chats.push_back(PendingChat {
                    channel,
                    sender,
                    target,
                    text,
                });
            }
            ServerMsg::PartyState { .. } => {
                // Stash the whole message so the binary's
                // drain step can take ownership of the
                // members vec without cloning. Latest wins:
                // a stale snapshot in the queue would only
                // overwrite a fresher one we already applied.
                self.pending_party_state = Some(msg);
            }
            ServerMsg::PartyInviteIncoming { from } => {
                self.pending_party_invites.push_back(from);
            }
            ServerMsg::PartyError { reason } => {
                self.pending_party_errors.push_back(reason);
            }
            ServerMsg::PortalPrompt {
                proposer,
                start_floor,
                mode,
                seconds_remaining,
            } => {
                self.pending_portal_prompt = Some(Some(PendingPortalPrompt {
                    proposer,
                    start_floor,
                    mode,
                    seconds_remaining,
                }));
            }
            ServerMsg::PortalPromptClosed => {
                self.pending_portal_prompt = Some(None);
            }
            ServerMsg::DeepestFloorCleared { value } => {
                self.pending_deepest_floor = Some(value);
            }
            ServerMsg::MeterSnapshot {
                elapsed_seconds,
                entries,
            } => {
                self.pending_meters = Some((elapsed_seconds, entries));
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

    /// Drain the most recent authoritative party-state snapshot
    /// the server pushed (if any). The binary forwards it into
    /// `state.party.ingest_state(...)`.
    pub fn take_pending_party_state(&mut self) -> Option<rift_net::messages::ServerMsg> {
        self.pending_party_state.take()
    }

    /// Drain queued incoming-invite toasts (one per
    /// `ServerMsg::PartyInviteIncoming`).
    pub fn take_pending_party_invites(&mut self) -> Vec<String> {
        self.pending_party_invites.drain(..).collect()
    }

    /// Drain queued party-error toasts.
    pub fn take_pending_party_errors(&mut self) -> Vec<String> {
        self.pending_party_errors.drain(..).collect()
    }

    /// Drain a portal-prompt edge. `Some(Some(p))` means the
    /// server opened a per-member confirm modal for us;
    /// `Some(None)` means it just closed (timeout / proposer
    /// cancelled / we already resolved).
    pub fn take_pending_portal_prompt(&mut self) -> Option<Option<PendingPortalPrompt>> {
        self.pending_portal_prompt.take()
    }

    /// Drain a deepest-floor watermark edge.
    pub fn take_pending_deepest_floor(&mut self) -> Option<u32> {
        self.pending_deepest_floor.take()
    }

    /// Drain the most-recent combat-meter snapshot, if any.
    /// Returns `(elapsed_seconds, entries)` matching the wire
    /// format. Replaced (not accumulated) by the server, so a
    /// frame that sees `None` should keep showing the previous
    /// values.
    pub fn take_pending_meters(&mut self) -> Option<(f32, Vec<rift_net::messages::MeterEntry>)> {
        self.pending_meters.take()
    }

    /// Look up a remote player's `NetId` by character name
    /// (case-insensitive). Returns `None` for unknown names
    /// and for the local player (who isn't in `profiles`).
    /// Used by friendly-target click handlers to resolve
    /// "the player with this name on my party frame" into a
    /// wire-stable id we can ship to the server.
    pub fn net_id_for_name(&self, name: &str) -> Option<NetId> {
        let needle = name.to_ascii_lowercase();
        self.profiles
            .iter()
            .find(|(_, p)| p.character_name.to_ascii_lowercase() == needle)
            .map(|(nid, _)| *nid)
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
    pub fn drain_inventory_sync(&mut self) -> Option<Vec<Option<rift_net::messages::ItemBlob>>> {
        self.pending_inventory_sync.take()
    }

    /// Take the most recent `EquipmentSync` if one has arrived
    /// since the last call. Mirrors [`Self::drain_inventory_sync`]
    /// in shape: the binary rebuilds [`LootClientState::equipment`]
    /// from the returned slot list whole-cloth. `None` means no
    /// fresh sync.
    pub fn drain_equipment_sync(&mut self) -> Option<Vec<(u8, rift_net::messages::ItemBlob)>> {
        self.pending_equipment_sync.take()
    }

    /// Drain every `PeerEquipmentVisuals` payload received since
    /// the last call. Returned in arrival order so the binary can
    /// apply them in sequence; coalescing already happened on
    /// receive so each peer appears at most once.
    pub fn drain_peer_equipment_visuals(&mut self) -> Vec<(ClientId, Vec<u16>)> {
        std::mem::take(&mut self.pending_peer_equipment_visuals)
            .into_iter()
            .collect()
    }

    /// Resolve a peer's `ClientId` to the `hecs::Entity` of their
    /// remote-avatar in the local world, if one has been spawned.
    /// Used by the equipment-visuals dispatcher to know which
    /// avatar to dress when a `PeerEquipmentVisuals` arrives.
    pub fn avatar_for_client(&self, client_id: ClientId) -> Option<hecs::Entity> {
        let net_id = self
            .profiles
            .iter()
            .find_map(|(nid, p)| (p.client_id == client_id).then_some(*nid))?;
        self.avatar_entities.get(&net_id).copied()
    }

    /// Resolve a peer's `ClientId` to their cosmetic profile
    /// (character name, class, gender, ...). Used by the
    /// equipment-visuals dispatcher to pick the gendered model
    /// path when dressing the remote avatar.
    pub fn profile_for_client(&self, client_id: ClientId) -> Option<&RemoteProfile> {
        self.profiles.values().find(|p| p.client_id == client_id)
    }

    /// Take the most recent `StashSync` if one has arrived
    /// since the last call. Returns the dense `[0..n)` tab list.
    pub fn drain_stash_sync(&mut self) -> Option<Vec<rift_net::messages::StashTabBlob>> {
        self.pending_stash_sync.take()
    }

    /// Take the latest shared stash-chest visual state, if the
    /// server broadcast one since the last frame.
    pub fn drain_stash_chest_open(&mut self) -> Option<bool> {
        self.pending_stash_chest_open.take()
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

    /// Take the most recent `TalentsSync` reply if one has
    /// arrived since the last call. The binary mirrors the
    /// invested ranks + unspent count onto
    /// `PlayerState::talents`.
    pub fn drain_talents(&mut self) -> Option<(Vec<(u16, u8)>, u32)> {
        self.pending_talents.take()
    }

    /// Drain the latest authoritative shard balance, if one
    /// has arrived since the last call. The binary mirrors
    /// the result onto `PlayerState::shards` once per frame.
    pub fn drain_shards(&mut self) -> Option<u32> {
        self.pending_shards.take()
    }

    /// Take the most recent `RiftProgress` reply if one has
    /// arrived since the last call. Tuple matches the wire
    /// shape: `(progress, required, boss_spawned, boss_killed,
    /// floor_complete)`. The binary mirrors these into
    /// `RiftState`.
    pub fn drain_rift_progress(&mut self) -> Option<(u32, u32, bool, bool, bool)> {
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

    /// Whether a NetId belongs to a player we know about, even if
    /// their ECS avatar has not spawned yet or has already been
    /// hidden by a death/ghost transition.
    pub fn is_player_net_id(&self, net_id: NetId) -> bool {
        Some(net_id) == self.our_net_id
            || self.avatar_entities.contains_key(&net_id)
            || self.profiles.contains_key(&net_id)
            || self
                .remote
                .get(&net_id)
                .map(|row| matches!(row.kind, rift_net::messages::EntityKind::Player { .. }))
                .unwrap_or(false)
    }

    /// Record that the reliable event stream confirmed an enemy
    /// death. Snapshot/world sync owns visual removal from here.
    pub fn mark_enemy_death(&mut self, net_id: NetId) {
        self.dead_net_ids.insert(net_id);
        self.enemy_death_cues
            .entry(net_id)
            .or_insert_with(EnemyDeathCue::new);
    }

    /// ECS entity for a replicated enemy that is currently visible
    /// to this client.
    pub fn enemy_entity(&self, net_id: NetId) -> Option<hecs::Entity> {
        self.enemy_entities.get(&net_id).copied()
    }

    /// Latest essence pool fraction (0..=1) the server reported
    /// for the local player, or `1.0` before the first snapshot
    /// with our row arrives. The HUD reads this every frame to
    /// drive the essence bar; the canonical scalar is on the
    /// server.
    pub fn local_resource_pct(&self) -> f32 {
        let Some(nid) = self.our_net_id else {
            return 1.0;
        };
        self.remote.get(&nid).map(|r| r.resource_pct).unwrap_or(1.0)
    }

    pub fn local_summon_effects(&self) -> Vec<rift_engine::ecs::components::ActiveEffect> {
        let Some(owner_id) = self.our_net_id else {
            return Vec::new();
        };
        self.remote
            .values()
            .filter_map(|entity| match entity.kind {
                rift_net::messages::EntityKind::Minion { owner, role, .. } if owner == owner_id => {
                    let role = rift_game::monsters::MonsterRole::from_wire_byte(role)?;
                    let presentation = rift_game::minions::presentation_for_role(role);
                    let effect_id = presentation.hud_effect?;
                    let duration = rift_game::effects::lookup(effect_id)
                        .map(|def| def.default_duration)
                        .unwrap_or(28.0);
                    Some(rift_engine::ecs::components::ActiveEffect {
                        id: effect_id,
                        remaining: (entity.resource_pct.clamp(0.0, 1.0) * duration).max(0.0),
                        duration,
                    })
                }
                _ => None,
            })
            .collect()
    }

    /// Look up a remote player's display name by `NetId`. Returns
    /// `None` for unknown ids and for our own player (we aren't
    /// in `profiles`; callers that need to label our row should
    /// fall back to [`Self::character_name`]).
    pub fn name_for_net_id(&self, net_id: NetId) -> Option<&str> {
        self.profiles
            .get(&net_id)
            .map(|p| p.character_name.as_str())
    }

    pub fn selection_names(&self) -> std::collections::HashMap<NetId, String> {
        let mut names = std::collections::HashMap::new();
        if let (Some(net_id), Some(name)) = (self.our_net_id, self.character_name()) {
            names.insert(net_id, name.to_string());
        }
        for (net_id, profile) in &self.profiles {
            names.insert(*net_id, profile.character_name.clone());
        }
        names
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

    /// Fatal `ServerMsg::Reject` reason, if the server told us to
    /// go away (protocol mismatch, bad credentials in a future
    /// auth pass, etc.). The binary surfaces this to the user and
    /// exits cleanly instead of leaving them staring at a frozen
    /// connect screen.
    pub fn fatal_reject_reason(&self) -> Option<&str> {
        self.fatal_reject_reason.as_deref()
    }
}
