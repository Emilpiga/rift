//! Right-side drawer layout for the inventory screen.
//!
//! The inventory occupies the right ~38 % of the screen, full
//! height. Vertically it splits into:
//!
//! ```text
//!   ┌──────────── header ────────────┐
//!   │                                │
//!   │         paperdoll              │
//!   │ (9-col × 6-row equipment grid) │
//!   │                                │
//!   ├────────── divider ─────────────┤
//!   │                                │
//!   │        bag grid                │
//!   │ (cols × rows of flush squares) │
//!   │                                │
//!   ├────── toggle chips ────────────┤
//!   │   (collapsible subsections)    │
//!   ├────────── currency bar ────────┤
//!   └────────────────────────────────┘
//! ```
//!
//! The paperdoll runs on a fixed 9-cell wide × 6-cell tall
//! grid. Each [`EquipSlotIdx`] is mapped to a `(col, row, w,
//! h)` rectangle (see [`PAPERDOLL_LAYOUT`]); empty grid cells
//! produce no slot.

use rift_ui_im::{Pos2, Rect, Ui};
use rift_ui_types::inventory::EquipSlotIdx;

/// Drawer width as a fraction of the screen width. Clamped
/// against [`DRAWER_MIN_W_PX`] / [`DRAWER_MAX_W_PX`].
pub const DRAWER_FRAC_W: f32 = 0.38;
pub const DRAWER_MIN_W_PX: f32 = 460.0;
pub const DRAWER_MAX_W_PX: f32 = 760.0;

/// Side-drawer (stats / stash) sizing. Each opens to the
/// left of the inventory drawer as its own carved-stone
/// slab. They share these clamps so the two panels line up
/// visually when both are open.
pub const SIDE_DRAWER_MIN_W_PX: f32 = 280.0;
pub const SIDE_DRAWER_MAX_W_PX: f32 = 380.0;
pub const SIDE_DRAWER_FRAC_W: f32 = 0.22;

pub const PANEL_PAD_X: f32 = 18.0;
pub const PANEL_PAD_Y: f32 = 16.0;
pub const HEADER_H: f32 = 38.0;
pub const SECTION_GAP: f32 = 14.0;
/// Compact shards "pill" anchored under the bag section.
/// Smaller than the old full-width currency bar so it reads
/// as a badge belonging to the bag rather than a row.
pub const CURRENCY_BAR_H: f32 = 26.0;
pub const TOGGLE_BAR_H: f32 = 30.0;

/// Pixel inset between the inside edge of a section's stone
/// chrome and the slot grid it contains. Stops the
/// paperdoll + bag slots from kissing the section border
/// and lets the inner-shadow bevel read as a real recessed
/// niche around the grid.
pub const SECTION_INNER_PAD: f32 = 10.0;

/// Pixel gap between adjacent paperdoll slots (each side gets
/// half this inset). Keeps the slots from looking like they
/// share borders.
pub const PAPERDOLL_SLOT_GAP: f32 = 6.0;

/// Paperdoll grid is locked to the same column count as the
/// bag so the equipment block is exactly the bag's width.
/// Cell size is identical to a bag cell, which means the
/// largest 2×H equipment slot reads as "two bag tiles wide".
pub const PAPERDOLL_COLS: u8 = 10;
pub const PAPERDOLL_ROWS: u8 = 7;

/// Static mapping from `EquipSlotIdx.0` to a `(col, row, w,
/// h)` rectangle on the [`PAPERDOLL_COLS`] × [`PAPERDOLL_ROWS`]
/// grid. Indexing follows `EquipSlot::ALL`:
/// `[Weapon, Helm, Chest, Legs, Hands, Boots, Ring1, Ring2,
///   Amulet, Shoulders]`.
///
/// Layout (cols 0..10 × rows 0..7):
/// ```text
///   col: 0 1 2 3 4 5 6 7 8 9
///   r 0: W W . . H H . . S S
///   r 1: W W . A H H . . S S
///   r 2: W W . . C C . . . .
///   r 3: . . . R C C R . . .
///   r 4: . . . . C C . . . .
///   r 5: G G . . L L . . B B
///   r 6: G G . . L L . . B B
/// ```
/// Every cell is unique — no two slots overlap. Sizes:
/// weapon and chest are 2×3; helm, hands, boots, legs, and
/// shoulders are 2×2; rings + amulet are 1×1.
pub const PAPERDOLL_LAYOUT: [(u8, u8, u8, u8); EquipSlotIdx::COUNT] = [
    // Weapon            (left column, 2×3 — same as chest)
    (0, 0, 2, 3),
    // Helm              (top center, 2×2)
    (4, 0, 2, 2),
    // Chest             (center, 2×3)
    (4, 2, 2, 3),
    // Legs              (below chest, 2×2)
    (4, 5, 2, 2),
    // Hands / gloves    (lower-left, 2×2)
    (0, 5, 2, 2),
    // Boots             (lower-right, 2×2)
    (8, 5, 2, 2),
    // Ring1             (left of chest middle)
    (3, 3, 1, 1),
    // Ring2             (right of chest middle)
    (6, 3, 1, 1),
    // Amulet            (next to helm)
    (3, 1, 1, 1),
    // Shoulders         (top-right, 2×2)
    (8, 0, 2, 2),
];

/// Per-frame computed layout. All rects are screen-pixel
/// space; cell sizes are square pixel sides.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub drawer: Rect,
    #[allow(dead_code)]
    pub content: Rect,
    #[allow(dead_code)]
    pub header: Rect,
    pub paperdoll: Rect,
    pub paperdoll_cell: f32,
    pub paperdoll_origin: Pos2,
    pub bag: Rect,
    pub bag_cell: f32,
    pub bag_origin: Pos2,
    pub toggle_bar: Rect,
    /// Side drawer for the Stats panel. Sits to the LEFT of
    /// the inventory drawer (or further left if the stash
    /// drawer is also open). `Rect::ZERO` when not visible.
    pub stats_drawer: Rect,
    /// Side drawer for the Stash panel. Sits between the
    /// stats drawer and the inventory drawer. `Rect::ZERO`
    /// when not visible.
    pub stash_drawer: Rect,
    pub currency_bar: Rect,
    pub fit: f32,
}

impl Layout {
    pub fn compute(
        ui: &Ui<'_>,
        bag_cols: u8,
        bag_rows: u8,
        show_stats: bool,
        show_stash: bool,
    ) -> Self {
        let theme = *ui.theme();
        let screen = ui.screen_size();
        let fit = theme.scale.max(0.5);

        let drawer_w = (screen.x * DRAWER_FRAC_W)
            .clamp(DRAWER_MIN_W_PX * fit, DRAWER_MAX_W_PX * fit)
            .min(screen.x);
        let drawer = Rect::from_xywh(screen.x - drawer_w, 0.0, drawer_w, screen.y);

        let pad_x = PANEL_PAD_X * fit;
        let pad_y = PANEL_PAD_Y * fit;
        let content = Rect::from_xywh(
            drawer.x() + pad_x,
            drawer.y() + pad_y,
            drawer.width() - pad_x * 2.0,
            drawer.height() - pad_y * 2.0,
        );

        let header_h = HEADER_H * fit;
        let header = Rect::from_xywh(content.x(), content.y(), content.width(), header_h);

        let currency_h = CURRENCY_BAR_H * fit;
        let toggle_bar_h = TOGGLE_BAR_H * fit;

        // Vertical layout: header → paperdoll → toggle bar →
        // bag → currency pill. The toggle bar lives between
        // the two grids so the actions sit at the boundary
        // they act on; the currency pill sits as a small
        // badge under the bag.
        let section_gap = SECTION_GAP * fit;
        let avail_h_for_grids =
            (content.height() - header_h - section_gap * 4.0 - toggle_bar_h - currency_h).max(0.0);

        // Cell size constraint: width / cols, OR fit both
        // grids stacked vertically (paperdoll + bag).
        let section_pad = SECTION_INNER_PAD * fit;
        let cell_w_limit = (content.width() - section_pad * 2.0) / PAPERDOLL_COLS as f32;
        let cell_h_limit =
            (avail_h_for_grids - section_pad * 4.0) / (PAPERDOLL_ROWS as f32 + bag_rows as f32);
        let cell = cell_w_limit.min(cell_h_limit).max(10.0);

        let pd_cell = cell;
        let pd_w = pd_cell * PAPERDOLL_COLS as f32;
        let pd_h = pd_cell * PAPERDOLL_ROWS as f32;
        // The section's outer rect is the grid plus an inner
        // pad on every side; the grid origin sits inset by
        // `section_pad` from the section's top-left.
        let paperdoll = Rect::from_xywh(
            content.x() + (content.width() - (pd_w + section_pad * 2.0)) * 0.5,
            header.max.y + section_gap,
            pd_w + section_pad * 2.0,
            pd_h + section_pad * 2.0,
        );
        let paperdoll_origin =
            Pos2::new(paperdoll.min.x + section_pad, paperdoll.min.y + section_pad);

        // Toggle bar sits between the two sections.
        let toggle_top = paperdoll.max.y + section_gap;
        let toggle_bar = Rect::from_xywh(content.x(), toggle_top, content.width(), toggle_bar_h);

        let bag_top = toggle_bar.max.y + section_gap;
        let bag_cell = cell;
        let bag_w = bag_cell * bag_cols as f32;
        let bag_h = bag_cell * bag_rows as f32;
        let bag = Rect::from_xywh(
            content.x() + (content.width() - (bag_w + section_pad * 2.0)) * 0.5,
            bag_top,
            bag_w + section_pad * 2.0,
            bag_h + section_pad * 2.0,
        );
        let bag_origin = Pos2::new(bag.min.x + section_pad, bag.min.y + section_pad);

        // Currency "pill": compact badge under the bag.
        // Width is just enough to fit the glyph + amount +
        // label rather than the full content width, anchored
        // flush with the right edge of the bag section.
        let currency_w = (180.0 * fit).min(content.width());
        let currency_bar = Rect::from_xywh(
            bag.max.x - currency_w,
            bag.max.y + section_gap * 0.5,
            currency_w,
            currency_h,
        );

        // Side drawers — stats opens to the LEFT of the
        // inventory drawer. Stash is its own full-size
        // drawer anchored to the LEFT edge of the screen
        // (mirror of the inventory drawer) so it reads as a
        // standalone container that happens to be open at
        // the same time, not a sub-pane wedged between them.
        let side_w = (screen.x * SIDE_DRAWER_FRAC_W)
            .clamp(SIDE_DRAWER_MIN_W_PX * fit, SIDE_DRAWER_MAX_W_PX * fit)
            .min(screen.x);
        let stash_drawer = if show_stash {
            Rect::from_xywh(0.0, 0.0, drawer_w, screen.y)
        } else {
            Rect::from_xywh(0.0, 0.0, 0.0, 0.0)
        };
        let stats_drawer = if show_stats {
            // If the stash drawer is open on the left, push
            // stats further right so it doesn't overlap.
            let left_limit = if show_stash { stash_drawer.max.x } else { 0.0 };
            let x = (drawer.x() - side_w).max(left_limit);
            Rect::from_xywh(x, 0.0, side_w, screen.y)
        } else {
            Rect::from_xywh(0.0, 0.0, 0.0, 0.0)
        };

        Self {
            drawer,
            content,
            header,
            paperdoll,
            paperdoll_cell: pd_cell,
            paperdoll_origin,
            bag,
            bag_cell,
            bag_origin,
            toggle_bar,
            stats_drawer,
            stash_drawer,
            currency_bar,
            fit,
        }
    }

    /// Rectangle of the equipment slot at `slot_idx` on the
    /// paperdoll, in screen pixels. Each slot is inset by
    /// half [`PAPERDOLL_SLOT_GAP`] on every side so adjacent
    /// slots have visible breathing room instead of sharing
    /// borders.
    pub fn paperdoll_slot_rect(&self, slot_idx: u8) -> Rect {
        let (cx, cy, cw, ch) = PAPERDOLL_LAYOUT[slot_idx as usize];
        let raw = Rect::from_xywh(
            self.paperdoll_origin.x + cx as f32 * self.paperdoll_cell,
            self.paperdoll_origin.y + cy as f32 * self.paperdoll_cell,
            cw as f32 * self.paperdoll_cell,
            ch as f32 * self.paperdoll_cell,
        );
        let half_gap = (PAPERDOLL_SLOT_GAP * self.fit) * 0.5;
        Rect::from_xywh(
            raw.x() + half_gap,
            raw.y() + half_gap,
            (raw.width() - half_gap * 2.0).max(1.0),
            (raw.height() - half_gap * 2.0).max(1.0),
        )
    }

    /// Rectangle covering `(cw × ch)` cells of the bag grid
    /// starting at `(cx, cy)`, in screen pixels.
    pub fn bag_rect(&self, cx: u8, cy: u8, cw: u8, ch: u8) -> Rect {
        Rect::from_xywh(
            self.bag_origin.x + cx as f32 * self.bag_cell,
            self.bag_origin.y + cy as f32 * self.bag_cell,
            cw as f32 * self.bag_cell,
            ch as f32 * self.bag_cell,
        )
    }
}

/// Index-as-anchor packer: each non-`None` slot is rendered
/// at the cell `(idx % cols, idx / cols)`. The server
/// guarantees no two items' footprints overlap (see
/// `place_inventory_item` in rift-server), so we never need a
/// fallback. Items whose footprint would overflow the grid
/// (defensive — shouldn't happen) return `None`.
pub fn pack_bag<'a, T: 'a>(
    items: &'a [Option<T>],
    cell_w_h: impl Fn(usize, &'a T) -> (u8, u8) + 'a,
    cols: u8,
    rows: u8,
) -> Vec<Option<(u8, u8, u8, u8)>> {
    let cols_us = cols as usize;
    let rows_us = rows as usize;
    let mut out = Vec::with_capacity(items.len());
    for (idx, slot) in items.iter().enumerate() {
        let Some(item) = slot.as_ref() else {
            out.push(None);
            continue;
        };
        let (w, h) = cell_w_h(idx, item);
        let w = w.max(1) as usize;
        let h = h.max(1) as usize;
        let cx = idx % cols_us;
        let cy = idx / cols_us;
        if cx + w <= cols_us && cy + h <= rows_us {
            out.push(Some((cx as u8, cy as u8, w as u8, h as u8)));
        } else {
            out.push(None);
        }
    }
    out
}
