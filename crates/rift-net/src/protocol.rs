//! Protocol-level constants. Bump [`PROTOCOL_VERSION`] whenever the
//! wire format changes in a way that breaks compatibility — clients
//! and servers refuse to connect across mismatched versions.

/// 64-bit "magic" identifying the Rift Crawler protocol family.
/// Renet uses this in its connect token to reject foreign packets
/// before they reach our codec. Generated once; never change.
pub const PROTOCOL_ID: u64 = 0x5249_4654_4352_5731; // "RIFTCRW1"

/// Wire-format version. Increment on any breaking message change.
///
/// Version history (most recent first):
/// - v13 (2026-05-15): `EquipItem` / `EquipFromStash` carry optional `target_slot`
///   so drag-to-paperdoll picks ring 2 vs ring 1 explicitly.
/// - v12 (2026-05-15): `ItemBlob` affix ids now span main + resonance
///   pools as one contiguous index space (`AFFIX_POOL` then
///   `RESONANCE_POOL`).
/// - v11 (2026-05-15): added anvil enchanting request messages and
///   enchanted item metadata.
/// - v10 (2026-05-15): character appearance added to roster,
///   enter-world, and player-joined messages.
/// - v7 (2026-05-11): added `ClientMsg::SortInventory` and
///   `ClientMsg::SortStashTab` for one-click auto-sort.
/// - v6 (2026-05-11): added `ClientMsg::EquipFromStash` and
///   `ClientMsg::UnequipToStashSlot` for atomic stash\u2194equip
///   drag.
/// - v5 (2025-...): `LoginTicket` / `SteamTicket` variants
///   now share one opaque-ticket wire shape; the server's
///   installed verifier (chosen at startup) decides how to
///   parse the bytes.
/// - v4 (2026-05-11): `Hello.account_name` replaced with
///   `Hello.auth: AuthCredential`; `RequestRoster` removed; the
///   roster is now bundled into `Welcome.roster` so the client
///   can render character-select straight after the auth round-
///   trip.
/// - v3: previous schema (free-form `account_name` string,
///   pre-Hello `RequestRoster` lookup).
pub const PROTOCOL_VERSION: u16 = 13;

/// Hard cap on simultaneous connected clients per server. Matches the
/// design target of 4-player co-op (one slot is the host on a listen
/// server; on a dedicated server all four are remote).
pub const MAX_CLIENTS: usize = 4;

/// Server simulation rate. Fixed-step; both server simulate and
/// client reconciliation use this `dt`.
pub const TICK_HZ: u32 = 30;

/// Snapshot broadcast rate. Lower than tick rate so each snapshot
/// covers ~1.5 sim ticks worth of change.
pub const SNAPSHOT_HZ: u32 = 20;

/// Bundled connection settings used by both client and server when
/// constructing their renet endpoints. Tweak as we learn what the
/// real bandwidth profile looks like.
///
/// Named to avoid clashing with [`renet::ConnectionConfig`], which
/// callers also need to import (and which we build *from* this).
pub struct NetSettings {
    /// Renet's per-tick send budget, bytes. Renet ticks at the call
    /// rate of `update`, so on our 30 Hz simulation the effective
    /// bandwidth is `available_bytes_per_tick * 30`. We pick a value
    /// well above our 15 KB/s estimated payload so bursts (floor
    /// transitions, party-wide debuff applies) don't stall.
    pub available_bytes_per_tick: u64,
}

impl Default for NetSettings {
    fn default() -> Self {
        Self {
            // ~32 KB/tick × 60 Hz transport rate = ~1.9 MB/s
            // ceiling per peer. Headroom matters: a fully-loaded
            // rift floor with 100+ enemies + projectiles + loot
            // can push a single 20 Hz snapshot to 10–20 KB, and
            // the previous 9 KB/tick budget left renet throttling
            // the snapshot channel — dropped/late snapshots made
            // enemies appear to freeze on the client even though
            // server-side they were happily moving and swinging.
            available_bytes_per_tick: 32 * 1024,
        }
    }
}

impl NetSettings {
    /// Translate to the renet [`ConnectionConfig`](renet::ConnectionConfig)
    /// expected by [`RenetClient::new`](renet::RenetClient::new) /
    /// [`RenetServer::new`](renet::RenetServer::new). Both ends need
    /// the same channel layout so we install [`crate::channel_config`]
    /// on both directions.
    pub fn to_renet(&self) -> renet::ConnectionConfig {
        let channels = crate::channel::channel_config();
        renet::ConnectionConfig {
            available_bytes_per_tick: self.available_bytes_per_tick,
            server_channels_config: channels.clone(),
            client_channels_config: channels,
        }
    }
}
