//! Inventory, equipment, and stash handlers. Every client message
//! that mutates a player's items lands in this file. Each handler
//! validates → mutates the sim → broadcasts the resulting
//! inventory + equipment + stash state → queues a persistence
//! reset so the database row set matches the post-mutation
//! layout.

use rift_net::{Channel, ClientId, NetId, ServerMsg};
use rift_persistence::{PersistedItem, PersistedStashTab};

use super::item_to_blob;
use crate::Server;

impl Server {
    /// Validate a loot pickup, broadcast `LootClaimed`, and queue
    /// the persistent inventory append.
    pub(crate) fn handle_pick_up_loot(&mut self, from: ClientId, net_id: NetId) {
        let item = match self.sim_for_client_mut(from).try_pickup_loot(from, net_id) {
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
        //
        // **Skipped for unstable (rift) pickups.** Unstable
        // items must never reach the database; the
        // server-authoritative bag carries them in memory until
        // the run extracts (which calls `persist_inventory_state`
        // *after* `stabilize_inventory` has flipped the flag).
        // If the player disconnects, dies in-rift, or returns
        // to the hub without extracting, the in-memory copy is
        // stripped and the DB still reflects their pre-rift
        // bag — which is exactly the "unstable loot shatters"
        // contract.
        if item.unstable {
            return;
        }
        if let Some(handle) = &self.persistence {
            if let Some(rec) = self.sessions.get(from).and_then(|s| s.record.as_ref()) {
                let (base_id, rarity, ilvl, affixes, anchored) = item.to_persisted();
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
                    anchored,
                    tab_index: 0,
                    provenance: super::provenance_to_persisted(&item),
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
        if !self.sim_for_client_mut(from).equip_from_bag(from, inventory_index) {
            log::debug!(
                "equip: rejected equip request from {from:?} idx={inventory_index}"
            );
            return;
        }
        self.broadcast_inventory_state(from);
        self.broadcast_peer_equipment_visuals(from);
        self.persist_inventory_state(from);
    }

    /// Move whatever's currently in `slot` back into the bag.
    /// No-op for unknown slot bytes or empty slots.
    pub(crate) fn handle_unequip_item(&mut self, from: ClientId, slot_byte: u8) {
        let Some(slot) = rift_game::loot::EquipSlot::from_u8(slot_byte) else {
            log::debug!("unequip: unknown slot byte {slot_byte} from {from:?}");
            return;
        };
        if !self.sim_for_client_mut(from).unequip_to_bag(from, slot) {
            return;
        }
        self.broadcast_inventory_state(from);
        self.broadcast_peer_equipment_visuals(from);
        self.persist_inventory_state(from);
    }

    /// Send the picker fresh `InventorySync` + `EquipmentSync`
    /// frames reflecting the current authoritative state.
    pub(crate) fn broadcast_inventory_state(&mut self, to: ClientId) {
        let bag = self.sim_for_client(to).player_inventory(to);
        let equip = self.sim_for_client(to).player_equipment(to);
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

    /// Compute the list of base-item indices currently equipped by
    /// `who` whose `BaseItem::model_path` is `Some` — i.e. the
    /// pieces that translate into a visible avatar attachment on
    /// other clients. Items without a `model_path` are still
    /// included; the receiving client filters them. Sending all
    /// equipped base ids keeps the wire shape stable as art
    /// catches up to gameplay.
    pub(crate) fn current_visible_base_ids(&self, who: ClientId) -> Vec<u16> {
        self.sim_for_client(who)
            .player_equipment(who)
            .iter()
            .map(|(_, item)| {
                rift_game::loot::BASE_ITEMS
                    .iter()
                    .position(|b| b.id == item.base.id)
                    .map(|p| p as u16)
            })
            .flatten()
            .collect()
    }

    /// Push `who`'s current visible-equipment list to every
    /// other client sharing their world (instance or hub) so
    /// remote avatars stay dressed in lockstep with server-side
    /// equipment changes. The owning client doesn't need this
    /// message — its own attachments are driven by the local
    /// `EquipmentSync` flow.
    pub(crate) fn broadcast_peer_equipment_visuals(&mut self, who: ClientId) {
        let base_ids = self.current_visible_base_ids(who);
        let recipients: Vec<ClientId> = self
            .clients_in_world_with(who)
            .into_iter()
            .filter(|cid| *cid != who)
            .collect();
        if recipients.is_empty() {
            return;
        }
        let msg = ServerMsg::PeerEquipmentVisuals {
            client_id: who,
            base_ids,
        };
        for cid in recipients {
            self.send_to(cid, Channel::Control, &msg);
        }
    }

    /// Catch `to` up on the visible equipment of every other
    /// player currently sharing their world. Called whenever a
    /// client joins a new world group (initial connect, or
    /// crossing between hub and a rift instance) so their
    /// remote-avatar attachments are dressed on first frame.
    pub(crate) fn catch_up_peer_equipment_visuals(&mut self, to: ClientId) {
        let peers: Vec<ClientId> = self
            .clients_in_world_with(to)
            .into_iter()
            .filter(|cid| *cid != to)
            .collect();
        let payloads: Vec<(ClientId, Vec<u16>)> = peers
            .into_iter()
            .map(|cid| (cid, self.current_visible_base_ids(cid)))
            .collect();
        for (cid, base_ids) in payloads {
            self.send_to(
                to,
                Channel::Control,
                &ServerMsg::PeerEquipmentVisuals {
                    client_id: cid,
                    base_ids,
                },
            );
        }
    }

    /// Snapshot the picker's bag + equipment into a flat
    /// `Vec<PersistedItem>` and queue a `ResetCharacterInventory`
    /// so the database row set matches the post-swap layout.
    ///
    /// **Hub-only.** While a player is inside an active rift
    /// instance their inventory is run-state — unstable items
    /// must never reach the database, and stable items don't
    /// change while in-rift (equip/unequip swaps in-rift would
    /// otherwise leak deferred state to disk). The extract
    /// path lifts every player back to the hub *first* and
    /// then calls this, so the post-extraction snapshot still
    /// lands in the DB.
    pub(crate) fn persist_inventory_state(&mut self, from: ClientId) {
        if !self.sim_for_client(from).is_hub() {
            return;
        }
        let Some(handle) = &self.persistence else {
            return;
        };
        let Some(rec_id) = self.sessions.record_id(from) else {
            return;
        };
        let dump = self.sim_for_client(from).dump_player_inventory(from);
        let rows: Vec<PersistedItem> = dump
            .into_iter()
            .map(|(slot, slot_index, item)| {
                let (base_id, rarity, ilvl, affixes, anchored) = item.to_persisted();
                PersistedItem {
                    base_id,
                    rarity: rarity as i16,
                    ilvl: ilvl as i32,
                    affixes,
                    equipped_slot: slot.map(|b| b as i16),
                    slot_index,
                    anchored,
                    tab_index: 0,
                    provenance: super::provenance_to_persisted(&item),
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
    /// purely visual hub asset in the hub the player is
    /// effectively at the chest the moment they open the panel.
    pub(crate) fn handle_open_stash(&mut self, from: ClientId) {
        if self.floor_for_client(from) != 0 {
            log::debug!("stash: open rejected for {from:?} not in hub");
            return;
        }
        // Stash lives only on the hub sim; players in the rift
        // can't reach the chest at all.
        self.hub.set_stash_open(from, true);
        self.send_stash_state(from);
    }

    /// End the stash session for `from`. Subsequent
    /// deposit / withdraw requests are rejected until a fresh
    /// `OpenStash` arrives.
    pub(crate) fn handle_close_stash(&mut self, from: ClientId) {
        self.hub.set_stash_open(from, false);
    }

    /// Move the bag item at `inventory_index` into stash tab
    /// `tab_index` and re-broadcast both inventories. Persists
    /// both tables.
    pub(crate) fn handle_deposit_to_stash(
        &mut self,
        from: ClientId,
        inventory_index: usize,
        tab_index: usize,
    ) {
        if !self.hub.is_stash_open(from) {
            log::debug!("stash: deposit rejected for {from:?} stash not open");
            return;
        }
        if !self.hub.deposit_to_stash(from, inventory_index, tab_index) {
            log::debug!(
                "stash: deposit rejected for {from:?} idx={inventory_index} tab={tab_index}"
            );
            return;
        }
        self.broadcast_inventory_state(from);
        self.send_stash_state(from);
        self.persist_inventory_state(from);
        self.persist_stash_state(from);
    }

    /// Move the stash item at `(tab_index, stash_index)` back
    /// into the bag and re-broadcast both inventories. Persists
    /// both tables.
    pub(crate) fn handle_withdraw_from_stash(
        &mut self,
        from: ClientId,
        tab_index: usize,
        stash_index: usize,
    ) {
        if !self.hub.is_stash_open(from) {
            log::debug!("stash: withdraw rejected for {from:?} stash not open");
            return;
        }
        if !self.hub.withdraw_from_stash(from, tab_index, stash_index) {
            log::debug!(
                "stash: withdraw rejected for {from:?} tab={tab_index} idx={stash_index}"
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
        tab_index: usize,
        stash_index: usize,
    ) {
        if !self.hub.is_stash_open(from) {
            log::debug!("stash: deposit-slot rejected for {from:?} stash not open");
            return;
        }
        if !self
            .hub
            .deposit_to_stash_slot(from, inventory_index, tab_index, stash_index)
        {
            log::debug!(
                "stash: deposit-slot rejected for {from:?} inv={inventory_index} \
                 tab={tab_index} stash={stash_index}"
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
        tab_index: usize,
        stash_index: usize,
        inventory_index: usize,
    ) {
        if !self.hub.is_stash_open(from) {
            log::debug!("stash: withdraw-slot rejected for {from:?} stash not open");
            return;
        }
        if !self
            .hub
            .withdraw_from_stash_slot(from, tab_index, stash_index, inventory_index)
        {
            log::debug!(
                "stash: withdraw-slot rejected for {from:?} tab={tab_index} \
                 stash={stash_index} inv={inventory_index}"
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
        let stash = self.hub.player_stash(to);
        let tabs: Vec<rift_net::messages::StashTabBlob> = stash
            .into_iter()
            .map(|tab| rift_net::messages::StashTabBlob {
                name: tab.name,
                color: tab.color,
                items: tab.items.iter().map(|s| s.as_ref().map(item_to_blob)).collect(),
            })
            .collect();
        self.send_to(to, Channel::Control, &ServerMsg::StashSync { tabs });
    }

    /// Snapshot the picker's stash into a flat
    /// `Vec<PersistedItem>` (one row per tab × slot) plus the
    /// per-tab metadata, and queue a `ResetCharacterStash` so
    /// the database row set matches the post-transfer layout.
    pub(crate) fn persist_stash_state(&mut self, from: ClientId) {
        let Some(handle) = &self.persistence else {
            return;
        };
        let Some(rec_id) = self.sessions.record_id(from) else {
            return;
        };
        let stash = self.hub.dump_player_stash(from);
        let mut tab_rows: Vec<PersistedStashTab> = Vec::with_capacity(stash.len());
        let mut item_rows: Vec<PersistedItem> = Vec::new();
        for (tab_index, tab) in stash.into_iter().enumerate() {
            tab_rows.push(PersistedStashTab {
                tab_index: tab_index as i16,
                name: tab.name,
                color: tab.color as i32,
            });
            for (slot_index, opt) in tab.items.into_iter().enumerate() {
                let Some(item) = opt else { continue };
                let (base_id, rarity, ilvl, affixes, anchored) = item.to_persisted();
                item_rows.push(PersistedItem {
                    base_id,
                    rarity: rarity as i16,
                    ilvl: ilvl as i32,
                    affixes,
                    equipped_slot: None,
                    slot_index: slot_index as i32,
                    anchored,
                    tab_index: tab_index as i16,
                    provenance: super::provenance_to_persisted(&item),
                });
            }
        }
        if !handle.reset_character_stash(rec_id, tab_rows, item_rows) {
            log::warn!("persistence: reset_character_stash dropped for {from:?}");
        }
    }

    /// Reorder the bag: swap two slots. Either may be empty;
    /// see [`Sim::swap_inventory_slots`].
    pub(crate) fn handle_swap_inventory_slots(&mut self, from: ClientId, a: usize, b: usize) {
        if !self.sim_for_client_mut(from).swap_inventory_slots(from, a, b) {
            log::debug!("inv: swap rejected for {from:?} a={a} b={b}");
            return;
        }
        self.broadcast_inventory_state(from);
        self.persist_inventory_state(from);
    }

    /// Reorder the stash: swap two slots within `tab_index`.
    /// Either may be empty. Requires an open stash session.
    /// Persists the stash on success.
    pub(crate) fn handle_swap_stash_slots(
        &mut self,
        from: ClientId,
        tab_index: usize,
        a: usize,
        b: usize,
    ) {
        if !self.hub.is_stash_open(from) {
            log::debug!("stash: swap rejected for {from:?} stash not open");
            return;
        }
        if !self.hub.swap_stash_slots(from, tab_index, a, b) {
            log::debug!("stash: swap rejected for {from:?} tab={tab_index} a={a} b={b}");
            return;
        }
        self.send_stash_state(from);
        self.persist_stash_state(from);
    }

    /// Spend shards to add another stash tab. Replies with
    /// fresh `StashSync` + `ShardsSync` on success and persists
    /// both the tab list (via `persist_stash_state`) and the
    /// new shard balance (via `persist_shards_for`).
    pub(crate) fn handle_buy_stash_tab(&mut self, from: ClientId) {
        if !self.hub.is_stash_open(from) {
            log::debug!("stash: buy-tab rejected for {from:?} stash not open");
            return;
        }
        let Some((new_shards, new_count)) = self.hub.buy_stash_tab(from) else {
            log::debug!("stash: buy-tab rejected for {from:?} insufficient or capped");
            return;
        };
        log::info!("stash: {from:?} bought tab #{new_count} (shards={new_shards})");
        self.send_stash_state(from);
        self.send_to(from, Channel::Control, &ServerMsg::ShardsSync { amount: new_shards });
        self.persist_stash_state(from);
        self.persist_shards_for(from, new_shards);
    }

    /// Rename a stash tab. Empty / whitespace-only names are
    /// dropped silently (the client should validate before
    /// sending).
    pub(crate) fn handle_rename_stash_tab(
        &mut self,
        from: ClientId,
        tab_index: usize,
        name: &str,
    ) {
        if !self.hub.is_stash_open(from) {
            log::debug!("stash: rename rejected for {from:?} stash not open");
            return;
        }
        if !self.hub.rename_stash_tab(from, tab_index, name) {
            log::debug!("stash: rename rejected for {from:?} tab={tab_index}");
            return;
        }
        self.send_stash_state(from);
        self.persist_stash_state(from);
    }

    /// Recolor a stash tab.
    pub(crate) fn handle_recolor_stash_tab(
        &mut self,
        from: ClientId,
        tab_index: usize,
        color: u32,
    ) {
        if !self.hub.is_stash_open(from) {
            log::debug!("stash: recolor rejected for {from:?} stash not open");
            return;
        }
        if !self.hub.recolor_stash_tab(from, tab_index, color) {
            log::debug!("stash: recolor rejected for {from:?} tab={tab_index}");
            return;
        }
        self.send_stash_state(from);
        self.persist_stash_state(from);
    }

    /// Drop the bag item at `inventory_index` onto the ground at
    /// the picker's current position. Removes the row, spawns a
    /// fresh `ServerLoot` tagged with a [`crate::sim::SHARE_WINDOW_TICKS`]
    /// eligibility snapshot, and queues `WorldEvent::LootDropped`
    /// so every observer's loot pillar appears.
    ///
    /// Refuses while the dropper is in the hub: under the
    /// level-requirement system, dropping endgame gear in town
    /// is the obvious twink vector, so the rule is simply "no
    /// drops in town." Stash + bag still cover storage in the
    /// hub.
    pub(crate) fn handle_drop_inventory_item(&mut self, from: ClientId, inventory_index: usize) {
        if self.sim_for_client(from).is_hub() {
            log::debug!(
                "inv: drop rejected for {from:?} idx={inventory_index} (in hub)"
            );
            return;
        }
        let Some((item, pos)) = self.sim_for_client_mut(from).pop_inventory_item(from, inventory_index) else {
            log::debug!(
                "inv: drop rejected for {from:?} idx={inventory_index}"
            );
            return;
        };
        log::info!(
            "inv: {from:?} dropped {} at {pos:?}",
            item.display_name(),
        );
        self.sim_for_client_mut(from).spawn_player_drop(item, pos, from);
        self.broadcast_inventory_state(from);
        self.persist_inventory_state(from);
    }

    /// Salvage the bag item at `inventory_index` for shards.
    /// Sends back a fresh `InventorySync` (slot is gone) and a
    /// `ShardsSync` (new total), and persists both the bag and
    /// the new shard balance. Anchored items are rejected
    /// server-side (see `Sim::salvage_inventory_item`) so the
    /// client doesn't have to special-case them — a rejected
    /// salvage just produces no broadcast.
    pub(crate) fn handle_salvage_inventory_item(
        &mut self,
        from: ClientId,
        inventory_index: usize,
    ) {
        let Some(new_total) = self
            .sim_for_client_mut(from)
            .salvage_inventory_item(from, inventory_index)
        else {
            log::debug!(
                "inv: salvage rejected for {from:?} idx={inventory_index}"
            );
            return;
        };
        log::info!(
            "inv: {from:?} salvaged idx={inventory_index} -> {new_total} shards"
        );
        self.broadcast_inventory_state(from);
        self.send_to(
            from,
            Channel::Control,
            &ServerMsg::ShardsSync { amount: new_total },
        );
        self.persist_inventory_state(from);
        self.persist_shards_for(from, new_total);
    }

    /// Bulk-salvage every non-anchored bag item whose rarity is
    /// at most `rarity_max`. Always emits an
    /// `InventorySync` + `ShardsSync` even when nothing
    /// matched, so the client UI can re-render and reset its
    /// "confirm" state. Persists when at least one item was
    /// actually salvaged.
    pub(crate) fn handle_salvage_inventory_bulk(
        &mut self,
        from: ClientId,
        rarity_max: u8,
    ) {
        let Some((count, new_total)) = self
            .sim_for_client_mut(from)
            .salvage_inventory_bulk(from, rarity_max)
        else {
            log::debug!("inv: bulk salvage rejected for {from:?}");
            return;
        };
        log::info!(
            "inv: {from:?} bulk-salvaged {count} items (rarity<={rarity_max}) -> {new_total} shards"
        );
        self.broadcast_inventory_state(from);
        self.send_to(
            from,
            Channel::Control,
            &ServerMsg::ShardsSync { amount: new_total },
        );
        if count > 0 {
            self.persist_inventory_state(from);
            self.persist_shards_for(from, new_total);
        }
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
        if !self.sim_for_client_mut(from).unequip_to_bag_slot(from, slot, inventory_index) {
            return;
        }
        self.broadcast_inventory_state(from);
        self.broadcast_peer_equipment_visuals(from);
        self.persist_inventory_state(from);
    }
}
