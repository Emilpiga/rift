//! Drag/drop and click routing for inventory slots.

use rift_ui_im::{Color, ItemSlot, Rect, SlotInteraction};
use rift_ui_types::inventory::{
    DragSource, EnchantSourceView, EquipSlotIdx, InventoryAction, ItemView,
};

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
    Equip {
        slot: EquipSlotIdx,
        occupied: bool,
    },
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
        if it.forge_locked {
            s = s.dim_alpha(0.74);
        }
    }
    s
}

/// `true` when dropping `src → tgt` would move or swap the forge-
/// anchored item (bag anchor index or equip slot).
pub fn forge_drop_blocked(
    forge: Option<EnchantSourceView>,
    src: DragSource,
    tgt: DropTarget,
) -> bool {
    let Some(f) = forge else {
        return false;
    };
    match f {
        EnchantSourceView::Bag(lock) => {
            if matches!(src, DragSource::Bag(i) if i == lock) {
                return true;
            }
            if matches!(tgt, DropTarget::Bag(j) if j == lock) {
                return true;
            }
            false
        }
        EnchantSourceView::Equip(lock_s) => {
            if matches!(src, DragSource::Equip(s) if s == lock_s) {
                return true;
            }
            if matches!(
                tgt,
                DropTarget::Equip { slot, .. } if slot == lock_s
            ) {
                return true;
            }
            false
        }
    }
}

/// Strip inventory mutations that would relocate the forge-selected item.
pub fn forge_action_mutates_locked(
    forge: Option<EnchantSourceView>,
    act: &InventoryAction,
) -> bool {
    let Some(f) = forge else {
        return false;
    };
    match f {
        EnchantSourceView::Bag(lock) => match act {
            InventoryAction::SwapBag { a, b } => *a == lock || *b == lock,
            InventoryAction::DepositToStash {
                inventory_index, ..
            }
            | InventoryAction::DepositToStashSlot {
                inventory_index, ..
            }
            | InventoryAction::WithdrawFromStashSlot {
                inventory_index, ..
            }
            | InventoryAction::Salvage { inventory_index } => *inventory_index == lock,
            InventoryAction::SortBag | InventoryAction::SalvageBulk { .. } => true,
            _ => false,
        },
        EnchantSourceView::Equip(lock_s) => {
            let lock_u8 = lock_s.0;
            match act {
                InventoryAction::UnequipToStashSlot { slot, .. } => *slot == lock_u8,
                InventoryAction::EquipFromStash {
                    target_slot: Some(t),
                    ..
                } => *t == lock_u8,
                _ => false,
            }
        }
    }
}

/// Fan a slot's [`SlotInteraction`] out into the appropriate
/// `InventoryAction`(s).
pub fn route_slot(
    r: SlotInteraction<DragSource>,
    target: DropTarget,
    stash_open: bool,
    ctrl: bool,
    active_tab: u8,
    is_consumable: bool,
    void_forge_open: bool,
    out: &mut Vec<InventoryAction>,
) {
    if let Some(drop) = r.dropped {
        handle_drop(drop.payload, target, stash_open, active_tab, out);
    }
    if r.clicked {
        let src = match target {
            DropTarget::Bag(idx) => DragSource::Bag(idx),
            DropTarget::Equip { slot, .. } => DragSource::Equip(slot),
            DropTarget::Stash(idx) => DragSource::Stash(idx),
        };
        handle_click(src, stash_open, ctrl, active_tab, out);
    }
    if r.right_clicked {
        let src = match target {
            DropTarget::Bag(idx) => DragSource::Bag(idx),
            DropTarget::Equip { slot, .. } => DragSource::Equip(slot),
            DropTarget::Stash(idx) => DragSource::Stash(idx),
        };
        // Consumables hijack right-click: instead of the
        // generic "fast transfer" verb (equip / deposit-to-
        // stash), they emit a `UseConsumable` so the host can
        // dispatch by `ConsumableKind`. Only meaningful when
        // the source is a bag slot \u2014 stash / equip
        // sources can't host consumables today.
        if is_consumable {
            if let DragSource::Bag(idx) = src {
                out.push(InventoryAction::UseConsumable {
                    inventory_index: idx,
                });
            }
        } else if void_forge_open {
            match src {
                DragSource::Bag(idx) => {
                    out.push(InventoryAction::SelectEnchantSource {
                        source: EnchantSourceView::Bag(idx),
                    });
                }
                DragSource::Equip(slot) => {
                    out.push(InventoryAction::SelectEnchantSource {
                        source: EnchantSourceView::Equip(slot),
                    });
                }
                DragSource::Stash(_) => {
                    handle_right_click(src, stash_open, active_tab, out);
                }
            }
        } else {
            handle_right_click(src, stash_open, active_tab, out);
        }
    }
}

/// Same as [`route_slot`] but also records the drop's source
/// into `in_transit` so the renderer can hide that slot until
/// the server's mutation reply arrives. Eliminates the
/// "pop back to source then jump to target" flash.
///
/// Only records the source when the drop actually produced an
/// [`InventoryAction`] — otherwise the source slot would
/// "ghost" indefinitely (no server reply ever clears it). A
/// drop that resolves to no action (illegal target, same-slot
/// drop, stash-closed sidecast, etc.) leaves the icon in
/// place.
pub fn route_slot_capture(
    r: SlotInteraction<DragSource>,
    target: DropTarget,
    stash_open: bool,
    ctrl: bool,
    active_tab: u8,
    is_consumable: bool,
    void_forge_open: bool,
    out: &mut Vec<InventoryAction>,
    in_transit: &mut Option<rift_ui_types::inventory::InTransitSource>,
) {
    let dropped_payload = r.dropped.as_ref().map(|dp| dp.payload);
    let actions_before = out.len();
    route_slot(
        r,
        target,
        stash_open,
        ctrl,
        active_tab,
        is_consumable,
        void_forge_open,
        out,
    );
    if let Some(payload) = dropped_payload {
        if out.len() > actions_before {
            *in_transit = Some(rift_ui_types::inventory::InTransitSource::from_drag(
                payload, active_tab,
            ));
        }
    }
}

/// Same as [`route_slot_capture`], plus optional destination rect for drops onto paperdoll cells
/// (feeds the destination ghost until the server applies equip mutations).
pub fn route_slot_capture_equip_dest(
    r: SlotInteraction<DragSource>,
    target: DropTarget,
    stash_open: bool,
    ctrl: bool,
    active_tab: u8,
    is_consumable: bool,
    void_forge_open: bool,
    out: &mut Vec<InventoryAction>,
    in_transit: &mut Option<rift_ui_types::inventory::InTransitSource>,
    dest_rect: &mut Option<[f32; 4]>,
    equip_cell_screen_rect: Rect,
) {
    let dropped_payload = r.dropped.as_ref().map(|dp| dp.payload);
    let actions_before = out.len();
    route_slot(
        r,
        target,
        stash_open,
        ctrl,
        active_tab,
        is_consumable,
        void_forge_open,
        out,
    );
    if let Some(payload) = dropped_payload {
        if out.len() > actions_before {
            *in_transit = Some(rift_ui_types::inventory::InTransitSource::from_drag(
                payload, active_tab,
            ));
            if matches!(target, DropTarget::Equip { .. }) {
                *dest_rect = Some([
                    equip_cell_screen_rect.x(),
                    equip_cell_screen_rect.y(),
                    equip_cell_screen_rect.width(),
                    equip_cell_screen_rect.height(),
                ]);
            }
        }
    }
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
                    target_slot: None,
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
                    target_slot: None,
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
        (DragSource::Bag(idx), DropTarget::Equip { slot, occupied }) => {
            if occupied {
                out.push(InventoryAction::UnequipToSlot {
                    slot: slot.0,
                    inventory_index: idx,
                });
            } else {
                out.push(InventoryAction::Equip {
                    inventory_index: idx,
                    target_slot: Some(slot.0),
                });
            }
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
        (DragSource::Equip(a), DropTarget::Equip { slot: b, .. }) if a != b => {
            // Only ring1 ↔ ring2 is a legal equip-to-equip
            // swap; every other pair would either be a no-op
            // (same slot) or violate `Equipment::accepts`.
            // Ring slot bytes are 6 (Ring1) and 7 (Ring2) per
            // `EquipSlot::ALL`.
            const RING1: u8 = 6;
            const RING2: u8 = 7;
            if matches!((a.0, b.0), (RING1, RING2) | (RING2, RING1)) {
                out.push(InventoryAction::SwapEquip { a: a.0, b: b.0 });
            }
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
        (DragSource::Stash(idx), DropTarget::Equip { slot, occupied }) if stash_open => {
            if occupied {
                out.push(InventoryAction::UnequipToStashSlot {
                    slot: slot.0,
                    tab_index: active_tab,
                    stash_index: idx,
                });
            } else {
                out.push(InventoryAction::EquipFromStash {
                    tab_index: active_tab,
                    stash_index: idx,
                    target_slot: Some(slot.0),
                });
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bag_drop_on_occupied_ring2_replaces_that_exact_slot() {
        let mut out = Vec::new();

        handle_drop(
            DragSource::Bag(4),
            DropTarget::Equip {
                slot: EquipSlotIdx(7),
                occupied: true,
            },
            false,
            0,
            &mut out,
        );

        assert!(matches!(
            out.as_slice(),
            [InventoryAction::UnequipToSlot {
                slot: 7,
                inventory_index: 4
            }]
        ));
    }

    #[test]
    fn bag_drop_on_empty_equipment_slot_keeps_generic_equip() {
        let mut out = Vec::new();

        handle_drop(
            DragSource::Bag(4),
            DropTarget::Equip {
                slot: EquipSlotIdx(7),
                occupied: false,
            },
            false,
            0,
            &mut out,
        );

        assert!(matches!(
            out.as_slice(),
            [InventoryAction::Equip {
                inventory_index: 4,
                target_slot: Some(7),
            }]
        ));
    }

    #[test]
    fn stash_drop_on_occupied_ring2_replaces_that_exact_slot() {
        let mut out = Vec::new();

        handle_drop(
            DragSource::Stash(3),
            DropTarget::Equip {
                slot: EquipSlotIdx(7),
                occupied: true,
            },
            true,
            2,
            &mut out,
        );

        assert!(matches!(
            out.as_slice(),
            [InventoryAction::UnequipToStashSlot {
                slot: 7,
                tab_index: 2,
                stash_index: 3
            }]
        ));
    }
}
