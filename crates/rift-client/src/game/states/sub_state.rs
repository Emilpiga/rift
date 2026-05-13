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
use rift_game::abilities::AbilityWireId;
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
    /// Loadout-slot mutations the binary forwards to the server
    /// as `ClientMsg::SetLoadoutSlot`. Each tuple is
    /// `(slot_index, ability_id)`. The server replies with
    /// `ServerMsg::Loadout`, so we never mutate the local
    /// loadout optimistically.
    pub pending_loadout_changes: Vec<(u8, AbilityWireId)>,
    /// Talent-investment requests the binary forwards as
    /// `ClientMsg::InvestTalent`. The server replies with
    /// `ServerMsg::TalentsSync`; we never mutate the local
    /// tree optimistically.
    pub pending_talent_invests: Vec<u16>,
    /// Lesser-respec requests — refund every rank of a single
    /// talent node. Forwarded as `ClientMsg::RespecTalent`.
    /// Server enforces the orphan-rejection rule
    /// (`TALENT_TREE.md` §7); reply is the fresh
    /// `ServerMsg::TalentsSync`.
    pub pending_talent_respecs: Vec<u16>,
    /// Greater-respec request flag — fires once on press,
    /// drained into `ClientMsg::RespecAllTalents`. A bool not a
    /// counter because spamming the button would just yield
    /// repeat no-ops on an empty tree.
    pub pending_talent_respec_all: bool,
    /// Use-consumable requests — `(inventory_index,
    /// target_arg)`. Drained into `ClientMsg::UseItem`.
    /// `target_arg = u16::MAX` for self-targeted consumables;
    /// for two-step consumables (e.g. `LesserRespecToken`) the
    /// UI fills the target before pushing here.
    pub pending_use_item: Vec<(u32, u16)>,
    /// "Two-step consumable" pick mode \u2014 holds the bag
    /// index of the consumable the player armed (e.g. a
    /// `LesserRespecToken`). While set, the talent panel
    /// enters "choose a node" mode and the next right-click
    /// on an invested talent fires `UseItem` with that node
    /// as `target_arg`. Cleared by Esc, by the talent panel
    /// closing, or by the consumable being consumed.
    pub pending_consume_bag_idx: Option<u32>,
    /// `true` for one frame when the local player F-presses the
    /// rift-spawn portal, asking the binary to fire
    /// `ClientMsg::RiftExitVoteStart`. Server validates +
    /// either short-circuits to a solo exit or broadcasts the
    /// fresh `RiftExitVote` snapshot.
    pub pending_exit_vote_start: bool,
    /// Per-frame queue of exit-vote casts. Each entry is
    /// `true` for Yes / `false` for No. Drained by the binary
    /// into `ClientMsg::RiftExitVoteCast`. A `Vec` rather than
    /// a single `Option` so a fast Y→N double-tap (which we
    /// won't accept anyway, but shouldn't deadlock the queue)
    /// can flush both to the server.
    pub pending_exit_vote_casts: Vec<bool>,
    /// Cache of our authoritative `NetId`, mirrored each frame
    /// from `NetClient::our_net_id()` so gameplay-thread code
    /// (`update_gameplay`, HUD) can identify the local player
    /// without holding a reference to `NetClient`. `None`
    /// before `Welcome` arrives.
    pub our_net_id_cached: Option<rift_net::NetId>,
    /// Mirror of `NetClient::is_local_ghost()`, refreshed each
    /// frame from the binary so gameplay-thread gating
    /// (`is_player_dead` discriminator, ghost-only HUD bits)
    /// can read it without a `NetClient` reference.
    pub local_ghost_cached: bool,
    /// Edge-triggered revive-shrine channel intent the binary
    /// must forward to the server. `Some(payload)` when the
    /// gameplay tick has detected a transition; the binary
    /// `take()`s it and ships `ClientMsg::SetShrineChannel`.
    /// The inner `Option<NetId>` is the new intent (i.e.
    /// `Some(shrine)` to start, `None` to stop).
    pub pending_shrine_intent: Option<Option<rift_net::NetId>>,
    /// Outbound chat lines: `(channel, target, text)`. Filled
    /// by the chat HUD on `Enter` (or `/command` parsing),
    /// drained by the binary into
    /// `NetClient::send_chat`. `target` is meaningful only for
    /// the whisper channel.
    pub pending_chats_out: Vec<(u8, Option<String>, String)>,
    /// Outbound party-control commands: invite/accept/decline
    /// /leave/kick/promote. Each entry is a fully-formed
    /// `ClientMsg::Party*` ready for the binary to ship on
    /// the Control channel. Filled by the chat slash-command
    /// parser and the right-click party-frame context menu.
    pub pending_party_msgs: Vec<rift_net::messages::ClientMsg>,
    /// Outbound rift-entry proposal from the portal modal.
    /// `(start_floor, mode)` — `mode` is one of
    /// `rift_net::messages::party_mode::*`. Drained by the
    /// binary into `ClientMsg::ProposeRiftEntry`.
    pub pending_propose_rift_entry: Option<(u32, u8)>,
    /// Outbound per-member portal-confirm reply. `Some(true)`
    /// for accept, `Some(false)` for decline. Drained by the
    /// binary into `ClientMsg::PortalConfirm`.
    pub pending_portal_confirm: Option<bool>,
    /// Edge-triggered request to open the local portal modal.
    /// Set when the player walks up to the hub portal and
    /// presses F. The UI phase reads + clears it; the modal
    /// itself is owned by `GameState.party`.
    pub pending_open_portal_modal: bool,
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
    pub ability_id: AbilityWireId,
    pub origin: Vec3,
    pub aim_dir: Vec3,
    pub placed_target: Option<Vec3>,
    /// Friendly entity target for heal-style casts. `None` for
    /// ability kinds that don't use it.
    pub target_net_id: Option<rift_net::NetId>,
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
    pub ability_id: AbilityWireId,
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
    pub ability_id: AbilityWireId,
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
    /// True once a frame has hit the `is_local` branch — i.e.
    /// the local player owns this visual *and* had a matching
    /// `state.channel.active` row at least once. Drives the
    /// `local_release_pending` short-circuit in
    /// `tick_channel_visuals`: we only collapse a visual on
    /// "local channel just released" when we actually saw the
    /// local channel running for it (otherwise short
    /// transform-driven beams — Embercrown's Fireball Beam,
    /// which never sets `state.channel.active` because it's a
    /// finite-duration channel — would be killed on their
    /// first frame).
    pub saw_local_active: bool,
    /// Accumulator for the impact-burst cadence (Frost Ray spawns
    /// `frost_impact` at every pierced target every ~0.10 s rather
    /// than every frame, to keep the particle count bounded).
    pub impact_acc: f32,
    /// Pulse travel duration (seconds) for transforms whose
    /// finisher is telegraphed by a bead riding the beam from
    /// caster to terminus (currently `FrostRayShatter`). `0.0`
    /// when no pulse is active. Set on `WorldEvent::ChannelPulse`,
    /// cleared once `pulse_t` reaches it (the server emits the
    /// next `ChannelPulse` to start the following cycle).
    pub pulse_travel_time: f32,
    /// Seconds elapsed in the current pulse cycle. Frame-stepped
    /// in `tick_channel_visuals`. Drives the bead's lerp
    /// fraction `pulse_t / pulse_travel_time`.
    pub pulse_t: f32,
    /// Cadence accumulator for the bead's per-frame spark
    /// emission. Same pattern as `impact_acc` — the bead
    /// re-emits a small frost burst every ~0.04 s along its
    /// path so the trail reads as continuous without
    /// spawning a particle every frame.
    pub pulse_emit_acc: f32,
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
    /// authoritative deposit / withdraw / tab edit). Empty
    /// until the player opens the chest. One [`StashTabClient`]
    /// per page; items inside each tab are sparse — `None`
    /// slots represent gaps the player has carved out via
    /// drag-and-drop.
    pub stash_tabs: Vec<StashTabClient>,
    /// Stash transfer requests (deposit / withdraw) the binary
    /// forwards to the server. Drained per frame.
    pub pending_stash_requests: Vec<StashRequest>,
    /// Local mirror of the server-side per-player `stash_open`
    /// flag. Toggled by [`super::stash_system::tick`] when the
    /// player presses F near the hub chest, mirrored to the
    /// server via `NetState::stash_session_requests`. Gates
    /// stash bag-click semantics (deposit vs equip) and forces
    /// the inventory panel open in the UI layer; panel
    /// visibility itself stays in [`super::mp_inventory_ui`].
    pub stash_session: bool,
    /// Set when `equipment` was rewritten by an `EquipmentSync`
    /// but the local-player avatar's modular outfit attachments
    /// haven't been refreshed yet. Consumed by the binary's
    /// frame loop, which re-runs `apply_local_equipment_visuals`
    /// once a `LocalPlayer` entity exists. Necessary because
    /// the first `EquipmentSync` arrives during `EnteringWorld`
    /// — before the avatar is spawned — so a one-shot apply on
    /// receive would silently no-op.
    pub equipment_visuals_dirty: bool,
}

/// Client-side mirror of one stash tab. Built fresh from each
/// `ServerMsg::StashSync`. Items use the runtime
/// `rift_game::loot::Item` type; the wire-blob conversion
/// happens once in `NetClient`'s `apply_stash_sync` path.
#[derive(Clone, Debug, Default)]
pub struct StashTabClient {
    pub name: String,
    /// Packed `0xRRGGBB` (alpha implicit, opaque).
    pub color: u32,
    pub items: Vec<Option<rift_game::loot::Item>>,
}

impl LootClientState {
    /// Wipe per-floor loot state on a regen. The actual
    /// `items` / `equipment` mirrors and the outbound request
    /// queues are cross-floor and stay untouched; only floor-
    /// scoped visuals + the stash session flag are cleared.
    /// VFX emitters owned by `drops` are invalidated by
    /// `renderer.vfx_system.clear_all()` upstream, so this just
    /// drops bookkeeping.
    pub fn reset_for_floor(&mut self) {
        self.drops.clear();
        self.pending_pickups.clear();
        self.claimed_ids.clear();
        // Stash UI must close on transition: the chest only
        // exists in the hub, and a stale "stash open" flag
        // would cause bag clicks to deposit into nothing.
        self.stash_session = false;
        self.stash_tabs.clear();
    }
}

/// Outgoing stash transfer request shape, queued by the
/// inventory UI and drained by the binary into
/// `NetClient::request_deposit_to_stash` /
/// `NetClient::request_withdraw_from_stash`. Open / close are
/// handled separately by the proximity prompt; only the
/// item-movement and tab-management events flow through here.
#[derive(Clone, Debug)]
pub enum StashRequest {
    Deposit {
        inventory_index: u32,
        tab_index: u8,
    },
    Withdraw {
        tab_index: u8,
        stash_index: u32,
    },
    /// Reorder stash: swap two stash slots in place. Either
    /// index may be empty (past the current stash length); the
    /// server grows the tab with `None` placeholders to fit
    /// and then trims back to the last filled slot.
    Swap {
        tab_index: u8,
        a: u32,
        b: u32,
    },
    /// Deposit an inventory item into a specific stash slot.
    /// If the destination is occupied the two items swap
    /// (occupant comes back to `inventory_index`); if empty,
    /// the item simply moves into the requested slot.
    DepositToSlot {
        inventory_index: u32,
        tab_index: u8,
        stash_index: u32,
    },
    /// Withdraw a stash item into a specific bag slot.
    /// Same swap semantics as `DepositToSlot`.
    WithdrawToSlot {
        tab_index: u8,
        stash_index: u32,
        inventory_index: u32,
    },
    /// Equip a stash item directly into its canonical
    /// equipment slot, swapping any displaced item back into
    /// the freed stash cell.
    EquipFromStash {
        tab_index: u8,
        stash_index: u32,
    },
    /// Unequip the item in `slot` directly into a specific
    /// stash cell (mirror of `EquipFromStash`).
    UnequipToStashSlot {
        slot: u8,
        tab_index: u8,
        stash_index: u32,
    },
    /// Buy a new stash tab with shards. Server validates cost
    /// and tab cap; on success the new tab is appended at the
    /// end and the player's shard total drops accordingly.
    BuyTab,
    /// Rename a stash tab.
    RenameTab {
        tab_index: u8,
        name: String,
    },
    /// Recolor a stash tab. `color` is packed `0xRRGGBB`.
    RecolorTab {
        tab_index: u8,
        color: u32,
    },
    /// Auto-sort one stash tab.
    SortTab {
        tab_index: u8,
    },
}

/// Outgoing equip / unequip request shape, queued by the
/// inventory UI and drained by the binary into
/// `NetClient::request_equip` / `request_unequip`.
#[derive(Clone, Copy, Debug)]
pub enum EquipRequest {
    Equip {
        inventory_index: u32,
    },
    Unequip {
        slot: u8,
    },
    /// Reorder bag: swap two inventory slots in place.
    SwapBag {
        a: u32,
        b: u32,
    },
    /// Drop the bag item out onto the ground at the player's
    /// position (server picks the spawn point).
    DropToWorld {
        inventory_index: u32,
    },
    /// Drop an equipped item directly onto the ground.
    /// Mirrors [`Self::DropToWorld`] but for the equipment
    /// slot side.
    DropEquipToWorld {
        slot: u8,
    },
    /// Salvage the bag item for shards. Server validates the
    /// item isn't anchored, removes it from the bag, and
    /// pushes back fresh `InventorySync` + `ShardsSync`.
    Salvage {
        inventory_index: u32,
    },
    /// Bulk-salvage every non-anchored bag item whose rarity is
    /// at most `rarity_max`. Wired to the inventory panel's
    /// "Salvage Trash" button (with a 2-stage confirm).
    SalvageBulk {
        rarity_max: u8,
    },
    /// Unequip into a specific bag index. The previous occupant
    /// of that slot is shoved into `slot` if it fits, otherwise
    /// appended at the end of the bag.
    UnequipToSlot {
        slot: u8,
        inventory_index: u32,
    },
    /// Swap two equipment slots directly. Only ring1 ↔ ring2
    /// is legal today; the server validates both
    /// `Equipment::accepts` directions before applying.
    SwapEquip {
        a: u8,
        b: u8,
    },
    /// Auto-sort the bag in place. Server compacts items by
    /// rarity desc, ilvl desc, footprint area desc.
    SortBag,
    /// Use the consumable bag item at `inventory_index` with
    /// `target_arg` (`u16::MAX` for self-targeted kinds, or
    /// e.g. a `TalentId` for `LesserRespecToken`). Drained
    /// into `ClientMsg::UseItem`.
    UseConsumable {
        inventory_index: u32,
        target_arg: u16,
    },
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
    /// VFX handle for the gold-cyan halo spawned only when
    /// `item.anchored` is true. `None` for ordinary drops.
    pub anchored_emitter: Option<rift_engine::renderer::vfx::EffectId>,
    /// Optional 3D bind-pose mesh laid on the ground. `None`
    /// when the item's [`rift_game::loot::BaseItem::models`]
    /// has no art for the local player's gender (or the file
    /// failed to decode); in that case the pillar / base /
    /// halo VFX above are the only visual.
    pub ground_mesh: Option<LootGroundMesh>,
}

/// State for a loot drop's on-ground 3D model: the renderer
/// object slot, the ground-rest position the animation settles
/// onto, and the current animation phase. Spawned at
/// `anim_t = 0` and ticked by
/// [`crate::game::loot_system::tick_drop_animation`].
#[derive(Debug)]
pub struct LootGroundMesh {
    /// Renderer object index returned by `add_dynamic_mesh`.
    /// We never update vertices on it — it stays in bind pose
    /// — but the renderer still requires a dynamic slot to
    /// expose the per-frame `model_matrix` for animation.
    pub object_index: usize,
    /// Rest position the pop animation settles onto (slightly
    /// above the spawn point so the model doesn't z-fight with
    /// the floor).
    pub rest_position: glam::Vec3,
    /// Constant scale applied to the model so the bind-pose
    /// mesh lands at a recognisable on-ground size regardless
    /// of how the artist authored the source bounds.
    pub base_scale: f32,
    /// Yaw the model rests at, randomised per drop so chains
    /// of identical drops don't all face the same way.
    pub rest_yaw: f32,
    /// Mesh-local AABB min, captured at spawn so the per-frame
    /// transform can centre the visual centroid under the loot
    /// beam and lift the lowest point above the floor without
    /// re-fetching the model cache every tick.
    pub bounds_min: glam::Vec3,
    /// Mesh-local AABB max, paired with [`Self::bounds_min`].
    pub bounds_max: glam::Vec3,
    /// Animation timer in seconds. Drives the pop-up arc and
    /// scale-in tween for the first `POP_DURATION` seconds, then
    /// holds the rest pose with a slow ambient bob.
    pub anim_t: f32,
}

/// Client-side mirror of every revive-shrine row currently
/// replicated from the server. Visual emitters are owned here
/// so floor changes / channel completions can despawn cleanly.
#[derive(Default)]
pub struct ShrineClientState {
    pub visuals: Vec<crate::game::shrine_system::ShrineVisual>,
    /// `Some(shrine_id)` while the *local* player is intending
    /// to channel that shrine. Mirror of the server's
    /// `channeling_shrine` we toggle optimistically on F-press
    /// so the HUD prompt swaps to "STOP CHANNELING" without a
    /// snapshot roundtrip. Cleared automatically when the
    /// shrine row drops out of the snapshot.
    pub local_intent: Option<rift_net::NetId>,
    /// Active beam emitter id while channeling. Spawned on the
    /// edge `local_intent` flips from None -> Some, despawned
    /// on the inverse edge or when the shrine despawns.
    pub channel_beam: Option<rift_engine::renderer::vfx::EffectId>,
    /// `prev_local_intent` snapshot from the previous frame.
    /// Used to edge-detect channel start / stop in
    /// `shrine_system::tick_channel_pose` so the SpellCast pose
    /// + beam are toggled exactly once per transition.
    pub prev_local_intent: Option<rift_net::NetId>,
}

impl ShrineClientState {
    /// Wipe per-floor shrine state on a regen. VFX emitters
    /// owned by `visuals` + `channel_beam` are invalidated by
    /// `renderer.vfx_system.clear_all()` upstream.
    pub fn reset_for_floor(&mut self) {
        self.visuals.clear();
        self.local_intent = None;
        self.prev_local_intent = None;
        self.channel_beam = None;
    }
}
