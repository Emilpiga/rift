//! Stash, shards, and salvage methods on [`Sim`]. Split out of
//! the main `sim/mod.rs` so the per-domain surface area stays
//! browsable. Pure `impl Sim` block — every method is already
//! defined on `Sim` and migrated here verbatim.

use rift_net::ids::ClientId;

use super::actor::Vitals;
use super::player::{ServerPlayer, StashTab};
use super::{
    build_stash_occupancy, footprint_fits_stash, place_inventory_item, place_inventory_item_at,
    place_stash_item, place_stash_item_at, salvage_yield, snapshot_talents, sort_grid_items,
    trim_trailing_none, Sim,
};

impl Sim {
    /// Borrow the player's stash (read-only). Used by the
    /// server's dispatch path to encode `StashSync` payloads.
    /// One [`StashTab`] per page; items inside each tab are
    /// sparse like [`Self::player_inventory`].
    pub fn player_stash(&self, client_id: ClientId) -> Vec<StashTab> {
        self.sessions
            .get(&client_id)
            .and_then(|&e| self.world.get::<&ServerPlayer>(e).ok())
            .map(|p| p.stash.clone())
            .unwrap_or_default()
    }

    /// Hydrate a freshly-spawned player's stash from the
    /// pre-loaded tab list (typically built by
    /// `handlers::session::hydrate_player_state` from the rows
    /// fetched by `PersistenceHandle::load_stash_blocking`).
    /// Idempotent. Empty input is replaced with one default
    /// tab so the player always sees at least "Tab 1".
    pub fn set_player_stash(&mut self, client_id: ClientId, mut tabs: Vec<StashTab>) {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return;
        };
        if let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) {
            if tabs.is_empty() {
                tabs.push(StashTab::fresh(0));
            }
            for tab in tabs.iter_mut() {
                trim_trailing_none(&mut tab.items);
            }
            p.stash = tabs;
        }
    }

    /// Hydrate a freshly-spawned player's salvage currency from
    /// the persisted record (`characters.shards`). Idempotent;
    /// called once at hello time after `set_player_experience`.
    pub fn set_player_shards(&mut self, client_id: ClientId, shards: u32) {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return;
        };
        if let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) {
            p.shards = shards;
        }
    }

    /// Read a player's current shard balance. `None` when the
    /// client has no entity in this sim.
    pub fn player_shards(&self, client_id: ClientId) -> Option<u32> {
        let &entity = self.sessions.get(&client_id)?;
        let p = self.world.get::<&ServerPlayer>(entity).ok()?;
        Some(p.shards)
    }

    /// Salvage the bag item at `inventory_index` for shards.
    /// Returns `Some(new_total)` on success and `None` if the
    /// slot was empty / out of range. Anchored items (the
    /// special legendary trait) are protected: they can't be
    /// salvaged so the player never accidentally loses the
    /// rare drop they intentionally locked.
    ///
    /// Yield: a per-rarity base value scaled gently by ilvl.
    /// Common 1, Magic 3, Rare 8, Legendary 25 — multiplied by
    /// `1 + ilvl/20` so deeper-floor drops are worth more
    /// without making early-game salvage feel useless.
    pub fn salvage_inventory_item(
        &mut self,
        client_id: ClientId,
        inventory_index: usize,
    ) -> Option<u32> {
        let &entity = self.sessions.get(&client_id)?;
        let mut p = self.world.get::<&mut ServerPlayer>(entity).ok()?;
        let slot = p.inventory.get_mut(inventory_index)?;
        let item = slot.as_ref()?;
        if item.anchored {
            return None;
        }
        if item.consumable_kind().is_some() {
            // Consumables aren't currency-bearing items \u2014
            // shredding a respec token would silently destroy
            // it for zero shards. Reject so the UI can keep the
            // token in the bag.
            return None;
        }
        let yield_amt = salvage_yield(item.rarity, item.ilvl);
        *slot = None;
        trim_trailing_none(&mut p.inventory);
        p.shards = p.shards.saturating_add(yield_amt);
        Some(p.shards)
    }

    /// Consume a bag-only consumable item.
    ///
    /// Returns `Some(touched_talents)` on success, where
    /// `touched_talents` is `Some((invested, unspent))` iff the
    /// consumable mutated the player's talent tree (so the
    /// handler knows to push a `TalentsSync` alongside the
    /// always-emitted `InventorySync`). Outer `None` is
    /// rejection (out-of-range index, empty slot, item is not a
    /// consumable, or the consumable's effect refused to apply
    /// \u2014 e.g. `LesserRespecToken` with an orphaning
    /// target).
    ///
    /// On accept the bag slot is cleared and trailing `None`s
    /// are trimmed, mirroring the salvage path. The dispatch
    /// is by [`rift_game::loot::ConsumableKind`]; new kinds
    /// add an arm here + a `BaseItem` row.
    pub fn use_bag_consumable(
        &mut self,
        client_id: ClientId,
        inventory_index: usize,
        target_arg: u16,
    ) -> Option<Option<(Vec<(u16, u8)>, u32)>> {
        use rift_game::loot::ConsumableKind;
        let &entity = self.sessions.get(&client_id)?;
        let mut p = self.world.get::<&mut ServerPlayer>(entity).ok()?;
        let slot = p.inventory.get(inventory_index)?;
        let item = slot.as_ref()?;
        let kind = item.consumable_kind()?;
        let talent_snapshot: Option<(Vec<(u16, u8)>, u32)> = match kind {
            ConsumableKind::GreaterRespecToken => {
                p.talents.refund_all();
                Some(snapshot_talents(&p.talents))
            }
            ConsumableKind::LesserRespecToken => {
                let id = rift_game::talents::TalentId(target_arg);
                if p.talents.refund_one(id) == 0 {
                    // Refund refused (unknown id, no ranks, or
                    // would orphan a downstream node). Leave
                    // the token in the bag so the player can
                    // retry against a different target.
                    return None;
                }
                Some(snapshot_talents(&p.talents))
            }
        };
        // Effect applied successfully \u2014 burn the token.
        p.inventory[inventory_index] = None;
        trim_trailing_none(&mut p.inventory);
        Some(talent_snapshot)
    }

    /// Bulk-salvage every non-anchored bag item whose rarity
    /// is at most `rarity_max`. Returns
    /// `(items_salvaged, new_shard_total)` so the handler can
    /// log a useful summary. A no-op (returns `(0, current)`)
    /// when nothing matches — the handler still issues a sync
    /// in that case so the client UI re-renders cleanly.
    pub fn salvage_inventory_bulk(
        &mut self,
        client_id: ClientId,
        rarity_max: u8,
    ) -> Option<(u32, u32)> {
        let &entity = self.sessions.get(&client_id)?;
        let mut p = self.world.get::<&mut ServerPlayer>(entity).ok()?;
        let mut count: u32 = 0;
        let mut gained: u32 = 0;
        for slot in p.inventory.iter_mut() {
            // Take the slot only if it matches; otherwise leave
            // it in place so we don't churn `Option` payloads.
            let salvage = match slot.as_ref() {
                Some(it)
                    if !it.anchored
                        && (it.rarity as u8) <= rarity_max
                        && it.consumable_kind().is_none() =>
                {
                    true
                }
                _ => false,
            };
            if salvage {
                if let Some(it) = slot.take() {
                    gained = gained.saturating_add(salvage_yield(it.rarity, it.ilvl));
                    count += 1;
                }
            }
        }
        if count > 0 {
            trim_trailing_none(&mut p.inventory);
            p.shards = p.shards.saturating_add(gained);
        }
        Some((count, p.shards))
    }

    /// Toggle the per-player "stash session is open" flag.
    /// Set to `true` on a successful `OpenStash`, `false` on
    /// `CloseStash` / disconnect / floor transition. Gates
    /// every deposit / withdraw so an out-of-band transfer
    /// from a far-away client is rejected at the server edge.
    pub fn set_stash_open(&mut self, client_id: ClientId, open: bool) {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return;
        };
        if let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) {
            p.stash_open = open;
        }
    }

    /// Whether `client_id`'s current session has the chest open.
    pub fn is_stash_open(&self, client_id: ClientId) -> bool {
        self.sessions
            .get(&client_id)
            .and_then(|&e| self.world.get::<&ServerPlayer>(e).ok())
            .map(|p| p.stash_open)
            .unwrap_or(false)
    }

    /// Shared hub chest visual state: `true` while one or more
    /// players have their private stash session open. This is
    /// intentionally aggregate-only so clients can animate the
    /// world prop without learning whose stash is open or seeing
    /// private contents.
    pub fn any_stash_open(&self) -> bool {
        self.sessions.values().any(|&entity| {
            self.world
                .get::<&ServerPlayer>(entity)
                .map(|p| p.stash_open)
                .unwrap_or(false)
        })
    }

    /// Move the bag item at `inventory_index` into the first
    /// free anchor of stash tab `tab_index` whose footprint
    /// fits. Returns `true` on success; `false` if either index
    /// is out of range or no anchor fits the item.
    pub fn deposit_to_stash(
        &mut self,
        client_id: ClientId,
        inventory_index: usize,
        tab_index: usize,
    ) -> bool {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
            return false;
        };
        if tab_index >= p.stash.len() {
            return false;
        }
        let Some(item) = p.inventory.get_mut(inventory_index).and_then(|s| s.take()) else {
            return false;
        };
        if place_stash_item(&mut p.stash[tab_index].items, item.clone()).is_some() {
            true
        } else {
            // Restore on failure — never silently drop loot.
            if inventory_index < p.inventory.len() {
                p.inventory[inventory_index] = Some(item);
            } else {
                let _ = place_inventory_item(&mut p.inventory, item);
            }
            false
        }
    }

    /// Move the stash item at `(tab_index, stash_index)` into
    /// the first free anchor of the bag whose footprint fits.
    /// Returns `true` on success; `false` on out-of-range or
    /// when the bag has no room.
    pub fn withdraw_from_stash(
        &mut self,
        client_id: ClientId,
        tab_index: usize,
        stash_index: usize,
    ) -> bool {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
            return false;
        };
        if tab_index >= p.stash.len() {
            return false;
        }
        let Some(item) = p.stash[tab_index]
            .items
            .get_mut(stash_index)
            .and_then(|s| s.take())
        else {
            return false;
        };
        if place_inventory_item(&mut p.inventory, item.clone()).is_some() {
            true
        } else {
            // Restore on failure.
            let items = &mut p.stash[tab_index].items;
            if stash_index < items.len() {
                items[stash_index] = Some(item);
            } else {
                let _ = place_stash_item(items, item);
            }
            false
        }
    }

    /// Take the stash item at `(tab_index, stash_index)` and
    /// equip it into its canonical slot. If the slot is already
    /// filled, the displaced item is pushed back into the
    /// vacated stash cell (preferred), or the first free bag
    /// anchor as a fallback. Returns `false` and rolls back on
    /// any failure.
    pub fn equip_from_stash(
        &mut self,
        client_id: ClientId,
        tab_index: usize,
        stash_index: usize,
    ) -> bool {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok((p, vitals)) = self
            .world
            .query_one_mut::<(&mut ServerPlayer, &mut Vitals)>(entity)
        else {
            return false;
        };
        if tab_index >= p.stash.len() {
            return false;
        }
        let Some(mut item) = p.stash[tab_index]
            .items
            .get_mut(stash_index)
            .and_then(|s| s.take())
        else {
            return false;
        };
        // Self-bind legacy / unprovenanced items, mirroring
        // `equip_from_bag`.
        if item.provenance.is_none() {
            if let Some(uuid) = p.character_id {
                item.provenance = Some(rift_game::loot::LootProvenance::from_ids([
                    uuid.into_bytes()
                ]));
            }
        }
        // Level requirement gate. Restore on failure.
        if p.level < item.required_level() {
            log::debug!(
                "equip_from_stash: rejected (under-level) client={client_id:?} \
                 tab={tab_index} idx={stash_index} player_level={} item={} req={}",
                p.level,
                item.display_name(),
                item.required_level(),
            );
            p.stash[tab_index].items[stash_index] = Some(item);
            return false;
        }
        let Some(slot) = p.equipment.default_slot(&item) else {
            // Bag-only item (consumable) with no target slot.
            // Restore and bail — nothing else to validate.
            p.stash[tab_index].items[stash_index] = Some(item);
            return false;
        };
        if !rift_game::loot::Equipment::accepts(slot, &item) {
            p.stash[tab_index].items[stash_index] = Some(item);
            return false;
        }
        let displaced = p.equipment.set(slot, Some(item));
        if let Some(prev) = displaced {
            // Try to put the displaced item back where the new
            // one came from in the stash. If that footprint
            // doesn't fit, fall back to the first free bag
            // anchor; if even that fails, fall back to any free
            // stash anchor; if still nothing, undo the swap.
            let items = &mut p.stash[tab_index].items;
            let restored_to_stash = stash_index < items.len() && items[stash_index].is_none() && {
                items[stash_index] = Some(prev.clone());
                true
            };
            if !restored_to_stash {
                let placed_in_bag = place_inventory_item(&mut p.inventory, prev.clone()).is_some();
                if !placed_in_bag {
                    let placed_in_stash =
                        place_stash_item(&mut p.stash[tab_index].items, prev.clone()).is_some();
                    if !placed_in_stash {
                        // Worst case: undo the equip swap.
                        if let Some(newly_equipped) = p.equipment.take(slot) {
                            p.stash[tab_index].items[stash_index] = Some(newly_equipped);
                        }
                        p.equipment.set(slot, Some(prev));
                        return false;
                    }
                }
            }
        }
        p.recompute_stats(vitals);
        true
    }

    /// Take the equipped item in `slot` and place it into
    /// `(tab_index, stash_index)`. If that stash cell is
    /// occupied by a single item, the two swap (occupant
    /// becomes the new equip, if it accepts the slot;
    /// otherwise the swap is rejected). Multi-item footprint
    /// overlaps reject. Returns `false` and rolls back on any
    /// failure.
    pub fn unequip_to_stash_slot(
        &mut self,
        client_id: ClientId,
        slot: rift_game::loot::EquipSlot,
        tab_index: usize,
        stash_index: usize,
    ) -> bool {
        use rift_net::messages::STASH_TAB_SLOTS;
        if stash_index >= STASH_TAB_SLOTS {
            return false;
        }
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok((p, vitals)) = self
            .world
            .query_one_mut::<(&mut ServerPlayer, &mut Vitals)>(entity)
        else {
            return false;
        };
        if tab_index >= p.stash.len() {
            return false;
        }
        let Some(unequipped) = p.equipment.take(slot) else {
            return false;
        };
        // Ensure the tab is sized so we can address the cell.
        if p.stash[tab_index].items.len() < STASH_TAB_SLOTS {
            p.stash[tab_index]
                .items
                .resize_with(STASH_TAB_SLOTS, || None);
        }
        // Take the existing occupant out so the occupancy
        // mask doesn't include it when we test the new item's
        // footprint.
        let displaced = p.stash[tab_index].items[stash_index].take();
        let (uw, uh) = unequipped.footprint();
        let occ = build_stash_occupancy(&p.stash[tab_index].items);
        if !footprint_fits_stash(&occ, stash_index, uw, uh) {
            // Restore everything.
            p.stash[tab_index].items[stash_index] = displaced;
            p.equipment.set(slot, Some(unequipped));
            return false;
        }
        p.stash[tab_index].items[stash_index] = Some(unequipped);
        if let Some(prev) = displaced {
            // Try to equip the displaced item back into the
            // freed slot so the player keeps something on. If
            // it doesn't fit the slot type, push it into the
            // first free bag anchor; if even that fails, fall
            // back to a free stash anchor; if nothing works,
            // roll the whole op back.
            if rift_game::loot::Equipment::accepts(slot, &prev) {
                p.equipment.set(slot, Some(prev));
            } else if place_inventory_item(&mut p.inventory, prev.clone()).is_none() {
                if place_stash_item(&mut p.stash[tab_index].items, prev.clone()).is_none() {
                    let new_unequipped = p.stash[tab_index].items[stash_index].take();
                    p.stash[tab_index].items[stash_index] = Some(prev);
                    if let Some(it) = new_unequipped {
                        p.equipment.set(slot, Some(it));
                    }
                    return false;
                }
            }
        }
        p.recompute_stats(vitals);
        true
    }
    /// swap; multi-item overlaps reject. Mirrors the bag's
    /// `swap_inventory_slots` semantics.
    pub fn deposit_to_stash_slot(
        &mut self,
        client_id: ClientId,
        inventory_index: usize,
        tab_index: usize,
        stash_index: usize,
    ) -> bool {
        use rift_net::messages::{INVENTORY_CAPACITY, STASH_COLS, STASH_ROWS, STASH_TAB_SLOTS};
        if stash_index >= STASH_TAB_SLOTS || inventory_index >= INVENTORY_CAPACITY {
            return false;
        }
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
            return false;
        };
        if tab_index >= p.stash.len() {
            return false;
        }
        if p.inventory.len() < INVENTORY_CAPACITY {
            p.inventory.resize_with(INVENTORY_CAPACITY, || None);
        }
        if p.stash[tab_index].items.len() < STASH_TAB_SLOTS {
            p.stash[tab_index]
                .items
                .resize_with(STASH_TAB_SLOTS, || None);
        }
        let Some(item) = p.inventory[inventory_index].take() else {
            return false;
        };
        let (iw, ih) = item.footprint();

        // Identify which other item(s) the new footprint
        // would cover at the stash anchor. Multi-item overlap
        // means the swap can't resolve cleanly.
        let blockers = stash_blockers(&p.stash[tab_index].items, stash_index, iw, ih);
        if blockers.len() > 1 {
            p.inventory[inventory_index] = Some(item);
            return false;
        }
        let displaced_idx = blockers.first().copied();
        let displaced = displaced_idx.and_then(|i| p.stash[tab_index].items[i].take());

        let occ = build_stash_occupancy(&p.stash[tab_index].items);
        if !footprint_fits_stash(&occ, stash_index, iw, ih) {
            // Restore both.
            if let (Some(i), Some(it)) = (displaced_idx, displaced) {
                p.stash[tab_index].items[i] = Some(it);
            }
            p.inventory[inventory_index] = Some(item);
            return false;
        }
        let _ = STASH_COLS;
        let _ = STASH_ROWS;
        p.stash[tab_index].items[stash_index] = Some(item);

        if let Some(prev) = displaced {
            // Try to place the displaced item back into the
            // bag at the source anchor; fall back to first-fit;
            // last resort = full rollback.
            if !place_inventory_item_at(&mut p.inventory, prev.clone(), inventory_index)
                && place_inventory_item(&mut p.inventory, prev.clone()).is_none()
            {
                // Rollback.
                let item = p.stash[tab_index].items[stash_index].take().unwrap();
                if let Some(i) = displaced_idx {
                    p.stash[tab_index].items[i] = Some(prev);
                }
                p.inventory[inventory_index] = Some(item);
                return false;
            }
        }
        true
    }

    /// Withdraw the stash item at `(tab_index, stash_index)`
    /// into `inventory_index`. Mirror of
    /// [`Self::deposit_to_stash_slot`].
    pub fn withdraw_from_stash_slot(
        &mut self,
        client_id: ClientId,
        tab_index: usize,
        stash_index: usize,
        inventory_index: usize,
    ) -> bool {
        use rift_net::messages::{INVENTORY_CAPACITY, STASH_TAB_SLOTS};
        if stash_index >= STASH_TAB_SLOTS || inventory_index >= INVENTORY_CAPACITY {
            return false;
        }
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
            return false;
        };
        if tab_index >= p.stash.len() {
            return false;
        }
        if p.inventory.len() < INVENTORY_CAPACITY {
            p.inventory.resize_with(INVENTORY_CAPACITY, || None);
        }
        if p.stash[tab_index].items.len() < STASH_TAB_SLOTS {
            p.stash[tab_index]
                .items
                .resize_with(STASH_TAB_SLOTS, || None);
        }
        let Some(item) = p.stash[tab_index].items[stash_index].take() else {
            return false;
        };
        let (iw, ih) = item.footprint();
        let blockers = bag_blockers(&p.inventory, inventory_index, iw, ih);
        if blockers.len() > 1 {
            p.stash[tab_index].items[stash_index] = Some(item);
            return false;
        }
        let displaced_idx = blockers.first().copied();
        let displaced = displaced_idx.and_then(|i| p.inventory[i].take());

        if !place_inventory_item_at(&mut p.inventory, item.clone(), inventory_index) {
            // Restore both.
            if let (Some(i), Some(it)) = (displaced_idx, displaced) {
                p.inventory[i] = Some(it);
            }
            p.stash[tab_index].items[stash_index] = Some(item);
            return false;
        }

        if let Some(prev) = displaced {
            if !place_stash_item_at(&mut p.stash[tab_index].items, prev.clone(), stash_index)
                && place_stash_item(&mut p.stash[tab_index].items, prev.clone()).is_none()
            {
                // Rollback.
                let it = p.inventory[inventory_index].take().unwrap();
                if let Some(i) = displaced_idx {
                    p.inventory[i] = Some(prev);
                }
                p.stash[tab_index].items[stash_index] = Some(it);
                return false;
            }
        }
        true
    }

    /// Snapshot the player's stash. Used by the persistence
    /// layer to produce a `ResetCharacterStash` payload after
    /// every deposit / withdraw / tab edit. Returns a clone of
    /// the current tab list.
    pub fn dump_player_stash(&self, client_id: ClientId) -> Vec<StashTab> {
        self.player_stash(client_id)
    }

    /// Spend shards to unlock another stash tab. Returns
    /// `Some((new_shards, new_tab_count))` on success and
    /// `None` if the player can't afford the next tab or
    /// already owns [`MAX_STASH_TABS`]. Cost scales linearly
    /// with the current tab count: `100 * tabs_owned`.
    pub fn buy_stash_tab(&mut self, client_id: ClientId) -> Option<(u32, u32)> {
        let &entity = self.sessions.get(&client_id)?;
        let mut p = self.world.get::<&mut ServerPlayer>(entity).ok()?;
        let owned = p.stash.len();
        if owned >= rift_net::messages::MAX_STASH_TABS {
            return None;
        }
        let cost = (owned as u32).saturating_mul(100);
        if p.shards < cost {
            return None;
        }
        p.shards -= cost;
        p.stash.push(StashTab::fresh(owned));
        Some((p.shards, p.stash.len() as u32))
    }

    /// Rename `tab_index`. Empty / whitespace-only names are
    /// rejected; otherwise the name is trimmed and clamped to
    /// 18 characters so the tab strip stays readable.
    pub fn rename_stash_tab(&mut self, client_id: ClientId, tab_index: usize, name: &str) -> bool {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return false;
        }
        let clamped: String = trimmed.chars().take(18).collect();
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
            return false;
        };
        let Some(tab) = p.stash.get_mut(tab_index) else {
            return false;
        };
        tab.name = clamped;
        true
    }

    /// Recolor `tab_index` with a packed `0xRRGGBB` color.
    /// The high byte is masked off so callers can't smuggle
    /// alpha (the stash strip renders opaque).
    pub fn recolor_stash_tab(&mut self, client_id: ClientId, tab_index: usize, color: u32) -> bool {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
            return false;
        };
        let Some(tab) = p.stash.get_mut(tab_index) else {
            return false;
        };
        tab.color = color & 0x00FF_FFFF;
        true
    }

    /// Swap two stash slots within `tab_index`. Honours
    /// multi-cell footprints: both anchors must fit after the
    /// swap without overlapping any other item. Either may be
    /// empty. Returns `true` on success.
    pub fn swap_stash_slots(
        &mut self,
        client_id: ClientId,
        tab_index: usize,
        a: usize,
        b: usize,
    ) -> bool {
        use rift_net::messages::STASH_TAB_SLOTS;
        if a == b {
            return false;
        }
        if a.max(b) >= STASH_TAB_SLOTS {
            return false;
        }
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
            return false;
        };
        let Some(tab) = p.stash.get_mut(tab_index) else {
            return false;
        };
        if tab.items.len() < STASH_TAB_SLOTS {
            tab.items.resize_with(STASH_TAB_SLOTS, || None);
        }
        let it_a = tab.items[a].take();
        let it_b = tab.items[b].take();

        let occ = build_stash_occupancy(&tab.items);
        let a_fits = match &it_b {
            Some(it) => {
                let (w, h) = it.footprint();
                footprint_fits_stash(&occ, a, w, h)
            }
            None => true,
        };
        let b_fits = match &it_a {
            Some(it) => {
                let (w, h) = it.footprint();
                footprint_fits_stash(&occ, b, w, h)
            }
            None => true,
        };
        // Cross-clash: a's footprint at b can't cover b's anchor (and vice-versa)
        let cross = match (&it_a, &it_b) {
            (Some(ia), Some(ib)) => {
                let (aw, ah) = ia.footprint();
                let (bw, bh) = ib.footprint();
                covers(b, aw, ah, a) || covers(a, bw, bh, b)
            }
            _ => false,
        };
        if !a_fits || !b_fits || cross {
            tab.items[a] = it_a;
            tab.items[b] = it_b;
            return false;
        }
        tab.items[a] = it_b;
        tab.items[b] = it_a;
        true
    }

    /// Auto-sort one stash tab in place. Returns `true` iff
    /// the tab actually changed.
    pub fn sort_stash_tab(&mut self, client_id: ClientId, tab_index: usize) -> bool {
        use rift_net::messages::{STASH_COLS, STASH_ROWS};
        if !self.is_stash_open(client_id) {
            return false;
        }
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
            return false;
        };
        let Some(tab) = p.stash.get_mut(tab_index) else {
            return false;
        };
        if tab.items.iter().all(|s| s.is_none()) {
            return false;
        }
        sort_grid_items(&mut tab.items, STASH_COLS, STASH_ROWS);
        true
    }
}

// ── Module-local helpers ──

/// List of distinct anchor indices in `slots` whose footprint
/// would be covered by `(w, h)` anchored at `anchor`, treating
/// `slots` as a stash-sized grid.
fn stash_blockers(
    slots: &[Option<rift_game::loot::Item>],
    anchor: usize,
    w: u8,
    h: u8,
) -> Vec<usize> {
    use rift_net::messages::{STASH_COLS, STASH_ROWS};
    grid_blockers(slots, anchor, w, h, STASH_COLS, STASH_ROWS)
}

/// As [`stash_blockers`] but for the bag grid.
fn bag_blockers(
    slots: &[Option<rift_game::loot::Item>],
    anchor: usize,
    w: u8,
    h: u8,
) -> Vec<usize> {
    use rift_net::messages::{BAG_COLS, BAG_ROWS};
    grid_blockers(slots, anchor, w, h, BAG_COLS, BAG_ROWS)
}

fn grid_blockers(
    slots: &[Option<rift_game::loot::Item>],
    anchor: usize,
    w: u8,
    h: u8,
    cols: usize,
    rows: usize,
) -> Vec<usize> {
    // Build a cell-owner map so a multi-cell other item shows
    // up as exactly one blocker entry no matter how many of
    // its covered cells overlap the proposed footprint.
    let mut owner: Vec<Option<usize>> = vec![None; cols * rows];
    for (idx, slot) in slots.iter().enumerate() {
        let Some(it) = slot else { continue };
        if idx >= cols * rows {
            break;
        }
        let (iw, ih) = it.footprint();
        let cx = idx % cols;
        let cy = idx / cols;
        for dy in 0..ih as usize {
            for dx in 0..iw as usize {
                let nx = cx + dx;
                let ny = cy + dy;
                if nx < cols && ny < rows {
                    owner[ny * cols + nx] = Some(idx);
                }
            }
        }
    }
    let mut out: Vec<usize> = Vec::new();
    let ax = anchor % cols;
    let ay = anchor / cols;
    for dy in 0..h as usize {
        for dx in 0..w as usize {
            let nx = ax + dx;
            let ny = ay + dy;
            if nx >= cols || ny >= rows {
                continue;
            }
            if let Some(o) = owner[ny * cols + nx] {
                if !out.contains(&o) {
                    out.push(o);
                }
            }
        }
    }
    out
}

/// `true` iff a `(w, h)` footprint anchored at `anchor_idx`
/// would cover the cell `target_idx`. Used for swap cross-
/// clash detection regardless of grid dimensions (both
/// indices share the same grid stride).
fn covers(anchor_idx: usize, w: u8, h: u8, target_idx: usize) -> bool {
    use rift_net::messages::STASH_COLS as COLS;
    let ax = anchor_idx % COLS;
    let ay = anchor_idx / COLS;
    let tx = target_idx % COLS;
    let ty = target_idx / COLS;
    tx >= ax && tx < ax + w as usize && ty >= ay && ty < ay + h as usize
}
