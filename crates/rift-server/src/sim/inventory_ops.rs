//! Inventory + equip-slot mutators on [`Sim`]. Split out of the
//! main `sim/mod.rs` so the per-domain surface area stays
//! browsable. Pure `impl Sim` block — every method is already
//! defined on `Sim` and migrated here verbatim.

use rift_net::ids::ClientId;

use super::player::ServerPlayer;
use super::{push_into_sparse, trim_trailing_none, Sim};

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
                item.provenance = Some(
                    rift_game::loot::LootProvenance::from_ids([uuid.into_bytes()]),
                );
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
        let slot = p.equipment.default_slot(&item);
        if !rift_game::loot::Equipment::accepts(slot, &item) {
            // Item base has no equip slot we accept — put it back
            // and bail. (Currently every BaseItem has a real slot,
            // so this branch is defensive.)
            p.inventory[inventory_index] = Some(item);
            return false;
        }
        let displaced = p.equipment.set(slot, Some(item));
        if let Some(prev) = displaced {
            // Re-occupy the same bag slot so the UI position the
            // client just saw stays stable across the swap.
            p.inventory[inventory_index] = Some(prev);
        }
        trim_trailing_none(&mut p.inventory);
        p.recompute_stats();
        true
    }

    /// Move the item currently in `slot` back into the bag (at
    /// the end). Returns `true` if anything actually moved;
    /// `false` for an empty slot or a stale client byte.
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
        push_into_sparse(&mut p.inventory, item);
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

    pub fn swap_inventory_slots(
        &mut self,
        client_id: ClientId,
        a: usize,
        b: usize,
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
        let max = a.max(b);
        if max >= p.inventory.len() {
            // Both indices off the end of the bag = no-op.
            if a >= p.inventory.len() && b >= p.inventory.len() {
                return false;
            }
            p.inventory.resize_with(max + 1, || None);
        }
        p.inventory.swap(a, b);
        trim_trailing_none(&mut p.inventory);
        true
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
        let item = p.inventory.get_mut(inventory_index).and_then(|s| s.take())?;
        trim_trailing_none(&mut p.inventory);
        Some((item, p.k.position))
    }

    /// Move whatever's currently in `slot` into the bag at
    /// `inventory_index`, swapping with whatever is already
    /// there (or growing the bag if the index is past the end).
    /// Returns `true` on success.
    pub fn unequip_to_bag_slot(
        &mut self,
        client_id: ClientId,
        slot: rift_game::loot::EquipSlot,
        inventory_index: usize,
    ) -> bool {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
            return false;
        };
        let Some(unequipped) = p.equipment.take(slot) else {
            return false;
        };
        // Grow the bag to fit the requested index. The displaced
        // item (if any) gets re-equipped if it's compatible with
        // the slot, otherwise it lands at the first free bag
        // slot (or the end).
        if inventory_index >= p.inventory.len() {
            p.inventory.resize_with(inventory_index + 1, || None);
            p.inventory[inventory_index] = Some(unequipped);
        } else {
            let displaced = std::mem::replace(
                &mut p.inventory[inventory_index],
                Some(unequipped),
            );
            if let Some(prev) = displaced {
                if rift_game::loot::Equipment::accepts(slot, &prev) {
                    p.equipment.set(slot, Some(prev));
                } else {
                    push_into_sparse(&mut p.inventory, prev);
                }
            }
        }
        trim_trailing_none(&mut p.inventory);
        p.recompute_stats();
        true
    }
}
