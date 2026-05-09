//! Stash, shards, and salvage methods on [`Sim`]. Split out of
//! the main `sim/mod.rs` so the per-domain surface area stays
//! browsable. Pure `impl Sim` block — every method is already
//! defined on `Sim` and migrated here verbatim.

use rift_net::ids::ClientId;

use super::player::{ServerPlayer, StashTab};
use super::{push_into_sparse, salvage_yield, trim_trailing_none, Sim};

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
        let yield_amt = salvage_yield(item.rarity, item.ilvl);
        *slot = None;
        trim_trailing_none(&mut p.inventory);
        p.shards = p.shards.saturating_add(yield_amt);
        Some(p.shards)
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
                Some(it) if !it.anchored && (it.rarity as u8) <= rarity_max => true,
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

    /// Move the bag item at `inventory_index` to the end of
    /// stash tab `tab_index`. Returns `true` on success;
    /// `false` if either index is out of range or the
    /// destination tab is at [`STASH_TAB_SLOTS`] capacity.
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
        // Reject up front when the tab is already full so the
        // bag item isn't taken out of the source slot only to
        // be put back. Empty trailing slots aren't counted.
        let filled = p.stash[tab_index].items.iter().filter(|s| s.is_some()).count();
        if filled >= rift_net::messages::STASH_TAB_SLOTS {
            return false;
        }
        let Some(item) = p.inventory.get_mut(inventory_index).and_then(|s| s.take()) else {
            return false;
        };
        push_into_sparse(&mut p.stash[tab_index].items, item);
        trim_trailing_none(&mut p.inventory);
        true
    }

    /// Move the stash item at `(tab_index, stash_index)` to the
    /// end of the bag. Returns `true` on success; `false` if
    /// either index is out of range.
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
        trim_trailing_none(&mut p.stash[tab_index].items);
        push_into_sparse(&mut p.inventory, item);
        true
    }

    /// Deposit the bag item at `inventory_index` into a specific
    /// `(tab_index, stash_index)`. If the destination is already
    /// occupied the two items swap (the prior stash occupant
    /// goes back to the freed bag slot). Grows the tab with
    /// `None` placeholders when `stash_index` is past the
    /// current length, then trims trailing `None`s on both
    /// containers. Rejects requests past [`STASH_TAB_SLOTS`].
    pub fn deposit_to_stash_slot(
        &mut self,
        client_id: ClientId,
        inventory_index: usize,
        tab_index: usize,
        stash_index: usize,
    ) -> bool {
        if stash_index >= rift_net::messages::STASH_TAB_SLOTS {
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
        let Some(item) = p.inventory.get_mut(inventory_index).and_then(|s| s.take()) else {
            return false;
        };
        let displaced = {
            let items = &mut p.stash[tab_index].items;
            if stash_index >= items.len() {
                items.resize_with(stash_index + 1, || None);
            }
            let prev = items[stash_index].take();
            items[stash_index] = Some(item);
            prev
        };
        if let Some(prev) = displaced {
            if inventory_index >= p.inventory.len() {
                p.inventory.resize_with(inventory_index + 1, || None);
            }
            p.inventory[inventory_index] = Some(prev);
        }
        trim_trailing_none(&mut p.stash[tab_index].items);
        trim_trailing_none(&mut p.inventory);
        true
    }

    /// Withdraw the stash item at `(tab_index, stash_index)`
    /// into a specific `inventory_index`. Mirror of
    /// [`Self::deposit_to_stash_slot`].
    pub fn withdraw_from_stash_slot(
        &mut self,
        client_id: ClientId,
        tab_index: usize,
        stash_index: usize,
        inventory_index: usize,
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
        // Pull the item out of the stash and drop the borrow
        // before touching `p.inventory` so the borrow checker
        // doesn't complain about overlapping mutable derefs.
        let Some(item) = p.stash[tab_index]
            .items
            .get_mut(stash_index)
            .and_then(|s| s.take())
        else {
            return false;
        };
        if inventory_index >= p.inventory.len() {
            p.inventory.resize_with(inventory_index + 1, || None);
        }
        let displaced = p.inventory[inventory_index].take();
        p.inventory[inventory_index] = Some(item);
        if let Some(prev) = displaced {
            let items = &mut p.stash[tab_index].items;
            if stash_index >= items.len() {
                items.resize_with(stash_index + 1, || None);
            }
            items[stash_index] = Some(prev);
        }
        trim_trailing_none(&mut p.stash[tab_index].items);
        trim_trailing_none(&mut p.inventory);
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
    pub fn rename_stash_tab(
        &mut self,
        client_id: ClientId,
        tab_index: usize,
        name: &str,
    ) -> bool {
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
    pub fn recolor_stash_tab(
        &mut self,
        client_id: ClientId,
        tab_index: usize,
        color: u32,
    ) -> bool {
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

    /// Swap two bag slots, used by the inventory UI's
    /// drag-and-drop reorder path. Either index may be empty
    /// (past the current bag length); the bag is grown with
    /// `None` placeholders to fit, then trimmed back to the
    /// last filled slot. Returns `true` on success.
    pub fn swap_stash_slots(
        &mut self,
        client_id: ClientId,
        tab_index: usize,
        a: usize,
        b: usize,
    ) -> bool {
        if a == b {
            return false;
        }
        if a.max(b) >= rift_net::messages::STASH_TAB_SLOTS {
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
        let items = &mut tab.items;
        let max = a.max(b);
        if max >= items.len() {
            if a >= items.len() && b >= items.len() {
                return false;
            }
            items.resize_with(max + 1, || None);
        }
        items.swap(a, b);
        trim_trailing_none(items);
        true
    }
}
