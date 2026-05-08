//! Drag/drop and click routing for inventory slots.
//!
//! Three slot worlds (bag, equip, stash) all share the same
//! interaction grammar — click to do the canonical action,
//! drag to move between worlds. This module hosts:
//!
//! * [`DragSource`] / [`DropTarget`] — the typed payload that
//!   travels through the IM stack.
//! * [`build_item_slot`] — single source of truth for slot
//!   visuals so the in-place draw and the drag ghost stay
//!   pixel-identical.
//! * [`route_slot`] — fans a [`SlotInteraction`] out into the
//!   right [`EquipRequest`] / [`StashRequest`].
//! * [`compare_target`] — picks the equipped item to compare a
//!   hovered item against.
//! * [`item_for_source`] — resolves a [`DragSource`] back into
//!   the live [`Item`] reference (for ghost rendering).

use rift_engine::ui::im::{Color, ItemSlot, SlotInteraction};
use rift_game::loot::{Equipment, EquipSlot, Item};

use crate::game::sub_state::{EquipRequest, StashRequest};

use super::layout::SLOT_SIZE;

/// Where the drag started. Travels through the IM stack as an
/// opaque payload; the drop targets downcast to this enum and
/// branch on the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragSource {
    Bag(usize),
    Equip(EquipSlot),
    Stash(usize),
}

#[derive(Clone, Copy, Debug)]
pub enum DropTarget {
    Bag(usize),
    Equip(EquipSlot),
    Stash(usize),
}

/// Build a fully-configured `ItemSlot` for the given item (or
/// an empty placeholder). The same builder feeds in-place
/// slot drawing AND drag-ghost drawing so the two stay
/// pixel-identical.
pub fn build_item_slot<'a>(item: Option<&'a Item>) -> ItemSlot<'a> {
    let mut s = ItemSlot::new(SLOT_SIZE);
    if let Some(it) = item {
        let c = it.rarity.color();
        s = s.rarity_tint(Color::rgba(c[0], c[1], c[2], 1.0));
        if it.anchored {
            s = s.anchored(true);
        }
        if !it.base.icon.is_empty() {
            s = s.icon(it.base.icon);
        } else if let Some(ch) = it.base.name.chars().next() {
            s = s.fallback_glyph(ch.to_ascii_uppercase());
        }
    }
    s
}

/// Translate a slot's [`SlotInteraction`] into the appropriate
/// equip/stash request. Bundles the click → action and drop →
/// action mappings so each slot is a single function call.
pub fn route_slot(
    r: SlotInteraction<DragSource>,
    target: DropTarget,
    stash_open: bool,
    // `true` when the player is holding Ctrl this frame. A
    // Ctrl+click on a Bag slot fires a Salvage request
    // instead of the default Equip / Deposit.
    ctrl: bool,
    // Active stash tab. Threaded into every produced
    // `StashRequest` variant so the server applies the action
    // to the right page.
    active_tab: u8,
    pending: &mut Vec<EquipRequest>,
    stash_pending: &mut Vec<StashRequest>,
) {
    if let Some(drop) = r.dropped {
        handle_drop(drop.payload, target, stash_open, active_tab, pending, stash_pending);
    }
    if r.clicked {
        // Source identity is implicit in the target rect (the
        // slot the user clicked on), so derive it from `target`.
        let src = match target {
            DropTarget::Bag(idx) => DragSource::Bag(idx),
            DropTarget::Equip(slot) => DragSource::Equip(slot),
            DropTarget::Stash(idx) => DragSource::Stash(idx),
        };
        handle_click(src, stash_open, ctrl, active_tab, pending, stash_pending);
    }
}

fn handle_click(
    src: DragSource,
    stash_open: bool,
    // Ctrl modifier; only meaningful for `Bag` clicks where it
    // flips the action from "equip / deposit" to "salvage".
    ctrl: bool,
    // Active stash tab — destination for bag deposits and
    // source for stash clicks while a stash session is open.
    active_tab: u8,
    pending: &mut Vec<EquipRequest>,
    stash_pending: &mut Vec<StashRequest>,
) {
    match src {
        DragSource::Bag(idx) => {
            if ctrl {
                pending.push(EquipRequest::Salvage { inventory_index: idx as u32 });
            } else if stash_open {
                stash_pending.push(StashRequest::Deposit {
                    inventory_index: idx as u32,
                    tab_index: active_tab,
                });
            } else {
                pending.push(EquipRequest::Equip { inventory_index: idx as u32 });
            }
        }
        DragSource::Equip(slot) => {
            pending.push(EquipRequest::Unequip { slot: slot.to_u8() });
        }
        DragSource::Stash(idx) => {
            stash_pending.push(StashRequest::Withdraw {
                tab_index: active_tab,
                stash_index: idx as u32,
            });
        }
    }
}

fn handle_drop(
    src: DragSource,
    target: DropTarget,
    stash_open: bool,
    active_tab: u8,
    pending: &mut Vec<EquipRequest>,
    stash_pending: &mut Vec<StashRequest>,
) {
    match (src, target) {
        // Bag → Bag: reorder
        (DragSource::Bag(a), DropTarget::Bag(b)) if a != b => {
            pending.push(EquipRequest::SwapBag { a: a as u32, b: b as u32 });
        }
        // Bag → Equip: equip
        (DragSource::Bag(idx), DropTarget::Equip(_)) => {
            pending.push(EquipRequest::Equip { inventory_index: idx as u32 });
        }
        // Bag → Stash: deposit into the dropped-on slot of the
        // currently-active stash tab.
        (DragSource::Bag(a), DropTarget::Stash(b)) if stash_open => {
            stash_pending.push(StashRequest::DepositToSlot {
                inventory_index: a as u32,
                tab_index: active_tab,
                stash_index: b as u32,
            });
        }
        // Equip → Bag(idx): unequip into a specific slot
        (DragSource::Equip(slot), DropTarget::Bag(idx)) => {
            pending.push(EquipRequest::UnequipToSlot {
                slot: slot.to_u8(),
                inventory_index: idx as u32,
            });
        }
        // Stash → Bag(idx): withdraw to the dropped-on slot.
        (DragSource::Stash(a), DropTarget::Bag(b)) if stash_open => {
            stash_pending.push(StashRequest::WithdrawToSlot {
                tab_index: active_tab,
                stash_index: a as u32,
                inventory_index: b as u32,
            });
        }
        // Stash → Stash: reorder within the active tab.
        (DragSource::Stash(a), DropTarget::Stash(b)) if stash_open && a != b => {
            stash_pending.push(StashRequest::Swap {
                tab_index: active_tab,
                a: a as u32,
                b: b as u32,
            });
        }
        _ => {}
    }
}

/// Resolve the equipment slot we'd compare a hovered item
/// against (the current occupant of the item's default slot).
pub fn compare_target<'a>(equipment: &'a Equipment, hovered: &Item) -> Option<&'a Item> {
    let slot = equipment.default_slot(hovered);
    equipment.get(slot)
}

/// Translate a live drag's `payload` back into the source
/// item, used to render the drag ghost.
pub fn item_for_source<'a>(
    src: DragSource,
    items: &'a [Option<Item>],
    equipment: &'a Equipment,
    stash_items: &'a [Option<Item>],
) -> Option<&'a Item> {
    match src {
        DragSource::Bag(idx) => items.get(idx).and_then(|o| o.as_ref()),
        DragSource::Equip(slot) => equipment.get(slot),
        DragSource::Stash(idx) => stash_items.get(idx).and_then(|o| o.as_ref()),
    }
}
