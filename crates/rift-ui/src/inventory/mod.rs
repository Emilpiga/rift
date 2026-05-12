//! Inventory drawer orchestrator.
//!
//! `frame_inventory` runs one frame of the right-side
//! inventory drawer: handles input + open/close, computes
//! layout, dispatches to paperdoll / bag / stash / stats /
//! currency renderers, draws tooltips and the drag ghost,
//! and returns a vector of [`InventoryAction`] for the host
//! to forward onto the wire.
//!
//! All persistent state lives in [`InventoryUiState`] owned
//! by the host so this crate stays hot-reloadable.

mod bag_panel;
mod drag;
mod grid_drop;
mod layout;
mod stash_panel;
mod stats_panel;
mod tooltips;

use rift_ui_im::{Button, ButtonSize, Color, Frame, Id, ImKey, Pad, Pos2, Rect, Ui};
use rift_ui_types::inventory::{
    DragSource, InventoryAction, InventoryUiState, InventoryView, ItemView,
};

use self::bag_panel::{render_bag_grid, render_paperdoll, BagPanelIn};
use self::layout::{Layout, HEADER_H, PANEL_PAD_X, PANEL_PAD_Y};
use self::stash_panel::{render_stash_panel, FilterStateRef, RenameStateRef, StashPanelIn};
use self::stats_panel::render_stats_panel;
use self::tooltips::{
    render_compare_delta_side_of, render_item_tooltip, render_item_tooltip_anchored,
    render_item_tooltip_side_of,
};

/// Run one frame of the inventory drawer.
///
/// * `view` — borrowed per-frame snapshot.
/// * `state` — persistent UI state (lives in the host).
/// * `time` — monotonic seconds; powers the salvage 2-stage
///   confirm window.
///
/// Returns `(open, actions)`.
pub fn frame_inventory(
    ui: &mut Ui<'_>,
    view: &InventoryView<'_>,
    state: &mut InventoryUiState,
    time: f64,
) -> (bool, Vec<InventoryAction>) {
    let mut actions: Vec<InventoryAction> = Vec::new();
    let stash_session = view.stash.is_some();

    if state.salvage_confirm_window_s <= 0.0 {
        state.salvage_confirm_window_s = 3.0;
    }

    // Tab toggles when no chest session is forcing the panel.
    if ui.input().key_just_pressed(ImKey::Tab) && !stash_session {
        state.open = !state.open;
        if !state.open {
            actions.push(InventoryAction::Close);
        }
    }
    if stash_session {
        state.open = true;
        state.show_stash = true;
    } else {
        // Stash is only accessible while interacting with a
        // chest — no manual toggle.
        state.show_stash = false;
    }
    if !state.open {
        ui.cancel_drag();
        state.cached_bag_rect = [0.0; 4];
        state.cached_stats_rect = [0.0; 4];
        state.cached_stash_rect = [0.0; 4];
        state.rename_target_tab = None;
        state.rename_buffer.clear();
        state.rename_has_focused = false;
        state.salvage_armed_at = None;
        state.salvage_armed_bag_idx = None;
        return (false, actions);
    }

    let layout = Layout::compute(
        ui,
        view.bag_cols.max(1),
        view.bag_rows.max(1),
        state.show_stats,
        state.show_stash || stash_session,
    );

    state.cached_bag_rect = rect_to_array(layout.drawer);
    state.cached_stats_rect = rect_to_array(layout.stats_drawer);
    state.cached_stash_rect = if stash_session {
        rect_to_array(layout.stash_drawer)
    } else {
        [0.0; 4]
    };

    if let Some(t0) = state.salvage_armed_at {
        if time - t0 > state.salvage_confirm_window_s {
            state.salvage_armed_at = None;
        }
    }
    if let Some(stash) = view.stash.as_ref() {
        if !stash.tabs.is_empty() {
            let max = (stash.tabs.len() - 1) as u8;
            if state.active_stash_tab > max {
                state.active_stash_tab = max;
            }
        } else {
            state.active_stash_tab = 0;
        }
        if let Some(idx) = state.rename_target_tab {
            if (idx as usize) >= stash.tabs.len() {
                state.rename_target_tab = None;
                state.rename_buffer.clear();
                state.rename_has_focused = false;
            }
        }
    } else {
        state.rename_target_tab = None;
        state.rename_buffer.clear();
        state.rename_has_focused = false;
    }

    // ── Drawer chrome ───────────────────────────────
    let theme = *ui.theme();
    Frame::stone(&theme)
        .with_padding(Pad::all(0.0))
        .show_only(ui, layout.drawer);

    // ── Header ──────────────────────────────────────
    ui.draw_text(
        Pos2::new(layout.header.x(), layout.header.y()),
        "INVENTORY",
        theme.fonts.size_lg,
        theme.colors.text,
    );
    ui.draw_rect(
        Rect::from_xywh(
            layout.header.x(),
            layout.header.max.y,
            layout.header.width(),
            1.0,
        ),
        theme.colors.border_stone,
    );

    let active_tab_u8 = state.active_stash_tab;
    let stash_active_items: &[Option<ItemView<'_>>] = view
        .stash
        .as_ref()
        .and_then(|s| s.tabs.get(active_tab_u8 as usize))
        .map(|t| t.items)
        .unwrap_or(&[]);
    let bag_in = BagPanelIn {
        items: view.items,
        equipment: view.equipment,
        stash_active: stash_active_items,
        bag_cols: view.bag_cols.max(1),
        bag_rows: view.bag_rows.max(1),
        stash_open: stash_session,
        active_tab_u8,
        in_transit: state.in_transit_source,
    };

    // ── Paperdoll ───────────────────────────────────
    // Expire stale in-transit hides if the server reply
    // didn't land within the timeout (lost packet, etc.).
    if state.in_transit_source.is_some() && time - state.in_transit_set_at > 0.40 {
        state.in_transit_source = None;
        state.in_transit_dest_rect = None;
    }
    let hovered_equip = render_paperdoll(
        ui,
        &layout,
        &bag_in,
        &mut actions,
        &mut state.in_transit_source,
    );
    if state.in_transit_source.is_some() {
        state.in_transit_set_at = time;
    }

    // ── Bag grid ────────────────────────────────────
    let bag_out = render_bag_grid(
        ui,
        &layout,
        &bag_in,
        state.salvage_armed_bag_idx,
        &mut actions,
    );
    if let Some(src) = bag_out.in_transit_from_drop {
        state.in_transit_source = Some(src);
        state.in_transit_set_at = time;
        state.in_transit_dest_rect = bag_out.in_transit_dest_rect_from_drop;
    }

    // Salvage latch updates.
    if let Some(idx) = bag_out.salvage_press_bag_idx {
        state.salvage_armed_bag_idx = Some(idx);
    }
    if let Some(idx) = bag_out.salvage_release_bag_idx {
        actions.push(InventoryAction::Salvage {
            inventory_index: idx,
        });
        state.salvage_armed_bag_idx = None;
    }
    if state.salvage_armed_bag_idx.is_some()
        && ui.input().left_just_released()
        && bag_out.salvage_release_bag_idx.is_none()
    {
        state.salvage_armed_bag_idx = None;
    }

    // ── Toggle bar ──────────────────────────────────
    let toggle_out = render_toggle_bar(ui, &layout, state, stash_session, view.bulk_salvage.count);

    if toggle_out.sort_bag_clicked {
        actions.push(InventoryAction::SortBag);
    }

    // 2-stage Salvage Trash bulk button.
    if toggle_out.salvage_trash_clicked && view.bulk_salvage.count > 0 {
        if let Some(t0) = state.salvage_armed_at {
            if time - t0 <= state.salvage_confirm_window_s {
                actions.push(InventoryAction::SalvageBulk { rarity_max: 1 });
                state.salvage_armed_at = None;
            } else {
                state.salvage_armed_at = Some(time);
            }
        } else {
            state.salvage_armed_at = Some(time);
        }
    }

    // ── Stats subsection (own drawer, left of inventory) ─
    if state.show_stats {
        Frame::stone(&theme)
            .with_padding(Pad::all(0.0))
            .show_only(ui, layout.stats_drawer);
        let inner = layout.stats_drawer.shrink2(Pad::symmetric(
            PANEL_PAD_X * layout.fit,
            PANEL_PAD_Y * layout.fit,
        ));
        render_stats_panel(ui, inner, &view.stats, layout.fit);
    }

    // ── Stash subsection (own drawer, mirror of inventory on the left) ─
    let stash_hovered: Option<(u32, Rect)> = if let Some(stash_view) = view.stash.as_ref() {
        if state.show_stash || stash_session {
            Frame::stone(&theme)
                .with_padding(Pad::all(0.0))
                .show_only(ui, layout.stash_drawer);
            let content = layout.stash_drawer.shrink2(Pad::symmetric(
                PANEL_PAD_X * layout.fit,
                PANEL_PAD_Y * layout.fit,
            ));
            // STASH header, mirroring the INVENTORY header chrome.
            let header_h = HEADER_H * layout.fit;
            let stash_header = Rect::from_xywh(content.x(), content.y(), content.width(), header_h);
            ui.draw_text(
                Pos2::new(stash_header.x(), stash_header.y()),
                "STASH",
                theme.fonts.size_lg,
                theme.colors.text,
            );
            ui.draw_rect(
                Rect::from_xywh(
                    stash_header.x(),
                    stash_header.max.y,
                    stash_header.width(),
                    1.0,
                ),
                theme.colors.border_stone,
            );
            let inner = Rect::from_xywh(
                content.x(),
                stash_header.max.y + 6.0 * layout.fit,
                content.width(),
                (content.max.y - (stash_header.max.y + 6.0 * layout.fit)).max(0.0),
            );
            let pre = actions.len();
            let out = render_stash_panel(
                ui,
                inner,
                StashPanelIn {
                    view: stash_view,
                    bag_items: &view.items,
                    equipment: &view.equipment,
                    active_idx: state.active_stash_tab as usize,
                    fit: layout.fit,
                    bag_cell: layout.bag_cell,
                    in_transit: state.in_transit_source,
                },
                RenameStateRef {
                    target_tab: &mut state.rename_target_tab,
                    buffer: &mut state.rename_buffer,
                    has_focused: &mut state.rename_has_focused,
                },
                FilterStateRef {
                    rarity_mask: &mut state.stash_filter_rarity_mask,
                    stat_keys: &mut state.stash_filter_stats,
                },
                time as f32,
                &mut actions,
            );
            for act in &actions[pre..] {
                if let InventoryAction::SwitchStashTab { tab_index } = act {
                    state.active_stash_tab = *tab_index;
                    state.rename_target_tab = None;
                    state.rename_buffer.clear();
                    state.rename_has_focused = false;
                }
            }
            if let Some(src) = out.in_transit_from_drop {
                state.in_transit_source = Some(src);
                state.in_transit_set_at = time;
                state.in_transit_dest_rect = out.in_transit_dest_rect_from_drop;
            }
            out.hovered_stash.zip(out.hovered_stash_rect)
        } else {
            None
        }
    } else {
        None
    };

    // ── Currency bar ────────────────────────────────
    render_currency_bar(ui, layout.currency_bar, view.currency_shards, layout.fit);

    // ── Tooltips ────────────────────────────────────
    // Resolve which item is hovered AND a "panel band" rect:
    // the panel's full horizontal extent at the hovered
    // slot's vertical position. The tooltip uses this as its
    // anchor so the side decision pushes it OUTSIDE the
    // panel rather than just outside the slot — otherwise the
    // tooltip lands on top of neighboring slots.
    let tip: Option<(&ItemView<'_>, TooltipSource, Rect)> = if let Some(idx) = bag_out.hovered_bag {
        let slot_r = bag_out.hovered_bag_rect.unwrap_or_else(|| {
            Rect::from_xywh(layout.drawer.x() - 8.0, ui.mouse_pos().y, 1.0, 1.0)
        });
        let band = Rect::from_xywh(
            layout.drawer.x(),
            slot_r.y(),
            layout.drawer.width(),
            slot_r.height(),
        );
        view.items
            .get(idx as usize)
            .and_then(|o| o.as_ref())
            .map(|it| (it, TooltipSource::Bag, band))
    } else if let Some(slot) = hovered_equip {
        let slot_r = layout.paperdoll_slot_rect(slot.0);
        let band = Rect::from_xywh(
            layout.drawer.x(),
            slot_r.y(),
            layout.drawer.width(),
            slot_r.height(),
        );
        view.equipment
            .get(slot.0 as usize)
            .and_then(|o| o.as_ref())
            .map(|it| (it, TooltipSource::Equip, band))
    } else if let Some((idx, slot_r)) = stash_hovered {
        let band = Rect::from_xywh(
            layout.stash_drawer.x(),
            slot_r.y(),
            layout.stash_drawer.width(),
            slot_r.height(),
        );
        view.stash
            .as_ref()
            .and_then(|s| s.tabs.get(state.active_stash_tab as usize))
            .and_then(|t| t.items.get(idx as usize).and_then(|o| o.as_ref()))
            .map(|it| (it, TooltipSource::Stash, band))
    } else {
        None
    };

    if let Some((item, src, anchor_rect)) = tip {
        // Pick the side (left vs right of the *panel*) with
        // more breathing room so the tooltip never spills
        // off-screen and never lands on top of neighboring
        // slots in the same panel.
        let screen_w = ui.screen_size().x;
        let space_right = screen_w - anchor_rect.max.x;
        let space_left = anchor_rect.x();
        let prefer_left = space_left > space_right;
        let primary_anchor = if prefer_left {
            Pos2::new(anchor_rect.x(), anchor_rect.y())
        } else {
            Pos2::new(anchor_rect.max.x, anchor_rect.y())
        };
        let primary = render_item_tooltip_anchored(
            ui,
            item,
            "Hovered",
            anchor_rect,
            prefer_left,
            primary_anchor,
        );

        if src == TooltipSource::Bag && ui.ctrl_held() {
            let hint = if item.anchored {
                "Anchored \u{2014} cannot be salvaged".to_string()
            } else {
                format!(
                    "Ctrl+click \u{2192} Salvage for {} \u{25C6}",
                    item.salvage_yield
                )
            };
            let hint_size = theme.fonts.size_md;
            let hw = ui.measure_text(&hint, hint_size);
            let pad = 8.0 * layout.fit;
            let hint_rect = Rect::from_xywh(
                primary.x(),
                primary.max.y + 4.0 * layout.fit,
                hw + pad * 2.0,
                hint_size + pad,
            );
            let bg = if item.anchored {
                Color::rgba(0.40, 0.20, 0.18, 0.92)
            } else {
                Color::rgba(0.18, 0.30, 0.22, 0.92)
            };
            ui.draw_rect(hint_rect, bg);
            ui.draw_text(
                Pos2::new(hint_rect.x() + pad, hint_rect.y() + pad * 0.5),
                &hint,
                hint_size,
                theme.colors.text,
            );
        }

        if src != TooltipSource::Equip {
            if let Some(equipped) = item.compare_with {
                // Chain the Equipped tooltip in the SAME
                // direction the primary extended away from
                // its panel — otherwise it loops back and
                // covers slots in the same panel. The Δ
                // panel chains one step further in that
                // direction.
                let eq_rect =
                    render_item_tooltip_side_of(ui, equipped, "Equipped", primary, prefer_left);
                if ui.shift_held() && !item.compare_delta.is_empty() {
                    render_compare_delta_side_of(ui, item.compare_delta, eq_rect, prefer_left);
                }
                // Secondary equipped (rings only) stacks
                // vertically *below* the primary equipped
                // tooltip, with its own Δ panel chained next
                // to it in the same `prefer_left` direction
                // so the player can read both ring slots at
                // a glance instead of having to swap rings
                // around to compare.
                if let Some(equipped2) = item.compare_with_secondary {
                    let gap = 6.0 * layout.fit;
                    let anchor2 = Pos2::new(eq_rect.x(), eq_rect.max.y + gap);
                    let eq2_rect = render_item_tooltip(ui, equipped2, "Equipped", anchor2);
                    if ui.shift_held() && !item.compare_delta_secondary.is_empty() {
                        render_compare_delta_side_of(
                            ui,
                            item.compare_delta_secondary,
                            eq2_rect,
                            prefer_left,
                        );
                    }
                }
            }
        }
    }

    // ── Destination ghost ────────────────────────────
    // While a move is in flight (dropped client-side, server
    // reply not yet landed) paint a translucent copy of the
    // source item at the destination rect. Without this the
    // target slot reads as empty between the drop frame and
    // the next inventory snapshot, producing a one-frame
    // "flicker on target before it lands".
    if let (Some(src), Some(rect_arr)) = (state.in_transit_source, state.in_transit_dest_rect) {
        use rift_ui_types::inventory::InTransitSource;
        let item = match src {
            InTransitSource::Bag(idx) => view.items.get(idx as usize).and_then(|o| o.as_ref()),
            InTransitSource::Equip(slot) => {
                view.equipment.get(slot as usize).and_then(|o| o.as_ref())
            }
            InTransitSource::Stash { tab, idx } => view
                .stash
                .as_ref()
                .and_then(|s| s.tabs.get(tab as usize))
                .and_then(|t| t.items.get(idx as usize).and_then(|o| o.as_ref())),
        };
        if let Some(it) = item {
            let rect = Rect::from_xywh(rect_arr[0], rect_arr[1], rect_arr[2], rect_arr[3]);
            self::drag::build_item_slot(Some(it))
                .dim_alpha(0.55)
                .show_rect(ui, rect, Id::root("inv").child("in_transit_dest_ghost"));
        }
    }

    // ── Drag ghost + outside-drop ───────────────────
    if let Some(payload) = ui.drag_payload::<DragSource>().copied() {
        let item = match payload {
            DragSource::Bag(idx) => view.items.get(idx as usize).and_then(|o| o.as_ref()),
            DragSource::Equip(slot) => view.equipment.get(slot.0 as usize).and_then(|o| o.as_ref()),
            DragSource::Stash(idx) => view
                .stash
                .as_ref()
                .and_then(|s| s.tabs.get(state.active_stash_tab as usize))
                .and_then(|t| t.items.get(idx as usize).and_then(|o| o.as_ref())),
        };
        if let Some(it) = item {
            // Ghost size mirrors the source slot's actual
            // footprint so the in-flight rectangle matches the
            // real cell area the drop will occupy.
            let (w, h) = match payload {
                DragSource::Bag(_) | DragSource::Stash(_) => {
                    let cw = it.cell_w.max(1) as f32;
                    let ch = it.cell_h.max(1) as f32;
                    (cw * layout.bag_cell, ch * layout.bag_cell)
                }
                DragSource::Equip(slot) => {
                    let r = layout.paperdoll_slot_rect(slot.0);
                    (r.width(), r.height())
                }
            };
            self::drag::build_item_slot(Some(it)).show_ghost_size(ui, w, h);
        }
    }
    if let Some(drop) = ui.take_drop_outside::<DragSource>() {
        match drop.payload {
            DragSource::Bag(idx) => {
                actions.push(InventoryAction::DropToWorld {
                    inventory_index: idx,
                });
            }
            DragSource::Equip(slot) => {
                actions.push(InventoryAction::DropEquipToWorld { slot: slot.0 });
            }
            DragSource::Stash(_) => {
                // Stash items aren't allowed to drop onto the
                // ground — they're already safe-stored.
            }
        }
    }

    (true, actions)
}

#[derive(Default)]
struct ToggleBarOut {
    salvage_trash_clicked: bool,
    sort_bag_clicked: bool,
}

fn render_toggle_bar(
    ui: &mut Ui<'_>,
    layout: &Layout,
    state: &mut InventoryUiState,
    _stash_forced: bool,
    bulk_count: u32,
) -> ToggleBarOut {
    let mut out = ToggleBarOut::default();
    let bar = layout.toggle_bar;
    if bar.width() <= 0.0 {
        return out;
    }
    let chip_w = 110.0 * layout.fit;

    // Stats chip
    let stats_rect = Rect::from_xywh(bar.x(), bar.y(), chip_w, bar.height());
    let stats_label = if state.show_stats {
        "Stats \u{25BC}"
    } else {
        "Stats \u{25B6}"
    };
    let r = if state.show_stats {
        Button::active(stats_label)
    } else {
        Button::new(stats_label)
    }
    .size(ButtonSize::Small)
    .show_with_id(ui, Id::root("inv").child("toggle_stats"), stats_rect);
    if r.clicked {
        state.show_stats = !state.show_stats;
    }

    // Sort chip immediately to the right of Stats.
    let sort_lbl = "\u{21C5} Sort";
    let sort_w = 100.0 * layout.fit;
    let sort_rect = Rect::from_xywh(
        bar.x() + chip_w + 6.0 * layout.fit,
        bar.y(),
        sort_w,
        bar.height(),
    );
    let sort_btn = Button::new(sort_lbl).size(ButtonSize::Small).show_with_id(
        ui,
        Id::root("inv").child("toggle_sort"),
        sort_rect,
    );
    if sort_btn.clicked {
        out.sort_bag_clicked = true;
    }

    // Salvage Trash on the right (2-stage handled by caller).
    let salvage_armed = state.salvage_armed_at.is_some();
    let armed_label = if salvage_armed {
        "\u{2713} Confirm Salvage"
    } else {
        "\u{267B} Salvage Trash"
    };
    let st_w = 170.0 * layout.fit;
    let st_rect = Rect::from_xywh(bar.max.x - st_w, bar.y(), st_w, bar.height());
    let id = Id::root("inv").child(("salvage_trash", salvage_armed as u32));
    let r = if salvage_armed {
        Button::danger(armed_label)
    } else {
        Button::new(armed_label)
    }
    .size(ButtonSize::Small)
    .enabled(bulk_count > 0)
    .show_with_id(ui, id, st_rect);
    if r.clicked && bulk_count > 0 {
        out.salvage_trash_clicked = true;
    }
    out
}

fn render_currency_bar(ui: &mut Ui<'_>, rect: Rect, shards: u32, fit: f32) {
    if rect.width() <= 0.0 {
        return;
    }
    let theme = *ui.theme();
    // Compact "badge" look: pill-rounded, noticeably darker
    // than the bag section behind it, with a thin warm-gold
    // outline so it reads as a currency tag rather than a
    // generic toolbar row.
    let radius = (rect.height() * 0.5).min(14.0 * fit);
    ui.draw_rounded_rect(rect, radius, Color::rgba(0.05, 0.06, 0.08, 0.92));
    ui.draw_rounded_outline(rect, radius, 1.0, Color::rgba(0.78, 0.62, 0.30, 0.65));

    let pad = 10.0 * fit;
    let glyph = "\u{25C6}";
    let glyph_size = theme.fonts.size_md;
    let amount = format_amount(shards);
    let amount_size = theme.fonts.size_md;
    let glyph_color = Color::rgba(0.60, 0.85, 1.00, 1.0);

    ui.draw_text(
        Pos2::new(
            rect.x() + pad,
            rect.y() + (rect.height() - glyph_size) * 0.5,
        ),
        glyph,
        glyph_size,
        glyph_color,
    );
    let glyph_w = ui.measure_text(glyph, glyph_size);
    let amount_x = rect.x() + pad + glyph_w + 6.0 * fit;
    ui.draw_text(
        Pos2::new(amount_x, rect.y() + (rect.height() - amount_size) * 0.5),
        &amount,
        amount_size,
        theme.colors.text,
    );
    let label_size = theme.fonts.size_sm;
    let label_x = amount_x + ui.measure_text(&amount, amount_size) + 6.0 * fit;
    ui.draw_text(
        Pos2::new(label_x, rect.y() + (rect.height() - label_size) * 0.5),
        "Shards",
        label_size,
        theme.colors.text_dim,
    );
}

fn format_amount(n: u32) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum TooltipSource {
    Bag,
    Equip,
    Stash,
}

fn rect_to_array(r: Rect) -> [f32; 4] {
    [r.min.x, r.min.y, r.max.x - r.min.x, r.max.y - r.min.y]
}
