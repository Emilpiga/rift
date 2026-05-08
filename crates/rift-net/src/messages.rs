//! Wire-format messages.
//!
//! Phase 1 deliberately keeps the field sets minimal: enough to
//! round-trip a handshake, an input command, and a "here's where the
//! players are" snapshot. Each variant is documented with a TODO when
//! it's a placeholder we'll grow in later phases.
//!
//! ### Quantization
//!
//! Most fields are sent as `f32` for now. We will tighten these into
//! quantized integer fields (i16 positions, u8 health %, etc.) in a
//! later phase once the schema has settled — premature quantization
//! makes the codec hard to evolve.

use crate::ids::{ClientId, NetId, NetTick};
use serde::{Deserialize, Serialize};

/// Maximum number of bag slots a player can carry. Mirrored on the
/// client UI (`mp_inventory_ui.rs` lays out a 5×6 grid). The server
/// enforces this on every `PickUpLoot`; the client checks it locally
/// to avoid a wasted round-trip and to surface an instant warning.
pub const INVENTORY_CAPACITY: usize = 30;

/// Time (in seconds) every living player must keep their
/// `ToggleShrineChannel` intent active while standing within
/// the shrine's interact radius before the revive triggers.
/// Shared by server (channel tick) and client (HUD bar).
pub const SHRINE_CHANNEL_DURATION: f32 = 3.0;

/// Interact radius of a revive shrine in world units. The server
/// uses this to validate `ToggleShrineChannel` requests and to
/// auto-cancel a player's channel intent when they walk out;
/// the client mirrors it for the F-prompt range check.
pub const SHRINE_INTERACT_RADIUS: f32 = 2.5;

/// Why the server refused a [`ClientMsg::PickUpLoot`]. Sent back to
/// the requesting client only (loot stays on the ground for everyone
/// else). Today there's a single reason; the enum exists so we can
/// grow it (range, ownership window, weight, …) without a wire break.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PickupRejectReason {
    /// Picker's bag has [`INVENTORY_CAPACITY`] filled slots.
    InventoryFull,
}

// ─── Client → Server ─────────────────────────────────────────────────────

/// Anything the client sends to the server.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientMsg {
    /// First message after the renet connection comes up. Carries the
    /// authoritative profile the client wants to play as. Server
    /// validates against the player's account (TODO: auth) and
    /// responds with [`ServerMsg::Welcome`] or [`ServerMsg::Reject`].
    Hello {
        protocol_version: u16,
        /// Account display name (UTF-8, <=18 chars). Server uses
        /// this to look up / create the persistent `accounts`
        /// row that owns this client's character roster. For now
        /// it doubles as a (very) light-weight identity — real
        /// auth lands in a later phase.
        account_name: String,
        /// Display name (UTF-8, <=18 chars per character_select). Final
        /// authority lies with the server's account record; this is
        /// what the client *thinks* it's playing as.
        character_name: String,
        /// Class id as a stable `&'static str` from rift-game's
        /// `classes` module. We pass it as `String` on the wire so
        /// rift-net stays decoupled from rift-game's content tables.
        class_id: String,
        /// Cosmetic gender choice (drives base mesh).
        gender: Gender,
    },

    /// Per-frame coalesced input. Sent at the client's render rate
    /// (capped to ~60 Hz). `seq` is monotonic — server acks the
    /// highest seq it has applied via [`ServerMsg::Snapshot::ack_seq`].
    Input(InputCmd),

    /// Request to pick up a loot drop. Server validates range +
    /// availability, broadcasts [`ServerMsg::LootClaimed`] on success.
    PickUpLoot { net_id: NetId },

    /// Heartbeat / keepalive when the client is otherwise idle. Renet
    /// sends its own keepalive but this also lets the server track
    /// "client knows about tick N" for ack purposes.
    Ack { last_received_tick: NetTick },

    /// Clean disconnect. Optional — the renet layer detects drops on
    /// its own — but lets the server log a friendly reason.
    Goodbye,

    /// Player asked to leave the hub and start the rift (or, once
    /// inside, advance to the next floor). The server is the
    /// authority on whether the request is currently valid; it
    /// responds by broadcasting [`ServerMsg::LoadFloor`] to the
    /// whole session if it accepts.
    RequestEnterRift,

    /// Player asked to return to the safe hub (e.g. via a "leave
    /// rift" portal or after a death respawn). Same shape as
    /// [`ClientMsg::RequestEnterRift`].
    RequestReturnToHub,

    /// Player wants to fire an ability. Server is the authority on
    /// whether the request is honoured (cooldowns, range, line of
    /// sight, etc.) and on every outcome (projectile spawn, damage
    /// dealt, debuffs applied). Reliable on `Channel::Event` so
    /// nothing gets dropped under packet loss.
    CastAbility {
        /// Ability id. Stable u8 enum shared with the server (see
        /// `rift_server::sim::ability` and the matching client
        /// table in `rift_game::abilities::wire_id`).
        ability_id: u8,
        /// World-space cast origin. Client passes its current
        /// player position; server uses the simulated position so
        /// it's always anchored even if the client lied.
        origin: [f32; 3],
        /// Horizontal aim direction, unit-length. Used for
        /// projectile direction and instant-cast aim.
        aim_dir: [f32; 2],
        /// Optional reticle target for placed AoE abilities.
        /// `None` for instant casts.
        placed_target: Option<[f32; 3]>,
        /// Optional entity target for friendly single-target
        /// casts (heals). The server validates the target is
        /// alive, on the same team, in range, and has line of
        /// sight; rejected casts are silently dropped (no
        /// cooldown burned). `None` for non-targeted abilities
        /// — the server ignores it for kinds that don't need
        /// it.
        target_net_id: Option<NetId>,
    },

    /// Player released the action button or moved while channeling.
    /// Server cancels the matching active channel (if any). Reliable
    /// on `Channel::Event` so a dropped release doesn't lock the
    /// caster into the channel for its full duration.
    EndChannel {
        ability_id: u8,
    },

    /// Pre-Hello account roster lookup. Sent right after the
    /// renet connection comes up so the client can populate the
    /// character-select screen with the account's characters.
    /// Server replies with [`ServerMsg::Roster`] (or
    /// [`ServerMsg::Reject`] if the persistence layer is
    /// unreachable).
    RequestRoster {
        account_name: String,
    },

    /// Move the item at `inventory_index` (into the bag mirror
    /// the client renders) into its default equipment slot. The
    /// server validates the index, picks the canonical slot via
    /// `Equipment::default_slot`, swaps in any previously-equipped
    /// item, and replies with both a fresh `InventorySync` and an
    /// `EquipmentSync` so client mirrors stay coherent. Reliable
    /// on `Channel::Control` so a dropped equip never silently
    /// loses the item.
    EquipItem {
        inventory_index: u32,
    },

    /// Move whatever's currently in `slot` back into the bag.
    /// Server replies with the same dual sync as `EquipItem`. No-op
    /// (silently dropped) if the slot is empty or the byte doesn't
    /// match a known [`rift_game::loot::EquipSlot`].
    UnequipItem {
        slot: u8,
    },

    /// Ask the server to start a stash session. Server validates
    /// the player is in the hub and within interact range of the
    /// chest, marks the session as "stash open", and replies with
    /// a fresh [`ServerMsg::StashSync`]. Reliable on
    /// `Channel::Control`.
    OpenStash,

    /// Ask the server to end the current stash session. Future
    /// `DepositToStash` / `WithdrawFromStash` are rejected until
    /// a fresh `OpenStash` succeeds. Reliable on `Channel::Control`.
    CloseStash,

    /// Move the bag item at `inventory_index` into the stash.
    /// Server validates the index + that a stash session is open,
    /// then replies with both a fresh `InventorySync` and a fresh
    /// `StashSync`. Reliable on `Channel::Control`.
    DepositToStash {
        inventory_index: u32,
    },

    /// Like `DepositToStash` but moves the item into a specific
    /// stash slot. If the slot is already occupied the two items
    /// swap (the previous stash occupant is placed back into
    /// `inventory_index`). Reliable on `Channel::Control`.
    DepositToStashSlot {
        inventory_index: u32,
        stash_index: u32,
    },

    /// Move the stash item at `stash_index` back into the bag.
    /// Server validates the index + that a stash session is open,
    /// then replies with both a fresh `InventorySync` and a fresh
    /// `StashSync`. Reliable on `Channel::Control`.
    WithdrawFromStash {
        stash_index: u32,
    },

    /// Like `WithdrawFromStash` but places the item into a
    /// specific bag slot. Same swap semantics as
    /// `DepositToStashSlot`. Reliable on `Channel::Control`.
    WithdrawFromStashSlot {
        stash_index: u32,
        inventory_index: u32,
    },

    /// Reorder the bag: swap the items at `a` and `b` (either may
    /// be an empty slot, in which case the filled item moves into
    /// the empty cell). Server replies with a fresh
    /// `InventorySync`. Reliable on `Channel::Control`.
    SwapInventorySlots {
        a: u32,
        b: u32,
    },

    /// Reorder the stash: swap the items at `a` and `b` (either
    /// may be an empty slot, in which case the filled item moves
    /// into the empty cell). Server validates a stash session is
    /// open and replies with a fresh `StashSync`. Reliable on
    /// `Channel::Control`.
    SwapStashSlots {
        a: u32,
        b: u32,
    },

    /// Drop the bag item at `inventory_index` onto the ground at
    /// the picker's current position. Server removes the row from
    /// the bag, spawns a `ServerLoot` entity, and pushes the
    /// usual `WorldEvent::LootDropped` so every observer's loot
    /// pillar appears. Replies with a fresh `InventorySync` to
    /// the picker. Reliable on `Channel::Control`.
    DropInventoryItem {
        inventory_index: u32,
    },

    /// Take whatever's currently in `slot` and place it into the
    /// bag at `inventory_index` (extending the bag if the index
    /// is past the end). Used by the inventory UI's drag-and-drop
    /// path so the user can pick the destination slot, instead of
    /// always appending to the end as `UnequipItem` does. Server
    /// replies with fresh `InventorySync` + `EquipmentSync`.
    UnequipToBagSlot {
        slot: u8,
        inventory_index: u32,
    },

    /// Mutate one slot of the player's persisted ability loadout.
    /// `slot_index` is the action-bar slot (0..6); `ability_id`
    /// is the wire id of the ability to put there. Server
    /// validates the ability is player-castable, updates its
    /// authoritative `ServerPlayer.loadout`, persists, and
    /// replies with [`ServerMsg::Loadout`] so every client
    /// stays in sync with what the server thinks is equipped.
    /// Reliable on `Channel::Control`.
    SetLoadoutSlot {
        slot_index: u8,
        ability_id: u8,
    },

    /// Open the rift exit vote. Sent when a living player
    /// presses F at the rift-spawn portal. Server validates the
    /// caster is alive and on a non-hub floor, with no vote
    /// already active and no cooldown remaining; on success it
    /// either:
    /// - **Solo:** instantly transitions the party to the hub
    ///   (with loot wipe for any ghosts).
    /// - **Multiplayer:** opens a 15s vote window, auto-records
    ///   the initiator as `Yes`, and broadcasts
    ///   [`ServerMsg::RiftExitVote`] to every player on the
    ///   floor so HUD panels light up. Ghosts (dead players)
    ///   don't vote — the threshold is unanimous YES from
    ///   *living* players.
    /// Reliable on `Channel::Control`.
    RiftExitVoteStart,

    /// Cast a vote on the currently-active rift exit vote. Sent
    /// when a living player presses Y or N. Silently dropped if
    /// no vote is active, the caster is dead, or the caster has
    /// already voted (no changing your mind). Reliable on
    /// `Channel::Control`.
    RiftExitVoteCast { yes: bool },

    /// Set the local player's revive-shrine channel intent.
    /// `Some(shrine)` means "I am holding F within range of
    /// this shrine right now"; `None` means "I released F /
    /// walked out of range / I'm not channeling anything."
    /// The client edge-triggers this whenever its computed
    /// intent changes (key transitions or range transitions),
    /// so the wire traffic stays sparse. Server validates
    /// alive + within radius before accepting `Some`. The
    /// channel itself only ticks while every living player on
    /// the floor has matching `Some` intent. Reliable on
    /// `Channel::Control`.
    SetShrineChannel { shrine: Option<NetId> },

    /// Player typed a chat message. Server validates length /
    /// rate limit, routes to the right recipient set based on
    /// `channel`, and replies (per-recipient) with
    /// [`ServerMsg::Chat`]. `target` is meaningful only for
    /// [`chat_channel::WHISPER`] (recipient character name);
    /// every other channel ignores it.
    ///
    /// Reliable on `Channel::Control` so a dropped chat line
    /// never silently disappears. Length cap and rate-limit
    /// rejections are silent today — future revision can add
    /// a [`ServerMsg::Chat`] system reply describing the
    /// rejection if useful.
    ChatSend {
        /// Wire id from [`chat_channel`] picking the routing
        /// scope (global / hub / floor / party / whisper).
        /// `chat_channel::SYSTEM` is server-emit-only; clients
        /// sending it are silently dropped.
        channel: u8,
        /// Recipient's character name for `WHISPER`. `None`
        /// for every other channel; if `Some` on a non-
        /// whisper channel the server ignores it.
        target: Option<String>,
        /// UTF-8 message body. Server clamps to
        /// [`CHAT_MAX_LEN`] characters before re-broadcast.
        text: String,
    },
}

/// One voter's choice in an active [`VoteState`]. `Pending` means
/// the player has been included in the vote roll but hasn't cast
/// yes or no yet. Wire stable: don't reorder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VoteChoice {
    Pending,
    Yes,
    No,
}

/// What an active [`VoteState`] is asking the party to decide.
/// Drives the HUD title + the resolution path on the server
/// (Exit → transition to hub; Descend → transition to the next
/// rift floor). Wire stable: don't reorder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VoteKind {
    /// Leave the rift and return to the hub. Initiated by F at
    /// the rift-spawn portal.
    Exit,
    /// Descend to the next rift floor. Initiated by F at the
    /// boss-room exit portal once the floor is complete.
    Descend,
}

/// Snapshot of the rift exit vote, broadcast on
/// [`ServerMsg::RiftExitVote`] whenever the underlying state
/// changes (vote opened, vote cast, vote resolved). Sent to every
/// connected client (not just players on the rift floor) so the
/// HUD comes up cleanly even for players who join mid-vote.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VoteState {
    /// What this vote is asking the party to decide. Drives
    /// the HUD panel title and the server resolution path.
    pub kind: VoteKind,
    /// `true` while the 15s window is open. `false` when the
    /// vote is idle (cooldown ticking down, or no recent
    /// attempt at all).
    pub active: bool,
    /// Seconds remaining on the active vote window. Drives the
    /// HUD countdown ring. `0.0` when `active` is false.
    pub time_remaining: f32,
    /// Seconds remaining before another vote may be opened.
    /// `0.0` once cooldown has expired.
    pub cooldown_remaining: f32,
    /// One row per *living* player on the rift floor at the
    /// moment the vote opened. Includes the initiator
    /// (auto-`Yes`). Ordered by client_id ascending so the HUD
    /// layout is stable across the vote's lifetime.
    pub voters: Vec<(NetId, VoteChoice)>,
}

/// Cosmetic body type. Mirrors `rift_game::character::Gender`. Kept
/// here as a wire enum so rift-net doesn't depend on rift-game.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Gender {
    Male,
    Female,
}

/// One row in a [`ServerMsg::Roster`] response. Decoupled from
/// `rift_persistence::CharacterRecord` so rift-net stays free of a
/// database dependency — the server fills these in from whatever
/// storage backend it ends up using.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RosterEntry {
    pub character_name: String,
    pub class_id: String,
    pub gender: Gender,
    pub level: u32,
    /// Six ability wire ids the player has slotted on the action
    /// bar. See `rift_game::loadout::Loadout`. Sent so the
    /// character-select / future "preview" UI can render the
    /// per-character ability bar before the player has logged in.
    pub loadout: [u8; 6],
}

/// One frame of player input. Compact by design — we'll send these
/// at up to 60 Hz.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct InputCmd {
    /// Monotonic sequence number for reconciliation. Wraps; compare
    /// with `wrapping_sub`.
    pub seq: u32,
    /// Client's best estimate of the server tick this input is for.
    /// Server uses it to detect speedhacks and to schedule
    /// late-arriving inputs.
    pub tick_estimate: NetTick,
    /// World-space horizontal move axis, normalized.
    pub move_dir: [f32; 2],
    /// World-space horizontal aim direction, normalized. Drives the
    /// spine twist (see `skinning_system`) and projectile direction.
    pub aim_dir: [f32; 2],
    /// Bitfield of held buttons. See [`button_bits`].
    pub buttons: u16,
    /// Optional reticle target for placed AoE abilities. `None` for
    /// instant-cast abilities.
    pub cast_target: Option<[f32; 3]>,
}

/// Bit positions inside [`InputCmd::buttons`]. New buttons append at
/// the next free bit; never reorder.
pub mod button_bits {
    pub const MOVE_FORWARD: u16 = 1 << 0;
    pub const MOVE_BACK: u16 = 1 << 1;
    pub const MOVE_LEFT: u16 = 1 << 2;
    pub const MOVE_RIGHT: u16 = 1 << 3;
    pub const ROLL: u16 = 1 << 4;
    pub const JUMP: u16 = 1 << 5;
    pub const INTERACT: u16 = 1 << 6;
    pub const ABILITY_1: u16 = 1 << 7;
    pub const ABILITY_2: u16 = 1 << 8;
    pub const ABILITY_3: u16 = 1 << 9;
    pub const ABILITY_4: u16 = 1 << 10;
    pub const ABILITY_5: u16 = 1 << 11;
    pub const ABILITY_6: u16 = 1 << 12;
    pub const ATTACK: u16 = 1 << 13;
}

/// Bit positions inside [`EntitySnapshot::flags`]. New flags append
/// at the next free bit; never reorder.
pub mod entity_flags {
    /// Player is mid-air (vy != 0 or position.y > 0).
    pub const AIRBORNE: u8 = 1 << 0;
    /// Player is dead. Reserved for the death pose / respawn flow.
    pub const DEAD: u8 = 1 << 1;
    /// Player has risen as a ghost: still `hp == 0`, but moves
    /// freely and is invisible to LIVING teammates (server
    /// filters them out of remote snapshots, owner-only).
    pub const GHOST: u8 = 1 << 2;
}

// ─── Server → Client ─────────────────────────────────────────────────────

/// Anything the server sends to a client.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ServerMsg {
    /// Reply to [`ClientMsg::RequestRoster`]. Lists every
    /// character that belongs to the supplied account, in
    /// creation order. Empty for brand-new accounts.
    Roster {
        entries: Vec<RosterEntry>,
    },

    /// Response to a successful [`ClientMsg::Hello`]. Tells the client
    /// which net id is theirs and how to set up its world.
    Welcome {
        your_client_id: ClientId,
        your_net_id: NetId,
        /// Floor seed + index so the client regenerates the world
        /// locally. We don't ship geometry over the wire.
        floor_seed: u64,
        floor_index: u32,
        /// Server's current tick at the moment of welcome — clients
        /// use this to anchor their tick estimate.
        tick: NetTick,
    },

    /// Connection rejected. Reason is human-readable for now; we'll
    /// graduate to an enum once we have more failure modes.
    Reject { reason: String },

    /// World state at a given tick. See [`Snapshot`].
    Snapshot(Snapshot),

    /// Reliable, one-shot world events that don't fit per-tick
    /// snapshots (damage numbers, ability casts, deaths). See
    /// [`WorldEvent`].
    Event(WorldEvent),

    /// Full inventory replication for the local player. Sent once
    /// per session right after [`ServerMsg::Welcome`] so the
    /// freshly-connected client can hydrate its `mp_inventory`
    /// from the persisted bag, and again whenever the server
    /// authoritatively rewrites the bag (future: trades, loot
    /// pruning). Items are addressed to *this* client only \u2014
    /// other players' inventories are private.
    ///
    /// `items[i] == None` means slot `i` is empty. The bag is
    /// sparse so drag-and-drop reorders preserve gaps.
    InventorySync {
        items: Vec<Option<ItemBlob>>,
    },

    /// Full equipment replication for the local player. Sent
    /// alongside [`ServerMsg::InventorySync`] on session start
    /// (so the equipped set is hot before the world appears) and
    /// again after every authoritative equip / unequip. `slots`
    /// only contains *filled* slots — absent entries are empty.
    /// `slot` is the byte returned by
    /// `rift_game::loot::EquipSlot::to_u8`. Reliable on
    /// `Channel::Control`.
    EquipmentSync {
        slots: Vec<(u8, ItemBlob)>,
    },

    /// Full stash replication for the local player. Sent on the
    /// server's reply to [`ClientMsg::OpenStash`] (with the freshly
    /// loaded persisted rows) and again after every authoritative
    /// deposit / withdraw. Reliable on `Channel::Control`. Items
    /// are addressed to *this* client only — stashes are
    /// per-character private storage. Sparse like
    /// [`Self::InventorySync`].
    StashSync {
        items: Vec<Option<ItemBlob>>,
    },

    /// Floor transition. The client clears its local world and
    /// regenerates from `(seed, index)` before applying the next
    /// snapshot. Carries the spawn position the server has placed
    /// every connected player at, so client-side prediction can
    /// snap to the same place authoritative simulation lives at.
    /// Reliable-ordered (Channel::Control) so two transitions can't
    /// be observed out of order.
    LoadFloor {
        seed: u64,
        index: u32,
        /// Whether this floor is the safe hub or a rift floor. Drives
        /// monster spawning + portal placement on the client.
        is_hub: bool,
        /// World-space position the server has snapped every
        /// connected player to. Clients use this both to seat the
        /// local prediction and to render the freshly-spawned hub /
        /// rift transition without a one-frame teleport.
        spawn_pos: [f32; 3],
        /// Server tick this transition went into effect at. Used by
        /// the client to discard stale snapshots that were in flight
        /// from the *old* floor when the change happened.
        tick: NetTick,
    },

    /// Confirmation that a [`ClientMsg::PickUpLoot`] succeeded for
    /// some client. Broadcast to everyone so the loot drop can
    /// disappear from all worlds.
    LootClaimed {
        loot: NetId,
        claimed_by: ClientId,
    },

    /// Sent only to the requester when their [`ClientMsg::PickUpLoot`]
    /// was refused server-side (e.g. bag full). The drop stays on the
    /// ground; the client uses this to show a warning and ignore the
    /// pending request. Reliable on `Channel::Control`.
    PickupRejected {
        loot: NetId,
        reason: PickupRejectReason,
    },

    /// Server-initiated kick (idle timeout, version mismatch caught
    /// late, lobby closing). Renet will tear down the connection
    /// shortly after.
    Kick { reason: String },

    /// A player (possibly the receiver themselves) entered the
    /// session. Sent reliably on Hello-accept: the new player gets a
    /// `PlayerJoined` for every already-connected player; everyone
    /// already connected gets a `PlayerJoined` for the newcomer.
    /// Carries the cosmetic profile so clients can pick the right
    /// mesh + animation set.
    PlayerJoined {
        net_id: NetId,
        client_id: ClientId,
        character_name: String,
        class_id: String,
        gender: Gender,
    },

    /// A player disconnected. Clients remove the corresponding remote
    /// avatar entity + renderer slot. Snapshots after this point will
    /// no longer carry the player's `net_id`.
    PlayerLeft { net_id: NetId },

    /// Authoritative ability-loadout snapshot for the local
    /// character. Sent once right after [`ServerMsg::Welcome`]
    /// and again after every [`ClientMsg::SetLoadoutSlot`] the
    /// server accepts. Carries the full six-slot vector so the
    /// client can resync after a partial-message drop.
    /// Reliable on `Channel::Control`.
    Loadout { slots: [u8; 6] },

    /// Authoritative XP / level snapshot for the local character.
    /// Sent once at Welcome and again whenever the server's
    /// `Experience` row changes (kill XP, level up). Reliable on
    /// `Channel::Control`. The client mirrors this into
    /// `PlayerState::experience` and recomputes stats on level
    /// change. `xp_to_next` is the threshold the client compares
    /// `xp` against to draw the bar — server is the single
    /// source of the formula so the bar can never lie.
    CharacterStats {
        level: u32,
        xp: u64,
        xp_to_next: u64,
    },

    /// Authoritative rift-progress snapshot for the current floor.
    /// Drives the client's progress bar, boss-spawned banner, and
    /// "enter portal" prompt. Sent on every change (kill,
    /// boss-spawn, boss-kill, floor reset). Reliable on
    /// `Channel::Control`.
    RiftProgress {
        /// Kills counted toward boss spawn so far.
        progress: u32,
        /// Kills required before the boss spawns.
        required: u32,
        /// `true` once the floor's boss has been spawned.
        boss_spawned: bool,
        /// `true` once the boss has been killed (sets
        /// `floor_complete` simultaneously).
        boss_killed: bool,
        /// `true` once the floor is fully cleared and the player
        /// can advance via the portal.
        floor_complete: bool,
    },

    /// Snapshot of the rift exit vote. Broadcast on every state
    /// change (vote opened, a player cast their vote, vote
    /// resolved, cooldown ticked across a 1s boundary). Clients
    /// drive their HUD vote panel directly off this. Reliable on
    /// `Channel::Control`. See [`VoteState`].
    RiftExitVote(VoteState),

    /// One chat message destined for the receiving client. Sent
    /// per-recipient after the server has resolved the
    /// [`ClientMsg::ChatSend`] routing — clients never receive a
    /// message they aren't a routed recipient of.
    ///
    /// `sender == None` indicates a server-emitted system event
    /// (joins, deaths, boss kills, level-ups). The client renders
    /// these in a distinct system colour.
    ///
    /// `target == Some(name)` rides on whisper messages so the
    /// recipient's HUD can render `[from <sender>]` and the
    /// sender's own echo can render `[to <target>]`.
    ///
    /// Reliable on `Channel::Control`.
    Chat {
        /// Wire id from [`chat_channel`]. The client uses this
        /// to colour-code the line and to keep per-channel
        /// scrollback buffers.
        channel: u8,
        /// Sender's character name. `None` for system events.
        sender: Option<String>,
        /// Whisper recipient's character name. `Some` only on
        /// the `WHISPER` channel; `None` everywhere else.
        target: Option<String>,
        /// UTF-8 message body. Already length-clamped server-
        /// side.
        text: String,
    },
}

/// Per-tick snapshot. Phase 1 ships the *full* state every tick — we
/// will layer delta encoding on top in a later phase once the field
/// set is stable.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub tick: NetTick,
    /// Highest [`InputCmd::seq`] we have applied for the receiving
    /// client. Used by the client to truncate its prediction buffer.
    pub ack_seq: u32,
    /// All replicated entities visible to the receiving client.
    pub entities: Vec<EntitySnapshot>,
}

/// Per-entity snapshot. The `kind` discriminator selects the trailing
/// archetype-specific fields; everything else (position, yaw,
/// velocity) is shared.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EntitySnapshot {
    pub net_id: NetId,
    pub kind: EntityKind,
    /// World-space position.
    pub position: [f32; 3],
    /// Body yaw in radians. Aim yaw, when different (players), rides
    /// in the [`EntityKind::Player`] payload.
    pub yaw: f32,
    /// Horizontal velocity for client-side extrapolation between
    /// snapshots. Zeroed for static entities.
    pub velocity: [f32; 3],
    /// Health 0..=1. Used for HP bars; the canonical HP value lives
    /// only on the server.
    pub health_pct: f32,
    /// State flags (airborne, dead, hidden, ...).
    pub flags: u8,
    /// Currently-active buffs / debuffs on this entity. Empty for
    /// most rows; populated for any entity carrying a server-side
    /// `EffectStack`. Drives HUD icons + duration rings on the
    /// client. See `rift_game::effects` for the id table.
    #[serde(default)]
    pub effects: Vec<ActiveEffect>,
}

/// One active buff / debuff entry on a snapshot row. Replaces
/// the older `debuffs: u32` bitmask so the HUD can render a
/// radial duration ring without reverse-engineering tick
/// timing from snapshot deltas.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ActiveEffect {
    /// Effect id (`rift_game::effects::id::*`).
    pub id: u8,
    /// Seconds left until the effect expires. The HUD divides
    /// by `duration` to drive the ring fill.
    pub remaining: f32,
    /// Duration the effect was applied for (`default_duration`
    /// or the override the caster passed). Lets the HUD show
    /// progress relative to the original duration even after a
    /// refresh.
    pub duration: f32,
}

/// What a snapshot row represents. Trailing fields are kept on the
/// row itself rather than a sum-type payload to keep bincode output
/// compact (no extra tag for the common case).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum EntityKind {
    Player {
        client_id: ClientId,
        /// Aim direction, may differ from body yaw. Drives spine twist.
        aim_yaw: f32,
        /// Locomotion bucket id; clients map to clip names locally.
        locomotion: u8,
        /// Active full-body action (Roll/JumpLand/Hit/Death/...). Zero
        /// when no action is playing.
        action: u8,
        /// Tick this action started at, for synchronized playback.
        action_start: NetTick,
    },
    Enemy {
        /// Stable role id (Skull, Demon, Yeti, ...) so the client
        /// picks the right mesh + animation set.
        role: u8,
        anim: u8,
    },
    Projectile {
        /// Ability id that spawned it; clients use this to pick the
        /// right visual + particle preset.
        ability: u16,
    },
    AoeZone {
        ability: u16,
        radius: f32,
        remaining: f32,
    },
    Loot {
        /// Full rolled-item payload. Lets the client render
        /// rarity-aware visuals and tooltips without an extra
        /// lookup roundtrip.
        item: ItemBlob,
    },
    /// A revive shrine sitting on the floor. Rare spawn on
    /// rift floors >= 2. While ALL living players on the floor
    /// are channeling it (proximity + F-press intent), `progress`
    /// ramps from 0 to 255 over `SHRINE_CHANNEL_DURATION` seconds;
    /// on completion the server revives every ghost on the
    /// floor and broadcasts [`WorldEvent::PlayersRevived`].
    /// `channelers` / `required` give the HUD a "1 / 2 channeling"
    /// readout without needing to track player positions client-
    /// side. `required` is 0 when no living players exist (which
    /// the channel-tick gate also rejects).
    ReviveShrine {
        /// Channel progress, 0..=255 mapping to 0.0..=1.0.
        progress: u8,
        /// Living players currently channeling this shrine.
        channelers: u8,
        /// Living players on the floor (channel target count).
        required: u8,
    },
}

/// Wire-serialisable rolled item. Reconstructable via
/// `rift_game::loot::Item::from_blob`. `base_id` and the inner
/// `affix_id` indices reference the static
/// `rift_game::loot::BASE_ITEMS` and `AFFIX_POOL` tables — both
/// sides are guaranteed to share the same build of `rift-game`,
/// so indices are stable for one build.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ItemBlob {
    /// Index into `rift_game::loot::BASE_ITEMS`.
    pub base_id: u16,
    /// Rarity tier as raw discriminant byte (Common=0, Magic=1,
    /// Rare=2, Legendary=3).
    pub rarity: u8,
    pub ilvl: u16,
    /// `(affix-pool index, rolled value)` pairs.
    pub affixes: Vec<(u16, f32)>,
    /// `true` if this drop rolled the rare "Anchored" trait.
    /// Anchored items skip the wipe-on-death loot reset and
    /// render with a unique pillar / tooltip cue. Defaults to
    /// `false` — the wire format is forward-compatible: old
    /// builds deserialise as non-anchored.
    #[serde(default)]
    pub anchored: bool,
}

/// One-shot reliable event broadcast to interested clients.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum WorldEvent {
    /// An ability has been *cast* — clients play the cast animation
    /// + spawn local particles. Damage is *not* applied here; that's
    /// signalled by [`WorldEvent::Damage`].
    AbilityCast {
        caster: NetId,
        ability: u16,
        origin: [f32; 3],
        dir: [f32; 2],
        target: Option<[f32; 3]>,
        start_tick: NetTick,
    },

    /// Damage was applied. Client spawns floating combat text.
    Damage {
        target: NetId,
        amount: f32,
        crit: bool,
        /// World-space position to spawn the number at.
        position: [f32; 3],
    },

    /// Entity died. Clients trigger the death animation if applicable
    /// and start the despawn fade.
    Death { entity: NetId, killer: Option<NetId> },

    /// Entity was hit (non-fatal). Used to start the hit-react clip
    /// without waiting for the next snapshot.
    Hit { target: NetId, start_tick: NetTick },

    /// Loot dropped at `position`. Replicated as a normal entity in
    /// the next snapshot too — the event just lets the client
    /// pre-spawn the visual without waiting.
    LootDropped {
        loot: NetId,
        item: ItemBlob,
        position: [f32; 3],
    },

    /// One tick of a channeled ability fired. Clients spawn the
    /// per-tick visual (beam impact, whirlwind sweep, ...). The
    /// `position` is the caster's location at the tick, `dir` the
    /// caster's aim at the tick, both in world space.
    ChannelTick {
        caster: NetId,
        ability: u16,
        position: [f32; 3],
        dir: [f32; 2],
        tick: NetTick,
    },

    /// Channel ended (duration elapsed or cancelled). Clients stop
    /// any per-channel looping visual / audio.
    ChannelEnd {
        caster: NetId,
        ability: u16,
    },

    /// A dead player has finished their down-pose timer and risen
    /// as a ghost. Server stops including their row in remote
    /// snapshots after this fires, so the client uses the event
    /// as a cue to play a "poof" VFX at their last position
    /// (otherwise the avatar just pops out of existence). The
    /// owning client suppresses the VFX for themselves so their
    /// own rise doesn't slap them in the face.
    PlayerGhosted {
        entity: NetId,
        position: [f32; 3],
    },

    /// One or more ghosts have been revived back to full HP by
    /// a completed revive shrine channel. Each NetId in the
    /// list refers to a player who was a ghost (or in the
    /// down-pose) before the channel completed. Clients use
    /// this to clear their local ghost-tint / vignette and
    /// spawn a celebration VFX at each revived position. The
    /// shrine entity itself is despawned in the same tick so
    /// the next snapshot drops its row.
    /// One or more ghosts have been revived back to full HP by
    /// a completed revive shrine channel. Each NetId in the
    /// list refers to a player who was a ghost (or in the
    /// down-pose) before the channel completed. Clients use
    /// this to clear their local ghost-tint / vignette and
    /// spawn a celebration VFX at each revived position. The
    /// shrine entity itself is despawned in the same tick so
    /// the next snapshot drops its row.
    PlayersRevived {
        entities: Vec<NetId>,
    },

    /// Healing was applied to a player. Mirrors
    /// [`WorldEvent::Damage`] for the friendly path so clients
    /// can spawn floating-green combat text and trigger
    /// heal-burst VFX. `caster` may be the same as `target`
    /// for self-casts.
    Heal {
        caster: NetId,
        target: NetId,
        amount: f32,
        /// `true` if this came from a heal-over-time tick
        /// rather than the original cast — clients use it to
        /// suppress the heavy burst VFX on tick rows and
        /// keep just the floating number.
        over_time: bool,
        position: [f32; 3],
    },

    /// An enemy is *winding up* a generic action that doesn't
    /// flow through the [ability] registry — currently brute
    /// melee swings, stalker dashes, and caster bolts. Sent
    /// at wind-up start so the client can play a directional
    /// SFX cue and (optionally) flash the enemy briefly.
    /// Damage / projectile spawn arrives separately on resolve.
    ///
    /// `kind` discriminates the SFX bucket — see
    /// [`telegraph_kind`] for the stable id list. Lightweight
    /// on purpose: just `(source, kind, position)`. Anything
    /// richer (radius, aim, ...) belongs on
    /// [`WorldEvent::AbilityCast`].
    EnemyTelegraph {
        source: NetId,
        kind: u8,
        position: [f32; 3],
    },
}

/// Stable wire ids for [`WorldEvent::EnemyTelegraph::kind`].
/// Append-only — never reorder or repurpose existing values.
pub mod telegraph_kind {
    /// Brute / boss melee wind-up — short, percussive cue.
    pub const MELEE_WINDUP: u8 = 0;
    /// Caster bolt wind-up — magical / chargey cue.
    pub const RANGED_WINDUP: u8 = 1;
    /// Stalker dash wind-up — sharp inhale / hiss cue.
    pub const DASH_WINDUP: u8 = 2;
}

/// Maximum UTF-8 character count of a [`ClientMsg::ChatSend::text`]
/// body. Server clamps anything longer before re-broadcast — the
/// constant lives here so the client can show a "you've typed
/// too much" cue without having to round-trip and rely on a
/// silent truncation. Tuned for one-line readability in the
/// scrollback panel without wrapping wider than the panel.
pub const CHAT_MAX_LEN: usize = 256;

/// Stable wire ids for [`ClientMsg::ChatSend::channel`] and
/// [`ServerMsg::Chat::channel`]. Append-only — never reorder
/// or repurpose. The client uses these to keep per-channel
/// scrollback buffers and to colour-code lines.
pub mod chat_channel {
    /// Server-emitted system events (join / leave, death, boss
    /// kill, level-up). Clients sending this id on
    /// [`super::ClientMsg::ChatSend`] are silently dropped.
    pub const SYSTEM: u8 = 0;
    /// Visible to every connected player on the server.
    pub const GLOBAL: u8 = 1;
    /// Visible to every player currently in the hub.
    pub const HUB: u8 = 2;
    /// Visible to every player currently on the same rift floor
    /// as the sender.
    pub const FLOOR: u8 = 3;
    /// Visible to every player in the sender's party. Until a
    /// real party system lands, every player is in a singleton
    /// party of themselves, so PARTY messages echo back to the
    /// sender only — but the wire path is in place so the
    /// surface doesn't need to grow on the day parties land.
    pub const PARTY: u8 = 4;
    /// Whisper from one player to another, addressed by
    /// character name in [`super::ClientMsg::ChatSend::target`].
    /// Visible to the sender (echo) and the named recipient
    /// only.
    pub const WHISPER: u8 = 5;
}
