//! Sibling-file `impl Server` blocks for client-message dispatch
//! handlers. Split out of `main.rs` so the loop / connection
//! plumbing stays readable; each submodule owns one feature
//! cluster.
//!
//! Module breakdown:
//! - [`session`]   — Hello / hydrate / spawn / welcome flow.
//! - [`inventory`] — bag, equipment, stash, drop, pickup. Every
//!   handler that mutates a player's items lives here.
//! - [`persistence`] — character record loads + XP saves +
//!   roster lookups.
//!
//! Two small helpers used across handlers (`item_to_blob`,
//! `place_at_slot_index`, `gender_from_i16`) are pinned here at
//! `pub(crate)` so they're reachable from every submodule
//! without re-exporting through `main.rs`.

pub(crate) use crate::wire::{
    gender_from_i16, gender_to_i16, item_to_blob, loadout_to_u8, provenance_from_persisted,
    provenance_to_persisted,
};

pub mod chat;
pub mod inventory;
pub mod party;
pub mod persistence;
pub mod portal;
pub mod session;

use std::fmt;

use rift_net::ClientId;

use crate::Server;

/// `Display`-able client tag used to prefix per-client log
/// lines so multi-player issues are easier to correlate. Renders
/// as `[cid=N char=Foo]` when the session has a known character
/// name, or `[cid=N]` when the client is pre-Hello (still
/// negotiating roster, etc.).
pub(crate) struct ClientTag<'a> {
    pub cid: ClientId,
    pub character: Option<&'a str>,
}

impl fmt::Display for ClientTag<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.character {
            Some(name) => write!(f, "[cid={} char={}]", self.cid.0, name),
            None => write!(f, "[cid={}]", self.cid.0),
        }
    }
}

impl Server {
    /// Render `[cid=N char=Foo]` for `from`. Cheap (single
    /// session map lookup, no allocation); use it inline in
    /// any `log::warn!` / `log::error!` / `log::info!` line
    /// where a human reading the log will want to know which
    /// player the message refers to. Falls back to `[cid=N]`
    /// for pre-Hello connections.
    pub(crate) fn client_tag(&self, from: ClientId) -> ClientTag<'_> {
        let character = self
            .sessions
            .get(from)
            .and_then(|s| s.character_name.as_deref());
        ClientTag {
            cid: from,
            character,
        }
    }
}

/// Insert `item` at `slot_index` in a sparse bag/stash, growing
/// the vector with `None` placeholders as needed. If the target
/// slot is somehow already occupied (corrupted data, duplicate
/// `slot_index`), the new item lands at the next free slot
/// instead so nothing gets lost.
pub(crate) fn place_at_slot_index(
    bag: &mut Vec<Option<rift_game::loot::Item>>,
    slot_index: i32,
    item: rift_game::loot::Item,
) {
    let idx = slot_index.max(0) as usize;
    if idx >= bag.len() {
        bag.resize_with(idx + 1, || None);
    }
    if bag[idx].is_some() {
        // Fall back: walk forward to the first hole, else append.
        if let Some(slot) = bag.iter_mut().find(|s| s.is_none()) {
            *slot = Some(item);
        } else {
            bag.push(Some(item));
        }
    } else {
        bag[idx] = Some(item);
    }
}
