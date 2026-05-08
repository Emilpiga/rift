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
// Sizing tuned for readability on 1080p+. Everything below is
// the *unscaled* baseline; per-frame the [`Layout`] struct
// scales each value by `fit` (the smaller of the global theme
// scale and the screen-fit factor) so the whole composition
// shrinks to fit instead of overflowing on small screens.
//
// Layout shape:
//
//     ┌────────────────────────────────┐  ┌──────────────┐
//     │ INVENTORY            EQUIPPED  │  │ CHARACTER    │
//     │ [E][E][E][E][E][E][E][E][E]    │  │              │
//     │ ─────────────────────────────  │  │ Lv.X  Class  │
//     │ [B][B][B][B][B][B]             │  │              │
//     │ [B][B][B][B][B][B]             │  │ OFFENSE      │
//     │ [B][B][B][B][B][B]             │  │  ...         │
//     │ [B][B][B][B][B][B]             │  │              │
//     │ [B][B][B][B][B][B]             │  │ DEFENSE      │
//     │ TAB: close ...                 │  │  ...         │
//     └────────────────────────────────┘  └──────────────┘
//
// 6-col bag with the 9 equipment slots laid horizontally
// above it reads more like other ARPG inventories and gives
// the stats panel real estate to actually breathe.

const SLOT_SIZE: f32 = 64.0;
const SLOT_GAP: f32 = 8.0;
const COLS: usize = 6;
const ROWS: usize = 5;
/// Equipment slots laid out in a single row above the bag
/// grid. There are 9 [`EquipSlot`] variants; the panel is
/// sized to fit all of them on one line.
const EQUIP_COLS: usize = 9;
const PANEL_PAD: f32 = 22.0;
const HEADER_H: f32 = 44.0;
const FOOTER_H: f32 = 30.0;
/// Vertical gap between the equipment row and the bag grid.
const INNER_GAP: f32 = 18.0;
/// Stats panel sits to the right of the bag panel. Wider than
/// before because long row labels ("Cooldown Reduction",
/// "Lightning Damage") need real estate next to their values.
const STATS_W: f32 = 340.0;
const STATS_GAP: f32 = 14.0;
const STASH_COLS: usize = 6;
const STASH_ROWS: usize = 6;

/// Per-frame computed layout. All `f32` fields are already
/// multiplied by [`Self::fit`], so call sites read pixel
/// values directly without re-applying the scale. `fit` is
/// the smaller of the user's preferred theme scale and the
/// largest scale at which the full composition still fits on
/// screen — so a window resize squeezes the panel down rather
/// than letting it spill off the edges.
#[derive(Clone, Copy, Debug)]
struct Layout {
    fit: f32,
    slot: f32,
    gap: f32,
    pad: f32,
    header_h: f32,
    footer_h: f32,
    inner_gap: f32,
    /// Bag (6×5 grid) + horizontal equip row panel.
    bag_panel: Rect,
    /// Stats panel that sits to the right of the bag panel.
    stats_panel: Rect,
    /// Optional stash panel (only resolved when stash is open).
    stash_panel: Rect,
}

impl Layout {
    /// Compute a fit-scaled layout for the inventory triptych.
    /// Centres the (stash? + bag + stats) row on screen and
    /// shrinks every dimension uniformly so the panel never
    /// overflows. `stash_open` widens the composition to
    /// include the stash slab in the fit calculation.
    fn compute(ui: &Ui<'_>, stash_open: bool) -> Self {
        let theme = *ui.theme();
        let screen = ui.screen_size();

        // Unscaled total dimensions.
        // Bag width is the *content* width of the bag panel;
        // we size the panel to whichever of (bag, equip row)
        // is wider so neither clips. The 9-slot equip row
        // wins over the 6-slot bag, so the panel is `EQUIP_COLS`
        // slots wide and the bag grid is centred inside it.
        let bag_grid_w_u = COLS as f32 * (SLOT_SIZE + SLOT_GAP) - SLOT_GAP;
        let equip_row_w_u = EQUIP_COLS as f32 * (SLOT_SIZE + SLOT_GAP) - SLOT_GAP;
        let content_w_u = bag_grid_w_u.max(equip_row_w_u);
        let bag_panel_w_u = content_w_u + PANEL_PAD * 2.0;

        let bag_grid_h_u = ROWS as f32 * (SLOT_SIZE + SLOT_GAP) - SLOT_GAP;
        let body_h_u = SLOT_SIZE + INNER_GAP + bag_grid_h_u;
        let bag_panel_h_u = body_h_u + PANEL_PAD * 2.0 + HEADER_H + FOOTER_H;

        let stash_w_u = STASH_COLS as f32 * (SLOT_SIZE + SLOT_GAP) - SLOT_GAP + PANEL_PAD * 2.0;

        let total_w_u = if stash_open {
            stash_w_u + STATS_GAP + bag_panel_w_u + STATS_GAP + STATS_W
        } else {
            bag_panel_w_u + STATS_GAP + STATS_W
        };
        let total_h_u = bag_panel_h_u;

        // Leave a screen-edge margin so the panel never kisses
        // the bezel — a few px of dead space looks far better
        // than a full-width slab and gives tooltips room to
        // breathe.
        let margin = theme.spacing.panel_margin();
        let avail_w = (screen.x - margin * 2.0).max(64.0);
        let avail_h = (screen.y - margin * 2.0).max(64.0);

        let fit = theme
            .scale
            .min(avail_w / total_w_u)
            .min(avail_h / total_h_u)
            .max(0.4);

        // Scale every dimension by the chosen fit.
        let slot = SLOT_SIZE * fit;
        let gap = SLOT_GAP * fit;
        let pad = PANEL_PAD * fit;
        let header_h = HEADER_H * fit;
        let footer_h = FOOTER_H * fit;
        let inner_gap = INNER_GAP * fit;
        let stats_gap = STATS_GAP * fit;

        let bag_panel_w = bag_panel_w_u * fit;
        let bag_panel_h = bag_panel_h_u * fit;
        let stash_panel_w = stash_w_u * fit;
        let stats_panel_w = STATS_W * fit;

        let total_w = total_w_u * fit;
        let row_x = ((screen.x - total_w) * 0.5).max(margin);
        let row_y = ((screen.y - bag_panel_h) * 0.5).max(margin);

        let (stash_x, bag_x) = if stash_open {
            let sx = row_x;
            let bx = sx + stash_panel_w + stats_gap;
            (sx, bx)
        } else {
            // Stash placeholder rect lives off-screen left so
            // its (zero-area) hit-test never matches.
            (-1.0, row_x)
        };
        let stats_x = bag_x + bag_panel_w + stats_gap;

        let bag_panel = Rect::from_xywh(bag_x, row_y, bag_panel_w, bag_panel_h);
        let stats_panel = Rect::from_xywh(stats_x, row_y, stats_panel_w, bag_panel_h);
        // Stash uses only as much vertical room as it needs;
        // no footer.
        let stash_body_h = (STASH_ROWS as f32 * (SLOT_SIZE + SLOT_GAP) - SLOT_GAP) * fit;
        let stash_h = stash_body_h + pad * 2.0 + header_h;
        let stash_panel = if stash_open {
            Rect::from_xywh(stash_x, row_y, stash_panel_w, stash_h)
        } else {
            Rect::from_xywh(-1.0, -1.0, 0.0, 0.0)
        };

        Self {
            fit,
            slot,
            gap,
            pad,
            header_h,
            footer_h,
            inner_gap,
            bag_panel,
            stats_panel,
            stash_panel,
        }
    }
}

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
    /// Cached layout rects from the last `frame()` call so the
    /// input layer's [`Self::consumes_mouse`] check (which
    /// runs without a `Ui`) can hit-test the *actual* on-
    /// screen positions instead of recomputing them with a
    /// stale theme scale.
    cached_bag: Rect,
    cached_stats: Rect,
    cached_stash: Rect,
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
        let layout = Layout::compute(ui, stash_open);
        // Stash these for `consumes_mouse` (input layer has no
        // `Ui`, so it can't recompute the layout itself).
        self.cached_bag = layout.bag_panel;
        self.cached_stats = layout.stats_panel;
        self.cached_stash = layout.stash_panel;

        // ─── Bag + equipment panel ──────────────────────────────
        let panel_rect = layout.bag_panel;
        let mut hovered_item: Option<Item> = None;
        // True when `hovered_item` came from an equipment slot
        // (not a bag / stash slot). The compare-side panel
        // would otherwise compare the item to itself, which
        // both wastes a tooltip slab and reads as a duplicate
        // of the legendary line.
        let mut hovered_from_equip = false;
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

                // Footer hint.
                let hint = if stash_open {
                    "F: close stash  \u{00B7}  drag bag\u{2194}stash"
                } else {
                    "TAB: close  \u{00B7}  drag to reorder/equip/drop  \u{00B7}  SHIFT: compare"
                };
                ui.draw_rect(
                    Rect::from_xywh(body.x(), body.max.y - layout.footer_h + 4.0, body.width(), 1.0),
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
            let stash_rect = layout.stash_panel;
            Frame::panel(&theme)
                .with_padding(Pad::all(layout.pad))
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
                    let grid_y = body.y() + layout.header_h;
                    for row in 0..STASH_ROWS {
                        for col in 0..STASH_COLS {
                            let idx = row * STASH_COLS + col;
                            let pos = Pos2::new(
                                body.x() + col as f32 * (layout.slot + layout.gap),
                                grid_y + row as f32 * (layout.slot + layout.gap),
                            );
                            let id = Id::root("inv").child(("stash", idx));
                            let rect = Rect::from_xywh(pos.x, pos.y, layout.slot, layout.slot);
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
        let stats_rect = layout.stats_panel;
        render_stats_panel(ui, stats_rect, player_state, &layout);

        // ─── Tooltip(s) ────────────────────────────────────────
        let tip_target = hovered_item.as_ref().or(stash_hovered.as_ref());
        if let Some(item) = tip_target {
            let mp = ui.mouse_pos();
            let screen_w = ui.screen_size().x;
            // Default anchor: just to the right of the cursor.
            // If we're past the screen midpoint we *prefer* the
            // left side so the cursor stays free of the
            // tooltip slab and the compare/delta panes don't
            // immediately overflow the edge.
            let primary_anchor = if mp.x > screen_w * 0.5 {
                Pos2::new(mp.x - ui.s(18.0), mp.y)
            } else {
                Pos2::new(mp.x + ui.s(18.0), mp.y)
            };
            let primary = render_item_tooltip(
                ui,
                item,
                "Hovered",
                primary_anchor,
                Some(&player_state.loadout),
            );
            // Compare side-by-side. Pick the side with more
            // remaining room so two- and three-pane tooltips
            // don't push past the screen edge on right-half
            // hovers. SHIFT additionally surfaces a per-stat
            // delta column so the player can see exactly what
            // they'd gain or lose. Skip the compare when the
            // hover originated from an equipment slot —
            // comparing an equipped item to itself is noise.
            if !hovered_from_equip {
                if let Some(equipped) = compare_target(equipment, item) {
                let stack_right = primary.max.x + ui.s(8.0) + 200.0 < screen_w;
                let eq_anchor = if stack_right {
                    Pos2::new(primary.max.x + ui.s(8.0), primary.y())
                } else {
                    // Stack to the left of `primary`. The
                    // tooltip's own width-clamp will pull it
                    // back if it can't fit there either.
                    Pos2::new(primary.x() - ui.s(8.0) - 200.0, primary.y())
                };
                let eq_rect = render_item_tooltip(
                    ui,
                    equipped,
                    "Equipped",
                    eq_anchor,
                    Some(&player_state.loadout),
                );
                if ui.shift_held() {
                    let delta_anchor = if stack_right {
                        Pos2::new(eq_rect.max.x + ui.s(8.0), eq_rect.y())
                    } else {
                        Pos2::new(eq_rect.x() - ui.s(8.0) - 200.0, eq_rect.y())
                    };
                    render_compare_delta(ui, item, equipped, delta_anchor);
                }
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

    pub fn consumes_mouse(&self, mx: f32, my: f32, _screen_w: f32, _screen_h: f32) -> bool {
        if !self.open {
            return false;
        }
        let hit = |r: Rect| {
            r.width() > 0.0
                && r.height() > 0.0
                && mx >= r.min.x
                && mx < r.max.x
                && my >= r.min.y
                && my < r.max.y
        };
        if hit(self.cached_bag) || hit(self.cached_stats) {
            return true;
        }
        if self.stash_visible && hit(self.cached_stash) {
            return true;
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
/// adjacent tooltips horizontally. `loadout` enables the
/// synergy footer ("→ Boosts <ability>") — pass `None` from
/// previews / character-select where the player has no slotted
/// abilities yet.
fn render_item_tooltip(
    ui: &mut Ui<'_>,
    item: &Item,
    header: &str,
    anchor: Pos2,
    loadout: Option<&rift_game::loadout::Loadout>,
) -> Rect {
    let theme = *ui.theme();
    let raw: Vec<String> = item.tooltip(loadout);
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
            } else if s.starts_with('\u{2500}') {
                // Divider between signature and bonus blocks.
                theme.colors.text_dim
            } else if s.starts_with('★') {
                // Legendary effect — gold tint.
                Color::rgba(1.00, 0.70, 0.20, 1.0)
            } else if s.starts_with('⚓') {
                // Anchored trait — saturated gold so the
                // chase-line reads at a glance.
                Color::rgba(1.00, 0.82, 0.25, 1.0)
            } else if s.starts_with('→') {
                // Synergy footer — accent.
                theme.colors.accent
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
        Stat::CritChance,
        Stat::CritDamage,
        Stat::AttackSpeed,
        Stat::Health,
        Stat::Vitality,
        Stat::Armor,
        Stat::Evasion,
        Stat::CooldownReduction,
        Stat::ResourceRegen,
        Stat::MoveSpeed,
        Stat::WeaponDamage,
        Stat::SpellDamage,
        Stat::PhysicalDamage,
        Stat::FireDamage,
        Stat::IceDamage,
        Stat::LightningDamage,
        Stat::ProjectileDamage,
        Stat::BeamDamage,
        Stat::AoeDamage,
        Stat::MeleeDamage,
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
//
// Layout used to live in three free functions taking
// `(screen_w, screen_h)`; that didn't account for the global
// theme scale or the screen-fit clamp, so the panel could
// overflow on small windows. Everything is now driven through
// [`Layout::compute`] above. `consumes_mouse` reads the cached
// rects from the previous frame.

// ─── Stats panel ────────────────────────────────────────────────────

/// Render the resolved character sheet (level, class, name +
/// every CharacterStats field) in a panel that mirrors the
/// inventory chrome. Read-only — no interaction.
fn render_stats_panel(ui: &mut Ui<'_>, rect: Rect, ps: &PlayerState, layout: &Layout) {
    let theme = *ui.theme();
    Frame::panel(&theme)
        .with_padding(Pad::all(layout.pad))
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
            let name_y = body.y() + layout.header_h;
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
                // Measure the value first, then ellipsize the
                // label to whatever space is left after the
                // right-aligned value (with a small gap). The
                // old `body.width() * 0.55` cap was relative
                // to the panel and ignored the value width
                // entirely — so when the panel scaled down
                // for a small screen, long labels like
                // "Cooldown Reduction" punched right through
                // their own values. The two columns now
                // *cannot* overlap.
                let vw = ui.measure_text(&value, theme.fonts.size_md);
                let gap = 8.0_f32 * layout.fit;
                let label_max = (body.width() - vw - gap).max(0.0);
                ui.draw_text_ellipsized(
                    Pos2::new(body.x(), *y),
                    label,
                    theme.fonts.size_md,
                    label_max,
                    theme.colors.text_dim,
                );
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
            row(ui, &mut y, "Damage", int(s.damage), txt);
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
