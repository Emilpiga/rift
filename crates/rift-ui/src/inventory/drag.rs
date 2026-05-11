//! Drag/drop and click routing for inventory slots.

use rift_ui_im::{Color, ItemSlot, SlotInteraction};
use rift_ui_types::inventory::{DragSource, EquipSlotIdx, InventoryAction, ItemView};

/// Default size of an `ItemSlot` widget the drag-ghost uses.
/// The actual rect on the bag/paperdoll is whatever the
/// caller passes to `interact`; this is only the ghost size.
const GHOST_SIZE: f32 = 64.0;

/// Where a drop landed on screen. Matches [`DragSource`] in
/// shape; the panels emit one and call [`route_slot`] to fan
/// the click + drop into the right [`InventoryAction`].
#[derive(Copy, Clone, Debug)]
pub enum DropTarget {
    /// Drop onto a specific bag slot (filled or empty
    /// rectangle the user clearly aimed at).
    Bag(u32),
    Equip(EquipSlotIdx),
    Stash(u32),
}

/// Build a fully-configured `ItemSlot` for the given view.
pub fn build_item_slot<'a>(item: Option<&'a ItemView<'a>>) -> ItemSlot<'a> {
    let mut s = ItemSlot::new(GHOST_SIZE).transparent_bg(true);
    if let Some(it) = item {
        let [r, g, b, a] = it.rarity_color;
        s = s.rarity_tint(Color::rgba(r, g, b, a));
        if it.anchored {
            s = s.anchored(true);
        }
        if !it.icon_key.is_empty() {
            s = s.icon(it.icon_key);
        } else if let Some(ch) = it.fallback_glyph {
            s = s.fallback_glyph(ch);
        }
    }
    s
}

/// Fan a slot's [`SlotInteraction`] out into the appropriate
/// `InventoryAction`(s).
pub fn route_slot(
    r: SlotInteraction<DragSource>,
    target: DropTarget,
    stash_open: bool,
    ctrl: bool,
    active_tab: u8,
    out: &mut Vec<InventoryAction>,
) {
    if let Some(drop) = r.dropped {
        handle_drop(drop.payload, target, stash_open, active_tab, out);
    }
    if r.clicked {
        let src = match target {
            DropTarget::Bag(idx) => DragSource::Bag(idx),
            DropTarget::Equip(slot) => DragSource::Equip(slot),
            DropTarget::Stash(idx) => DragSource::Stash(idx),
        };
        handle_click(src, stash_open, ctrl, active_tab, out);
    }
    if r.right_clicked {
        let src = match target {
            DropTarget::Bag(idx) => DragSource::Bag(idx),
            DropTarget::Equip(slot) => DragSource::Equip(slot),
            DropTarget::Stash(idx) => DragSource::Stash(idx),
        };
        handle_right_click(src, stash_open, active_tab, out);
    }
}

/// Same as [`route_slot`] but also records the drop's source
/// into `in_transit` so the renderer can hide that slot until
/// the server's mutation reply arrives. Eliminates the
/// "pop back to source then jump to target" flash.
pub fn route_slot_capture(
    r: SlotInteraction<DragSource>,
    target: DropTarget,
    stash_open: bool,
    ctrl: bool,
    active_tab: u8,
    out: &mut Vec<InventoryAction>,
    in_transit: &mut Option<rift_ui_types::inventory::InTransitSource>,
) {
    if let Some(dp) = r.dropped.as_ref() {
        *in_transit = Some(rift_ui_types::inventory::InTransitSource::from_drag(
            dp.payload,
            active_tab,
        ));
    }
    route_slot(r, target, stash_open, ctrl, active_tab, out);
}

/// Right-click is the "fast transfer" verb: send the item
/// across the active boundary without touching the mouse-up
/// drag flow. Bag ↔ stash when a stash session is open;
/// otherwise bag → equip / equip → bag.
fn handle_right_click(
    src: DragSource,
    stash_open: bool,
    active_tab: u8,
    out: &mut Vec<InventoryAction>,
) {
    match src {
        DragSource::Bag(idx) => {
            if stash_open {
                out.push(InventoryAction::DepositToStash {
                    inventory_index: idx,
                    tab_index: active_tab,
                });
            } else {
                out.push(InventoryAction::Equip {
                    inventory_index: idx,
                });
            }
        }
        DragSource::Equip(slot) => {
            out.push(InventoryAction::Unequip { slot: slot.0 });
        }
        DragSource::Stash(idx) => {
            out.push(InventoryAction::WithdrawFromStash {
                tab_index: active_tab,
                stash_index: idx,
            });
        }
    }
}

fn handle_click(
    src: DragSource,
    _stash_open: bool,
    ctrl: bool,
    _active_tab: u8,
    out: &mut Vec<InventoryAction>,
) {
    match src {
        DragSource::Bag(idx) => {
            if ctrl {
                out.push(InventoryAction::Salvage {
                    inventory_index: idx,
                });
            } else {
                // Left-click always equips; right-click is the
                // fast bag\u2194stash transfer verb (see
                // `handle_right_click`).
                out.push(InventoryAction::Equip {
                    inventory_index: idx,
                });
            }
        }
        DragSource::Equip(slot) => {
            out.push(InventoryAction::Unequip { slot: slot.0 });
        }
        DragSource::Stash(_idx) => {
            // Left-click on a stash slot is reserved for drag /
            // future selection; the fast withdraw lives on
            // right-click via `handle_right_click`.
        }
    }
}

/// Fan a `(source, target)` drop pair into the appropriate
/// `InventoryAction`(s). Public so the bag's snap-anchor
/// resolver can emit the same actions as `route_slot` after
/// it consumes the drop centrally.
pub fn handle_drop(
    src: DragSource,
    target: DropTarget,
    stash_open: bool,
    active_tab: u8,
    out: &mut Vec<InventoryAction>,
) {
    match (src, target) {
        (DragSource::Bag(a), DropTarget::Bag(b)) if a != b => {
            out.push(InventoryAction::SwapBag { a, b });
        }
        (DragSource::Bag(idx), DropTarget::Equip(_)) => {
            out.push(InventoryAction::Equip {
                inventory_index: idx,
            });
        }
        (DragSource::Bag(a), DropTarget::Stash(b)) if stash_open => {
            out.push(InventoryAction::DepositToStashSlot {
                inventory_index: a,
                tab_index: active_tab,
                stash_index: b,
            });
        }
        (DragSource::Equip(slot), DropTarget::Bag(idx)) => {
            out.push(InventoryAction::UnequipToSlot {
                slot: slot.0,
                inventory_index: idx,
            });
        }
        (DragSource::Equip(slot), DropTarget::Stash(idx)) if stash_open => {
            out.push(InventoryAction::UnequipToStashSlot {
                slot: slot.0,
                tab_index: active_tab,
                stash_index: idx,
            });
        }
        (DragSource::Stash(a), DropTarget::Bag(b)) if stash_open => {
            out.push(InventoryAction::WithdrawFromStashSlot {
                tab_index: active_tab,
                stash_index: a,
                inventory_index: b,
            });
        }
        (DragSource::Stash(idx), DropTarget::Equip(_)) if stash_open => {
            out.push(InventoryAction::EquipFromStash {
                tab_index: active_tab,
                stash_index: idx,
            });
        }
        (DragSource::Stash(a), DropTarget::Stash(b)) if stash_open && a != b => {
            out.push(InventoryAction::SwapStash {
                tab_index: active_tab,
                a,
                b,
            });
        }
        _ => {}
    }
}
