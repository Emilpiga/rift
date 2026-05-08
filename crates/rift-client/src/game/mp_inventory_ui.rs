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

use super::sub_state::{EquipRequest, StashRequest, StashTabClient};
use super::PlayerState;
use std::time::Instant;

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
/// grid. There are 10 [`EquipSlot`] variants; the panel is
/// sized to fit all of them on one line.
const EQUIP_COLS: usize = 10;
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
    /// Currently-selected stash tab index. Clamped against the
    /// authoritative `tabs` slice every frame so a server-side
    /// tab removal never leaves us pointing at thin air.
    /// `0` is the default starter tab.
    active_stash_tab: usize,
    /// Tab index currently being renamed via the inline text
    /// input, plus the in-progress edit buffer. `None` when
    /// no rename is active.
    rename_tab: Option<(usize, String)>,
    /// Set once the rename `text_field` has reported `focused`
    /// at least once. Used to detect a focus *transition* to
    /// unfocused so we can commit on click-away — without it,
    /// the very first frame (before the field has grabbed
    /// focus) would look like "focus lost" and instantly
    /// commit/cancel the rename before the player typed a
    /// single character.
    rename_seen_focus: bool,
    /// First-click timestamp of the "Salvage Trash" button. The
    /// button is a 2-stage commit: first click arms it (label
    /// flips to "Confirm? Click again"), second click within
    /// `SALVAGE_CONFIRM_WINDOW_S` actually fires the bulk
    /// salvage. Auto-disarms after the window expires so a
    /// stale arm can't surprise the player on a later open.
    salvage_confirm_at: Option<f64>,
    /// Bag slot whose press happened while Ctrl was held. The
    /// slot's `clicked` only resolves on release; if the player
    /// releases Ctrl before releasing the mouse, the naive
    /// "is Ctrl held *right now*" check at click time misses
    /// the intent and the click silently equips instead of
    /// salvaging. Latching the slot at press time and consuming
    /// it on the matching click closes that window so single
    /// salvages feel reliable.
    salvage_armed_bag_idx: Option<usize>,
}

/// Window (seconds) the "Salvage Trash" button stays armed
/// after the first click. A second click within this window
/// commits the bulk salvage; otherwise the button auto-disarms
/// and the player has to click twice again.
const SALVAGE_CONFIRM_WINDOW_S: f64 = 3.0;

/// Process-wide monotonic epoch for confirmation timestamps.
/// Lazily initialised on first call so we don't pay an
/// `Instant::now()` cost during static init.
fn ui_now() -> f64 {
    use std::sync::OnceLock;
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = EPOCH.get_or_init(Instant::now);
    Instant::now().duration_since(*epoch).as_secs_f64()
}

impl MpInventoryUI {
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` while a text-input widget owned by the inventory
    /// is active (currently: the inline stash-tab rename
    /// field). Drives `Input::set_text_capture` from
    /// `GameState::update` so typed letters like W / A / S / D
    /// or T can't leak into world bindings before the rename
    /// has even rendered for the frame.
    pub fn wants_text_input(&self) -> bool {
        self.rename_tab.is_some()
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
        stash_tabs: &[StashTabClient],
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
        // Active stash tab as a wire-ready u8. Bag and equip
        // slots emit per-tab StashRequest variants when items
        // shift-click into the stash, so we need this even when
        // the stash panel itself isn't rendered.
        let active_tab_u8 = self.active_stash_tab as u8;

        // ─── Bag + equipment panel ──────────────────────────────
        let panel_rect = layout.bag_panel;
        let mut hovered_item: Option<Item> = None;
        // True when `hovered_item` came from an equipment slot
        // (not a bag / stash slot). The compare-side panel
        // would otherwise compare the item to itself, which
        // both wastes a tooltip slab and reads as a duplicate
        // of the legendary line.
        let mut hovered_from_equip = false;
        // True when `hovered_item` came from a bag slot. Drives
        // the "Ctrl+click: Salvage for N shards" tooltip line —
        // salvage only applies to bag items, so it would mis-
        // lead on stash / equip hovers.
        let mut hovered_from_bag = false;
        // Pre-compute the bulk salvage preview so the button
        // label can show the player exactly what they'd gain
        // before they click. Mirrors the server's
        // `salvage_inventory_bulk` filter (Common+Magic only,
        // skip anchored).
        let (bulk_count, bulk_yield) = {
            let mut c: u32 = 0;
            let mut y: u32 = 0;
            for slot in items.iter() {
                if let Some(it) = slot {
                    if !it.anchored && (it.rarity as u8) <= rift_game::loot::Rarity::Magic as u8 {
                        c += 1;
                        y = y.saturating_add(rift_game::loot::salvage_yield(it.rarity, it.ilvl));
                    }
                }
            }
            (c, y)
        };
        // Auto-disarm a stale confirm so a second click days
        // later doesn't surprise the player. Cheap to do every
        // frame; the comparison is just two f64 ops.
        if let Some(t) = self.salvage_confirm_at {
            if ui_now() - t > SALVAGE_CONFIRM_WINDOW_S {
                self.salvage_confirm_at = None;
            }
        }
        let pressed_salvage_trash = std::cell::Cell::new(false);
        // Press-time ctrl latch shared between the bag closure
        // (which both reads it and updates it on press/click)
        // and the post-closure logic that copies the final
        // value back into `self`. Seed it with whatever is
        // currently latched so an in-flight arm survives
        // re-renders.
        let armed_cell: std::cell::Cell<Option<usize>> =
            std::cell::Cell::new(self.salvage_armed_bag_idx);
        let armed_idx_set = &armed_cell;
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

                // Footer: divider, Salvage Trash button on the
                // right, hint text on the left. Button has a
                // 2-stage commit — first click arms it, second
                // click within `SALVAGE_CONFIRM_WINDOW_S`
                // commits the bulk salvage. Hidden when the bag
                // has nothing to salvage so the inventory
                // doesn't grow chrome it can't use.
                let hint = if stash_open {
                    "F: close stash  \u{00B7}  drag bag\u{2194}stash"
                } else {
                    "TAB: close  \u{00B7}  drag/equip/drop  \u{00B7}  CTRL+click: salvage  \u{00B7}  SHIFT: compare"
                };
                ui.draw_rect(
                    Rect::from_xywh(body.x(), body.max.y - layout.footer_h + 4.0, body.width(), 1.0),
                    theme.colors.border,
                );
                let armed = self.salvage_confirm_at.is_some();
                let btn_lbl_text;
                let btn_lbl: &str = if bulk_count == 0 {
                    "No trash"
                } else if armed {
                    btn_lbl_text = format!("Confirm? {} items \u{2192} {} \u{25C6}", bulk_count, bulk_yield);
                    btn_lbl_text.as_str()
                } else {
                    btn_lbl_text = format!("Salvage Trash ({} \u{2192} {} \u{25C6})", bulk_count, bulk_yield);
                    btn_lbl_text.as_str()
                };
                let btn_size = theme.fonts.size_md;
                let btn_w = ui.measure_text(btn_lbl, btn_size) + 16.0 * layout.fit;
                let btn_h = layout.footer_h - 8.0 * layout.fit;
                let btn_rect = Rect::from_xywh(
                    body.max.x - btn_w,
                    body.max.y - btn_h - 2.0 * layout.fit,
                    btn_w,
                    btn_h,
                );
                let enabled = bulk_count > 0;
                let btn_id = Id::root("inv").child(("salvage_trash", armed as u32));
                let btn_hov = enabled && ui.interact_hover(btn_id, btn_rect);
                let btn_bg = if !enabled {
                    Color::rgba(0.16, 0.16, 0.18, 0.5)
                } else if armed {
                    if btn_hov {
                        Color::rgba(0.85, 0.32, 0.20, 0.95)
                    } else {
                        Color::rgba(0.65, 0.25, 0.15, 0.85)
                    }
                } else if btn_hov {
                    Color::rgba(0.30, 0.30, 0.36, 0.95)
                } else {
                    Color::rgba(0.20, 0.20, 0.25, 0.80)
                };
                ui.draw_rect(btn_rect, btn_bg);
                let lw = ui.measure_text(btn_lbl, btn_size);
                ui.draw_text(
                    Pos2::new(
                        btn_rect.x() + (btn_rect.width() - lw) * 0.5,
                        btn_rect.y() + (btn_rect.height() - btn_size) * 0.5,
                    ),
                    btn_lbl,
                    btn_size,
                    if enabled { theme.colors.text } else { theme.colors.text_dim },
                );
                if btn_hov && ui.input().left_clicked() {
                    pressed_salvage_trash.set(true);
                }
                ui.draw_text_ellipsized(
                    Pos2::new(body.x(), body.max.y - theme.fonts.size_md),
                    hint,
                    theme.fonts.size_md,
                    (btn_rect.x() - body.x() - 8.0 * layout.fit).max(0.0),
                    theme.colors.text_dim,
                );
            });

        // Persist the press-time ctrl latch back into `self`
        // for the next frame; the bag closure could only mutate
        // the local cell.
        self.salvage_armed_bag_idx = armed_cell.get();

        // Resolve the Salvage Trash 2-stage button outside the
        // panel closure (where we can mutate `self`). First
        // click arms; second click within the window commits.
        if pressed_salvage_trash.get() && bulk_count > 0 {
            let now = ui_now();
            match self.salvage_confirm_at {
                Some(t) if now - t <= SALVAGE_CONFIRM_WINDOW_S => {
                    pending.push(EquipRequest::SalvageBulk {
                        rarity_max: rift_game::loot::Rarity::Magic as u8,
                    });
                    self.salvage_confirm_at = None;
                }
                _ => {
                    self.salvage_confirm_at = Some(now);
                }
            }
        }

        // ─── Stash panel ────────────────────────────────────────
        let mut stash_hovered: Option<Item> = None;
        // Clamp the active tab against the authoritative tab
        // list every frame — handles the rare case where the
        // server-pushed tab list shrinks (e.g. character
        // reset). Using `min` keeps us pointing at the last
        // tab instead of crashing.
        if !stash_tabs.is_empty() {
            self.active_stash_tab = self.active_stash_tab.min(stash_tabs.len() - 1);
        } else {
            self.active_stash_tab = 0;
        }
        // Cancel any in-flight rename whose tab vanished.
        if let Some((idx, _)) = &self.rename_tab {
            if *idx >= stash_tabs.len() {
                self.rename_tab = None;
                self.rename_seen_focus = false;
            }
        }
        if stash_open {
            let stash_rect = layout.stash_panel;
            let active_idx = self.active_stash_tab;
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
                        let id = Id::root("inv").child(("stash_tab_buy", owned_tabs));
                        let resp = ui.interact_hover(id, brect);
                        let bg = if !can_buy_tab {
                            Color::rgba(0.18, 0.18, 0.20, 0.6)
                        } else if resp {
                            Color::rgba(0.30, 0.55, 0.85, 0.85)
                        } else {
                            Color::rgba(0.22, 0.40, 0.65, 0.8)
                        };
                        ui.draw_rect(brect, bg);
                        let lbl = "+";
                        let lbl_size = 14.0 * layout.fit;
                        let lw = ui.measure_text(lbl, lbl_size);
                        ui.draw_text(
                            Pos2::new(brect.x() + (brect.width() - lw) * 0.5, brect.y() + (brect.height() - lbl_size) * 0.5),
                            lbl,
                            lbl_size,
                            if can_buy_tab { theme.colors.text } else { theme.colors.text_dim },
                        );
                        if resp && ui.input().left_clicked() && can_buy_tab {
                            pressed_buy_tab.set(true);
                        }
                        // Hover tooltip — shows the cost,
                        // current shard balance, and a red
                        // "Not enough shards" line when the
                        // player can't afford the purchase.
                        // Without this the "+" button is opaque:
                        // greyed out for an unknown reason and
                        // with no price quoted up front.
                        if resp {
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
                    let rename_resp = ui.interact_hover(
                        Id::root("inv").child(("stash_rename", active_idx)),
                        rename_rect,
                    );
                    let rename_bg = if rename_resp {
                        Color::rgba(0.30, 0.30, 0.35, 0.85)
                    } else {
                        Color::rgba(0.20, 0.20, 0.25, 0.7)
                    };
                    ui.draw_rect(rename_rect, rename_bg);
                    let rl_w = ui.measure_text(rename_lbl, theme.fonts.size_sm);
                    ui.draw_text(
                        Pos2::new(
                            rename_rect.x() + (rename_rect.width() - rl_w) * 0.5,
                            rename_rect.y() + (rename_rect.height() - theme.fonts.size_sm) * 0.5,
                        ),
                        rename_lbl,
                        theme.fonts.size_sm,
                        theme.colors.text,
                    );
                    if rename_resp && ui.input().left_clicked() {
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
                                stash_open,
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
                    // Enter commits, Escape (or click-away)
                    // cancels.
                    if let Some((idx, buf)) = self.rename_tab.as_mut() {
                        if *idx == active_idx {
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
                            //
                            // Click-away also commits: once the
                            // field has been focused at least
                            // once, a subsequent unfocused
                            // frame means the player clicked
                            // outside, which should save the
                            // current buffer (matches the
                            // muscle-memory of every other
                            // inline-edit UI). Empty buffers
                            // cancel instead of committing so
                            // we don't blank the tab name by
                            // accident.
                            let enter = ui.input().enter_just_pressed();
                            let escape = ui.input().key_just_pressed_raw(KeyCode::Escape);
                            if resp.focused {
                                self.rename_seen_focus = true;
                            }
                            let blurred = self.rename_seen_focus && !resp.focused;
                            if enter || blurred {
                                let name = buf.trim().to_string();
                                if !name.is_empty() {
                                    stash_pending.push(StashRequest::RenameTab {
                                        tab_index: active_idx as u8,
                                        name,
                                    });
                                }
                                self.rename_tab = None;
                                self.rename_seen_focus = false;
                            } else if escape {
                                self.rename_tab = None;
                                self.rename_seen_focus = false;
                            }
                        }
                    }
                });
            // Side-effects from the immediate-mode body run
            // here so we don't need `&mut self` inside the
            // closure.
            if let Some(i) = switch_to.get() {
                self.active_stash_tab = i;
                self.rename_tab = None;
                self.rename_seen_focus = false;
            }
            if let Some(i) = recolor_request.get() {
                if let Some(tab) = stash_tabs.get(i as usize) {
                    let next = next_tab_color(tab.color);
                    stash_pending.push(StashRequest::RecolorTab {
                        tab_index: i,
                        color: next,
                    });
                }
            }
            if pressed_buy_tab.get() {
                stash_pending.push(StashRequest::BuyTab);
            }
            if pressed_rename.get() {
                let current = stash_tabs
                    .get(self.active_stash_tab)
                    .map(|t| t.name.clone())
                    .unwrap_or_default();
                self.rename_tab = Some((self.active_stash_tab, current));
                self.rename_seen_focus = false;
            }
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
            // Ctrl-hover salvage hint. Drawn as a small banner
            // beneath the primary tooltip so it doesn't push the
            // compare panel sideways. Suppressed when the hover
            // came from an equipment slot (you can't salvage
            // equipped gear) or a stash slot (deposit/withdraw
            // only — bulk salvage acts on bag items).
            if hovered_from_bag && ui.ctrl_held() {
                let hint = if item.anchored {
                    "Anchored \u{2014} cannot be salvaged".to_string()
                } else {
                    let yld = rift_game::loot::salvage_yield(item.rarity, item.ilvl);
                    format!("Ctrl+click \u{2192} Salvage for {} \u{25C6}", yld)
                };
                let hint_size = ui.theme().fonts.size_md;
                let hw = ui.measure_text(&hint, hint_size);
                let pad = ui.s(8.0);
                let hint_rect = Rect::from_xywh(
                    primary.x(),
                    primary.max.y + ui.s(4.0),
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
                    ui.theme().colors.text,
                );
            }
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
        let active_stash_items: &[Option<Item>] = stash_tabs
            .get(self.active_stash_tab)
            .map(|t| t.items.as_slice())
            .unwrap_or(&[]);
        if let Some(payload) = ui.drag_payload::<DragSource>().copied() {
            if let Some(item) = item_for_source(payload, items, equipment, active_stash_items) {
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
    // `true` when the player is holding Ctrl this frame. A
    // Ctrl+click on a Bag slot fires a Salvage request
    // instead of the default Equip / Deposit.
    ctrl: bool,
    // Active stash tab. Threaded into every produced
    // `StashRequest` variant so the server applies the action
    // to the right page.
    active_tab: u8,
    pending: &mut Vec<EquipRequest>,
    stash_pending: &mut Vec<StashRequest>,
) {
    if let Some(drop) = r.dropped {
        handle_drop(drop.payload, target, stash_open, active_tab, pending, stash_pending);
    }
    if r.clicked {
        // Source identity is implicit in the target rect (the
        // slot the user clicked on), so derive it from `target`.
        let src = match target {
            DropTarget::Bag(idx) => DragSource::Bag(idx),
            DropTarget::Equip(slot) => DragSource::Equip(slot),
            DropTarget::Stash(idx) => DragSource::Stash(idx),
        };
        handle_click(src, stash_open, ctrl, active_tab, pending, stash_pending);
    }
}

fn handle_click(
    src: DragSource,
    stash_open: bool,
    // Ctrl modifier; only meaningful for `Bag` clicks where it
    // flips the action from "equip / deposit" to "salvage".
    ctrl: bool,
    // Active stash tab — destination for bag deposits and
    // source for stash clicks while a stash session is open.
    active_tab: u8,
    pending: &mut Vec<EquipRequest>,
    stash_pending: &mut Vec<StashRequest>,
) {
    match src {
        DragSource::Bag(idx) => {
            if ctrl {
                pending.push(EquipRequest::Salvage { inventory_index: idx as u32 });
            } else if stash_open {
                stash_pending.push(StashRequest::Deposit {
                    inventory_index: idx as u32,
                    tab_index: active_tab,
                });
            } else {
                pending.push(EquipRequest::Equip { inventory_index: idx as u32 });
            }
        }
        DragSource::Equip(slot) => {
            pending.push(EquipRequest::Unequip { slot: slot.to_u8() });
        }
        DragSource::Stash(idx) => {
            stash_pending.push(StashRequest::Withdraw {
                tab_index: active_tab,
                stash_index: idx as u32,
            });
        }
    }
}

fn handle_drop(
    src: DragSource,
    target: DropTarget,
    stash_open: bool,
    active_tab: u8,
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
        // Bag → Stash: deposit into the dropped-on slot of the
        // currently-active stash tab.
        (DragSource::Bag(a), DropTarget::Stash(b)) if stash_open => {
            stash_pending.push(StashRequest::DepositToSlot {
                inventory_index: a as u32,
                tab_index: active_tab,
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
                tab_index: active_tab,
                stash_index: a as u32,
                inventory_index: b as u32,
            });
        }
        // Stash → Stash: reorder within the active tab.
        (DragSource::Stash(a), DropTarget::Stash(b)) if stash_open && a != b => {
            stash_pending.push(StashRequest::Swap {
                tab_index: active_tab,
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

/// Cycle through a small fixed palette so right-clicking a tab
/// rotates its color. The first entry matches the default
/// neutral grey returned by the server for fresh tabs; the
/// rest are gentle, distinct hues that read clearly even when
/// dimmed in the inactive state.
fn next_tab_color(current: u32) -> u32 {
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
