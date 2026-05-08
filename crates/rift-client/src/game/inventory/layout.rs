//! Per-frame layout math for the inventory triptych
//! (stash? + bag + stats).
//!
//! All `f32` fields on [`Layout`] are pre-multiplied by the
//! chosen `fit` scale, so render code reads pixel values
//! directly without re-applying the scale. `fit` itself is
//! the smaller of the user's preferred theme scale and the
//! largest scale at which the full composition still fits on
//! screen — so a window resize squeezes the panel down rather
//! than letting it spill off the edges.

use rift_engine::ui::im::{Rect, Ui};

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

pub const SLOT_SIZE: f32 = 64.0;
pub const SLOT_GAP: f32 = 8.0;
pub const COLS: usize = 6;
pub const ROWS: usize = 5;
/// Equipment slots laid out in a single row above the bag
/// grid. There are 10 [`rift_game::loot::EquipSlot`] variants;
/// the panel is sized to fit all of them on one line.
pub const EQUIP_COLS: usize = 10;
pub const PANEL_PAD: f32 = 22.0;
pub const HEADER_H: f32 = 44.0;
pub const FOOTER_H: f32 = 30.0;
/// Vertical gap between the equipment row and the bag grid.
pub const INNER_GAP: f32 = 18.0;
/// Stats panel sits to the right of the bag panel. Wider than
/// before because long row labels ("Cooldown Reduction",
/// "Lightning Damage") need real estate next to their values.
pub const STATS_W: f32 = 340.0;
pub const STATS_GAP: f32 = 14.0;
pub const STASH_COLS: usize = 6;
pub const STASH_ROWS: usize = 6;

/// Per-frame computed layout. All `f32` fields are already
/// multiplied by [`Self::fit`], so call sites read pixel
/// values directly without re-applying the scale.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub fit: f32,
    pub slot: f32,
    pub gap: f32,
    pub pad: f32,
    pub header_h: f32,
    pub footer_h: f32,
    pub inner_gap: f32,
    /// Bag (6×5 grid) + horizontal equip row panel.
    pub bag_panel: Rect,
    /// Stats panel that sits to the right of the bag panel.
    pub stats_panel: Rect,
    /// Optional stash panel (only resolved when stash is open).
    pub stash_panel: Rect,
}

impl Layout {
    /// Compute a fit-scaled layout for the inventory triptych.
    /// Centres the (stash? + bag + stats) row on screen and
    /// shrinks every dimension uniformly so the panel never
    /// overflows. `stash_open` widens the composition to
    /// include the stash slab in the fit calculation.
    pub fn compute(ui: &Ui<'_>, stash_open: bool) -> Self {
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
