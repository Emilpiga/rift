//! Stash panel: tab strip + "+ Buy" button + grid + inline
//! tab-rename text field.
//!
//! The grid behaves identically to the bag — same drag/drop
//! routing, same slot visuals — so most of the body just
//! delegates to [`super::drag::route_slot`]. The tab strip is
//! the unique part: 8-pill horizontal list, recolour on RMB,
//! click-to-switch, hover-tooltipped Buy button, and an
//! inline rename text field that overlays the header when
//! active.

use rift_engine::ui::im::{
    Color, Frame, Id, InlineEditOutcome, InlineEditState, MiniButton, MiniButtonFills, Pad, Pos2,
    Rect, Tooltip, TooltipLine, Ui,
};
use rift_game::loot::Item;
use winit::keyboard::KeyCode;

use crate::game::sub_state::{EquipRequest, StashRequest, StashTabClient};
use crate::game::PlayerState;

use super::drag::{build_item_slot, route_slot, DragSource, DropTarget};
use super::layout::{Layout, STASH_COLS, STASH_ROWS};

/// Cycle through a small fixed palette so right-clicking a tab
/// rotates its color. The first entry matches the default
/// neutral grey returned by the server for fresh tabs; the
/// rest are gentle, distinct hues that read clearly even when
/// dimmed in the inactive state.
pub fn next_tab_color(current: u32) -> u32 {
    const PALETTE: &[u32] = &[
        0x6E6E78, // neutral grey (default)
        0xB95151, // muted red
        0xC68A3F, // amber
        0xC8B548, // yellow-gold
        0x6FAE5C, // green
        0x4DA0A8, // teal
        0x4E78C8, // blue
        0x9165B2, // violet
    ];
    let masked = current & 0x00FF_FFFF;
    let i = PALETTE.iter().position(|c| *c == masked).unwrap_or(0);
    PALETTE[(i + 1) % PALETTE.len()]
}

/// Rename state for the active stash tab. Wraps the engine's
/// generic [`InlineEditState`] with the tab index the rename
/// is targeting, so the panel can short-circuit when the
/// player clicks a different tab while editing.
#[derive(Default)]
pub struct RenameState {
    pub edit: InlineEditState,
    pub tab_idx: Option<usize>,
}

impl RenameState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_active(&self) -> bool {
        self.edit.is_active()
    }

    pub fn cancel(&mut self) {
        self.edit.cancel();
        self.tab_idx = None;
    }

    pub fn begin(&mut self, tab_idx: usize, current_name: String) {
        self.edit.begin(current_name);
        self.tab_idx = Some(tab_idx);
    }

    /// `true` while the rename targets `idx`.
    pub fn targets(&self, idx: usize) -> bool {
        self.tab_idx == Some(idx)
    }
}

/// Inputs into the stash panel. Mirrors the bag panel's
/// argument bag for symmetry.
pub struct StashPanelIn<'a> {
    pub stash_tabs: &'a [StashTabClient],
    pub player_state: &'a PlayerState,
    pub active_tab: usize,
}

#[derive(Default)]
pub struct StashPanelOut {
    /// Item under the cursor inside the stash grid (consumed
    /// by the tooltip layer).
    pub hovered_item: Option<Item>,
    /// Tab the player wants to switch to (LMB on a pill).
    pub switch_to: Option<usize>,
    /// Tab the player wants to recolor (RMB on a pill).
    pub recolor_request: Option<u8>,
    /// Player clicked the "+" Buy button.
    pub pressed_buy_tab: bool,
    /// Player clicked the "Rename" mini-button.
    pub pressed_rename: bool,
}

pub fn render_stash_panel(
    ui: &mut Ui<'_>,
    layout: &Layout,
    rename: &mut RenameState,
    stash_in: StashPanelIn<'_>,
    pending: &mut Vec<EquipRequest>,
    stash_pending: &mut Vec<StashRequest>,
) -> StashPanelOut {
    let theme = *ui.theme();
    let stash_rect = layout.stash_panel;
    let StashPanelIn {
        stash_tabs,
        player_state,
        active_tab: active_idx,
    } = stash_in;
    let active_tab_u8 = active_idx as u8;

    let active_items: &[Option<Item>] = stash_tabs
        .get(active_idx)
        .map(|t| t.items.as_slice())
        .unwrap_or(&[]);
    let owned_tabs = stash_tabs.len();
    let next_tab_cost: u32 = (owned_tabs as u32).saturating_mul(100);
    let can_buy_tab = owned_tabs < rift_net::messages::MAX_STASH_TABS
        && player_state.shards >= next_tab_cost;

    let pressed_buy_tab = std::cell::Cell::new(false);
    let pressed_rename = std::cell::Cell::new(false);
    let recolor_request: std::cell::Cell<Option<u8>> = std::cell::Cell::new(None);
    let switch_to: std::cell::Cell<Option<usize>> = std::cell::Cell::new(None);
    let mut stash_hovered: Option<Item> = None;

    Frame::panel(&theme)
        .with_padding(Pad::all(layout.pad))
        .show(ui, stash_rect, |ui, body| {
            // ── Tab strip (top row) ───────────────────
            // Pills sized to fit the panel width so 8
            // tabs + a "+" button never overflow. Each
            // pill shows the tab name tinted by its
            // color; the active tab gets a brighter
            // accent border.
            let tab_h = 22.0 * layout.fit;
            let tab_gap = 4.0 * layout.fit;
            let plus_w = if owned_tabs < rift_net::messages::MAX_STASH_TABS {
                tab_h + 4.0 * layout.fit
            } else {
                0.0
            };
            let avail_w = body.width() - plus_w;
            let tab_w = ((avail_w - tab_gap * (owned_tabs as f32 - 1.0).max(0.0))
                / owned_tabs.max(1) as f32)
                .max(28.0 * layout.fit);
            for (i, tab) in stash_tabs.iter().enumerate() {
                let tx = body.x() + i as f32 * (tab_w + tab_gap);
                let trect = Rect::from_xywh(tx, body.y(), tab_w, tab_h);
                let id = Id::root("inv").child(("stash_tab", i));
                let resp = ui.interact_hover(id, trect);
                let hov = resp;
                let active = i == active_idx;
                // Pill background tinted by tab color;
                // dim non-active tabs so the active one
                // pops without losing the color cue.
                let r = ((tab.color >> 16) & 0xFF) as f32 / 255.0;
                let g = ((tab.color >> 8) & 0xFF) as f32 / 255.0;
                let b = (tab.color & 0xFF) as f32 / 255.0;
                let alpha = if active { 0.95 } else if hov { 0.65 } else { 0.45 };
                ui.draw_rect(trect, Color::rgba(r * 0.55, g * 0.55, b * 0.55, alpha));
                // Color stripe along the bottom edge.
                ui.draw_rect(
                    Rect::from_xywh(trect.x(), trect.max.y - 2.0 * layout.fit, trect.width(), 2.0 * layout.fit),
                    Color::rgba(r, g, b, 1.0),
                );
                if active {
                    ui.draw_rect(
                        Rect::from_xywh(trect.x(), trect.y(), trect.width(), 1.0 * layout.fit),
                        Color::rgba(0.95, 0.95, 0.95, 0.8),
                    );
                }
                // Tab name centered.
                let lbl_size = 12.0 * layout.fit;
                let lw = ui.measure_text(&tab.name, lbl_size);
                let lx = trect.x() + (trect.width() - lw).max(0.0) * 0.5;
                let ly = trect.y() + (trect.height() - lbl_size) * 0.5;
                ui.draw_text_ellipsized(
                    Pos2::new(lx, ly),
                    &tab.name,
                    lbl_size,
                    trect.width() - 4.0 * layout.fit,
                    theme.colors.text,
                );
                // LMB → switch tabs. RMB → cycle color.
                if hov && ui.input().left_clicked() {
                    switch_to.set(Some(i));
                } else if hov && ui.input().right_clicked() {
                    recolor_request.set(Some(i as u8));
                }
            }
            // "+ Buy" button to the right of the last
            // pill. Disabled (greyed) when capped or
            // unaffordable; tooltip shows the cost.
            if owned_tabs < rift_net::messages::MAX_STASH_TABS {
                let bx = body.x() + owned_tabs as f32 * (tab_w + tab_gap);
                let brect = Rect::from_xywh(bx, body.y(), plus_w, tab_h);
                let buy_id = Id::root("inv").child(("stash_tab_buy", owned_tabs));
                let resp = MiniButton::new(
                    "+",
                    MiniButtonFills::explicit(
                        Color::rgba(0.22, 0.40, 0.65, 0.8),
                        Color::rgba(0.30, 0.55, 0.85, 0.85),
                        Color::rgba(0.18, 0.18, 0.20, 0.6),
                    ),
                )
                .text_size(14.0 * layout.fit)
                .enabled(can_buy_tab)
                .show(ui, buy_id, brect);
                if resp.clicked {
                    pressed_buy_tab.set(true);
                }
                // Hover tooltip — shows the cost,
                // current shard balance, and a red
                // "Not enough shards" line when the
                // player can't afford the purchase.
                // Without this the "+" button is opaque:
                // greyed out for an unknown reason and
                // with no price quoted up front. We
                // re-hit the rect with `interact_hover`
                // so the tooltip fires even while the
                // button is disabled (where MiniButton's
                // own hover branch returns false).
                let hover_for_tip = ui.interact_hover(buy_id, brect);
                if hover_for_tip {
                    let cost_str = format!("Cost: {next_tab_cost} shards");
                    let have_str = format!("You have: {} shards", player_state.shards);
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
                    if !can_buy_tab && owned_tabs < rift_net::messages::MAX_STASH_TABS {
                        let short = next_tab_cost.saturating_sub(player_state.shards);
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

            // ── Header row (tab name + slot counts) ───
            let header_y = body.y() + tab_h + 6.0 * layout.fit;
            let active_name = stash_tabs
                .get(active_idx)
                .map(|t| t.name.as_str())
                .unwrap_or("STASH");
            ui.draw_text(
                Pos2::new(body.x(), header_y),
                active_name,
                theme.fonts.size_lg,
                theme.colors.text,
            );
            let counts = format!(
                "{}/{}",
                active_items.iter().filter(|s| s.is_some()).count(),
                rift_net::messages::STASH_TAB_SLOTS,
            );
            let cw = ui.measure_text(&counts, theme.fonts.size_md);
            // "Rename" mini-button to the right of the
            // counts; opens an inline text field.
            let rename_lbl = "Rename";
            let rename_w = ui.measure_text(rename_lbl, theme.fonts.size_sm) + 12.0 * layout.fit;
            let rename_rect = Rect::from_xywh(
                body.max.x - cw - 14.0 * layout.fit - rename_w,
                header_y + 2.0 * layout.fit,
                rename_w,
                theme.fonts.size_md + 4.0 * layout.fit,
            );
            let rename_btn = MiniButton::new(
                rename_lbl,
                MiniButtonFills::explicit(
                    Color::rgba(0.20, 0.20, 0.25, 0.7),
                    Color::rgba(0.30, 0.30, 0.35, 0.85),
                    Color::rgba(0.14, 0.14, 0.16, 0.5),
                ),
            )
            .text_size(theme.fonts.size_sm)
            .show(
                ui,
                Id::root("inv").child(("stash_rename", active_idx)),
                rename_rect,
            );
            if rename_btn.clicked {
                pressed_rename.set(true);
            }
            ui.draw_text(
                Pos2::new(body.max.x - cw, header_y + 4.0),
                &counts,
                theme.fonts.size_md,
                theme.colors.text_dim,
            );
            let div_y = header_y + theme.fonts.size_lg + 8.0;
            ui.draw_rect(
                Rect::from_xywh(body.x(), div_y, body.width(), 1.0),
                theme.colors.border,
            );

            // ── Slot grid ─────────────────────────────
            let grid_y = div_y + 8.0 * layout.fit;
            for row in 0..STASH_ROWS {
                for col in 0..STASH_COLS {
                    let idx = row * STASH_COLS + col;
                    let pos = Pos2::new(
                        body.x() + col as f32 * (layout.slot + layout.gap),
                        grid_y + row as f32 * (layout.slot + layout.gap),
                    );
                    let id = Id::root("inv").child(("stash", active_idx, idx));
                    let rect = Rect::from_xywh(pos.x, pos.y, layout.slot, layout.slot);
                    let item = active_items.get(idx).and_then(|o| o.as_ref());
                    let payload = item.map(|_| DragSource::Stash(idx));
                    let r = build_item_slot(item)
                        .interact::<DragSource>(ui, rect, id, payload);
                    let hovered = r.response.hovered;
                    route_slot(
                        r,
                        DropTarget::Stash(idx),
                        true, // stash_open implied — this fn only runs when stash is open
                        false,
                        active_tab_u8,
                        pending,
                        stash_pending,
                    );
                    if let Some(it) = item {
                        if hovered {
                            stash_hovered = Some(it.clone());
                        }
                    }
                }
            }

            // Inline rename text field (overlaid on
            // header). Active only when the player has
            // pressed the Rename button on this tab;
            // Enter or click-away commits, Escape cancels.
            // The state machine lives in the engine's
            // `InlineEditState` so the click-away latch
            // (only commit AFTER the field has been
            // focused at least once) is shared with any
            // future inline-edit field.
            if rename.targets(active_idx) {
                if let Some(buf) = rename.edit.buffer_mut() {
                    let field_h = theme.fonts.size_md + 8.0 * layout.fit;
                    let field_w = body.width().min(220.0 * layout.fit);
                    let field_rect = Rect::from_xywh(
                        body.x(),
                        header_y - 2.0 * layout.fit,
                        field_w,
                        field_h,
                    );
                    // Tinted backdrop so the field
                    // visually replaces the tab name.
                    ui.draw_rect(field_rect, Color::rgba(0.10, 0.10, 0.13, 0.95));
                    let resp = rift_engine::ui::im::widgets::text_field(
                        ui,
                        Id::root("inv").child(("stash_rename_input", active_idx)),
                        field_rect,
                        buf,
                        "Tab name",
                        18,
                        player_state.experience.total_xp as f32 * 0.001,
                    );
                    // Drive commit/cancel. Both Enter
                    // and Escape have to use the
                    // text-input-aware accessors
                    // (`enter_just_pressed` /
                    // `key_just_pressed_raw`) because
                    // `text_capture` is on while the
                    // rename is active — the regular
                    // `key_just_pressed` is suppressed
                    // for typed-input safety and would
                    // never fire here, leaving the
                    // field with no way to commit.
                    let enter = ui.input().enter_just_pressed();
                    let escape = ui.input().key_just_pressed_raw(KeyCode::Escape);
                    match rename.edit.process(resp.focused, enter, escape) {
                        InlineEditOutcome::Editing => {}
                        InlineEditOutcome::Commit(name) => {
                            stash_pending.push(StashRequest::RenameTab {
                                tab_index: active_idx as u8,
                                name,
                            });
                            rename.tab_idx = None;
                        }
                        InlineEditOutcome::Cancel => {
                            rename.tab_idx = None;
                        }
                    }
                }
            }
        });

    StashPanelOut {
        hovered_item: stash_hovered,
        switch_to: switch_to.get(),
        recolor_request: recolor_request.get(),
        pressed_buy_tab: pressed_buy_tab.get(),
        pressed_rename: pressed_rename.get(),
    }
}
