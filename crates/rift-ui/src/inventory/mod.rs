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

use rift_ui_im::{
    widgets::{tooltip_at_mouse, TooltipLine, TooltipLineDecor},
    Button, ButtonSize, Color, Frame, Id, ImKey, Pad, PanelHeader, Pos2, Rect, Ui,
};
use rift_ui_types::inventory::{
    DragSource, EnchantSourceView, InTransitSource, InventoryAction, InventoryUiState,
    InventoryView, ItemView,
};

use self::bag_panel::{
    paint_bag_in_transit_cover, render_bag_grid, render_paperdoll, BagPanelIn, PaperdollOut,
};
use self::drag::build_item_slot;
use self::layout::{Layout, HEADER_H, PANEL_PAD_X, PANEL_PAD_Y};
use self::stash_panel::{render_stash_panel, FilterStateRef, RenameStateRef, StashPanelIn};
use self::stats_panel::render_stats_panel;
use self::tooltips::{
    render_compare_delta_side_of, render_item_tooltip, render_item_tooltip_anchored,
    render_item_tooltip_side_of,
};
use crate::icons::{draw_placeholder_icon, icon_rect_center, UiIcon};

/// Atlas path [`assets/icons/ui/shard.png`] → overlay key `ui/shard`.
pub(crate) const SHARD_ICON_ATLAS_KEY: &str = "ui/shard";

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
    let enchant_session = view.enchant.is_some();

    if state.salvage_confirm_window_s <= 0.0 {
        state.salvage_confirm_window_s = 3.0;
    }

    // Tab toggles when no chest session is forcing the panel.
    if ui.input().key_just_pressed(ImKey::Tab) && !stash_session && !enchant_session {
        state.open = !state.open;
        if !state.open {
            actions.push(InventoryAction::Close);
        }
    }
    if stash_session {
        state.open = true;
        state.show_stash = true;
    } else if enchant_session {
        state.open = true;
        state.show_stash = false;
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
        state.color_picker_tab = None;
        state.salvage_armed_at = None;
        state.salvage_armed_bag_idx = None;
        state.enchant_source = None;
        state.enchant_affix = None;
        return (false, actions);
    }

    let layout = Layout::compute(
        ui,
        view.bag_cols.max(1),
        view.bag_rows.max(1),
        state.show_stats || enchant_session,
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
        if let Some(idx) = state.color_picker_tab {
            if (idx as usize) >= stash.tabs.len() {
                state.color_picker_tab = None;
            }
        }
    } else {
        state.rename_target_tab = None;
        state.rename_buffer.clear();
        state.rename_has_focused = false;
        state.color_picker_tab = None;
    }

    // ── Drawer chrome ───────────────────────────────
    let theme = *ui.theme();
    Frame::stone(&theme)
        .with_padding(Pad::all(0.0))
        .show_only(ui, layout.drawer);

    // ── Header ──────────────────────────────────────
    let main_header = Rect::from_xywh(
        layout.drawer.x(),
        layout.drawer.y(),
        layout.drawer.width(),
        HEADER_H * layout.fit,
    );
    PanelHeader::new("INVENTORY").show(ui, main_header);

    let active_tab_u8 = state.active_stash_tab;
    let stash_active_items: &[Option<ItemView<'_>>] = view
        .stash
        .as_ref()
        .and_then(|s| s.tabs.get(active_tab_u8 as usize))
        .map(|t| t.items)
        .unwrap_or(&[]);

    // Expire stale optimistic hides if the server reply never lands.
    if state.in_transit_source.is_some() && time - state.in_transit_set_at > 0.40 {
        state.in_transit_source = None;
        state.in_transit_dest_rect = None;
    }

    let mut bag_in = BagPanelIn {
        items: view.items,
        equipment: view.equipment,
        stash_active: stash_active_items,
        bag_cols: view.bag_cols.max(1),
        bag_rows: view.bag_rows.max(1),
        stash_open: stash_session,
        active_tab_u8,
        in_transit: state.in_transit_source,
        forge_anchor: view.enchant.as_ref().and_then(|e| e.selected_source),
        void_forge_open: enchant_session,
    };

    // ── Bag grid (before paperdoll so equip-to-bag hides apply same frame) ─
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

    bag_in.in_transit = state.in_transit_source;

    // ── Paperdoll ───────────────────────────────────
    let mut paper_out = PaperdollOut::default();
    render_paperdoll(ui, &layout, &bag_in, &mut actions, &mut paper_out);
    if let Some(src) = paper_out.in_transit_from_drop {
        state.in_transit_source = Some(src);
        state.in_transit_set_at = time;
        if let Some(r) = paper_out.in_transit_dest_rect_from_drop {
            state.in_transit_dest_rect = Some(r);
        }
    }

    bag_in.in_transit = state.in_transit_source;
    if matches!(
        paper_out.in_transit_from_drop,
        Some(InTransitSource::Bag(_))
    ) {
        paint_bag_in_transit_cover(ui, &layout, &bag_in, view.items);
    }

    let hovered_equip = paper_out.hovered_equip;

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
    if let Some(enchant) = view.enchant.as_ref() {
        Frame::stone(&theme)
            .with_padding(Pad::all(0.0))
            .show_only(ui, layout.stats_drawer);
        let header_h = HEADER_H * layout.fit;
        let header = Rect::from_xywh(
            layout.stats_drawer.x(),
            layout.stats_drawer.y(),
            layout.stats_drawer.width(),
            header_h,
        );
        PanelHeader::new("VOID FORGE").show(ui, header);
        let padded = layout.stats_drawer.shrink2(Pad::symmetric(
            PANEL_PAD_X * layout.fit,
            PANEL_PAD_Y * layout.fit,
        ));
        // Extra air under the title bar — socket motif reads cleaner when not crowded.
        let below_header = 16.0 * layout.fit;
        let inner = Rect::from_xywh(
            padded.x(),
            header.max.y + below_header,
            padded.width(),
            (padded.max.y - header.max.y - below_header).max(0.0),
        );
        render_enchant_panel(ui, inner, enchant, layout.fit, &mut actions);
    } else if state.show_stats {
        Frame::stone(&theme)
            .with_padding(Pad::all(0.0))
            .show_only(ui, layout.stats_drawer);
        let header_h = HEADER_H * layout.fit;
        let stats_header = Rect::from_xywh(
            layout.stats_drawer.x(),
            layout.stats_drawer.y(),
            layout.stats_drawer.width(),
            header_h,
        );
        PanelHeader::new("STATS").show(ui, stats_header);
        let padded = layout.stats_drawer.shrink2(Pad::symmetric(
            PANEL_PAD_X * layout.fit,
            PANEL_PAD_Y * layout.fit,
        ));
        let inner = Rect::from_xywh(
            padded.x(),
            stats_header.max.y + 10.0 * layout.fit,
            padded.width(),
            (padded.max.y - (stats_header.max.y + 10.0 * layout.fit)).max(0.0),
        );
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
            let header_h = HEADER_H * layout.fit;
            let stash_header = Rect::from_xywh(
                layout.stash_drawer.x(),
                layout.stash_drawer.y(),
                layout.stash_drawer.width(),
                header_h,
            );
            PanelHeader::new("STASH").show(ui, stash_header);
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
                &mut state.color_picker_tab,
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
                    state.color_picker_tab = None;
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

    if enchant_session {
        let forge = state.enchant_source;
        actions.retain(|act| {
            !blocks_anvil_socket_flow(act) && !self::drag::forge_action_mutates_locked(forge, act)
        });
    }

    (true, actions)
}

fn blocks_anvil_socket_flow(action: &InventoryAction) -> bool {
    matches!(
        action,
        InventoryAction::Equip { .. }
            | InventoryAction::Unequip { .. }
            | InventoryAction::UnequipToSlot { .. }
            | InventoryAction::SwapEquip { .. }
            | InventoryAction::DropToWorld { .. }
            | InventoryAction::DropEquipToWorld { .. }
            | InventoryAction::UseConsumable { .. }
    )
}

/// Flat-top regular hexagon vertex `i` ∈ [0, 6), circumradius `r`.
fn enchant_hex_vertex(cx: f32, cy: f32, r: f32, i: u32) -> Pos2 {
    let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::FRAC_PI_3;
    Pos2::new(cx + r * a.cos(), cy + r * a.sin())
}

fn enchant_fill_hex(ui: &mut Ui<'_>, cx: f32, cy: f32, r: f32, color: Color) {
    let c = Pos2::new(cx, cy);
    for i in 0..6 {
        let p0 = enchant_hex_vertex(cx, cy, r, i);
        let p1 = enchant_hex_vertex(cx, cy, r, (i + 1) % 6);
        ui.draw_triangle(c, p0, p1, color);
    }
}

fn enchant_stroke_hex(ui: &mut Ui<'_>, cx: f32, cy: f32, r: f32, thickness: f32, color: Color) {
    for i in 0..6 {
        let p0 = enchant_hex_vertex(cx, cy, r, i);
        let p1 = enchant_hex_vertex(cx, cy, r, (i + 1) % 6);
        ui.draw_line(p0, p1, thickness, color);
    }
}

/// Hex-framed forge socket: recessed well + layered outlines behind the drop square.
fn draw_void_forge_socket_frame(
    ui: &mut Ui<'_>,
    cx: f32,
    cy: f32,
    hex_r: f32,
    pit: Rect,
    theme: &rift_ui_im::Theme,
    fit: f32,
) {
    let a = theme.colors.accent.0;

    // Plate under hex — subtle cool bloom so the forge reads as lit from below.
    ui.draw_glow_disc(
        Pos2::new(cx, cy),
        hex_r * 1.05,
        hex_r * 0.55,
        Color::rgba(a[0] * 0.35, a[1] * 0.28, a[2] * 0.55, 0.14),
    );

    enchant_fill_hex(ui, cx, cy, hex_r, Color::rgba(0.045, 0.035, 0.09, 0.96));
    enchant_stroke_hex(
        ui,
        cx,
        cy,
        hex_r * 0.985,
        2.0 * fit.max(1.0),
        Color::rgba(
            (a[0] * 1.05).min(1.0),
            (a[1] * 1.02).min(1.0),
            (a[2] * 1.06).min(1.0),
            0.78,
        ),
    );
    enchant_stroke_hex(
        ui,
        cx,
        cy,
        hex_r * 0.92,
        1.0 * fit.max(1.0),
        Color::rgba(0.22, 0.14, 0.42, 0.55),
    );

    // Inner tapered hex — sells depth between rim and pit.
    enchant_fill_hex(
        ui,
        cx,
        cy,
        hex_r * 0.76,
        Color::rgba(0.02, 0.015, 0.045, 0.94),
    );
    enchant_stroke_hex(
        ui,
        cx,
        cy,
        hex_r * 0.74,
        1.0 * fit.max(1.0),
        Color::rgba(0.55, 0.48, 0.92, 0.22),
    );

    // Square pit: radial darkness + inset shadow / lip highlight.
    let pit_r = (pit.height() * 0.12).clamp(4.0 * fit, 11.0 * fit);
    let edge_c = theme.colors.bg_slot.0;
    ui.draw_rounded_radial_rect(
        pit,
        pit_r,
        Color::rgba(edge_c[0] * 0.35, edge_c[1] * 0.28, edge_c[2] * 0.52, 0.98),
        Color::rgba(0.01, 0.008, 0.035, 0.97),
    );

    let ins = Rect::from_xywh(
        pit.x() + 3.0 * fit,
        pit.y() + 3.0 * fit,
        (pit.width() - 6.0 * fit).max(0.0),
        (pit.height() - 6.0 * fit).max(0.0),
    );
    if ins.width() > 2.0 && ins.height() > 2.0 {
        let w = ins.width();
        let h = ins.height();
        ui.draw_rect(
            Rect::from_xywh(ins.x(), ins.y(), w, (3.0 * fit).min(h * 0.22)),
            Color::rgba(0.0, 0.0, 0.0, 0.62),
        );
        ui.draw_rect(
            Rect::from_xywh(ins.x(), ins.y(), (3.0 * fit).min(w * 0.22), h),
            Color::rgba(0.0, 0.0, 0.0, 0.62),
        );
        ui.draw_rect(
            Rect::from_xywh(
                ins.x(),
                ins.max.y - (2.0 * fit).min(h * 0.12),
                w,
                (2.0 * fit).min(h * 0.12),
            ),
            Color::rgba(0.62, 0.56, 1.0, 0.07),
        );
        ui.draw_rect(
            Rect::from_xywh(
                ins.max.x - (2.0 * fit).min(w * 0.12),
                ins.y(),
                (2.0 * fit).min(w * 0.12),
                h,
            ),
            Color::rgba(0.62, 0.56, 1.0, 0.07),
        );
    }

    ui.draw_rounded_outline(pit, pit_r, 2.0 * fit.max(1.0), theme.colors.border_strong);
    ui.draw_rounded_outline(
        Rect::from_xywh(
            pit.x() + 2.0 * fit,
            pit.y() + 2.0 * fit,
            (pit.width() - 4.0 * fit).max(0.0),
            (pit.height() - 4.0 * fit).max(0.0),
        ),
        (pit_r - 2.0 * fit).max(1.0),
        1.0,
        Color::rgba(0.08, 0.05, 0.14, 0.85),
    );
}

fn render_enchant_panel(
    ui: &mut Ui<'_>,
    rect: Rect,
    view: &rift_ui_types::inventory::EnchantView<'_>,
    fit: f32,
    actions: &mut Vec<InventoryAction>,
) {
    let theme = *ui.theme();
    let text_sm = theme.fonts.size_sm;
    let gap_sm = 8.0 * fit;
    let pad_top = 20.0 * fit;
    // Space below the hex rim before item name / empty hint (motif extends past the square pit).
    let pad_below_socket_motif = 22.0 * fit;

    let socket_size = (88.0 * fit).clamp(72.0, 108.0);
    let cx = rect.x() + rect.width() * 0.5;
    let socket_top = rect.y() + pad_top;
    let socket = Rect::from_xywh(cx - socket_size * 0.5, socket_top, socket_size, socket_size);
    let sock_cy = socket.y() + socket.height() * 0.5;
    let hex_r = socket_size * 0.52 + 14.0 * fit;
    let motif_bottom = sock_cy + hex_r;

    draw_void_forge_socket_frame(ui, cx, sock_cy, hex_r, socket, &theme, fit);

    let sock_ia = build_item_slot(view.item)
        .transparent_bg(true)
        .interact::<DragSource>(
            ui,
            socket,
            Id::root("inv").child("enchant_item_socket"),
            None::<DragSource>,
        );
    if sock_ia.right_clicked && view.item.is_some() {
        actions.push(InventoryAction::ClearEnchantForge);
    }
    if let Some(payload) = sock_ia.dropped.map(|d| d.payload) {
        match payload {
            DragSource::Bag(idx) => actions.push(InventoryAction::SelectEnchantSource {
                source: EnchantSourceView::Bag(idx),
            }),
            DragSource::Equip(slot) => actions.push(InventoryAction::SelectEnchantSource {
                source: EnchantSourceView::Equip(slot),
            }),
            DragSource::Stash(_) => {}
        }
    }

    if view.item.is_none() {
        let label = "\u{25C7}";
        let sz = (socket_size * 0.38).clamp(theme.fonts.size_md, theme.fonts.size_lg + 4.0 * fit);
        let lw = ui.measure_text(label, sz);
        ui.draw_text(
            Pos2::new(
                socket.x() + (socket.width() - lw) * 0.5,
                socket.y() + (socket.height() - sz) * 0.5 - 2.0 * fit,
            ),
            label,
            sz,
            Color::rgba(0.42, 0.38, 0.58, 0.55),
        );
    }

    let mut y = motif_bottom + pad_below_socket_motif;

    if let Some(name) = view.item_name {
        let name_color = view
            .item
            .map(|it| {
                let [r, g, b, a] = it.rarity_color;
                Color::rgba(r, g, b, a.min(1.0))
            })
            .unwrap_or(theme.colors.text);
        let nw = ui.measure_text(name, theme.fonts.size_md);
        ui.draw_text(
            Pos2::new(rect.x() + (rect.width() - nw).max(0.0) * 0.5, y),
            name,
            theme.fonts.size_md,
            name_color,
        );
        y += theme.fonts.size_md + gap_sm;

        if let Some(lock) = view.locked_affix_index {
            let note = format!("Lane {} only · enchant-touched", lock + 1);
            let banner = Rect::from_xywh(rect.x(), y, rect.width(), 22.0 * fit);
            ui.draw_rounded_rect(banner, 4.0 * fit, Color::rgba(0.42, 0.18, 0.72, 0.12));
            ui.draw_rounded_outline(banner, 4.0 * fit, 1.0, Color::rgba(0.62, 0.48, 0.88, 0.35));
            let tw = ui.measure_text(&note, text_sm);
            ui.draw_text(
                Pos2::new(
                    banner.x() + (banner.width() - tw).max(0.0) * 0.5,
                    banner.y() + 5.0 * fit,
                ),
                &note,
                text_sm,
                theme.colors.text_dim,
            );
            y += 26.0 * fit;
        }

        let row_h = (32.0 * fit).max(text_sm + 12.0 * fit);
        for affix in view.affixes {
            if view
                .locked_affix_index
                .map(|lo| lo != affix.index)
                .unwrap_or(false)
            {
                continue;
            }
            let active = view.selected_affix == Some(affix.index);
            let label = if affix.locked {
                format!("{} · sealed", affix.text)
            } else {
                affix.text.to_string()
            };
            let r = Rect::from_xywh(rect.x(), y, rect.width(), row_h);
            let resp = if active {
                Button::active(&label)
            } else {
                Button::new(&label)
            }
            .size(ButtonSize::Small)
            .show_with_id(ui, Id::root("inv").child(("enchant_affix", affix.index)), r);
            if resp.clicked {
                actions.push(InventoryAction::SelectEnchantAffix {
                    affix_index: affix.index,
                });
            }
            if resp.hovered {
                enchant_options_tooltip(ui, affix.reroll_options, affix.reroll_excluded);
            }
            y += row_h + 4.0 * fit;
        }

        y += gap_sm * 0.5;
        let prerequisites_met = view.selected_source.is_some()
            && view.selected_affix.is_some()
            && view
                .locked_affix_index
                .map(|lock| Some(lock) == view.selected_affix)
                .unwrap_or(true);
        let shards_ok = view.player_shards >= view.cost;
        let can_reroll = prerequisites_met && shards_ok;

        let cost_label = format!("{}", view.cost);
        let r = Rect::from_xywh(rect.x(), y, rect.width(), row_h);
        let resp = Button::primary("")
            .compound_icon_row("Reroll", SHARD_ICON_ATLAS_KEY, cost_label.as_str())
            .size(ButtonSize::Small)
            .enabled(can_reroll)
            .show_with_id(ui, Id::root("inv").child("enchant_reroll"), r);
        if prerequisites_met && !shards_ok {
            let wcol = theme.colors.warning.0;
            ui.draw_outline(
                resp.rect,
                1.25,
                Color::rgba(wcol[0], wcol[1], wcol[2], 0.48),
            );
        }
        if prerequisites_met && !shards_ok && resp.hovered {
            let need = view.cost.saturating_sub(view.player_shards);
            let msg = format!(
                "Not enough shards to reroll. Need {} more (cost {}, you have {}).",
                need, view.cost, view.player_shards
            );
            let lines = [TooltipLine::new(
                msg.as_str(),
                theme.fonts.size_sm,
                theme.colors.text,
            )];
            tooltip_at_mouse(ui, None, &lines);
        }
        if resp.clicked {
            if let (Some(source), Some(affix_index)) = (view.selected_source, view.selected_affix) {
                actions.push(InventoryAction::RerollAffix {
                    source,
                    affix_index,
                });
            }
        }
    } else {
        let hint = "Drag or right-click gear";
        let hh = (36.0 * fit).max(30.0);
        let plate = Rect::from_xywh(rect.x(), y, rect.width(), hh);
        ui.draw_rounded_rect(plate, 5.0 * fit, Color::rgba(0.06, 0.045, 0.11, 0.72));
        ui.draw_rounded_outline(plate, 5.0 * fit, 1.0, Color::rgba(0.48, 0.38, 0.72, 0.28));
        let hw = ui.measure_text(hint, text_sm);
        ui.draw_text(
            Pos2::new(
                plate.x() + (plate.width() - hw).max(0.0) * 0.5,
                plate.y() + (hh - text_sm) * 0.5,
            ),
            hint,
            text_sm,
            theme.colors.text_muted,
        );
    }
}

/// Max reroll outcome lines before "+ more" in forge tooltip.
const VOID_FORGE_OUTCOME_PREVIEW_LINES: usize = 8;
/// Max excluded-pool lines before "+ more excluded".
const VOID_FORGE_EXCLUDED_PREVIEW_LINES: usize = 10;

fn enchant_options_tooltip(
    ui: &mut Ui<'_>,
    options: &[String],
    excluded: &[(String, &'static str)],
) {
    let theme = *ui.theme();

    let outcome_overflow = options.len() > VOID_FORGE_OUTCOME_PREVIEW_LINES;
    let more_excluded = excluded.len() > VOID_FORGE_EXCLUDED_PREVIEW_LINES;

    let mut owned_strings: Vec<String> = Vec::new();
    if outcome_overflow {
        owned_strings.push(format!(
            "… +{} more outcomes",
            options.len() - VOID_FORGE_OUTCOME_PREVIEW_LINES
        ));
    }
    for (preview, reason) in excluded.iter().take(VOID_FORGE_EXCLUDED_PREVIEW_LINES) {
        owned_strings.push(format!("{preview} · {reason}"));
    }
    if more_excluded {
        owned_strings.push(format!(
            "… +{} more excluded",
            excluded.len() - VOID_FORGE_EXCLUDED_PREVIEW_LINES
        ));
    }

    let mut lines: Vec<TooltipLine<'_>> = Vec::new();

    lines.push(TooltipLine::new(
        "Can roll",
        theme.fonts.size_sm,
        theme.colors.text_dim,
    ));
    if options.is_empty() {
        lines.push(TooltipLine::new(
            "— none —",
            theme.fonts.size_sm,
            theme.colors.warning,
        ));
    } else {
        for option in options.iter().take(VOID_FORGE_OUTCOME_PREVIEW_LINES) {
            lines.push(TooltipLine::new(
                option.as_str(),
                theme.fonts.size_sm,
                theme.colors.text,
            ));
        }
        if outcome_overflow {
            lines.push(TooltipLine::new(
                owned_strings[0].as_str(),
                theme.fonts.size_sm,
                theme.colors.text_dim,
            ));
        }
    }

    lines.push(
        TooltipLine::new("", theme.fonts.size_sm, theme.colors.text_dim)
            .decor(TooltipLineDecor::Divider),
    );

    lines.push(TooltipLine::new(
        "Excluded",
        theme.fonts.size_sm,
        theme.colors.text_dim,
    ));

    if excluded.is_empty() {
        lines.push(TooltipLine::new(
            "—",
            theme.fonts.size_sm,
            theme.colors.text_dim,
        ));
    } else {
        let body_lo = usize::from(outcome_overflow);
        let body_hi = owned_strings.len() - usize::from(more_excluded);
        for i in body_lo..body_hi {
            lines.push(TooltipLine::new(
                owned_strings[i].as_str(),
                theme.fonts.size_sm,
                theme.colors.text_muted,
            ));
        }
        if more_excluded {
            lines.push(TooltipLine::new(
                owned_strings.last().unwrap().as_str(),
                theme.fonts.size_sm,
                theme.colors.text_dim,
            ));
        }
    }

    tooltip_at_mouse(ui, Some("Reroll pool"), &lines);
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
    let theme = *ui.theme();
    if bar.width() <= 0.0 {
        return out;
    }
    let chip_w = bar.height().max(32.0 * layout.fit);
    let chip_gap = 6.0 * layout.fit;

    // Stats chip
    let stats_rect = Rect::from_xywh(bar.x(), bar.y(), chip_w, bar.height());
    let r = if state.show_stats {
        Button::active("")
    } else {
        Button::new("")
    }
    .size(ButtonSize::Small)
    .show_with_id(ui, Id::root("inv").child("toggle_stats"), stats_rect);
    if r.clicked {
        state.show_stats = !state.show_stats;
    }
    draw_placeholder_icon(
        ui,
        icon_rect_center(stats_rect, 18.0 * layout.fit),
        UiIcon::Stats,
        theme.colors.text,
    );
    if r.hovered {
        icon_tooltip(
            ui,
            if state.show_stats {
                "Hide character stats"
            } else {
                "Show character stats"
            },
        );
    }

    // Sort chip immediately to the right of Stats.
    let sort_w = chip_w;
    let sort_rect = Rect::from_xywh(bar.x() + chip_w + chip_gap, bar.y(), sort_w, bar.height());
    let sort_btn = Button::new("").size(ButtonSize::Small).show_with_id(
        ui,
        Id::root("inv").child("toggle_sort"),
        sort_rect,
    );
    if sort_btn.clicked {
        out.sort_bag_clicked = true;
    }
    draw_placeholder_icon(
        ui,
        icon_rect_center(sort_rect, 18.0 * layout.fit),
        UiIcon::Sort,
        theme.colors.text,
    );
    if sort_btn.hovered {
        icon_tooltip(ui, "Sort bag");
    }

    // Salvage Trash on the right (2-stage handled by caller).
    let salvage_armed = state.salvage_armed_at.is_some();
    let st_w = chip_w;
    let st_rect = Rect::from_xywh(bar.max.x - st_w, bar.y(), st_w, bar.height());
    let id = Id::root("inv").child(("salvage_trash", salvage_armed as u32));
    let r = if salvage_armed {
        Button::danger("")
    } else {
        Button::new("")
    }
    .size(ButtonSize::Small)
    .enabled(bulk_count > 0)
    .show_with_id(ui, id, st_rect);
    if r.clicked && bulk_count > 0 {
        out.salvage_trash_clicked = true;
    }
    draw_placeholder_icon(
        ui,
        icon_rect_center(st_rect, 18.0 * layout.fit),
        if salvage_armed {
            UiIcon::Check
        } else {
            UiIcon::Recycle
        },
        if salvage_armed {
            Color::rgba(1.0, 0.70, 0.48, 1.0)
        } else {
            theme.colors.text
        },
    );
    if r.hovered {
        icon_tooltip(
            ui,
            if salvage_armed {
                "Confirm salvage trash"
            } else if bulk_count > 0 {
                "Salvage trash"
            } else {
                "No trash to salvage"
            },
        );
    }
    out
}

fn icon_tooltip(ui: &mut Ui<'_>, text: &str) {
    let theme = *ui.theme();
    let lines = [TooltipLine::new(
        text,
        theme.fonts.size_sm,
        theme.colors.text,
    )];
    tooltip_at_mouse(ui, None, &lines);
}

fn render_currency_bar(ui: &mut Ui<'_>, rect: Rect, shards: u32, fit: f32) {
    if rect.width() <= 0.0 {
        return;
    }
    let theme = *ui.theme();
    // Compact badge: darker than the bag slab, violet outline.
    ui.draw_rect(rect, Color::rgba(0.08, 0.05, 0.14, 0.92));
    ui.draw_outline(rect, 1.0, Color::rgba(0.62, 0.48, 0.88, 0.72));

    let pad = 10.0 * fit;
    let icon_px = (rect.height() * 0.62).clamp(16.0, rect.height() * 0.78);
    let iy = rect.y() + (rect.height() - icon_px) * 0.5;
    let icon_rect = Rect::from_xywh(rect.x() + pad, iy, icon_px, icon_px);
    ui.draw_icon(
        icon_rect,
        SHARD_ICON_ATLAS_KEY,
        Color::rgba(0.94, 0.88, 1.0, 1.0),
    );
    let amount = format_amount(shards);
    let amount_size = theme.fonts.size_md;
    let amount_x = rect.x() + pad + icon_px + 6.0 * fit;
    ui.draw_text(
        Pos2::new(amount_x, rect.y() + (rect.height() - amount_size) * 0.5),
        &amount,
        amount_size,
        theme.colors.text,
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
