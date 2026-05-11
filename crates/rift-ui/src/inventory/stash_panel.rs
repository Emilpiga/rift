//! Collapsible stash subsection in the inventory drawer.
//!
//! Tab strip + "+ Buy" + slot grid + inline rename overlay.

use rift_ui_im::{
    widgets::text_field, Button, ButtonSize, Color, Id, ImKey, ItemSlot, Pos2, Rect, Tooltip,
    TooltipLine, Ui,
};
use rift_ui_types::inventory::{DragSource, InventoryAction, ItemView, StashView};

use super::drag::{build_item_slot, route_slot_capture, DropTarget};
use super::grid_drop::{snap_preview_and_resolve, GridSpec};
use super::layout::pack_bag;

pub const STASH_COLS: u8 = 10;
pub const STASH_ROWS: u8 = 8;

pub struct StashPanelIn<'a> {
    pub view: &'a StashView<'a>,
    /// Bag items (used so the snap-anchor preview can show
    /// the correct footprint when a bag-sourced drag is over
    /// the stash grid).
    pub bag_items: &'a [Option<ItemView<'a>>],
    /// Equipment items (so equip-sourced drags also preview).
    pub equipment: &'a [Option<ItemView<'a>>],
    pub active_idx: usize,
    pub fit: f32,
    /// Pixel side of one inventory bag cell. Stash grid is
    /// rendered at this exact cell size so drag ghosts and
    /// snap previews stay 1:1 when crossing between the two
    /// containers.
    pub bag_cell: f32,
    /// Optimistic source-hide. See [`BagPanelIn::in_transit`].
    pub in_transit: Option<rift_ui_types::inventory::InTransitSource>,
}

pub struct RenameStateRef<'a> {
    pub target_tab: &'a mut Option<u8>,
    pub buffer: &'a mut String,
    pub has_focused: &'a mut bool,
}

/// Mutable reference into the host's stash-filter state.
/// `rarity_mask` is a bitmask over rarity tiers (bit `n` =
/// tier `n` is allowed); `0` means "no rarity filter".
/// `stat_keys` is the active stat-chip set; an empty vec means
/// "no stat filter". Items are dimmed (not hidden) when they
/// fail the filter so the layout never reflows.
pub struct FilterStateRef<'a> {
    pub rarity_mask: &'a mut u8,
    pub stat_keys: &'a mut Vec<String>,
}

#[derive(Default)]
pub struct StashPanelOut {
    pub hovered_stash: Option<u32>,
    /// Screen rect of the hovered stash cell. See
    /// [`BagPanelOut::hovered_bag_rect`].
    pub hovered_stash_rect: Option<Rect>,
    /// See `BagPanelOut::in_transit_from_drop`.
    pub in_transit_from_drop: Option<rift_ui_types::inventory::InTransitSource>,
    /// See `BagPanelOut::in_transit_dest_rect_from_drop`.
    pub in_transit_dest_rect_from_drop: Option<[f32; 4]>,
}

pub fn render_stash_panel(
    ui: &mut Ui<'_>,
    panel_rect: Rect,
    stash_in: StashPanelIn<'_>,
    rename: RenameStateRef<'_>,
    filter: FilterStateRef<'_>,
    time: f32,
    out_actions: &mut Vec<InventoryAction>,
) -> StashPanelOut {
    let mut out = StashPanelOut::default();
    if panel_rect.width() <= 0.0 || panel_rect.height() <= 0.0 {
        return out;
    }
    let theme = *ui.theme();
    let StashPanelIn {
        view,
        bag_items,
        equipment,
        active_idx,
        fit,
        bag_cell,
        in_transit,
    } = stash_in;
    let active_tab_u8 = active_idx as u8;
    let active_items: &[Option<_>] = view.tabs.get(active_idx).map(|t| t.items).unwrap_or(&[]);
    let owned_tabs = view.tabs.len();
    let can_buy_tab = owned_tabs < view.max_tabs && view.player_shards >= view.next_tab_cost;
    let next_tab_cost = view.next_tab_cost;
    let player_shards = view.player_shards;

    // No inner frame — outer drawer paints the stone chrome.

    let body = panel_rect;

    let pressed_buy_tab = std::cell::Cell::new(false);
    let pressed_rename = std::cell::Cell::new(false);
    let recolor_request: std::cell::Cell<Option<u8>> = std::cell::Cell::new(None);
    let switch_to: std::cell::Cell<Option<usize>> = std::cell::Cell::new(None);
    let commit_rename: std::cell::Cell<Option<String>> = std::cell::Cell::new(None);
    let cancel_rename = std::cell::Cell::new(false);

    // ── Tab strip ────────────────────────────
    let tab_h = 32.0 * fit;
    let tab_gap = 4.0 * fit;
    let plus_w = if owned_tabs < view.max_tabs {
        tab_h + 4.0 * fit
    } else {
        0.0
    };
    let avail_w = body.width() - plus_w;
    let tab_w = ((avail_w - tab_gap * (owned_tabs as f32 - 1.0).max(0.0))
        / owned_tabs.max(1) as f32)
        .max(48.0 * fit);
    for (i, tab) in view.tabs.iter().enumerate() {
        let tx = body.x() + i as f32 * (tab_w + tab_gap);
        let trect = Rect::from_xywh(tx, body.y(), tab_w, tab_h);
        let id = Id::root("inv").child(("stash_tab", i));
        let active = i == active_idx;
        let btn = if active {
            Button::active(tab.name)
        } else {
            Button::new(tab.name)
        }
        .size(ButtonSize::Small);
        let resp = btn.show_with_id(ui, id, trect);
        // Color stripe along the bottom edge to keep the
        // per-tab color cue from the previous design.
        let r = ((tab.color >> 16) & 0xFF) as f32 / 255.0;
        let g = ((tab.color >> 8) & 0xFF) as f32 / 255.0;
        let b = (tab.color & 0xFF) as f32 / 255.0;
        ui.draw_rect(
            Rect::from_xywh(trect.x(), trect.max.y - 2.0 * fit, trect.width(), 2.0 * fit),
            Color::rgba(r, g, b, 1.0),
        );
        if resp.clicked {
            switch_to.set(Some(i));
        }
        // Right-click anywhere on the tab triggers the
        // recolor flow (kept from the original design).
        if ui.interact_hover(id, trect) && ui.input().right_clicked() {
            recolor_request.set(Some(i as u8));
        }
    }
    if owned_tabs < view.max_tabs {
        let bx = body.x() + owned_tabs as f32 * (tab_w + tab_gap);
        let brect = Rect::from_xywh(bx, body.y(), plus_w, tab_h);
        let buy_id = Id::root("inv").child(("stash_tab_buy", owned_tabs));
        let resp = Button::primary("+")
            .size(ButtonSize::Small)
            .enabled(can_buy_tab)
            .show_with_id(ui, buy_id, brect);
        if resp.clicked {
            pressed_buy_tab.set(true);
        }
        let hover_for_tip = ui.interact_hover(buy_id, brect);
        if hover_for_tip {
            let cost_str = format!("Cost: {next_tab_cost} shards");
            let have_str = format!("You have: {player_shards} shards");
            let mut lines: Vec<TooltipLine<'_>> = Vec::with_capacity(3);
            lines.push(TooltipLine::new(
                &cost_str,
                theme.fonts.size_sm,
                theme.colors.text,
            ));
            lines.push(TooltipLine::new(
                &have_str,
                theme.fonts.size_sm,
                if can_buy_tab {
                    theme.colors.text_dim
                } else {
                    Color::rgba(0.95, 0.40, 0.35, 1.0)
                },
            ));
            let short_str;
            if !can_buy_tab && owned_tabs < view.max_tabs {
                let short = next_tab_cost.saturating_sub(player_shards);
                short_str = format!("Need {short} more");
                lines.push(TooltipLine::new(
                    &short_str,
                    theme.fonts.size_sm,
                    Color::rgba(0.95, 0.40, 0.35, 1.0),
                ));
            }
            Tooltip::new()
                .header("Buy stash tab")
                .min_width(160.0)
                .anchor_to(brect)
                .show(ui, Pos2::new(brect.max.x, brect.y()), &lines);
        }
    }

    // ── Header row ───────────────────────────
    let header_y = body.y() + tab_h + 8.0 * fit;
    let active_name = view.tabs.get(active_idx).map(|t| t.name).unwrap_or("STASH");
    ui.draw_text(
        Pos2::new(body.x(), header_y),
        active_name,
        theme.fonts.size_lg,
        theme.colors.text,
    );
    let counts = format!(
        "{}/{}",
        active_items.iter().filter(|s| s.is_some()).count(),
        view.slots_per_tab,
    );
    let cw = ui.measure_text(&counts, theme.fonts.size_md);
    let rename_lbl = "Rename";
    let rename_w = ui.measure_text(rename_lbl, theme.fonts.size_sm) + 20.0 * fit;
    let rename_h = 28.0 * fit;
    let sort_lbl = "Sort";
    let sort_w = ui.measure_text(sort_lbl, theme.fonts.size_sm) + 20.0 * fit;
    let sort_rect = Rect::from_xywh(
        body.max.x - cw - 14.0 * fit - rename_w - 6.0 * fit - sort_w,
        header_y,
        sort_w,
        rename_h,
    );
    let sort_btn = Button::new(sort_lbl).size(ButtonSize::Small).show_with_id(
        ui,
        Id::root("inv").child(("stash_sort", active_idx)),
        sort_rect,
    );
    if sort_btn.clicked {
        out_actions.push(InventoryAction::SortStashTab {
            tab_index: active_tab_u8,
        });
    }
    let rename_rect = Rect::from_xywh(
        body.max.x - cw - 14.0 * fit - rename_w,
        header_y,
        rename_w,
        rename_h,
    );
    let rename_btn = Button::new(rename_lbl)
        .size(ButtonSize::Small)
        .show_with_id(
            ui,
            Id::root("inv").child(("stash_rename", active_idx)),
            rename_rect,
        );
    if rename_btn.clicked {
        pressed_rename.set(true);
    }
    ui.draw_text(
        Pos2::new(body.max.x - cw, header_y + 2.0),
        &counts,
        theme.fonts.size_md,
        theme.colors.text_dim,
    );
    let div_y = header_y + theme.fonts.size_lg + 8.0 * fit;
    ui.draw_rect(
        Rect::from_xywh(body.x(), div_y, body.width(), 1.0),
        theme.colors.border,
    );

    // ── Filter row ───────────────────────────
    // Two sub-rows: top = rarity chips + Clear; bottom =
    // dynamic stat-key chips wrapped to fit. The stat set is
    // built from the union of `stat_keys` across every item
    // in the active tab so adding a new `Stat` variant in
    // `rift-game` shows up here automatically.
    let chip_h = 22.0 * fit;
    let chip_pad_x = 10.0 * fit;
    let chip_gap = 4.0 * fit;
    let row_gap = 4.0 * fit;
    let filter_top = div_y + 6.0 * fit;

    // Build the dynamic stat-key set + sort it for stable
    // chip order across frames.
    let mut stat_chip_set: Vec<&str> = Vec::new();
    for slot in active_items.iter() {
        let Some(it) = slot.as_ref() else { continue };
        for k in it.stat_keys {
            if !stat_chip_set.contains(k) {
                stat_chip_set.push(*k);
            }
        }
    }
    stat_chip_set.sort_unstable();

    // ── Rarity sub-row ───────────────────────
    let rarity_labels: [(u8, &str, [f32; 4]); 4] = [
        (0, "Common", [0.85, 0.85, 0.85, 1.0]),
        (1, "Magic", [0.45, 0.65, 1.00, 1.0]),
        (2, "Rare", [1.00, 0.85, 0.30, 1.0]),
        (3, "Legend", [1.00, 0.50, 0.20, 1.0]),
    ];
    let mut x = body.x();
    for (tier, label, col) in rarity_labels {
        let w = ui.measure_text(label, theme.fonts.size_sm) + chip_pad_x * 2.0;
        let rect = Rect::from_xywh(x, filter_top, w, chip_h);
        let active = (*filter.rarity_mask & (1 << tier)) != 0;
        let id = Id::root("inv").child(("stash_filter_rar", tier));
        let btn = if active {
            Button::active(label)
        } else {
            Button::new(label)
        }
        .size(ButtonSize::Small)
        .show_with_id(ui, id, rect);
        // Color stripe along the bottom for the rarity hint.
        ui.draw_rect(
            Rect::from_xywh(rect.x(), rect.max.y - 2.0 * fit, rect.width(), 2.0 * fit),
            Color::rgba(col[0], col[1], col[2], 1.0),
        );
        if btn.clicked {
            *filter.rarity_mask ^= 1 << tier;
        }
        x += w + chip_gap;
    }

    // "Clear" sits on the far right of the rarity row when
    // any filter is active.
    let any_active = *filter.rarity_mask != 0 || !filter.stat_keys.is_empty();
    if any_active {
        let clr_lbl = "Clear";
        let clr_w = ui.measure_text(clr_lbl, theme.fonts.size_sm) + chip_pad_x * 2.0;
        let clr_rect = Rect::from_xywh(body.max.x - clr_w, filter_top, clr_w, chip_h);
        let clr_id = Id::root("inv").child(("stash_filter_clear", 0u32));
        let resp = Button::new(clr_lbl)
            .size(ButtonSize::Small)
            .show_with_id(ui, clr_id, clr_rect);
        if resp.clicked {
            *filter.rarity_mask = 0;
            filter.stat_keys.clear();
        }
    }

    // ── Stat sub-row(s) ──────────────────────
    let stats_top = filter_top + chip_h + row_gap;
    let mut sx = body.x();
    let mut sy = stats_top;
    let mut stat_rows_used: u32 = if stat_chip_set.is_empty() { 0 } else { 1 };
    for (i, key) in stat_chip_set.iter().enumerate() {
        let w = ui.measure_text(key, theme.fonts.size_sm) + chip_pad_x * 2.0;
        if sx + w > body.max.x && sx > body.x() {
            sx = body.x();
            sy += chip_h + row_gap;
            stat_rows_used += 1;
        }
        let rect = Rect::from_xywh(sx, sy, w, chip_h);
        let active = filter.stat_keys.iter().any(|s| s == *key);
        let id = Id::root("inv").child(("stash_filter_stat", i as u32));
        let btn = if active {
            Button::active(key)
        } else {
            Button::new(key)
        }
        .size(ButtonSize::Small)
        .show_with_id(ui, id, rect);
        if btn.clicked {
            if let Some(pos) = filter.stat_keys.iter().position(|s| s == *key) {
                filter.stat_keys.remove(pos);
            } else {
                filter.stat_keys.push((*key).to_string());
            }
        }
        sx += w + chip_gap;
    }
    let filter_h = chip_h + row_gap + stat_rows_used as f32 * (chip_h + row_gap);

    // ── Slot grid (flush, outlined, no rounding) ─────
    let grid_y = filter_top + filter_h + 4.0 * fit;
    let grid_avail_w = body.width();
    let grid_avail_h = (body.max.y - grid_y).max(0.0);
    // Lock to the inventory bag's cell size so dragging an
    // item between bag and stash never visually resizes it;
    // fall back to auto-fit only if the drawer is too small
    // to fit the requested 6×6 at that pixel size.
    let auto_cell = (grid_avail_w / STASH_COLS as f32)
        .min(grid_avail_h / STASH_ROWS as f32)
        .max(8.0);
    let cell = if bag_cell > 0.0 {
        bag_cell.min(auto_cell)
    } else {
        auto_cell
    };
    let grid_x = body.x() + (body.width() - cell * STASH_COLS as f32) * 0.5;
    let grid_rect = Rect::from_xywh(
        grid_x,
        grid_y,
        cell * STASH_COLS as f32,
        cell * STASH_ROWS as f32,
    );
    let cols_us = STASH_COLS as usize;
    let rows_us = STASH_ROWS as usize;

    // Stone backing behind the slot grid, matching the bag.
    super::bag_panel::draw_section_chrome(ui, &theme, grid_rect, false);

    // Pack items index-as-anchor using their footprint.
    let placements = pack_bag(
        active_items,
        |_, it: &ItemView<'_>| (it.cell_w.max(1), it.cell_h.max(1)),
        STASH_COLS,
        STASH_ROWS,
    );
    let mut filled = vec![false; cols_us * rows_us];
    let mut cell_owner: Vec<Option<u32>> = vec![None; cols_us * rows_us];
    for (idx, slot) in active_items.iter().enumerate() {
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

    // Snap-anchor drag preview + central drop resolver,
    // shared with the bag.
    let drag_pl = ui.drag_payload::<DragSource>().copied();
    if let Some(src) = drag_pl {
        let (src_w, src_h) = stash_source_footprint(src, active_items, bag_items, equipment);
        let source_anchor_idx = match src {
            DragSource::Stash(i) => Some(i),
            _ => None,
        };
        let grid = GridSpec {
            rect: grid_rect,
            cell_px: cell,
            cols: STASH_COLS,
            rows: STASH_ROWS,
            cell_owner: &cell_owner,
        };
        snap_preview_and_resolve(
            ui,
            &grid,
            src_w,
            src_h,
            source_anchor_idx,
            true,
            active_tab_u8,
            DropTarget::Stash,
            out_actions,
            &mut out.in_transit_from_drop,
            &mut out.in_transit_dest_rect_from_drop,
        );
    }

    // Empty-cell pass.
    for row in 0..STASH_ROWS {
        for col in 0..STASH_COLS {
            if filled[row as usize * cols_us + col as usize] {
                continue;
            }
            let idx = (row as u32) * STASH_COLS as u32 + col as u32;
            let pos = Pos2::new(grid_x + col as f32 * cell, grid_y + row as f32 * cell);
            let rect = Rect::from_xywh(pos.x, pos.y, cell, cell);
            // Subtle gold outline + inset highlight per cell to
            // match the bag/equipment slot styling.
            ui.draw_outline(rect, 1.0, Color::rgba(0.78, 0.62, 0.30, 0.55));
            let id = Id::root("inv").child(("stash_empty", active_idx, idx));
            let r = ItemSlot::new(cell)
                .transparent_bg(true)
                .interact::<DragSource>(ui, rect, id, None::<DragSource>);
            route_slot_capture(
                r,
                DropTarget::Stash(idx),
                true,
                false,
                active_tab_u8,
                out_actions,
                &mut out.in_transit_from_drop,
            );
        }
    }

    // Filled item pass — render multi-cell items at their
    // packed footprint.
    for (idx, slot_opt) in active_items.iter().enumerate() {
        let Some(item) = slot_opt.as_ref() else {
            continue;
        };
        let Some((x, y, w, h)) = placements[idx] else {
            continue;
        };
        let rect = Rect::from_xywh(
            grid_x + x as f32 * cell,
            grid_y + y as f32 * cell,
            w as f32 * cell,
            h as f32 * cell,
        );
        let dragging_this = matches!(
            ui.drag_payload::<DragSource>().copied(),
            Some(DragSource::Stash(i)) if i == idx as u32
        );
        // Combine the freshly-set drop result with the
        // (frame-stale) in-transit value so the source slot
        // stays hidden on the *same* frame the drop fires —
        // otherwise the source briefly re-renders for one
        // frame before the hide takes effect on frame+1.
        let effective_in_transit = out.in_transit_from_drop.or(in_transit);
        let in_transit_this = matches!(
            effective_in_transit,
            Some(rift_ui_types::inventory::InTransitSource::Stash { tab, idx: si })
                if tab == active_tab_u8 && si == idx as u32
        );
        let being_dragged = dragging_this || in_transit_this;
        // Hidden source: draw per-cell empty chrome over the
        // footprint so the grid still reads as a slot grid
        // while the ghost is in flight. Suppress the bigger
        // multi-cell outline in that case.
        if being_dragged {
            for dy in 0..h {
                for dx in 0..w {
                    let cr = Rect::from_xywh(
                        grid_x + (x + dx) as f32 * cell,
                        grid_y + (y + dy) as f32 * cell,
                        cell,
                        cell,
                    );
                    ui.draw_outline(cr, 1.0, Color::rgba(0.78, 0.62, 0.30, 0.55));
                }
            }
        } else {
            ui.draw_outline(rect, 1.0, Color::rgba(0.78, 0.62, 0.30, 0.85));
        }
        let id = Id::root("inv").child(("stash", active_idx, idx as u32));
        let payload = Some(DragSource::Stash(idx as u32));
        // Dim items that fail the filter (rarity AND/OR stat).
        // `1.0` = fully opaque. Empty filter sets pass
        // everything.
        let rarity_pass =
            *filter.rarity_mask == 0 || (*filter.rarity_mask & (1 << item.rarity_tier)) != 0;
        let stat_pass = filter.stat_keys.is_empty()
            || filter
                .stat_keys
                .iter()
                .any(|k| item.stat_keys.iter().any(|sk| *sk == k.as_str()));
        let filter_dim = if rarity_pass && stat_pass { 1.0 } else { 0.30 };
        let r = if being_dragged {
            ItemSlot::new(rect.width().min(rect.height()))
                .transparent_bg(true)
                .interact::<DragSource>(ui, rect, id, payload)
        } else {
            build_item_slot(Some(item))
                .dim_alpha(filter_dim)
                .interact::<DragSource>(ui, rect, id, payload)
        };
        let hovered = r.response.hovered;
        route_slot_capture(
            r,
            DropTarget::Stash(idx as u32),
            true,
            false,
            active_tab_u8,
            out_actions,
            &mut out.in_transit_from_drop,
        );
        if hovered {
            out.hovered_stash = Some(idx as u32);
            out.hovered_stash_rect = Some(rect);
        }

        if !being_dragged {
            let [rr, gg, bb, _] = item.rarity_color;
            ui.draw_outline(rect, 1.5, Color::rgba(rr, gg, bb, 0.95));
        }
    }

    // ── Inline rename overlay ────────────────
    if *rename.target_tab == Some(active_tab_u8) {
        let field_h = theme.fonts.size_md + 8.0 * fit;
        let field_w = body.width().min(220.0 * fit);
        let field_rect = Rect::from_xywh(body.x(), header_y - 2.0 * fit, field_w, field_h);
        ui.draw_rect(field_rect, Color::rgba(0.10, 0.10, 0.13, 0.95));
        let resp = text_field(
            ui,
            Id::root("inv").child(("stash_rename_input", active_idx)),
            field_rect,
            rename.buffer,
            "Tab name",
            18,
            time,
        );
        let enter = ui.input().enter_just_pressed();
        let escape = ui.input().key_just_pressed_raw(ImKey::Escape);
        let focused = resp.focused;
        let click_away = *rename.has_focused && !focused;
        if escape {
            cancel_rename.set(true);
        } else if enter || click_away {
            commit_rename.set(Some(rename.buffer.clone()));
        }
        if focused {
            *rename.has_focused = true;
        }
    }

    // Apply state mutations & emit actions
    if pressed_rename.get() {
        *rename.target_tab = Some(active_tab_u8);
        *rename.buffer = view
            .tabs
            .get(active_idx)
            .map(|t| t.name.to_string())
            .unwrap_or_default();
        *rename.has_focused = false;
    }
    if cancel_rename.get() {
        *rename.target_tab = None;
        rename.buffer.clear();
        *rename.has_focused = false;
    }
    if let Some(name) = commit_rename.take() {
        out_actions.push(InventoryAction::RenameTab {
            tab_index: active_tab_u8,
            name,
        });
        *rename.target_tab = None;
        rename.buffer.clear();
        *rename.has_focused = false;
    }
    if let Some(t) = switch_to.get() {
        out_actions.push(InventoryAction::SwitchStashTab { tab_index: t as u8 });
    }
    if let Some(t) = recolor_request.get() {
        out_actions.push(InventoryAction::RecolorTab { tab_index: t });
    }
    if pressed_buy_tab.get() {
        out_actions.push(InventoryAction::BuyTab);
    }

    out
}

/// Look up the dragged item's cell footprint from whichever
/// source view holds it.
fn stash_source_footprint(
    src: DragSource,
    active_items: &[Option<ItemView<'_>>],
    bag_items: &[Option<ItemView<'_>>],
    equipment: &[Option<ItemView<'_>>],
) -> (u8, u8) {
    let lookup = |list: &[Option<ItemView<'_>>], idx: usize| -> (u8, u8) {
        list.get(idx)
            .and_then(|o| o.as_ref())
            .map(|it| (it.cell_w.max(1), it.cell_h.max(1)))
            .unwrap_or((1, 1))
    };
    match src {
        DragSource::Stash(idx) => lookup(active_items, idx as usize),
        DragSource::Bag(idx) => lookup(bag_items, idx as usize),
        DragSource::Equip(slot) => lookup(equipment, slot.0 as usize),
    }
}
