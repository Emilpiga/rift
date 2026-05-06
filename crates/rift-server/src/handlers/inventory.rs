//! Inventory, equipment, and stash handlers. Every client message
//! that mutates a player's items lands in this file. Each handler
//! validates → mutates the sim → broadcasts the resulting
//! inventory + equipment + stash state → queues a persistence
//! reset so the database row set matches the post-mutation
//! layout.

use rift_net::{Channel, ClientId, NetId, ServerMsg};
use rift_persistence::PersistedItem;

use super::item_to_blob;
use crate::Server;

impl Server {
    /// Validate a loot pickup, broadcast `LootClaimed`, and queue
    /// the persistent inventory append.
    pub(crate) fn handle_pick_up_loot(&mut self, from: ClientId, net_id: NetId) {
        let item = match self.sim.try_pickup_loot(from, net_id) {
            Ok(item) => item,
            Err(Some(reason)) => {
                log::info!(
                    "loot pickup rejected for {from:?}: {reason:?}"
                );
                self.send_to(
                    from,
                    Channel::Control,
                    &ServerMsg::PickupRejected { loot: net_id, reason },
                );
                return;
            }
            Err(None) => return,
        };
        log::info!(
            "loot picked: {} (item-level {}) by {from:?}",
            item.display_name(),
            item.ilvl,
        );
        self.broadcast(
            Channel::Control,
            &ServerMsg::LootClaimed {
                loot: net_id,
                claimed_by: from,
            },
        );
        // Push a fresh `InventorySync` to the picker so their
        // local mirror lands in the *exact* slot the
        // authoritative bag chose (`push_into_sparse` fills the
        // first hole, which the client can't reproduce on its
        // own without re-implementing the same logic). Every
        // other inventory mutation does this; pickup needs it
        // too or drop-then-pickup leaves the UI desynced until
        // the next equip / swap forces a sync.
        self.broadcast_inventory_state(from);
        // Persist the pickup. We look up the picker's
        // `character_id` via the cached `CharacterRecord` in
        // their session and queue a fire-and-forget INSERT — the
        // server-authoritative bag has already been updated by
        // `try_pickup_loot`, so a dropped/late write is
        // recoverable on the next session by re-rolling.
        if let Some(handle) = &self.persistence {
            if let Some(rec) = self.sessions.get(from).and_then(|s| s.record.as_ref()) {
                let (base_id, rarity, ilvl, affixes) = item.to_persisted();
                let persisted = PersistedItem {
                    base_id,
                    rarity: rarity as i16,
                    ilvl: ilvl as i32,
                    affixes,
                    equipped_slot: None,
                    // The append SQL computes the next free
                    // bag-position via `MAX(slot_index)+1`, so
                    // the field's value here is ignored.
                    slot_index: 0,
                };
                if !handle.append_inventory_item(rec.id, persisted) {
                    log::warn!("persistence: append_inventory_item dropped for {from:?}");
                }
            }
        }
    }

    /// Move the picker's bag item at `inventory_index` into its
    /// canonical equipment slot, broadcast the resulting
    /// inventory + equipment state to the picker, and queue a
    /// `ResetCharacterInventory` so the persisted snapshot
    /// matches the in-memory swap.
    pub(crate) fn handle_equip_item(&mut self, from: ClientId, inventory_index: usize) {
        if !self.sim.equip_from_bag(from, inventory_index) {
            log::debug!(
                "equip: rejected equip request from {from:?} idx={inventory_index}"
            );
            return;
        }
        self.broadcast_inventory_state(from);
        self.persist_inventory_state(from);
    }

    /// Move whatever's currently in `slot` back into the bag.
    /// No-op for unknown slot bytes or empty slots.
    pub(crate) fn handle_unequip_item(&mut self, from: ClientId, slot_byte: u8) {
        let Some(slot) = rift_game::loot::EquipSlot::from_u8(slot_byte) else {
            log::debug!("unequip: unknown slot byte {slot_byte} from {from:?}");
            return;
        };
        if !self.sim.unequip_to_bag(from, slot) {
            return;
        }
        self.broadcast_inventory_state(from);
        self.persist_inventory_state(from);
    }

    /// Send the picker fresh `InventorySync` + `EquipmentSync`
    /// frames reflecting the current authoritative state.
    pub(crate) fn broadcast_inventory_state(&mut self, to: ClientId) {
        let bag = self.sim.player_inventory(to);
        let equip = self.sim.player_equipment(to);
        let inv_blobs: Vec<Option<rift_net::messages::ItemBlob>> =
            bag.iter().map(|s| s.as_ref().map(item_to_blob)).collect();
        let equip_blobs: Vec<(u8, rift_net::messages::ItemBlob)> = equip
            .iter()
            .map(|(s, it)| (s.to_u8(), item_to_blob(it)))
            .collect();
        self.send_to(
            to,
            Channel::Control,
            &ServerMsg::InventorySync { items: inv_blobs },
        );
        self.send_to(
            to,
            Channel::Control,
            &ServerMsg::EquipmentSync { slots: equip_blobs },
        );
    }

    /// Snapshot the picker's bag + equipment into a flat
    /// `Vec<PersistedItem>` and queue a `ResetCharacterInventory`
    /// so the database row set matches the post-swap layout.
    pub(crate) fn persist_inventory_state(&mut self, from: ClientId) {
        let Some(handle) = &self.persistence else {
            return;
        };
        let Some(rec_id) = self.sessions.record_id(from) else {
            return;
        };
        let dump = self.sim.dump_player_inventory(from);
        let rows: Vec<PersistedItem> = dump
            .into_iter()
            .map(|(slot, slot_index, item)| {
                let (base_id, rarity, ilvl, affixes) = item.to_persisted();
                PersistedItem {
                    base_id,
                    rarity: rarity as i16,
                    ilvl: ilvl as i32,
                    affixes,
                    equipped_slot: slot.map(|b| b as i16),
                    slot_index,
                }
            })
            .collect();
        if !handle.reset_character_inventory(rec_id, rows) {
            log::warn!("persistence: reset_character_inventory dropped for {from:?}");
        }
    }

    /// Begin a stash session for `from`: validate the player is
    /// in the hub, mark the per-player `stash_open` flag, and
    /// reply with the current stash contents. Range validation
    /// is left to the client because the chest position is a
    /// purely visual hub asset \u2014 in the hub the player is
    /// effectively at the chest the moment they open the panel.
    pub(crate) fn handle_open_stash(&mut self, from: ClientId) {
        if self.sim.floor_index != 0 {
            log::debug!("stash: open rejected for {from:?} — not in hub");
            return;
        }
        self.sim.set_stash_open(from, true);
        self.send_stash_state(from);
    }

    /// End the stash session for `from`. Subsequent
    /// deposit / withdraw requests are rejected until a fresh
    /// `OpenStash` arrives.
    pub(crate) fn handle_close_stash(&mut self, from: ClientId) {
        self.sim.set_stash_open(from, false);
    }

    /// Move the bag item at `inventory_index` into the stash and
    /// re-broadcast both inventories. Persists both tables.
    pub(crate) fn handle_deposit_to_stash(&mut self, from: ClientId, inventory_index: usize) {
        if !self.sim.is_stash_open(from) {
            log::debug!("stash: deposit rejected for {from:?} — stash not open");
            return;
        }
        if !self.sim.deposit_to_stash(from, inventory_index) {
            log::debug!(
                "stash: deposit rejected for {from:?} idx={inventory_index}"
            );
            return;
        }
        self.broadcast_inventory_state(from);
        self.send_stash_state(from);
        self.persist_inventory_state(from);
        self.persist_stash_state(from);
    }

    /// Move the stash item at `stash_index` back into the bag
    /// and re-broadcast both inventories. Persists both tables.
    pub(crate) fn handle_withdraw_from_stash(&mut self, from: ClientId, stash_index: usize) {
        if !self.sim.is_stash_open(from) {
            log::debug!("stash: withdraw rejected for {from:?} — stash not open");
            return;
        }
        if !self.sim.withdraw_from_stash(from, stash_index) {
            log::debug!(
                "stash: withdraw rejected for {from:?} idx={stash_index}"
            );
            return;
        }
        self.broadcast_inventory_state(from);
        self.send_stash_state(from);
        self.persist_inventory_state(from);
        self.persist_stash_state(from);
    }

    /// Deposit into a specific stash slot (drag-and-drop target).
    pub(crate) fn handle_deposit_to_stash_slot(
        &mut self,
        from: ClientId,
        inventory_index: usize,
        stash_index: usize,
    ) {
        if !self.sim.is_stash_open(from) {
            log::debug!("stash: deposit-slot rejected for {from:?} — stash not open");
            return;
        }
        if !self
            .sim
            .deposit_to_stash_slot(from, inventory_index, stash_index)
        {
            log::debug!(
                "stash: deposit-slot rejected for {from:?} inv={inventory_index} \
                 stash={stash_index}"
            );
            return;
        }
        self.broadcast_inventory_state(from);
        self.send_stash_state(from);
        self.persist_inventory_state(from);
        self.persist_stash_state(from);
    }

    /// Withdraw into a specific bag slot (drag-and-drop target).
    pub(crate) fn handle_withdraw_from_stash_slot(
        &mut self,
        from: ClientId,
        stash_index: usize,
        inventory_index: usize,
    ) {
        if !self.sim.is_stash_open(from) {
            log::debug!("stash: withdraw-slot rejected for {from:?} — stash not open");
            return;
        }
        if !self
            .sim
            .withdraw_from_stash_slot(from, stash_index, inventory_index)
        {
            log::debug!(
                "stash: withdraw-slot rejected for {from:?} stash={stash_index} \
                 inv={inventory_index}"
            );
            return;
        }
        self.broadcast_inventory_state(from);
        self.send_stash_state(from);
        self.persist_inventory_state(from);
        self.persist_stash_state(from);
    }

    /// Send the picker a fresh `StashSync` reflecting the
    /// current authoritative stash contents.
    pub(crate) fn send_stash_state(&mut self, to: ClientId) {
        let stash = self.sim.player_stash(to);
        let blobs: Vec<Option<rift_net::messages::ItemBlob>> =
            stash.iter().map(|s| s.as_ref().map(item_to_blob)).collect();
        self.send_to(
            to,
            Channel::Control,
            &ServerMsg::StashSync { items: blobs },
        );
    }

    /// Snapshot the picker's stash into a flat
    /// `Vec<PersistedItem>` and queue a `ResetCharacterStash`
    /// so the database row set matches the post-transfer layout.
    pub(crate) fn persist_stash_state(&mut self, from: ClientId) {
        let Some(handle) = &self.persistence else {
            return;
        };
        let Some(rec_id) = self.sessions.record_id(from) else {
            return;
        };
        let stash = self.sim.dump_player_stash(from);
        let rows: Vec<PersistedItem> = stash
            .into_iter()
            .enumerate()
            .filter_map(|(slot_index, opt)| {
                let item = opt?;
                let (base_id, rarity, ilvl, affixes) = item.to_persisted();
                Some(PersistedItem {
                    base_id,
                    rarity: rarity as i16,
                    ilvl: ilvl as i32,
                    affixes,
                    equipped_slot: None,
                    slot_index: slot_index as i32,
                })
            })
            .collect();
        if !handle.reset_character_stash(rec_id, rows) {
            log::warn!("persistence: reset_character_stash dropped for {from:?}");
        }
    }

    /// Reorder the bag: swap two slots. Either may be empty;
    /// see [`Sim::swap_inventory_slots`].
    pub(crate) fn handle_swap_inventory_slots(&mut self, from: ClientId, a: usize, b: usize) {
        if !self.sim.swap_inventory_slots(from, a, b) {
            log::debug!("inv: swap rejected for {from:?} a={a} b={b}");
            return;
        }
        self.broadcast_inventory_state(from);
        self.persist_inventory_state(from);
    }

    /// Reorder the stash: swap two slots. Either may be empty.
    /// Requires an open stash session. Persists the stash on
    /// success.
    pub(crate) fn handle_swap_stash_slots(&mut self, from: ClientId, a: usize, b: usize) {
        if !self.sim.is_stash_open(from) {
            log::debug!("stash: swap rejected for {from:?} — stash not open");
            return;
        }
        if !self.sim.swap_stash_slots(from, a, b) {
            log::debug!("stash: swap rejected for {from:?} a={a} b={b}");
            return;
        }
        self.send_stash_state(from);
        self.persist_stash_state(from);
    }

    /// Drop the bag item at `inventory_index` onto the ground at
    /// the picker's current position. Removes the row, spawns a
    /// fresh `ServerLoot`, and queues `WorldEvent::LootDropped`
    /// so every observer's loot pillar appears.
    pub(crate) fn handle_drop_inventory_item(&mut self, from: ClientId, inventory_index: usize) {
        let Some((item, pos)) = self.sim.pop_inventory_item(from, inventory_index) else {
            log::debug!(
                "inv: drop rejected for {from:?} idx={inventory_index}"
            );
            return;
        };
        log::info!(
            "inv: {from:?} dropped {} at {pos:?}",
            item.display_name(),
        );
        self.sim.spawn_dropped_loot(item, pos);
        self.broadcast_inventory_state(from);
        self.persist_inventory_state(from);
    }

    /// Move whatever's currently in `slot` into the bag at
    /// `inventory_index` (drag-drop counterpart to
    /// `handle_unequip_item`).
    pub(crate) fn handle_unequip_to_bag_slot(
        &mut self,
        from: ClientId,
        slot_byte: u8,
        inventory_index: usize,
    ) {
        let Some(slot) = rift_game::loot::EquipSlot::from_u8(slot_byte) else {
            log::debug!("unequip: unknown slot byte {slot_byte} from {from:?}");
            return;
        };
        if !self.sim.unequip_to_bag_slot(from, slot, inventory_index) {
            return;
        }
        self.broadcast_inventory_state(from);
        self.persist_inventory_state(from);
    }
}
