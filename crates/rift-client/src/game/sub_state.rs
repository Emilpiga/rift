//! Sub-state buckets for [`super::state::GameState`].
//!
//! Lifted out of `state.rs` to keep the orchestrator file
//! readable. Each bucket groups one concern:
//!
//! - [`LoadingState`] — staged init progress (icons, monsters).
//! - [`NetState`] — outbound / inbound multiplayer plumbing.
//! - [`ChannelState`] — local hold-to-channel + beam visuals.
//! - [`LootClientState`] — server-mirrored loot pillars,
//!   inventory bag, and pending equip requests.
//!
//! The supporting message / visual types these contain
//! (`LoadPhase`, `NetCastRequest`, `ActiveChannel`,
//! `ChannelVisual`, `EquipRequest`, `NetTransitionRequest`,
//! `LootDropVisual`) live here too so a single `use
//! super::sub_state::*` brings the whole bag in.

use glam::Vec3;
use rift_game::character;

/// Where we are in the per-frame staged init.
#[derive(Default)]
pub struct LoadingState {
    pub phase: LoadPhase,
    pub monster_index: usize,
}

/// Stages of `GameState::load_step`. Floor + outfits + walls happen
/// later, once the player has picked a character.
#[derive(Default)]
pub enum LoadPhase {
    /// Stream icon PNGs into the overlay atlas a few per frame
    /// so the loading screen stays responsive while hundreds of
    /// images are decoded + resampled.
    #[default]
    Icons,
    /// Pre-load skinned monster glTFs (one role per call).
    Monsters,
    /// Loading complete; subsequent calls return `Done` immediately.
    Done,
}

/// Multiplayer traffic plumbing. Filled by `GameState` methods, drained
/// each frame by the client binary's net loop.
#[derive(Default)]
pub struct NetState {
    /// Server-supplied dungeon seed (matches `LoadFloor.seed`).
    pub floor_seed: Option<u64>,
    /// SP-suppressed transition request.
    pub transition: Option<NetTransitionRequest>,
    /// Ability casts the local player wants to fire.
    pub casts: Vec<NetCastRequest>,
    /// One-shot cosmetic profile advertisement.
    pub profile: Option<character::CharacterProfile>,
    /// Account name for the picked character, drained alongside
    /// `profile`.
    pub account_name: Option<String>,
    /// Account name confirmed on the account-entry screen, drained
    /// independently of `profile` so the binary can fire
    /// `RequestRoster`.
    pub roster_request: Option<String>,
    /// Stash session toggle requests, drained per frame. `true`
    /// fires `OpenStash`, `false` fires `CloseStash`. Multiple
    /// queued events are forwarded in order so a quick
    /// open-then-close double-tap survives.
    pub stash_session_requests: Vec<bool>,
}

/// Multiplayer-only: a request for the binary to forward to the server.
#[derive(Clone, Copy, Debug)]
pub enum NetTransitionRequest {
    EnterRift,
    ReturnToHub,
}

/// Multiplayer ability cast request, queued locally and shipped to
/// the server next frame.
#[derive(Clone, Copy, Debug)]
pub struct NetCastRequest {
    pub ability_id: u8,
    pub origin: Vec3,
    pub aim_dir: Vec3,
    pub placed_target: Option<Vec3>,
}

/// Local channel-ability state (hold-to-channel input + visuals).
#[derive(Default)]
pub struct ChannelState {
    /// Currently-channeling ability, if any.
    pub active: Option<ActiveChannel>,
    /// Active beam / sweep visuals driven by `WorldEvent::ChannelTick`.
    pub visuals: Vec<ChannelVisual>,
    /// Channel-end requests the binary forwards to the server.
    pub pending_ends: Vec<u8>,
}

/// Locally-tracked channel state. We keep this client-side so the
/// hold-to-channel input loop can detect button release / movement
/// without round-tripping the server, and so the cast clip stays
/// looping for the channel's expected duration.
#[derive(Clone, Copy, Debug)]
pub struct ActiveChannel {
    /// Wire ability id of the channel ability in flight.
    pub ability_id: u8,
    /// Which action-bar slot the player is holding. Used to decide
    /// which input edge (left-click vs Digit1..5) ends the channel.
    pub slot_index: usize,
    /// Whether the ability cancels on movement input (mirrors the
    /// server flag so the client agrees with the server about when
    /// to send `EndChannel`).
    pub cancel_on_move: bool,
    /// Seconds remaining before we time the channel out locally.
    /// Server is authoritative — this is just so the client tears
    /// down its own state if `WorldEvent::ChannelEnd` is dropped.
    pub remaining: f32,
}

/// Per-channel visual (e.g. Frost Ray's beam). Spawned lazily on
/// the first `ChannelTick` for a given caster+ability and torn down
/// on `ChannelEnd` (or after a short idle timeout if ticks stop).
#[derive(Debug)]
pub struct ChannelVisual {
    /// Channeling caster (network id).
    pub caster: rift_net::NetId,
    /// Wire id of the ability driving the visual.
    pub ability_id: u8,
    /// Most recently reported caster position (chest-height-ish; we
    /// bias the beam upward in `update` so it leaves the hand).
    pub position: Vec3,
    /// Most recently reported aim direction (XZ unit vector with Y=0).
    pub aim: Vec3,
    /// Seconds since the last tick. Used to fade the visual out if
    /// ticks stop arriving without an explicit `ChannelEnd`.
    pub idle: f32,
    /// Renderer object index for the legacy beam mesh, allocated
    /// lazily on the first `update` frame after spawn. `None` for
    /// abilities that route their visuals through the declarative
    /// VFX system (Frost Ray uses [`Self::vfx_id`] instead).
    pub obj_idx: Option<usize>,
    /// Live VFX effect handle for ribbon-based channel visuals.
    /// Spawned lazily on first frame, despawned when the visual
    /// expires. `None` for abilities that still drive their beam
    /// through the legacy `Mesh::light_beam` path.
    pub vfx_id: Option<rift_engine::renderer::vfx::EffectId>,
    /// Set by `clear_channel_visual` when the server sends
    /// `ChannelEnd`. The next `update` frame zeros the mesh's
    /// model matrix and drops the entry.
    pub ending: bool,
    /// Accumulator for the impact-burst cadence (Frost Ray spawns
    /// `frost_impact` at every pierced target every ~0.10 s rather
    /// than every frame, to keep the particle count bounded).
    pub impact_acc: f32,
}

/// Server-mirrored loot state — visual pillars, pickup queue,
/// inventory bag.
#[derive(Default)]
pub struct LootClientState {
    /// Active ground-loot pillars (one per visible drop).
    pub drops: Vec<LootDropVisual>,
    /// Loot drops the local player has asked to pick up.
    pub pending_pickups: Vec<rift_net::NetId>,
    /// Loot ids that have been claimed (by anyone) this floor.
    pub claimed_ids: std::collections::HashSet<rift_net::NetId>,
    /// Local mirror of the server-authoritative inventory.
    pub items: Vec<Option<rift_game::loot::Item>>,
    /// Local mirror of the server-authoritative equipped set.
    /// Contributes to `PlayerState::stats` via
    /// `Equipment::active_affix_sum`. Rebuilt wholesale on every
    /// `ServerMsg::EquipmentSync`.
    pub equipment: rift_game::loot::Equipment,
    /// Equip / unequip requests the binary forwards to the
    /// server. Drained per frame; the server replies with fresh
    /// `InventorySync` + `EquipmentSync` so we never mutate the
    /// local mirror optimistically.
    pub pending_equip_requests: Vec<EquipRequest>,
    /// Local mirror of the per-character private stash. Replaced
    /// wholesale on every `ServerMsg::StashSync` (which the
    /// server sends in reply to `OpenStash` and after every
    /// authoritative deposit / withdraw). Empty until the player
    /// opens the chest. Sparse — `None` slots represent gaps
    /// the player has carved out via drag-and-drop.
    pub stash_items: Vec<Option<rift_game::loot::Item>>,
    /// Stash transfer requests (deposit / withdraw) the binary
    /// forwards to the server. Drained per frame.
    pub pending_stash_requests: Vec<StashRequest>,
}

/// Outgoing stash transfer request shape, queued by the
/// inventory UI and drained by the binary into
/// `NetClient::request_deposit_to_stash` /
/// `NetClient::request_withdraw_from_stash`. Open / close are
/// handled separately by the proximity prompt; only the
/// item-movement events flow through here.
#[derive(Clone, Copy, Debug)]
pub enum StashRequest {
    Deposit { inventory_index: u32 },
    Withdraw { stash_index: u32 },
    /// Reorder stash: swap two stash slots in place. Either
    /// index may be empty (past the current stash length); the
    /// server grows the stash with `None` placeholders to fit
    /// and then trims back to the last filled slot.
    Swap { a: u32, b: u32 },
}

/// Outgoing equip / unequip request shape, queued by the
/// inventory UI and drained by the binary into
/// `NetClient::request_equip` / `request_unequip`.
#[derive(Clone, Copy, Debug)]
pub enum EquipRequest {
    Equip { inventory_index: u32 },
    Unequip { slot: u8 },
    /// Reorder bag: swap two inventory slots in place.
    SwapBag { a: u32, b: u32 },
    /// Drop the bag item out onto the ground at the player's
    /// position (server picks the spawn point).
    DropToWorld { inventory_index: u32 },
    /// Unequip into a specific bag index. The previous occupant
    /// of that slot is shoved into `slot` if it fits, otherwise
    /// appended at the end of the bag.
    UnequipToSlot { slot: u8, inventory_index: u32 },
}

/// Visual + bookkeeping for a single ground-loot drop. Spawned by
/// `spawn_loot_drop_visual`; consumed by the pickup path which
/// translates the held `Item` into an inventory add and stops the
/// visual.
#[derive(Debug)]
pub struct LootDropVisual {
    /// Server-allocated loot id. Used for `PickUpLoot` requests
    /// and for de-duping when a drop arrives via both the
    /// `LootDropped` event and a snapshot `EntityKind::Loot` row.
    pub net_id: rift_net::NetId,
    pub position: Vec3,
    /// The fully-rolled item held by the drop. Cloned out on
    /// pickup; until then it just drives the visual's tier color
    /// and a hover-tooltip later.
    pub item: rift_game::loot::Item,
    /// VFX handle for the pillar of light.
    pub pillar_emitter: rift_engine::renderer::vfx::EffectId,
    /// VFX handle for the bright base pulse.
    pub base_emitter: rift_engine::renderer::vfx::EffectId,
}
