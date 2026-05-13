//! Inventory + equip-slot mutators on [`Sim`]. Split out of the
//! main `sim/mod.rs` so the per-domain surface area stays
//! browsable. Pure `impl Sim` block — every method is already
//! defined on `Sim` and migrated here verbatim.

use rift_net::ids::ClientId;

use super::player::ServerPlayer;
use super::{
    build_bag_occupancy, footprint_fits, place_inventory_item, place_inventory_item_at,
    sort_grid_items, trim_trailing_none, Sim,
};

impl Sim {
    /// Move the bag item at `inventory_index` into its canonical
    /// equipment slot. If the slot is already filled, the
    /// previously-equipped item is pushed back to the bag at the
    /// same index so the UI position stays stable.
    ///
    /// Returns `true` on success. `false` indicates a no-op: bad
    /// index, item has no compatible slot, or the player isn't
    /// connected.
    pub fn equip_from_bag(&mut self, client_id: ClientId, inventory_index: usize) -> bool {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
            return false;
        };
        let Some(mut item) = p.inventory.get_mut(inventory_index).and_then(|s| s.take()) else {
            return false;
        };
        // Self-bind legacy / unprovenanced items to the
        // equipping character so their lineage carries forward
        // across drop / re-pickup. New items already have
        // `provenance` set at drop time.
        if item.provenance.is_none() {
            if let Some(uuid) = p.character_id {
                item.provenance = Some(rift_game::loot::LootProvenance::from_ids([
                    uuid.into_bytes()
                ]));
            }
        }
        // Level requirement gate. Reject before mutating the
        // equipment slot so the bag entry can be restored.
        if p.level < item.required_level() {
            log::debug!(
                "equip: rejected (under-level) client={client_id:?} idx={inventory_index} \
                 player_level={} item={} req={}",
                p.level,
                item.display_name(),
                item.required_level(),
            );
            p.inventory[inventory_index] = Some(item);
            return false;
        }
        let Some(slot) = p.equipment.default_slot(&item) else {
            // Bag-only item (consumable) with no target slot.
            // Restore the bag entry and bail — the equip
            // request was never legal.
            p.inventory[inventory_index] = Some(item);
            return false;
        };
        if !rift_game::loot::Equipment::accepts(slot, &item) {
            // Item base has no equip slot we accept — put it
            // back and bail. (With `equip_slot: Option`, the
            // earlier `let Some` covers the consumable case;
            // this branch is the residual defensive guard for
            // any future slot mismatch.)
            p.inventory[inventory_index] = Some(item);
            return false;
        }
        let displaced = p.equipment.set(slot, Some(item));
        if let Some(prev) = displaced {
            // Try to place the displaced item back at the same
            // bag anchor so the UI position stays stable. If it
            // doesn't fit (footprint clash), fall back to the
            // first free anchor; if even that fails, re-equip
            // the previous item to keep the player's gear
            // intact.
            if !place_inventory_item_at(&mut p.inventory, prev.clone(), inventory_index) {
                if place_inventory_item(&mut p.inventory, prev.clone()).is_none() {
                    // Worst case: undo the equip swap.
                    if let Some(newly_equipped) = p.equipment.take(slot) {
                        p.inventory[inventory_index] = Some(newly_equipped);
                    }
                    p.equipment.set(slot, Some(prev));
                    return false;
                }
            }
        }
        p.recompute_stats();
        true
    }

    /// Move the item currently in `slot` back into the bag (at
    /// the first slot whose anchor fits the item's footprint).
    /// Returns `true` if anything actually moved; `false` for
    /// an empty slot, a stale client byte, or a full bag.
    pub fn unequip_to_bag(
        &mut self,
        client_id: ClientId,
        slot: rift_game::loot::EquipSlot,
    ) -> bool {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
            return false;
        };
        let Some(item) = p.equipment.take(slot) else {
            return false;
        };
        if place_inventory_item(&mut p.inventory, item.clone()).is_none() {
            // No room — put the item back on the equipment
            // slot to keep state consistent.
            p.equipment.set(slot, Some(item));
            return false;
        }
        p.recompute_stats();
        true
    }

    /// Snapshot the player's bag + equipment as a flat list of
    /// rolled items tagged with their current slot byte (or
    /// `None` for the bag) and the bag position the row last
    /// occupied. Bag rows carry their `Vec` index; equipped
    /// rows carry the equip-slot byte (the value is unused on
    /// load but kept stable so manual SQL inspection reads
    /// sensibly). Used by the persistence layer to produce a
    /// `ResetCharacterInventory` payload after every equip /
    /// unequip / reorder event.
    pub fn dump_player_inventory(
        &self,
        client_id: ClientId,
    ) -> Vec<(Option<u8>, i32, rift_game::loot::Item)> {
        let mut out = Vec::new();
        let Some(&entity) = self.sessions.get(&client_id) else {
            return out;
        };
        let Ok(p) = self.world.get::<&ServerPlayer>(entity) else {
            return out;
        };
        for (idx, slot) in p.inventory.iter().enumerate() {
            if let Some(it) = slot {
                out.push((None, idx as i32, it.clone()));
            }
        }
        for (slot, it) in p.equipment.iter() {
            out.push((Some(slot.to_u8()), slot.to_u8() as i32, it.clone()));
        }
        out
    }

    /// Borrow the player's bag (read-only). Used by the server's
    /// dispatch path to encode `InventorySync` payloads. Returns
    /// the sparse vec verbatim so empty slots are preserved on
    /// the wire.
    pub fn player_inventory(&self, client_id: ClientId) -> Vec<Option<rift_game::loot::Item>> {
        self.sessions
            .get(&client_id)
            .and_then(|&e| self.world.get::<&ServerPlayer>(e).ok())
            .map(|p| p.inventory.clone())
            .unwrap_or_default()
    }

    /// Borrow the player's equipment as `(slot, item)` pairs for
    /// every filled slot. Used by the server's dispatch path to
    /// encode `EquipmentSync` payloads.
    pub fn player_equipment(
        &self,
        client_id: ClientId,
    ) -> Vec<(rift_game::loot::EquipSlot, rift_game::loot::Item)> {
        self.sessions
            .get(&client_id)
            .and_then(|&e| self.world.get::<&ServerPlayer>(e).ok())
            .map(|p| p.equipment.iter().map(|(s, i)| (s, i.clone())).collect())
            .unwrap_or_default()
    }

    pub fn swap_inventory_slots(&mut self, client_id: ClientId, a: usize, b: usize) -> bool {
        use rift_net::messages::INVENTORY_CAPACITY;
        if a == b || a >= INVENTORY_CAPACITY || b >= INVENTORY_CAPACITY {
            return false;
        }
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
            return false;
        };
        if p.inventory.len() < INVENTORY_CAPACITY {
            p.inventory.resize_with(INVENTORY_CAPACITY, || None);
        }
        let item_a = p.inventory[a].take();
        let item_b = p.inventory[b].take();

        // Build occupancy WITHOUT a and b, then test that
        // each item fits at the destination's anchor. Both
        // must fit — partial swaps would visually corrupt the
        // grid — otherwise we restore the originals.
        let occ = build_bag_occupancy(&p.inventory);
        let a_fits_b = item_a
            .as_ref()
            .map(|it| {
                let (w, h) = it.footprint();
                footprint_fits(&occ, b, w, h)
            })
            .unwrap_or(true);
        let b_fits_a = item_b
            .as_ref()
            .map(|it| {
                let (w, h) = it.footprint();
                footprint_fits(&occ, a, w, h)
            })
            .unwrap_or(true);

        if a_fits_b && b_fits_a {
            // Cells we're about to fill don't conflict with
            // each other because a != b and each footprint
            // was tested against the same occ snapshot. The
            // only remaining risk is that one item's
            // footprint covers the other's anchor cell. Test
            // explicitly:
            let cross_clash = item_a
                .as_ref()
                .zip(item_b.as_ref())
                .map(|(ia, ib)| {
                    let (aw, ah) = ia.footprint();
                    let (bw, bh) = ib.footprint();
                    use rift_net::messages::BAG_COLS;
                    let bx = b % BAG_COLS;
                    let by = b / BAG_COLS;
                    let ax = a % BAG_COLS;
                    let ay = a / BAG_COLS;
                    let a_covers_a_anchor =
                        ax >= bx && ax < bx + aw as usize && ay >= by && ay < by + ah as usize;
                    let b_covers_b_anchor =
                        bx >= ax && bx < ax + bw as usize && by >= ay && by < ay + bh as usize;
                    a_covers_a_anchor || b_covers_b_anchor
                })
                .unwrap_or(false);
            if !cross_clash {
                p.inventory[b] = item_a;
                p.inventory[a] = item_b;
                return true;
            }
        }

        // Restore originals on any failure.
        p.inventory[a] = item_a;
        p.inventory[b] = item_b;
        false
    }

    /// Swap two stash slots within `tab_index`, used by the
    /// inventory UI's drag-and-drop reorder path inside the
    /// stash panel. Either index may be empty (past the
    /// current stash length); the tab is grown with `None`
    /// placeholders to fit, then trimmed back to the last
    /// filled slot. Returns `true` on success.

    /// Remove the bag item at `inventory_index` and return it,
    /// along with the player's current world position so the
    /// caller can spawn a `ServerLoot` entity at the player's
    /// feet. `None` if the index is out of range or the player
    /// isn't connected.
    pub fn pop_inventory_item(
        &mut self,
        client_id: ClientId,
        inventory_index: usize,
    ) -> Option<(rift_game::loot::Item, glam::Vec3)> {
        let &entity = self.sessions.get(&client_id)?;
        let mut p = self.world.get::<&mut ServerPlayer>(entity).ok()?;
        let item = p
            .inventory
            .get_mut(inventory_index)
            .and_then(|s| s.take())?;
        trim_trailing_none(&mut p.inventory);
        Some((item, p.k.position))
    }

    /// Remove the equipped item at `slot` and return it
    /// alongside the player's current position. Used by the
    /// "drag equip outside" → drop-to-world path. Recomputes
    /// the player's stats since equipment changed.
    pub fn pop_equipment_item(
        &mut self,
        client_id: ClientId,
        slot: rift_game::loot::EquipSlot,
    ) -> Option<(rift_game::loot::Item, glam::Vec3)> {
        let &entity = self.sessions.get(&client_id)?;
        let mut p = self.world.get::<&mut ServerPlayer>(entity).ok()?;
        let item = p.equipment.take(slot)?;
        p.recompute_stats();
        Some((item, p.k.position))
    }

    /// Move whatever's currently in `slot` into the bag at
    /// `inventory_index`, swapping with whatever is already
    /// there. Validates that footprints fit at the swapped
    /// anchors. Returns `true` on success, `false` if the
    /// swap would overlap.
    pub fn unequip_to_bag_slot(
        &mut self,
        client_id: ClientId,
        slot: rift_game::loot::EquipSlot,
        inventory_index: usize,
    ) -> bool {
        use rift_net::messages::INVENTORY_CAPACITY;
        if inventory_index >= INVENTORY_CAPACITY {
            return false;
        }
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
            return false;
        };
        let Some(unequipped) = p.equipment.take(slot) else {
            return false;
        };
        if p.inventory.len() < INVENTORY_CAPACITY {
            p.inventory.resize_with(INVENTORY_CAPACITY, || None);
        }
        // Take the existing occupant out so the occupancy
        // mask doesn't include it when we test the new item's
        // footprint.
        let displaced = p.inventory[inventory_index].take();
        let (uw, uh) = unequipped.footprint();
        let occ = build_bag_occupancy(&p.inventory);
        if !footprint_fits(&occ, inventory_index, uw, uh) {
            // Restore everything.
            p.inventory[inventory_index] = displaced;
            p.equipment.set(slot, Some(unequipped));
            return false;
        }
        p.inventory[inventory_index] = Some(unequipped);
        if let Some(prev) = displaced {
            // The displaced item goes to equipment if it fits
            // the same slot, else into the first free anchor.
            if rift_game::loot::Equipment::accepts(slot, &prev) {
                p.equipment.set(slot, Some(prev));
            } else if place_inventory_item(&mut p.inventory, prev.clone()).is_none() {
                // Last resort: roll back the entire op so we
                // don't lose the item.
                let new_unequipped = p.inventory[inventory_index].take();
                p.inventory[inventory_index] = Some(prev);
                if let Some(it) = new_unequipped {
                    p.equipment.set(slot, Some(it));
                }
                return false;
            }
        }
        p.recompute_stats();
        true
    }

    /// Swap the items between two equipment slots in place.
    /// Validates `Equipment::accepts` for each item against
    /// the destination slot; rejects the entire op if either
    /// direction would be illegal. An empty source slot is
    /// trivially "accepted" by the destination. Returns
    /// `true` if any swap actually occurred.
    pub fn swap_equip_slots(
        &mut self,
        client_id: ClientId,
        a: rift_game::loot::EquipSlot,
        b: rift_game::loot::EquipSlot,
    ) -> bool {
        if a == b {
            return false;
        }
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
            return false;
        };
        let item_a = p.equipment.take(a);
        let item_b = p.equipment.take(b);
        // Reject if either item would land in a slot that
        // doesn't accept it.
        let a_ok = item_a
            .as_ref()
            .map(|it| rift_game::loot::Equipment::accepts(b, it))
            .unwrap_or(true);
        let b_ok = item_b
            .as_ref()
            .map(|it| rift_game::loot::Equipment::accepts(a, it))
            .unwrap_or(true);
        if !a_ok || !b_ok {
            // Restore originals — no partial mutation.
            p.equipment.set(a, item_a);
            p.equipment.set(b, item_b);
            return false;
        }
        // At least one side must change for "true" to mean
        // something. Two empty slots is a no-op.
        if item_a.is_none() && item_b.is_none() {
            return false;
        }
        p.equipment.set(a, item_b);
        p.equipment.set(b, item_a);
        p.recompute_stats();
        true
    }

    /// Auto-sort the bag in place. Returns `true` iff
    /// anything actually moved (false for an empty bag).
    pub fn sort_inventory(&mut self, client_id: ClientId) -> bool {
        use rift_net::messages::{BAG_COLS, BAG_ROWS};
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
            return false;
        };
        if p.inventory.iter().all(|s| s.is_none()) {
            return false;
        }
        sort_grid_items(&mut p.inventory, BAG_COLS, BAG_ROWS);
        true
    }
}
