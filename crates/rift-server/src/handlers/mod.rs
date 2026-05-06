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

pub mod inventory;
pub mod persistence;
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

/// Convert an authoritative `rift_game::loot::Item` into the
/// `ItemBlob` shape that ships over the wire. Centralised so all
/// the `InventorySync` / `EquipmentSync` builders agree on the
/// field layout — bumping `Item::to_wire` only needs to be
/// reflected here.
pub(crate) fn item_to_blob(
    item: &rift_game::loot::Item,
) -> rift_net::messages::ItemBlob {
    let (base_id, rarity, ilvl, affixes) = item.to_wire();
    rift_net::messages::ItemBlob {
        base_id,
        rarity,
        ilvl,
        affixes,
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
