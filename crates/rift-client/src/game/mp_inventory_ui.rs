//! Multiplayer inventory UI.
//!
//! Built entirely on the engine's immediate-mode UI stack
//! ([`rift_engine::ui::im`]). All slot drawing, hover, drag /
//! drop, and tooltip logic comes from the shared widgets so
//! this file is just *layout + action routing*: where each
//! panel sits, and how a release-on-target maps to a server
//! request.
//!
//! Interactions:
//! * **Click** a bag slot → equip into its canonical slot
//!   (or deposit into the stash if a chest session is open).
//! * **Click** an equipment slot → unequip back into the bag.
//! * **Drag** a slot onto another to:
//!   - Bag → Bag: reorder
//!   - Bag → Equip: equip
//!   - Bag → Stash: deposit
//!   - Bag → World (released outside any panel): drop on ground
//!   - Equip → Bag(idx): unequip into a specific slot
//!   - Stash → Bag(idx): withdraw
//! * **Shift-click** a bag or stash item to render the
//!   currently-equipped item alongside the hovered item's
//!   tooltip for side-by-side stat comparison.

use rift_engine::ui::im::{
    Color, Frame, Id, ItemSlot, Pad, Pos2, Rect, SlotInteraction, Tooltip, TooltipLine, Ui,
};
use rift_game::loot::{Equipment, EquipSlot, Item};
use rift_game::stats::Stat;
use winit::keyboard::KeyCode;

use super::sub_state::{EquipRequest, StashRequest};
use super::PlayerState;

// ─── Layout constants ────────────────────────────────────────────────
//
// Sizing tuned for readability on 1080p+. Slot is the unit
// every other dimension is derived from; bumping it scales the
// whole panel proportionally.

const SLOT_SIZE: f32 = 72.0;
const SLOT_GAP: f32 = 8.0;
const COLS: usize = 5;
const ROWS: usize = 6;
const PANEL_PAD: f32 = 22.0;
const HEADER_H: f32 = 44.0;
const FOOTER_H: f32 = 30.0;
/// Gutter between bag and equipment column inside the bag panel.
const INNER_GAP: f32 = 22.0;
/// Stats panel sits to the right of the bag panel.
const STATS_W: f32 = 260.0;
const STATS_GAP: f32 = 14.0;
const EQUIP_COL_W: f32 = SLOT_SIZE;
const STASH_COLS: usize = 6;
const STASH_ROWS: usize = 6;
const STASH_COL_W: f32 =
    STASH_COLS as f32 * (SLOT_SIZE + SLOT_GAP) - SLOT_GAP + PANEL_PAD * 2.0;

// ─── Drag payload ───────────────────────────────────────────────────

/// Where the drag started. Travels through the IM stack as an
/// opaque payload; the drop targets downcast to this enum and
/// branch on the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragSource {
    Bag(usize),
    Equip(EquipSlot),
    Stash(usize),
}

// ─── State + behaviour ───────────────────────────────────────────────

#[derive(Default)]
pub struct MpInventoryUI {
    pub open: bool,
    stash_visible: bool,
}

impl MpInventoryUI {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run one frame of the inventory UI. Fuses input + draw
    /// through the engine's IM stack, so callers only need to
    /// hand it the data and the request queues.
    ///
    /// Returns `true` if the panel is open (i.e. it's currently
    /// claiming the screen for itself; gameplay input should be
    /// suppressed).
    pub fn frame(
        &mut self,
        ui: &mut Ui<'_>,
        items: &[Option<Item>],
        equipment: &Equipment,
        pending: &mut Vec<EquipRequest>,
        stash_open: bool,
        stash_items: &[Option<Item>],
        stash_pending: &mut Vec<StashRequest>,
        player_state: &PlayerState,
    ) -> bool {
        // Tab toggles only when no chest session owns the panel.
        if ui.input().key_just_pressed(KeyCode::Tab) && !stash_open {
            self.open = !self.open;
        }
        // Stash session forces the panel open while in range.
        self.stash_visible = stash_open;
        if stash_open {
            self.open = true;
        }
        if !self.open {
            // Cancel any drag whose source slot is no longer
            // visible — otherwise the drag ghost would persist
            // after a Tab close.
            ui.cancel_drag();
            return false;
        }

        let theme = *ui.theme();
        let screen = ui.screen_size();

        // ─── Bag + equipment panel ──────────────────────────────
        let (px, py, pw, ph) = panel_rect(screen.x, screen.y);
        let panel_rect = Rect::from_xywh(px, py, pw, ph);
        let mut hovered_item: Option<Item> = None;
        Frame::panel(&theme)
            .with_padding(Pad::all(PANEL_PAD))
            .show(ui, panel_rect, |ui, body| {
                // Title row.
                ui.draw_text(
                    Pos2::new(body.x(), body.y()),
                    "INVENTORY",
                    theme.fonts.size_lg,
                    theme.colors.text,
                );
                let counts = format!(
                    "{}/{}    Equipped {}/{}",
                    items.iter().filter(|s| s.is_some()).count(),
                    COLS * ROWS,
                    equipment.count(),
                    EquipSlot::COUNT,
                );
                let cw = ui.measure_text(&counts, theme.fonts.size_md);
                ui.draw_text(
                    Pos2::new(body.max.x - cw, body.y() + 4.0),
                    &counts,
                    theme.fonts.size_md,
                    theme.colors.text_dim,
                );

                // Header underline.
                ui.draw_rect(
                    Rect::from_xywh(body.x(), body.y() + theme.fonts.size_lg + 8.0, body.width(), 1.0),
                    theme.colors.border,
                );

                let grid_y = body.y() + HEADER_H;

                // Bag grid. One `ItemSlot::interact` call per
                // cell handles draw + drag-source + drop-zone +
                // click classification; nothing else is wired.
                for row in 0..ROWS {
                    for col in 0..COLS {
                        let idx = row * COLS + col;
                        let pos = Pos2::new(
                            body.x() + col as f32 * (SLOT_SIZE + SLOT_GAP),
                            grid_y + row as f32 * (SLOT_SIZE + SLOT_GAP),
                        );
                        let id = Id::root("inv").child(("bag", idx));
                        let rect = Rect::from_xywh(pos.x, pos.y, SLOT_SIZE, SLOT_SIZE);
                        let item = items.get(idx).and_then(|o| o.as_ref());
                        let payload = item.map(|_| DragSource::Bag(idx));
                        let r = build_item_slot(item).interact::<DragSource>(ui, rect, id, payload);
                        let hovered = r.response.hovered;
                        route_slot(
                            r,
                            DropTarget::Bag(idx),
                            stash_open,
                            pending,
                            stash_pending,
                        );
                        if let Some(it) = item {
                            if hovered {
                                hovered_item = Some(it.clone());
                            }
                        }
                    }
                }

                // Equipment column.
                let bag_w = COLS as f32 * (SLOT_SIZE + SLOT_GAP) - SLOT_GAP;
                let ex = body.x() + bag_w + INNER_GAP;
                // Vertical divider between bag and equipment.
                ui.draw_rect(
                    Rect::from_xywh(ex - INNER_GAP * 0.5, grid_y, 1.0,
                        EquipSlot::COUNT as f32 * (SLOT_SIZE + SLOT_GAP) - SLOT_GAP),
                    theme.colors.border,
                );
                // Column heading.
                ui.draw_text(
                    Pos2::new(ex, grid_y - theme.fonts.size_sm - 4.0),
                    "EQUIPPED",
                    theme.fonts.size_sm,
                    theme.colors.text_dim,
                );
                for (i, slot) in EquipSlot::ALL.iter().enumerate() {
                    let pos = Pos2::new(ex, grid_y + i as f32 * (SLOT_SIZE + SLOT_GAP));
                    let id = Id::root("inv").child(("equip", i));
                    let rect = Rect::from_xywh(pos.x, pos.y, SLOT_SIZE, SLOT_SIZE);
                    let item = equipment.get(*slot);
                    let payload = item.map(|_| DragSource::Equip(*slot));
                    let r = build_item_slot(item).interact::<DragSource>(ui, rect, id, payload);
                    let hovered = r.response.hovered;
                    route_slot(
                        r,
                        DropTarget::Equip(*slot),
                        stash_open,
                        pending,
                        stash_pending,
                    );
                    if let Some(it) = item {
                        if hovered {
                            hovered_item = Some(it.clone());
                        }
                    } else {
                        // Empty equip slot: overlay the slot
                        // label centred so the player knows
                        // what goes here without a tooltip.
                        let label = slot.label();
                        let lw = ui.measure_text(label, theme.fonts.size_sm);
                        ui.draw_text(
                            Pos2::new(
                                rect.x() + (SLOT_SIZE - lw) * 0.5,
                                rect.y() + (SLOT_SIZE - theme.fonts.size_sm) * 0.5,
                            ),
                            label,
                            theme.fonts.size_sm,
                            theme.colors.text_muted,
                        );
                    }
                }

                // Footer hint.
                let hint = if stash_open {
                    "F: close stash  \u{00B7}  drag bag\u{2194}stash"
                } else {
                    "TAB: close  \u{00B7}  drag to reorder/equip/drop  \u{00B7}  SHIFT: compare"
                };
                ui.draw_rect(
                    Rect::from_xywh(body.x(), body.max.y - FOOTER_H + 4.0, body.width(), 1.0),
                    theme.colors.border,
                );
                ui.draw_text_ellipsized(
                    Pos2::new(body.x(), body.max.y - theme.fonts.size_md),
                    hint,
                    theme.fonts.size_md,
                    body.width(),
                    theme.colors.text_dim,
                );
            });

        // ─── Stash panel ────────────────────────────────────────
        let mut stash_hovered: Option<Item> = None;
        if stash_open {
            let (spx, spy, spw, sph) = stash_panel_rect(screen.x, screen.y);
            let stash_rect = Rect::from_xywh(spx, spy, spw, sph);
            Frame::panel(&theme)
                .with_padding(Pad::all(PANEL_PAD))
                .show(ui, stash_rect, |ui, body| {
                    ui.draw_text(
                        Pos2::new(body.x(), body.y()),
                        "STASH",
                        theme.fonts.size_lg,
                        theme.colors.text,
                    );
                    let counts = format!(
                        "{}/{}",
                        stash_items.iter().filter(|s| s.is_some()).count(),
                        STASH_COLS * STASH_ROWS,
                    );
                    let cw = ui.measure_text(&counts, theme.fonts.size_md);
                    ui.draw_text(
                        Pos2::new(body.max.x - cw, body.y() + 4.0),
                        &counts,
                        theme.fonts.size_md,
                        theme.colors.text_dim,
                    );
                    ui.draw_rect(
                        Rect::from_xywh(body.x(), body.y() + theme.fonts.size_lg + 8.0, body.width(), 1.0),
                        theme.colors.border,
                    );
                    let grid_y = body.y() + HEADER_H;
                    for row in 0..STASH_ROWS {
                        for col in 0..STASH_COLS {
                            let idx = row * STASH_COLS + col;
                            let pos = Pos2::new(
                                body.x() + col as f32 * (SLOT_SIZE + SLOT_GAP),
                                grid_y + row as f32 * (SLOT_SIZE + SLOT_GAP),
                            );
                            let id = Id::root("inv").child(("stash", idx));
                            let rect = Rect::from_xywh(pos.x, pos.y, SLOT_SIZE, SLOT_SIZE);
                            let item = stash_items.get(idx).and_then(|o| o.as_ref());
                            let payload = item.map(|_| DragSource::Stash(idx));
                            let r = build_item_slot(item)
                                .interact::<DragSource>(ui, rect, id, payload);
                            let hovered = r.response.hovered;
                            route_slot(
                                r,
                                DropTarget::Stash(idx),
                                stash_open,
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
                });
        }

        // ─── Stats panel ───────────────────────────────────────
        // Always shown alongside the bag panel so the player can
        // see their resolved character sheet without leaving the
        // inventory screen.
        let (stx, sty, stw, sth) = stats_panel_rect(screen.x, screen.y);
        let stats_rect = Rect::from_xywh(stx, sty, stw, sth);
        render_stats_panel(ui, stats_rect, player_state);

        // ─── Tooltip(s) ────────────────────────────────────────
        let tip_target = hovered_item.as_ref().or(stash_hovered.as_ref());
        if let Some(item) = tip_target {
            let mp = ui.mouse_pos();
            let primary = render_item_tooltip(
                ui,
                item,
                "Hovered",
                Pos2::new(mp.x + 18.0, mp.y),
            );
            // Compare side-by-side. Always show the equipped
            // counterpart when one exists; SHIFT additionally
            // surfaces a per-stat delta column so the player
            // can see exactly what they'd gain or lose.
            if let Some(equipped) = compare_target(equipment, item) {
                let eq_rect = render_item_tooltip(
                    ui,
                    equipped,
                    "Equipped",
                    Pos2::new(primary.max.x + 8.0, primary.y()),
                );
                if ui.shift_held() {
                    render_compare_delta(
                        ui,
                        item,
                        equipped,
                        Pos2::new(eq_rect.max.x + 8.0, eq_rect.y()),
                    );
                }
            }
        }

        // ─── Drag ghost + outside-drop (drop to world) ─────────
        // The ghost is just an `ItemSlot` rendered on the
        // DragGhost layer using the same builder as the
        // in-place slot, so what the player picks up is what
        // they see floating under the cursor.
        if let Some(payload) = ui.drag_payload::<DragSource>().copied() {
            if let Some(item) = item_for_source(payload, items, equipment, stash_items) {
                build_item_slot(Some(item)).show_ghost(ui);
            }
        }
        // Released outside every slot? → drop-to-world (bag-source only).
        if let Some(drop) = ui.take_drop_outside::<DragSource>() {
            if let DragSource::Bag(idx) = drop.payload {
                pending.push(EquipRequest::DropToWorld { inventory_index: idx as u32 });
            }
        }

        true
    }

    pub fn consumes_mouse(&self, mx: f32, my: f32, screen_w: f32, screen_h: f32) -> bool {
        if !self.open {
            return false;
        }
        let (px, py, pw, ph) = panel_rect(screen_w, screen_h);
        if mx >= px && mx < px + pw && my >= py && my < py + ph {
            return true;
        }
        let (stx, sty, stw, sth) = stats_panel_rect(screen_w, screen_h);
        if mx >= stx && mx < stx + stw && my >= sty && my < sty + sth {
            return true;
        }
        if self.stash_visible {
            let (spx, spy, spw, sph) = stash_panel_rect(screen_w, screen_h);
            if mx >= spx && mx < spx + spw && my >= spy && my < spy + sph {
                return true;
            }
        }
        false
    }
}

// ─── Helpers ────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
enum DropTarget {
    Bag(usize),
    Equip(EquipSlot),
    Stash(usize),
}

/// Build a fully-configured `ItemSlot` for the given item (or
/// an empty placeholder). The same builder feeds in-place
/// slot drawing AND drag-ghost drawing so the two stay
/// pixel-identical.
fn build_item_slot<'a>(item: Option<&'a Item>) -> ItemSlot<'a> {
    let mut s = ItemSlot::new(SLOT_SIZE);
    if let Some(it) = item {
        let c = it.rarity.color();
        s = s.rarity_tint(Color::rgba(c[0], c[1], c[2], 1.0));
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
fn route_slot(
    r: SlotInteraction<DragSource>,
    target: DropTarget,
    stash_open: bool,
    pending: &mut Vec<EquipRequest>,
    stash_pending: &mut Vec<StashRequest>,
) {
    if let Some(drop) = r.dropped {
        handle_drop(drop.payload, target, stash_open, pending, stash_pending);
    }
    if r.clicked {
        // Source identity is implicit in the target rect (the
        // slot the user clicked on), so derive it from `target`.
        let src = match target {
            DropTarget::Bag(idx) => DragSource::Bag(idx),
            DropTarget::Equip(slot) => DragSource::Equip(slot),
            DropTarget::Stash(idx) => DragSource::Stash(idx),
        };
        handle_click(src, stash_open, pending, stash_pending);
    }
}

fn handle_click(
    src: DragSource,
    stash_open: bool,
    pending: &mut Vec<EquipRequest>,
    stash_pending: &mut Vec<StashRequest>,
) {
    match src {
        DragSource::Bag(idx) => {
            if stash_open {
                stash_pending.push(StashRequest::Deposit { inventory_index: idx as u32 });
            } else {
                pending.push(EquipRequest::Equip { inventory_index: idx as u32 });
            }
        }
        DragSource::Equip(slot) => {
            pending.push(EquipRequest::Unequip { slot: slot.to_u8() });
        }
        DragSource::Stash(idx) => {
            stash_pending.push(StashRequest::Withdraw { stash_index: idx as u32 });
        }
    }
}

fn handle_drop(
    src: DragSource,
    target: DropTarget,
    stash_open: bool,
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
        // Bag → Stash: deposit into the dropped-on slot if
        // possible, otherwise fall back to the index-less
        // "send to stash" op (DropTarget::Stash carries the
        // hovered slot index when the cursor was over a slot).
        (DragSource::Bag(a), DropTarget::Stash(b)) if stash_open => {
            stash_pending.push(StashRequest::DepositToSlot {
                inventory_index: a as u32,
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
                stash_index: a as u32,
                inventory_index: b as u32,
            });
        }
        // Stash → Stash: reorder
        (DragSource::Stash(a), DropTarget::Stash(b)) if stash_open && a != b => {
            stash_pending.push(StashRequest::Swap {
                a: a as u32,
                b: b as u32,
            });
        }
        _ => {}
    }
}

fn compare_target<'a>(equipment: &'a Equipment, hovered: &Item) -> Option<&'a Item> {
    let slot = equipment.default_slot(hovered);
    equipment.get(slot)
}

/// Render a single item tooltip with the upgraded sizing —
/// header in `size_md`, body lines in `size_md`. Returns the
/// drawn rect (after screen clamping) so callers can stack
/// adjacent tooltips horizontally.
fn render_item_tooltip(ui: &mut Ui<'_>, item: &Item, header: &str, anchor: Pos2) -> Rect {
    let theme = *ui.theme();
    let raw: Vec<String> = item.tooltip();
    let rarity = item.rarity.color();
    let rarity_col = Color::rgba(rarity[0], rarity[1], rarity[2], 1.0);
    let lines: Vec<TooltipLine<'_>> = raw
        .iter()
        .enumerate()
        .map(|(i, s)| TooltipLine {
            text: s.as_str(),
            // Name line gets `size_lg`; everything else
            // (`Item Level …`, implicits, affixes) sits at
            // `size_md` so the wall of stats is actually
            // legible at gameplay distance.
            size: if i == 0 { theme.fonts.size_lg } else { theme.fonts.size_md },
            color: if i == 0 {
                rarity_col
            } else if s.is_empty() {
                theme.colors.text
            } else if s.starts_with("Item Level") {
                theme.colors.text_dim
            } else {
                theme.colors.text
            },
        })
        .collect();
    Tooltip::new()
        .header(header)
        .min_width(240.0)
        .pad(10.0)
        .show(ui, anchor, &lines)
}

/// Per-stat delta panel: for every stat that appears on either
/// item, render `+N`/`-N` in green / red so the player can see
/// at a glance what an equip swap would gain or cost.
fn render_compare_delta(ui: &mut Ui<'_>, hovered: &Item, equipped: &Item, anchor: Pos2) -> Rect {
    let theme = *ui.theme();
    let h_stats = hovered.stats();
    let e_stats = equipped.stats();

    // Union of stats touched by either side, in `Stat`
    // declaration order (kept stable so the column doesn't
    // dance frame to frame as a hovered item changes).
    const ORDER: &[Stat] = &[
        Stat::Power,
        Stat::CritChance,
        Stat::CritDamage,
        Stat::AttackSpeed,
        Stat::Health,
        Stat::Armor,
        Stat::Evasion,
        Stat::CooldownReduction,
        Stat::ResourceRegen,
        Stat::MoveSpeed,
        Stat::FireDamage,
        Stat::IceDamage,
        Stat::LightningDamage,
    ];

    // Build the delta lines as owned strings; `TooltipLine`
    // borrows so we keep them in a local `Vec<String>`.
    let mut texts: Vec<(String, Color)> = Vec::new();
    for &stat in ORDER {
        let h = h_stats.get(stat);
        let e = e_stats.get(stat);
        let delta = h - e;
        if delta.abs() < 1e-4 {
            continue;
        }
        let text = if stat.is_percent() {
            format!("{:+.1}% {}", delta * 100.0, stat.name())
        } else {
            format!("{:+.0} {}", delta, stat.name())
        };
        let color = if delta > 0.0 {
            // Gain — soft green so it doesn't clash with
            // rarity highlights.
            Color::rgba(0.45, 0.92, 0.45, 1.0)
        } else {
            Color::rgba(0.96, 0.40, 0.40, 1.0)
        };
        texts.push((text, color));
    }

    if texts.is_empty() {
        // Both items roll the same stats with the same values
        // — surface that explicitly so the compare doesn't
        // look broken.
        texts.push((
            "No stat changes".to_string(),
            theme.colors.text_dim,
        ));
    }

    let lines: Vec<TooltipLine<'_>> = texts
        .iter()
        .map(|(t, c)| TooltipLine {
            text: t.as_str(),
            size: theme.fonts.size_md,
            color: *c,
        })
        .collect();

    Tooltip::new()
        .header("Change vs equipped")
        .header_color(Color::rgba(0.95, 0.85, 0.55, 1.0))
        .min_width(220.0)
        .pad(10.0)
        .show(ui, anchor, &lines)
}

fn item_for_source<'a>(
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

// ─── Layout helpers ──────────────────────────────────────────────────

fn panel_rect(screen_w: f32, screen_h: f32) -> (f32, f32, f32, f32) {
    let bag_w = COLS as f32 * (SLOT_SIZE + SLOT_GAP) - SLOT_GAP;
    let pw = bag_w + INNER_GAP + EQUIP_COL_W + PANEL_PAD * 2.0;
    let body_h =
        (EquipSlot::COUNT as f32).max(ROWS as f32) * (SLOT_SIZE + SLOT_GAP) - SLOT_GAP;
    let ph = body_h + PANEL_PAD * 2.0 + HEADER_H + FOOTER_H;
    // Center the *triptych* (bag panel + stats panel) on screen
    // so the whole composition stays balanced regardless of
    // whether the stash is visible.
    let total_w = pw + STATS_GAP + STATS_W;
    let px = (screen_w - total_w) * 0.5;
    let py = (screen_h - ph) * 0.5;
    (px, py, pw, ph)
}

fn stats_panel_rect(screen_w: f32, screen_h: f32) -> (f32, f32, f32, f32) {
    let (bx, by, bw, bh) = panel_rect(screen_w, screen_h);
    (bx + bw + STATS_GAP, by, STATS_W, bh)
}

fn stash_panel_rect(screen_w: f32, screen_h: f32) -> (f32, f32, f32, f32) {
    let (bx, by, _, _) = panel_rect(screen_w, screen_h);
    let stash_body_h = STASH_ROWS as f32 * (SLOT_SIZE + SLOT_GAP) - SLOT_GAP;
    let pw = STASH_COL_W;
    let ph = stash_body_h + PANEL_PAD * 2.0 + HEADER_H;
    let px = bx - pw - STATS_GAP;
    (px, by, pw, ph)
}

// `TooltipLine` is re-exported for symmetry with hud.rs's usage
// path; kept here so consumers don't have to learn the engine
// import path.
#[allow(dead_code)]
type _TooltipLine<'a> = TooltipLine<'a>;

// ─── Stats panel ────────────────────────────────────────────────────

/// Render the resolved character sheet (level, class, name +
/// every CharacterStats field) in a panel that mirrors the
/// inventory chrome. Read-only — no interaction.
fn render_stats_panel(ui: &mut Ui<'_>, rect: Rect, ps: &PlayerState) {
    let theme = *ui.theme();
    Frame::panel(&theme)
        .with_padding(Pad::all(PANEL_PAD))
        .show(ui, rect, |ui, body| {
            // Title.
            ui.draw_text_ellipsized(
                Pos2::new(body.x(), body.y()),
                "CHARACTER",
                theme.fonts.size_lg,
                body.width(),
                theme.colors.text,
            );
            // Level / class summary on the right.
            let class_name: &str = ps.config.name;
            let summary = format!("Lv.{}  {}", ps.experience.level, class_name);
            let sw = ui.measure_text(&summary, theme.fonts.size_md);
            let avail = body.width();
            // If the summary is wider than the available width
            // we let ellipsize handle it instead of clipping.
            ui.draw_text_ellipsized(
                Pos2::new(body.max.x - sw.min(avail), body.y() + 4.0),
                &summary,
                theme.fonts.size_md,
                avail,
                theme.colors.text_dim,
            );
            // Header underline.
            ui.draw_rect(
                Rect::from_xywh(
                    body.x(),
                    body.y() + theme.fonts.size_lg + 8.0,
                    body.width(),
                    1.0,
                ),
                theme.colors.border,
            );

            // Name (player-chosen or class fallback).
            let name = if ps.name.is_empty() {
                class_name
            } else {
                ps.name.as_str()
            };
            let name_y = body.y() + HEADER_H;
            ui.draw_text_ellipsized(
                Pos2::new(body.x(), name_y),
                name,
                theme.fonts.size_md,
                body.width(),
                theme.colors.text,
            );

            // Stats list. Two columns: label on the left,
            // value right-aligned. Section headers in dim
            // gold to break up the wall of numbers.
            let s = ps.stats();
            let mut y = name_y + theme.fonts.size_md + 12.0;
            let row_h = theme.fonts.size_md + 6.0;
            let header_col = Color::rgba(0.95, 0.85, 0.55, 1.0);

            let header = |ui: &mut Ui<'_>, y: &mut f32, label: &str| {
                ui.draw_text(
                    Pos2::new(body.x(), *y),
                    label,
                    theme.fonts.size_sm,
                    header_col,
                );
                *y += theme.fonts.size_sm + 6.0;
                ui.draw_rect(
                    Rect::from_xywh(body.x(), *y - 4.0, body.width(), 1.0),
                    theme.colors.border,
                );
            };

            let row = |ui: &mut Ui<'_>,
                           y: &mut f32,
                           label: &str,
                           value: String,
                           value_color: Color| {
                ui.draw_text_ellipsized(
                    Pos2::new(body.x(), *y),
                    label,
                    theme.fonts.size_md,
                    body.width() * 0.55,
                    theme.colors.text_dim,
                );
                let vw = ui.measure_text(&value, theme.fonts.size_md);
                ui.draw_text(
                    Pos2::new(body.max.x - vw, *y),
                    &value,
                    theme.fonts.size_md,
                    value_color,
                );
                *y += row_h;
            };

            let pct = |v: f32| format!("{:.1}%", v * 100.0);
            let int = |v: f32| format!("{:.0}", v);
            let txt = theme.colors.text;

            header(ui, &mut y, "OFFENSE");
            row(ui, &mut y, "Power", int(s.damage), txt);
            row(ui, &mut y, "Crit Chance", pct(s.crit_chance), txt);
            row(ui, &mut y, "Crit Damage", pct(s.crit_damage), txt);
            row(ui, &mut y, "Attack Speed", format!("{:.2}", s.attack_speed), txt);
            y += 6.0;

            header(ui, &mut y, "DEFENSE");
            row(ui, &mut y, "Health", int(s.max_hp), txt);
            row(ui, &mut y, "Armor", int(s.armor), txt);
            row(ui, &mut y, "Evasion", pct(s.evasion), txt);
            y += 6.0;

            header(ui, &mut y, "UTILITY");
            row(ui, &mut y, "Move Speed", format!("{:.1}", s.move_speed), txt);
            row(
                ui,
                &mut y,
                "Cooldown Reduction",
                pct(s.cooldown_reduction),
                txt,
            );
            row(ui, &mut y, "Resource Regen", format!("{:.2}x", s.resource_regen), txt);

            // Elemental section only when at least one bonus is
            // non-zero so the panel doesn't feel empty for
            // pure-physical builds.
            if s.fire_damage > 0.0 || s.ice_damage > 0.0 || s.lightning_damage > 0.0 {
                y += 6.0;
                header(ui, &mut y, "ELEMENTAL");
                if s.fire_damage > 0.0 {
                    row(
                        ui,
                        &mut y,
                        "Fire",
                        pct(s.fire_damage),
                        Color::rgba(0.96, 0.55, 0.30, 1.0),
                    );
                }
                if s.ice_damage > 0.0 {
                    row(
                        ui,
                        &mut y,
                        "Ice",
                        pct(s.ice_damage),
                        Color::rgba(0.55, 0.85, 0.96, 1.0),
                    );
                }
                if s.lightning_damage > 0.0 {
                    row(
                        ui,
                        &mut y,
                        "Lightning",
                        pct(s.lightning_damage),
                        Color::rgba(0.95, 0.85, 0.45, 1.0),
                    );
                }
            }
        });
}
