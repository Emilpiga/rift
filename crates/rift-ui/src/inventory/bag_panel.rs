//! Paperdoll equipment grid + flush-flat bag grid.
//!
//! Paperdoll: each `EquipSlotIdx` is laid out at the
//! position from `PAPERDOLL_LAYOUT`. Empty slots draw a
//! placeholder with the slot label; filled slots draw the
//! item icon stretched to the slot rectangle.
//!
//! Bag: a flush grid of `bag_cols × bag_rows` outlined cells.
//! Items are first-fit-packed by `pack_bag` and rendered as
//! `(cell_w × cell_h)` rectangles spanning multiple cells.
//!
//! IMPORTANT: empty bag cells AND empty paperdoll slots run
//! `ItemSlot::interact(payload=None)` so the engine's typed
//! drag system can resolve a drop into them. Without this the
//! click would draw an outline but `take_drop` would never
//! fire — that's the bug behind "I can't unequip by dragging
//! to an empty bag slot".

use rift_ui_im::{Color, Frame, Id, ItemSlot, Pad, Pos2, Rect, Stroke, Ui};
use rift_ui_types::inventory::{DragSource, EquipSlotIdx, InventoryAction, ItemView};

use super::drag::{build_item_slot, route_slot_capture, DropTarget};
use super::grid_drop::{snap_preview_and_resolve, GridSpec};
use super::layout::{pack_bag, Layout};

pub struct BagPanelIn<'a> {
    pub items: &'a [Option<ItemView<'a>>],
    pub equipment: &'a [Option<ItemView<'a>>],
    /// Active stash tab items (used so a stash→bag drag
    /// previews with the correct multi-cell footprint while
    /// hovering the bag grid; see `source_footprint`).
    pub stash_active: &'a [Option<ItemView<'a>>],
    pub bag_cols: u8,
    pub bag_rows: u8,
    pub stash_open: bool,
    pub active_tab_u8: u8,
    /// Optimistic source-hide: when set, the renderer hides
    /// the matching slot so the player doesn't see the item
    /// pop back to its source between drop and the server's
    /// authoritative reply.
    pub in_transit: Option<rift_ui_types::inventory::InTransitSource>,
}

#[derive(Default)]
pub struct BagPanelOut {
    pub hovered_bag: Option<u32>,
    /// Screen rect of the hovered bag cell. Used by the
    /// tooltip to anchor next to the slot and pick the side
    /// (left vs right) with more available space.
    pub hovered_bag_rect: Option<Rect>,
    pub salvage_press_bag_idx: Option<u32>,
    pub salvage_release_bag_idx: Option<u32>,
    /// `Some` the frame the player released a drag onto a
    /// bag/equip slot. Carries the drag's source so the host
    /// can hide that source until the server reply lands
    /// (eliminates the visual "pop back" flash).
    pub in_transit_from_drop: Option<rift_ui_types::inventory::InTransitSource>,
    /// Screen rect (`[x, y, w, h]`) the in-flight drop
    /// resolved to. Used by the host to render a translucent
    /// destination ghost while the server reply is in flight
    /// (so the target slot doesn't read as empty).
    pub in_transit_dest_rect_from_drop: Option<[f32; 4]>,
}

/// Warm honey-gold used for slot outlines. Reads as etched
/// metal against the carved-stone container behind it.
const GOLD_OUTLINE: Color = Color::rgba(0.78, 0.62, 0.30, 0.85);
/// Slightly brighter highlight drawn 1 px inside the gold
/// stroke so the cell reads as inset rather than painted on.
const INSET_HIGHLIGHT: Color = Color::rgba(1.0, 0.95, 0.82, 0.10);
/// 1px dark band painted along the top + left edges inside
/// a cell so it reads as recessed into the stone slab.
/// Pairs with [`CELL_INSET_LIGHT`] on the opposing edges.
const CELL_INSET_SHADOW: Color = Color::rgba(0.0, 0.0, 0.0, 0.55);
/// 1px cream highlight along the bottom + right edges of a
/// cell. Together with the top/left shadow it sells the
/// bevel without a full gradient.
const CELL_INSET_LIGHT: Color = Color::rgba(1.0, 0.95, 0.82, 0.08);
/// Inner-shadow band the [`draw_section_chrome`] helper
/// paints along the top + left edges of a section so the
/// niche reads as carved into the drawer rather than tiled
/// on top of it.
const SECTION_INNER_SHADOW: Color = Color::rgba(0.0, 0.0, 0.0, 0.42);
/// Highlight band along the bottom + right edges of a
/// section, matching `SECTION_INNER_SHADOW` on the opposing
/// edges so the niche reads as recessed at every viewing
/// distance.
const SECTION_INNER_LIGHT: Color = Color::rgba(1.0, 0.95, 0.82, 0.06);
/// Empty equipment-slot fill. The equipment container has a
/// stone texture; each slot gets a darker overlay so the
/// slot grid pops against the slab without losing the
/// underlying texture entirely. Darker than the bag empty-
/// cell wash so the equipped row reads as the focal column.
fn equip_slot_fill() -> Color {
    Color::rgba(0.0, 0.0, 0.0, 0.42)
}

/// Empty bag-cell fill. Lighter than `equip_slot_fill` so the
/// hierarchy reads bag < equipment when scanned: the bag is
/// the inventory you live in, equipment is the smaller,
/// more important set.
fn bag_empty_fill() -> Color {
    Color::rgba(0.0, 0.0, 0.0, 0.22)
}

/// Draw the textured stone backing used by both the bag and
/// the equipment container. `darker = true` darkens the
/// gradient slightly so the equipment slab reads as a
/// recessed niche behind the bag.
///
/// In addition to the carved-stone fill + border, paints a
/// 1px dark band along the top + left edges of the inside
/// rect (and a matching lit band along the bottom + right).
/// This gives the section a clear recessed feel against the
/// drawer behind it — without it the section + drawer both
/// use the same stone fill and blend into one surface at
/// any distance.
pub(super) fn draw_section_chrome(
    ui: &mut Ui<'_>,
    theme: &rift_ui_im::Theme,
    rect: rift_ui_im::Rect,
    darker: bool,
) {
    let fill = if darker {
        let s = theme.colors.bg_stone.0;
        Color::rgba(s[0] * 0.62, s[1] * 0.62, s[2] * 0.62, s[3])
    } else {
        let s = theme.colors.bg_stone.0;
        Color::rgba(s[0] * 0.85, s[1] * 0.85, s[2] * 0.85, s[3])
    };
    Frame::stone(theme)
        .with_fill(fill)
        .with_stroke(Stroke::new(2.0, theme.colors.border_stone))
        .with_padding(Pad::all(0.0))
        .show_only(ui, rect);

    // Inner shadow / bevel. Two pixels thick to read at
    // normal viewing distance; thinner on cells (1px) so the
    // hierarchy stays section > cell when scanned.
    let w = rect.width();
    let h = rect.height();
    if w > 8.0 && h > 8.0 {
        // Top dark band
        ui.draw_rect(
            rift_ui_im::Rect::from_xywh(rect.x() + 2.0, rect.y() + 2.0, w - 4.0, 2.0),
            SECTION_INNER_SHADOW,
        );
        // Left dark band
        ui.draw_rect(
            rift_ui_im::Rect::from_xywh(rect.x() + 2.0, rect.y() + 2.0, 2.0, h - 4.0),
            SECTION_INNER_SHADOW,
        );
        // Bottom highlight
        ui.draw_rect(
            rift_ui_im::Rect::from_xywh(rect.x() + 2.0, rect.max.y - 3.0, w - 4.0, 1.0),
            SECTION_INNER_LIGHT,
        );
        // Right highlight
        ui.draw_rect(
            rift_ui_im::Rect::from_xywh(rect.max.x - 3.0, rect.y() + 2.0, 1.0, h - 4.0),
            SECTION_INNER_LIGHT,
        );
    }
}

/// Outline a single slot cell: gold-toned outer stroke plus
/// a 1px brighter inset that fakes a chiselled bevel, and a
/// 1px inner-shadow on top + left (with a matching highlight
/// on the bottom + right) so the cell reads as a recessed
/// well rather than a stamped rectangle.
fn draw_cell_outline(ui: &mut Ui<'_>, rect: rift_ui_im::Rect) {
    ui.draw_outline(rect, 1.0, GOLD_OUTLINE);
    let inset = rift_ui_im::Rect::from_xywh(
        rect.x() + 1.0,
        rect.y() + 1.0,
        (rect.width() - 2.0).max(0.0),
        (rect.height() - 2.0).max(0.0),
    );
    ui.draw_outline(inset, 1.0, INSET_HIGHLIGHT);

    // Inner bevel: 1px dark band on top + left, 1px light
    // band on bottom + right. Sized off the inset rect so it
    // sits cleanly inside the gold outline.
    let w = inset.width();
    let h = inset.height();
    if w > 2.0 && h > 2.0 {
        ui.draw_rect(
            rift_ui_im::Rect::from_xywh(inset.x() + 1.0, inset.y() + 1.0, w - 2.0, 1.0),
            CELL_INSET_SHADOW,
        );
        ui.draw_rect(
            rift_ui_im::Rect::from_xywh(inset.x() + 1.0, inset.y() + 1.0, 1.0, h - 2.0),
            CELL_INSET_SHADOW,
        );
        ui.draw_rect(
            rift_ui_im::Rect::from_xywh(inset.x() + 1.0, inset.max.y - 2.0, w - 2.0, 1.0),
            CELL_INSET_LIGHT,
        );
        ui.draw_rect(
            rift_ui_im::Rect::from_xywh(inset.max.x - 2.0, inset.y() + 1.0, 1.0, h - 2.0),
            CELL_INSET_LIGHT,
        );
    }
}

pub fn render_paperdoll(
    ui: &mut Ui<'_>,
    layout: &Layout,
    bag_in: &BagPanelIn<'_>,
    out_actions: &mut Vec<InventoryAction>,
    in_transit: &mut Option<rift_ui_types::inventory::InTransitSource>,
) -> Option<EquipSlotIdx> {
    let theme = *ui.theme();
    let mut hovered_equip: Option<EquipSlotIdx> = None;

    // Section background — carved-stone slab, darker than the
    // bag so the equipment niche reads as recessed.
    draw_section_chrome(ui, &theme, layout.paperdoll, true);

    let slot_fill = equip_slot_fill();

    for i in 0..EquipSlotIdx::COUNT {
        let slot = EquipSlotIdx(i as u8);
        let rect = layout.paperdoll_slot_rect(slot.0);
        let id = Id::root("inv").child(("equip", i));
        let item = bag_in.equipment.get(i).and_then(|o| o.as_ref());

        // Cell chrome: darker fill + gold outline.
        ui.draw_rect(rect, slot_fill);
        draw_cell_outline(ui, rect);

        // ALWAYS call interact — `payload=None` for empty
        // slots still registers the rect as a drop target.
        let payload = item.map(|_| DragSource::Equip(slot));
        // Hide the source slot while it's being dragged so
        // only the in-flight ghost is visible.
        let dragging_this = matches!(ui.drag_payload::<DragSource>().copied(), Some(DragSource::Equip(s)) if s.0 == slot.0);
        let in_transit_this = matches!(
            bag_in.in_transit,
            Some(rift_ui_types::inventory::InTransitSource::Equip(s)) if s == slot.0
        );
        let being_dragged = dragging_this || in_transit_this;
        let render_item = if being_dragged { None } else { item };
        let r = if render_item.is_some() {
            build_item_slot(render_item).interact::<DragSource>(ui, rect, id, payload)
        } else if item.is_some() {
            // Item is hidden because it's being dragged but
            // we still need a drop target with the original
            // payload so swaps land back here.
            ItemSlot::new(rect.width().min(rect.height()))
                .transparent_bg(true)
                .interact::<DragSource>(ui, rect, id, payload)
        } else {
            // Plain empty slot: no icon, no rarity tint.
            ItemSlot::new(rect.width().min(rect.height()))
                .transparent_bg(true)
                .interact::<DragSource>(ui, rect, id, payload)
        };
        let hovered = r.response.hovered;
        route_slot_capture(
            r,
            DropTarget::Equip {
                slot,
                occupied: item.is_some(),
            },
            bag_in.stash_open,
            false,
            bag_in.active_tab_u8,
            false,
            out_actions,
            in_transit,
        );

        if item.is_some() {
            if hovered {
                hovered_equip = Some(slot);
            }
        } else {
            // Empty-slot label centered.
            let label = slot.label();
            let lw = ui.measure_text(label, theme.fonts.size_sm);
            let max_w = (rect.width() - 4.0 * layout.fit).max(0.0);
            let draw_w = lw.min(max_w);
            ui.draw_text_ellipsized(
                Pos2::new(
                    rect.x() + (rect.width() - draw_w) * 0.5,
                    rect.y() + (rect.height() - theme.fonts.size_sm) * 0.5,
                ),
                label,
                theme.fonts.size_sm,
                max_w,
                theme.colors.text_muted,
            );
        }
    }

    hovered_equip
}

pub fn render_bag_grid(
    ui: &mut Ui<'_>,
    layout: &Layout,
    bag_in: &BagPanelIn<'_>,
    armed_bag_idx_in: Option<u32>,
    out_actions: &mut Vec<InventoryAction>,
) -> BagPanelOut {
    let theme = *ui.theme();
    let mut out = BagPanelOut::default();

    // Section background — same carved-stone slab as the
    // paperdoll so the two read as parts of one container.
    draw_section_chrome(ui, &theme, layout.bag, false);

    let cols = bag_in.bag_cols;
    let rows = bag_in.bag_rows;

    // First-fit packing — work out which cells are filled so
    // we can paint empty-cell drop targets for the rest.
    let placements = pack_bag(
        bag_in.items,
        |_, it: &ItemView<'_>| (it.cell_w.max(1), it.cell_h.max(1)),
        cols,
        rows,
    );

    let cols_us = cols as usize;
    let rows_us = rows as usize;
    let mut filled = vec![false; cols_us * rows_us];
    // Per-cell owning anchor index: lets the snap-anchor
    // resolver look up "which item lives at this cell" so a
    // multi-cell ghost dropped onto another item's footprint
    // routes the swap against that item's anchor index, not a
    // random covered cell.
    let mut cell_owner: Vec<Option<u32>> = vec![None; cols_us * rows_us];
    for (idx, slot) in bag_in.items.iter().enumerate() {
        if slot.is_none() {
            continue;
        }
        let Some((x, y, w, h)) = placements[idx] else {
            continue;
        };
        for dy in 0..h as usize {
            for dx in 0..w as usize {
                let c = (y as usize + dy) * cols_us + (x as usize + dx);
                filled[c] = true;
                cell_owner[c] = Some(idx as u32);
            }
        }
    }

    // ── Snap-anchor drag preview & central drop resolver ──
    // Routes drops to a specific bag anchor (or the item
    // owning that anchor if the footprint overlaps it),
    // bypassing per-cell drop targets so swaps always land
    // where the user can see the green outline.
    let drag_pl = ui.drag_payload::<DragSource>().copied();
    if let Some(src) = drag_pl {
        let (src_w, src_h) = source_footprint(src, bag_in);
        let source_anchor_idx = match src {
            DragSource::Bag(i) => Some(i),
            _ => None,
        };
        let grid_rect = Rect::from_xywh(
            layout.bag_origin.x,
            layout.bag_origin.y,
            layout.bag_cell * cols as f32,
            layout.bag_cell * rows as f32,
        );
        let grid = GridSpec {
            rect: grid_rect,
            cell_px: layout.bag_cell,
            cols,
            rows,
            cell_owner: &cell_owner,
        };
        snap_preview_and_resolve(
            ui,
            &grid,
            src_w,
            src_h,
            source_anchor_idx,
            bag_in.stash_open,
            bag_in.active_tab_u8,
            DropTarget::Bag,
            out_actions,
            &mut out.in_transit_from_drop,
            &mut out.in_transit_dest_rect_from_drop,
        );
    }

    // Empty-cell pass — flush flat squares with drop targets.
    for cy in 0..rows {
        for cx in 0..cols {
            if filled[cy as usize * cols_us + cx as usize] {
                continue;
            }
            let cell_idx = cy as u32 * cols as u32 + cx as u32;
            let rect = layout.bag_rect(cx, cy, 1, 1);
            // Dark wash + outline so the empty cell reads as
            // a well in the stone slab. Transparent would
            // leave only the gold outline and the cell would
            // blend into the section's stone texture at
            // distance.
            ui.draw_rect(rect, bag_empty_fill());
            draw_cell_outline(ui, rect);

            // Map empty cell back to its inventory index 1:1
            // (bag storage is BAG_COLS × BAG_ROWS, row-major).
            // Drops resolve into this exact cell so the user
            // can position items deliberately.
            let inv_idx = cell_idx;
            let id = Id::root("inv").child(("bag_empty", inv_idx));
            let r = ItemSlot::new(layout.bag_cell)
                .transparent_bg(true)
                .interact::<DragSource>(ui, rect, id, None::<DragSource>);
            route_slot_capture(
                r,
                DropTarget::Bag(inv_idx),
                bag_in.stash_open,
                false,
                bag_in.active_tab_u8,
                false,
                out_actions,
                &mut out.in_transit_from_drop,
            );
        }
    }

    // Filled item pass — every filled item is rendered last so
    // its rect overlays the empty-cell outlines beneath it.
    for (idx, slot_opt) in bag_in.items.iter().enumerate() {
        let Some(item) = slot_opt.as_ref() else {
            continue;
        };
        let Some((x, y, w, h)) = placements[idx] else {
            continue;
        };
        let rect = layout.bag_rect(x, y, w, h);
        let id = Id::root("inv").child(("bag", idx as u32));
        let payload = Some(DragSource::Bag(idx as u32));

        // Transparent: let the stone frame and the rarity
        // outline carry the visual weight.

        let dragging_this = matches!(
            ui.drag_payload::<DragSource>().copied(),
            Some(DragSource::Bag(i)) if i == idx as u32
        );
        // Combine the freshly-set drop result with the
        // (frame-stale) in-transit value so the source slot
        // stays hidden on the *same* frame the drop fires —
        // otherwise the drop arms in_transit too late and the
        // source briefly re-renders for one frame before the
        // hide takes effect on frame+1.
        let effective_in_transit = out.in_transit_from_drop.or(bag_in.in_transit);
        let in_transit_this = matches!(
            effective_in_transit,
            Some(rift_ui_types::inventory::InTransitSource::Bag(i)) if i == idx as u32
        );
        let being_dragged = dragging_this || in_transit_this;
        // Hidden source: paint empty-cell chrome (gold
        // outline + inset highlight) for every covered cell
        // so the grid still reads as a slot grid while the
        // ghost is in flight.
        if being_dragged {
            for dy in 0..h {
                for dx in 0..w {
                    let cr = layout.bag_rect(x + dx, y + dy, 1, 1);
                    draw_cell_outline(ui, cr);
                }
            }
        }
        let r = if being_dragged {
            ItemSlot::new(rect.width().min(rect.height()))
                .transparent_bg(true)
                .interact::<DragSource>(ui, rect, id, payload)
        } else {
            build_item_slot(Some(item)).interact::<DragSource>(ui, rect, id, payload)
        };
        let hovered = r.response.hovered;

        if r.response.pressed && ui.ctrl_held() {
            out.salvage_press_bag_idx = Some(idx as u32);
        }
        let armed_for_this =
            armed_bag_idx_in == Some(idx as u32) || out.salvage_press_bag_idx == Some(idx as u32);
        let ctrl_release = armed_for_this && (r.clicked || r.response.drag_released);
        if ctrl_release {
            out.salvage_release_bag_idx = Some(idx as u32);
        } else {
            route_slot_capture(
                r,
                DropTarget::Bag(idx as u32),
                bag_in.stash_open,
                false,
                bag_in.active_tab_u8,
                item.is_consumable,
                out_actions,
                &mut out.in_transit_from_drop,
            );
        }
        if hovered {
            out.hovered_bag = Some(idx as u32);
            out.hovered_bag_rect = Some(rect);
        }

        // Rarity-tinted outline on top. Skipped while the
        // slot is the drag source so it reads as empty.
        if !being_dragged {
            let [rr, gg, bb, _] = item.rarity_color;
            ui.draw_outline(rect, 1.5, Color::rgba(rr, gg, bb, 0.95));
        }
    }

    out
}

/// Look up the dragged item's cell footprint from whichever
/// source view holds it. Stash items aren't carried through
/// [`BagPanelIn`], so a stash → bag drag falls back to a 1×1
/// preview (still anchor-correct, just visually conservative).
fn source_footprint(src: DragSource, bag_in: &BagPanelIn<'_>) -> (u8, u8) {
    match src {
        DragSource::Bag(idx) => bag_in
            .items
            .get(idx as usize)
            .and_then(|o| o.as_ref())
            .map(|it| (it.cell_w.max(1), it.cell_h.max(1)))
            .unwrap_or((1, 1)),
        DragSource::Equip(slot) => bag_in
            .equipment
            .get(slot.0 as usize)
            .and_then(|o| o.as_ref())
            .map(|it| (it.cell_w.max(1), it.cell_h.max(1)))
            .unwrap_or((1, 1)),
        DragSource::Stash(idx) => bag_in
            .stash_active
            .get(idx as usize)
            .and_then(|o| o.as_ref())
            .map(|it| (it.cell_w.max(1), it.cell_h.max(1)))
            .unwrap_or((1, 1)),
    }
}
