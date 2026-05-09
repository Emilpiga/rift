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

use rift_net::Gender;

pub mod chat;
pub mod inventory;
pub mod party;
pub mod persistence;
pub mod portal;
pub mod session;

/// Decode a stored gender column back to the wire enum. Unknown
/// codes (forward-compat with future variants) fall back to
/// Female so we never panic on a malformed row.
pub(crate) fn gender_from_i16(g: i16) -> Gender {
    match g {
        x if x == Gender::Male as i16 => Gender::Male,
        _ => Gender::Female,
    }
}

/// Coerce a persisted `[i16; 6]` ability loadout to the `u8` wire
/// shape. Out-of-range entries fall back to the empty-slot
/// sentinel (`u8::MAX`) so a malformed row leaves the slot
/// blank instead of accidentally re-binding Steady Shot.
pub(crate) fn loadout_to_u8(loadout: [i16; 6]) -> [u8; 6] {
    let mut out = [rift_game::loadout::EMPTY_SLOT; 6];
    for (i, &slot) in loadout.iter().enumerate() {
        out[i] = u8::try_from(slot).unwrap_or(rift_game::loadout::EMPTY_SLOT);
    }
    out
}

/// Convert an authoritative `rift_game::loot::Item` into the
/// `ItemBlob` shape that ships over the wire. Centralised so all
/// the `InventorySync` / `EquipmentSync` builders agree on the
/// field layout — bumping `Item::to_wire` only needs to be
/// reflected here.
pub(crate) fn item_to_blob(
    item: &rift_game::loot::Item,
) -> rift_net::messages::ItemBlob {
    let (base_id, rarity, ilvl, affixes, anchored) = item.to_wire();
    rift_net::messages::ItemBlob {
        base_id,
        rarity,
        ilvl,
        affixes,
        anchored,
        provenance: provenance_to_wire(item),
    }
}

/// Convert an [`Item`]'s in-memory [`rift_game::loot::LootProvenance`]
/// into the `Option<Vec<[u8; 16]>>` shape used on the wire and
/// in [`rift_net::messages::ItemBlob`]. Returns `None` for
/// legacy / unprovenanced items so the receiver can route them
/// to the self-bind-on-touch path.
pub(crate) fn provenance_to_wire(
    item: &rift_game::loot::Item,
) -> Option<Vec<[u8; 16]>> {
    item.provenance.as_ref().map(|p| p.eligible.clone())
}

/// Convert an [`Item`]'s provenance into the
/// `Option<Vec<rift_persistence::Uuid>>` shape stored as
/// `provenance UUID[]` in the database. Same `None` semantics
/// as [`provenance_to_wire`].
pub(crate) fn provenance_to_persisted(
    item: &rift_game::loot::Item,
) -> Option<Vec<rift_persistence::Uuid>> {
    item.provenance.as_ref().map(|p| {
        p.eligible
            .iter()
            .map(|bytes| rift_persistence::Uuid::from_bytes(*bytes))
            .collect()
    })
}

/// Decode the wire / persisted byte vector back into a runtime
/// [`rift_game::loot::LootProvenance`]. `None` round-trips to
/// `None`. Centralised so the inverse of
/// [`provenance_to_wire`] / [`provenance_to_persisted`] is
/// always evaluated the same way.
#[allow(dead_code)]
pub(crate) fn provenance_from_wire(
    bytes: Option<Vec<[u8; 16]>>,
) -> Option<rift_game::loot::LootProvenance> {
    bytes.map(|eligible| rift_game::loot::LootProvenance::from_ids(eligible))
}

/// Decode a persisted `Vec<Uuid>` back into a runtime
/// [`rift_game::loot::LootProvenance`]. Mirror of
/// [`provenance_from_wire`].
pub(crate) fn provenance_from_persisted(
    uuids: Option<Vec<rift_persistence::Uuid>>,
) -> Option<rift_game::loot::LootProvenance> {
    uuids.map(|v| {
        rift_game::loot::LootProvenance::from_ids(
            v.into_iter().map(|u| u.into_bytes()),
        )
    })
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
