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
        /// Bitmask of active debuffs (`1 << debuff_id`). Drives
        /// indicator pips above the enemy. See
        /// `rift_game::debuffs` for the id table.
        debuffs: u32,
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
}
