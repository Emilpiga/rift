//! Loot pickup + ground-spawn methods on [`Sim`]. Split out of
//! the main `sim/mod.rs`. Pure `impl Sim` block — every method
//! is already defined on `Sim` and migrated here verbatim.

use rift_game::kinematic::Kinematic;
use rift_net::ids::ClientId;
use rift_net::messages::WorldEvent;
use rift_net::{NetId, NetTick};

use super::loot;
use super::player::ServerPlayer;
use super::{count_filled, place_inventory_item, Sim, PICKUP_RANGE, SHARE_WINDOW_TICKS};

impl Sim {
    /// Try to claim a ground-loot drop for `client_id`. Validates
    /// the picker is within [`PICKUP_RANGE`] of the loot row and
    /// has a free bag slot (cap [`rift_net::messages::INVENTORY_CAPACITY`]).
    ///
    /// Returns:
    /// - `Ok(item)` on success \u2014 loot entity is despawned, item is
    ///   already in the picker's `ServerPlayer.inventory`, caller
    ///   broadcasts `LootClaimed` and persists.
    /// - `Err(Some(reason))` when the request was understood but
    ///   refused (e.g. bag full); the caller forwards the reason
    ///   back to the picker so the UI can react. Loot entity is
    ///   left on the ground.
    /// - `Err(None)` for silent failures (missing session, missing
    ///   loot row, out-of-range) \u2014 these aren't worth notifying
    ///   the client about.
    pub fn try_pickup_loot(
        &mut self,
        client_id: ClientId,
        loot: NetId,
    ) -> Result<rift_game::loot::Item, Option<rift_net::messages::PickupRejectReason>> {
        let &player_entity = self.sessions.get(&client_id).ok_or(None)?;
        let (player_pos, picker_char_id) = {
            let p = self
                .world
                .get::<&ServerPlayer>(player_entity)
                .map_err(|_| None)?;
            let kinematic = self
                .world
                .get::<&Kinematic>(player_entity)
                .map_err(|_| None)?;
            (kinematic.position, p.character_id)
        };

        // Find the loot ECS entity by net id.
        let target = self
            .world
            .query::<&loot::ServerLoot>()
            .iter()
            .find(|(_, l)| l.net_id == loot)
            .map(|(e, l)| (e, l.position, l.item.clone(), l.share.clone()))
            .ok_or(None)?;
        let (loot_entity, loot_pos, mut item, share) = target;

        // Provenance gate: while the share window is still
        // open, only characters listed on `Item::provenance`
        // may pick the drop up. After expiry the gate is
        // lifted (free-for-all to any Sim-peer). Legacy items
        // without provenance — those that pre-date the system
        // — fall through and self-bind to the picker below.
        let window_open = match &share {
            Some(w) => self.current_tick.0 < w.expires_at_tick.0,
            None => false,
        };
        if window_open {
            if let Some(prov) = &item.provenance {
                let allowed = picker_char_id
                    .map(|u| prov.allows(&u.into_bytes()))
                    .unwrap_or(false);
                if !allowed {
                    return Err(Some(rift_net::messages::PickupRejectReason::NotEligible));
                }
            }
        }

        let dx = loot_pos.x - player_pos.x;
        let dz = loot_pos.z - player_pos.z;
        if dx * dx + dz * dz > PICKUP_RANGE * PICKUP_RANGE {
            return Err(None);
        }

        // Capacity check before we mutate anything: leave the
        // loot row alive so the player can pick it up after
        // freeing a slot.
        // Capacity check: try to find an anchor where the
        // item's footprint fits without overlap. If no anchor
        // fits we treat the bag as full — multi-cell items
        // can be "too big" even when individual cells are
        // free.
        if let Ok(p) = self.world.get::<&ServerPlayer>(player_entity) {
            if count_filled(&p.inventory) >= rift_net::messages::INVENTORY_CAPACITY {
                return Err(Some(rift_net::messages::PickupRejectReason::InventoryFull));
            }
        }

        // Self-bind legacy / unprovenanced drops onto the
        // picker so the next equip / drop / re-pickup carries
        // the lineage forward. New drops already have
        // `provenance` set by `drop_for_enemy`.
        if item.provenance.is_none() {
            if let Some(uuid) = picker_char_id {
                item.provenance = Some(rift_game::loot::LootProvenance::from_ids([
                    uuid.into_bytes()
                ]));
            }
        }

        // Items picked up inside an active rift are flagged
        // "unstable": they live only in the in-memory
        // `ServerPlayer` snapshot until the run extracts. Hub
        // pickups (vendor drops, debug spawns, future safe-zone
        // chests) stay stable. The flag drives both the client
        // tooltip and every server-side strip / stabilise path.
        if !self.is_hub() {
            item.unstable = true;
        }
        // Try to place into the picker's `ServerPlayer.inventory`
        // *before* despawning the ground entity. If no anchor
        // fits the item's multi-cell footprint we reject and
        // leave the loot row alive so the player can sort
        // their bag and retry. Despawning first (the previous
        // ordering) and then rejecting left the entity gone
        // server-side but visually still on the ground for the
        // client — every subsequent pickup attempt silently
        // failed with `Err(None)` because the net id no
        // longer resolved, and the player had no way to tell
        // why the drop became un-pickupable.
        let placed = if let Ok(mut p) = self.world.get::<&mut ServerPlayer>(player_entity) {
            let ok = place_inventory_item(&mut p.inventory, item.clone()).is_some();
            if ok {
                log::debug!(
                    "sim: inventory for {:?} now has {} item(s)",
                    client_id,
                    count_filled(&p.inventory)
                );
            }
            ok
        } else {
            false
        };
        if !placed {
            log::debug!(
                "sim: inventory full for {:?}, no anchor fits item footprint",
                client_id
            );
            return Err(Some(rift_net::messages::PickupRejectReason::InventoryFull));
        }
        // Placed successfully — only now drop the ground entity.
        let _ = self.world.despawn(loot_entity);
        Ok(item)
    }

    /// Spawn a `ServerLoot` for `item` at `position`, tagged with
    /// a [`loot::ShareWindow`] expiring [`SHARE_WINDOW_TICKS`]
    /// in the future. Pickup eligibility itself rides on the
    /// item's [`rift_game::loot::LootProvenance`] (set when
    /// the item originally dropped); legacy items without
    /// provenance get self-bound to the dropper here so the
    /// drop carries that lineage forward instead of leaking
    /// the eligibility on first re-pickup.
    pub fn spawn_player_drop(
        &mut self,
        mut item: rift_game::loot::Item,
        position: glam::Vec3,
        dropper: ClientId,
    ) {
        if item.provenance.is_none() {
            if let Some(&entity) = self.sessions.get(&dropper) {
                if let Ok(p) = self.world.get::<&ServerPlayer>(entity) {
                    if let Some(uuid) = p.character_id {
                        item.provenance = Some(rift_game::loot::LootProvenance::from_ids([
                            uuid.into_bytes()
                        ]));
                    }
                }
            }
        }
        let expires_at_tick = NetTick(self.current_tick.0.wrapping_add(SHARE_WINDOW_TICKS));
        let share = loot::ShareWindow { expires_at_tick };
        self.spawn_loot_inner(item, position, Some(share));
    }

    /// Shared core of [`Self::spawn_dropped_loot`] /
    /// [`Self::spawn_player_drop`]. Allocates the net-id,
    /// constructs the [`loot::ServerLoot`] component (with the
    /// caller-supplied share window), spawns the ECS entity,
    /// and emits the `LootDropped` wire event.
    fn spawn_loot_inner(
        &mut self,
        item: rift_game::loot::Item,
        position: glam::Vec3,
        share: Option<loot::ShareWindow>,
    ) {
        use rift_net::messages::ItemBlob;
        let net_id = rift_net::NetId(self.next_loot_net_id);
        self.next_loot_net_id = self.next_loot_net_id.wrapping_add(1);
        if self.next_loot_net_id >= 0x4000_0000 {
            self.next_loot_net_id = 0x2000_0000;
        }
        let (base_id, rarity, ilvl, affixes, anchored, unique_id, unique_pick) = item.to_wire();
        let provenance = item.provenance.as_ref().map(|p| p.eligible.clone());
        let blob = ItemBlob {
            base_id,
            rarity,
            ilvl,
            affixes,
            anchored,
            // Loot still on the ground hasn't been picked up
            // yet; the unstable lifecycle starts at pickup.
            unstable: item.unstable,
            provenance,
            unique_id: unique_id.map(|s| s.to_string()),
            unique_pick,
            rift_touched: item.rift_touched_to_wire(),
        };
        let loot = loot::ServerLoot {
            net_id,
            position,
            item,
            share,
        };
        let _ = self.world.spawn((loot,));
        self.pending_events.push(WorldEvent::LootDropped {
            loot: net_id,
            item: blob,
            position: position.to_array(),
        });
    }
}
