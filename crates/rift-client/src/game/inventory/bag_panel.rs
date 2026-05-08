//! Bag + equipment row panel.
//!
//! 6×5 bag grid plus a 10-slot equipment row across the top,
//! framed in the standard panel chrome with a footer that
//! hosts the bulk Salvage Trash button. Owns the press-time
//! Ctrl+click salvage latch (so a 6-px mouse jiggle doesn't
//! convert a salvage into a no-op drag).

use rift_engine::ui::im::{
    Color, Frame, Id, MiniButton, MiniButtonFills, Pad, Pos2, Rect, Tooltip, TooltipLine, Ui,
};
use rift_game::loot::{Equipment, EquipSlot, Item};

use crate::game::sub_state::{EquipRequest, StashRequest};

use super::drag::{
    build_item_slot, route_slot, DragSource, DropTarget,
};
use super::layout::{Layout, COLS, EQUIP_COLS, ROWS};
use super::salvage::BulkSalvagePreview;

/// Inputs threaded into the bag panel that the panel itself
/// can't recompute (e.g. the persistent salvage-arm latch
/// stored on `MpInventoryUI`).
pub struct BagPanelIn<'a> {
    pub items: &'a [Option<Item>],
    pub equipment: &'a Equipment,
    pub stash_open: bool,
    pub active_tab_u8: u8,
    /// `true` while the Salvage Trash button is in the armed
    /// (red, awaiting-confirm) state. Drives label / colour
    /// only — the orchestrator owns the timer state.
    pub salvage_armed: bool,
    pub salvage_armed_bag_idx: Option<usize>,
}

/// Per-frame outputs the orchestrator needs after the panel
/// has run: hover state for tooltip routing, a press flag for
/// the 2-stage salvage button, and the (possibly updated)
/// armed-bag-slot latch.
#[derive(Default)]
pub struct BagPanelOut {
    pub hovered_item: Option<Item>,
    pub hovered_from_equip: bool,
    pub hovered_from_bag: bool,
    pub bulk_preview: BulkSalvagePreview,
    pub pressed_salvage_trash: bool,
    pub salvage_armed_bag_idx: Option<usize>,
}

pub fn render_bag_panel(
    ui: &mut Ui<'_>,
    layout: &Layout,
    bag_in: BagPanelIn<'_>,
    pending: &mut Vec<EquipRequest>,
    stash_pending: &mut Vec<StashRequest>,
) -> BagPanelOut {
    let theme = *ui.theme();
    let panel_rect = layout.bag_panel;
    let BagPanelIn {
        items,
        equipment,
        stash_open,
        active_tab_u8,
        salvage_armed,
        salvage_armed_bag_idx,
    } = bag_in;

    let mut out = BagPanelOut::default();
    out.bulk_preview = BulkSalvagePreview::scan(items);

    // Hover trackers for the bag-side panel only. Stash hover
    // is collected separately by the stash panel.
    let mut hovered_item: Option<Item> = None;
    let mut hovered_from_equip = false;
    let mut hovered_from_bag = false;

    let pressed_salvage_trash = std::cell::Cell::new(false);
    // Press-time ctrl latch shared between the bag closure
    // (which both reads it and updates it on press/click)
    // and the post-closure logic that copies the final
    // value back into `self`. Seed it with whatever is
    // currently latched so an in-flight arm survives
    // re-renders.
    let armed_cell: std::cell::Cell<Option<usize>> =
        std::cell::Cell::new(salvage_armed_bag_idx);
    let armed_idx_set = &armed_cell;
    let bulk_count = out.bulk_preview.count;
    let bulk_yield = out.bulk_preview.yield_shards;

    Frame::panel(&theme)
        .with_padding(Pad::all(layout.pad))
        .show(ui, panel_rect, |ui, body| {
            // Title row: "INVENTORY" left, "EQUIPPED Y/9"
            // right. Same row so we don't double-stack
            // labels (the old layout had EQUIPPED clipping
            // the bag count).
            ui.draw_text(
                Pos2::new(body.x(), body.y()),
                "INVENTORY",
                theme.fonts.size_lg,
                theme.colors.text,
            );
            let header_label = format!(
                "EQUIPPED  {}/{}        BAG  {}/{}",
                equipment.count(),
                EquipSlot::COUNT,
                items.iter().filter(|s| s.is_some()).count(),
                COLS * ROWS,
            );
            let cw = ui.measure_text(&header_label, theme.fonts.size_md);
            // If the combined string wouldn't fit (very
            // narrow `fit`) just ellipsize from the right
            // edge — `draw_text_ellipsized` keeps it from
            // bleeding under the title.
            let counts_max = body.width()
                - ui.measure_text("INVENTORY", theme.fonts.size_lg)
                - 12.0_f32 * layout.fit;
            ui.draw_text_ellipsized(
                Pos2::new(body.max.x - cw.min(counts_max), body.y() + 4.0),
                &header_label,
                theme.fonts.size_md,
                counts_max.max(0.0),
                theme.colors.text_dim,
            );

            // Header underline.
            ui.draw_rect(
                Rect::from_xywh(body.x(), body.y() + theme.fonts.size_lg + 8.0, body.width(), 1.0),
                theme.colors.border,
            );

            // ─── Equipment row (above bag) ──────────────
            // 9 slots laid out horizontally, centred in
            // the panel body so a 6-column bag below sits
            // visually inside the same column band.
            let equip_row_w =
                EQUIP_COLS as f32 * (layout.slot + layout.gap) - layout.gap;
            let equip_x = body.x() + (body.width() - equip_row_w) * 0.5;
            let equip_y = body.y() + layout.header_h;
            for (i, slot) in EquipSlot::ALL.iter().enumerate() {
                let pos = Pos2::new(
                    equip_x + i as f32 * (layout.slot + layout.gap),
                    equip_y,
                );
                let id = Id::root("inv").child(("equip", i));
                let rect = Rect::from_xywh(pos.x, pos.y, layout.slot, layout.slot);
                let item = equipment.get(*slot);
                let payload = item.map(|_| DragSource::Equip(*slot));
                let r = build_item_slot(item).interact::<DragSource>(ui, rect, id, payload);
                let hovered = r.response.hovered;
                route_slot(
                    r,
                    DropTarget::Equip(*slot),
                    stash_open,
                    false,
                    active_tab_u8,
                    pending,
                    stash_pending,
                );
                if let Some(it) = item {
                    if hovered {
                        hovered_item = Some(it.clone());
                        hovered_from_equip = true;
                    }
                } else {
                    // Empty equip slot: overlay the slot
                    // label centred so the player knows
                    // what goes here without a tooltip.
                    let label = slot.label();
                    let lw = ui.measure_text(label, theme.fonts.size_sm);
                    // Cap the label to the slot width
                    // minus a small margin — most slot
                    // labels (e.g. "Helmet", "Ring") fit,
                    // but "Necklace" / "Shoulders" can run
                    // long and would otherwise spill onto
                    // neighbours.
                    let max_lbl = layout.slot - 6.0 * layout.fit;
                    let draw_w = lw.min(max_lbl);
                    ui.draw_text_ellipsized(
                        Pos2::new(
                            rect.x() + (layout.slot - draw_w) * 0.5,
                            rect.y() + (layout.slot - theme.fonts.size_sm) * 0.5,
                        ),
                        label,
                        theme.fonts.size_sm,
                        max_lbl,
                        theme.colors.text_muted,
                    );
                }
            }

            // Horizontal divider between equip row and bag.
            let div_y = equip_y + layout.slot + layout.inner_gap * 0.5;
            ui.draw_rect(
                Rect::from_xywh(body.x(), div_y, body.width(), 1.0),
                theme.colors.border,
            );

            // ─── Bag grid (6 cols × 5 rows) ─────────────
            let bag_grid_w =
                COLS as f32 * (layout.slot + layout.gap) - layout.gap;
            let bag_x = body.x() + (body.width() - bag_grid_w) * 0.5;
            let bag_y = equip_y + layout.slot + layout.inner_gap;
            for row in 0..ROWS {
                for col in 0..COLS {
                    let idx = row * COLS + col;
                    let pos = Pos2::new(
                        bag_x + col as f32 * (layout.slot + layout.gap),
                        bag_y + row as f32 * (layout.slot + layout.gap),
                    );
                    let id = Id::root("inv").child(("bag", idx));
                    let rect = Rect::from_xywh(pos.x, pos.y, layout.slot, layout.slot);
                    let item = items.get(idx).and_then(|o| o.as_ref());
                    let payload = item.map(|_| DragSource::Bag(idx));
                    let r = build_item_slot(item).interact::<DragSource>(ui, rect, id, payload);
                    let hovered = r.response.hovered;
                    // Ctrl+click salvage path. Resolved
                    // **before** `route_slot` because the
                    // engine's drag-source starts a latent
                    // drag on press; if the player jiggles
                    // past the 6-px drag threshold the
                    // release fires `drag_released` /
                    // `dropped` instead of `clicked`, and
                    // the no-op bag→same-bag drop swallows
                    // the intent. Latching the slot at
                    // press time and firing on **either**
                    // `clicked` or `drag_released` makes
                    // the click feel reliable regardless
                    // of how steady the player's hand is.
                    // Anchored items intentionally still
                    // arm — the server rejects the salvage
                    // and the latch clears the same way.
                    if r.response.pressed && ui.ctrl_held() && item.is_some() {
                        armed_idx_set.set(Some(idx));
                    }
                    let armed_for_this = armed_idx_set.get() == Some(idx);
                    let ctrl_release = armed_for_this
                        && (r.clicked || r.response.drag_released);
                    if ctrl_release {
                        // Fire the salvage and short-circuit
                        // the regular slot routing so the
                        // same release can't *also* be
                        // interpreted as an equip / deposit.
                        pending.push(EquipRequest::Salvage {
                            inventory_index: idx as u32,
                        });
                        armed_idx_set.set(None);
                    } else {
                        route_slot(
                            r,
                            DropTarget::Bag(idx),
                            stash_open,
                            false,
                            active_tab_u8,
                            pending,
                            stash_pending,
                        );
                    }
                    if let Some(it) = item {
                        if hovered {
                            hovered_item = Some(it.clone());
                            hovered_from_bag = true;
                        }
                    }
                }
            }

            // Footer: divider, compact Salvage Trash icon
            // button on the right, hint text on the left.
            // Icon-only with a hover tooltip mirrors the
            // stash "+ Buy" affordance so the player learns
            // one mental model. 2-stage commit: first click
            // arms (icon flips to ✓ on red); second click
            // within `SALVAGE_CONFIRM_WINDOW_S` commits.
            let hint = if stash_open {
                "F: close stash  \u{00B7}  drag bag\u{2194}stash"
            } else {
                "TAB: close  \u{00B7}  drag/equip/drop  \u{00B7}  CTRL+click: salvage  \u{00B7}  SHIFT: compare"
            };
            ui.draw_rect(
                Rect::from_xywh(body.x(), body.max.y - layout.footer_h + 4.0, body.width(), 1.0),
                theme.colors.border,
            );
            let armed = salvage_armed;
            // Icon: recycle for idle, check-mark for the
            // armed "click again to confirm" state.
            let btn_icon: &str = if armed { "\u{2713}" } else { "\u{267B}" };
            let btn_size = theme.fonts.size_md;
            // Square button sized to the hint text height
            // plus a touch of vertical padding, top-aligned
            // with the hint baseline so the row reads as a
            // single horizontal stripe instead of two
            // floating elements.
            let btn_pad_v = 3.0 * layout.fit;
            let btn_h = btn_size + btn_pad_v * 2.0;
            let btn_w = btn_h;
            let btn_top = body.max.y - btn_size - btn_pad_v;
            let btn_rect = Rect::from_xywh(
                body.max.x - btn_w,
                btn_top,
                btn_w,
                btn_h,
            );
            let enabled = bulk_count > 0;
            // Armed = destructive red; idle = neutral panel
            // chrome.
            let fills = if armed {
                MiniButtonFills::explicit(
                    Color::rgba(0.65, 0.25, 0.15, 0.85),
                    Color::rgba(0.85, 0.32, 0.20, 0.95),
                    Color::rgba(0.16, 0.16, 0.18, 0.5),
                )
            } else {
                MiniButtonFills::explicit(
                    Color::rgba(0.20, 0.20, 0.25, 0.80),
                    Color::rgba(0.30, 0.30, 0.36, 0.95),
                    Color::rgba(0.16, 0.16, 0.18, 0.5),
                )
            };
            let btn_id = Id::root("inv").child(("salvage_trash", armed as u32));
            let resp = MiniButton::new(btn_icon, fills)
                .text_size(btn_size)
                .enabled(enabled)
                .show(ui, btn_id, btn_rect);
            if resp.clicked {
                pressed_salvage_trash.set(true);
            }
            // Tooltip — re-hit the rect with `interact_hover`
            // so it fires even while the button is disabled
            // (where MiniButton's own hover branch returns
            // false). Mirrors the stash "+ Buy" pattern.
            let tip_hov = ui.interact_hover(btn_id.child("tip"), btn_rect);
            if tip_hov {
                let count_str = format!("{} items", bulk_count);
                let yield_str = format!("Yield: {} \u{25C6}", bulk_yield);
                let mut lines: Vec<TooltipLine<'_>> = Vec::with_capacity(3);
                if bulk_count == 0 {
                    lines.push(TooltipLine::new(
                        "No salvageable items",
                        theme.fonts.size_sm,
                        Color::rgba(0.95, 0.40, 0.35, 1.0),
                    ));
                } else {
                    lines.push(TooltipLine::new(
                        &count_str,
                        theme.fonts.size_sm,
                        theme.colors.text,
                    ));
                    lines.push(TooltipLine::new(
                        &yield_str,
                        theme.fonts.size_sm,
                        theme.colors.text_dim,
                    ));
                    if armed {
                        lines.push(TooltipLine::new(
                            "Click again to confirm",
                            theme.fonts.size_sm,
                            Color::rgba(0.95, 0.55, 0.35, 1.0),
                        ));
                    } else {
                        lines.push(TooltipLine::new(
                            "Click to arm \u{00B7} click again to confirm",
                            theme.fonts.size_sm,
                            theme.colors.text_dim,
                        ));
                    }
                }
                Tooltip::new()
                    .header("Salvage Trash")
                    .min_width(180.0)
                    .anchor_to(btn_rect)
                    .show(ui, Pos2::new(btn_rect.x(), btn_rect.y()), &lines);
            }
            // Hint text on the left, top-aligned with the
            // button so the footer row reads as a single
            // baseline.
            ui.draw_text_ellipsized(
                Pos2::new(body.x(), btn_top + btn_pad_v),
                hint,
                theme.fonts.size_md,
                (btn_rect.x() - body.x() - 8.0 * layout.fit).max(0.0),
                theme.colors.text_dim,
            );
        });

    out.hovered_item = hovered_item;
    out.hovered_from_equip = hovered_from_equip;
    out.hovered_from_bag = hovered_from_bag;
    out.pressed_salvage_trash = pressed_salvage_trash.get();
    out.salvage_armed_bag_idx = armed_cell.get();
    out
}
