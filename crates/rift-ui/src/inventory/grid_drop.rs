//! Shared snap-anchor drag preview + drop resolver used by
//! both the bag and the stash panels. Given a `cols × rows`
//! grid laid out at a screen rect, and a `cell_owner` map
//! from per-cell index → owning anchor index, computes where
//! the in-flight ghost will land, draws the green/red
//! footprint preview, and consumes the drop via
//! `take_drop_at` so all per-cell `take_drop` calls inside
//! `ItemSlot::interact` can't intercept the release at the
//! wrong sub-cell.

use rift_ui_im::{Color, Rect, Ui};
use rift_ui_types::inventory::{DragSource, InventoryAction};

use super::drag::{handle_drop, DropTarget};

/// Per-grid description.
pub struct GridSpec<'a> {
    /// Screen rect of the grid (top-left aligned with cell 0).
    pub rect: Rect,
    /// Single cell side in pixels.
    pub cell_px: f32,
    pub cols: u8,
    pub rows: u8,
    /// Cell → owning anchor index. Same length as `cols*rows`.
    pub cell_owner: &'a [Option<u32>],
}

/// Drop-target factory: given the resolved anchor cell index
/// or the index of the single overlapped item (when swapping),
/// return the `DropTarget` to feed into `handle_drop`.
pub trait TargetFor {
    fn target(&self, anchor_or_owner_idx: u32) -> DropTarget;
}

impl<F: Fn(u32) -> DropTarget> TargetFor for F {
    fn target(&self, idx: u32) -> DropTarget {
        (self)(idx)
    }
}

/// Compute the snap anchor for the active drag, render the
/// footprint preview (green = valid, red = blocked by 2+
/// items), and on release dispatch the appropriate
/// `InventoryAction`. Returns `true` iff a drop was consumed
/// this frame (so the caller can skip per-cell drop
/// fallbacks).
///
/// `source_anchor_idx` should be the source item's own anchor
/// when it lives in *this* grid (so its cells don't count as
/// overlap with itself); `None` for cross-grid drags.
pub fn snap_preview_and_resolve<F>(
    ui: &mut Ui<'_>,
    grid: &GridSpec<'_>,
    src_w: u8,
    src_h: u8,
    source_anchor_idx: Option<u32>,
    stash_open: bool,
    active_tab_u8: u8,
    target_for: F,
    out_actions: &mut Vec<InventoryAction>,
    in_transit: &mut Option<rift_ui_types::inventory::InTransitSource>,
    in_transit_dest_rect: &mut Option<[f32; 4]>,
) -> bool
where
    F: Fn(u32) -> DropTarget,
{
    if src_w as usize > grid.cols as usize || src_h as usize > grid.rows as usize {
        return false;
    }
    let mp = ui.mouse_pos();
    if !grid.rect.contains(mp) {
        return false;
    }
    let (fx, fy) = ui.drag_grab_frac().unwrap_or((0.5, 0.5));
    let gx = (fx * src_w as f32).floor() as i32;
    let gy = (fy * src_h as f32).floor() as i32;
    let cur_cx = ((mp.x - grid.rect.x()) / grid.cell_px).floor() as i32;
    let cur_cy = ((mp.y - grid.rect.y()) / grid.cell_px).floor() as i32;
    let max_x = grid.cols as i32 - src_w as i32;
    let max_y = grid.rows as i32 - src_h as i32;
    let ax = (cur_cx - gx).clamp(0, max_x) as u8;
    let ay = (cur_cy - gy).clamp(0, max_y) as u8;

    // Single-item overlap is fine (the server will swap with
    // it); multi-item overlap means the drop can't resolve.
    let cols_us = grid.cols as usize;
    let mut blockers: Vec<u32> = Vec::new();
    for dy in 0..src_h as usize {
        for dx in 0..src_w as usize {
            let cx = ax as usize + dx;
            let cy = ay as usize + dy;
            if let Some(owner) = grid.cell_owner[cy * cols_us + cx] {
                if Some(owner) != source_anchor_idx && !blockers.contains(&owner) {
                    blockers.push(owner);
                }
            }
        }
    }
    let valid = blockers.len() <= 1;
    let preview = Rect::from_xywh(
        grid.rect.x() + ax as f32 * grid.cell_px,
        grid.rect.y() + ay as f32 * grid.cell_px,
        src_w as f32 * grid.cell_px,
        src_h as f32 * grid.cell_px,
    );
    let fill = if valid {
        Color::rgba(0.30, 0.95, 0.55, 0.18)
    } else {
        Color::rgba(0.95, 0.30, 0.30, 0.22)
    };
    let stroke = if valid {
        Color::rgba(0.55, 1.0, 0.75, 0.95)
    } else {
        Color::rgba(1.0, 0.45, 0.45, 0.95)
    };
    ui.draw_rect(preview, fill);
    ui.draw_outline(preview, 2.0, stroke);

    let mut consumed = false;
    if let Some(drop) = ui.take_drop_at::<DragSource>(grid.rect, preview.center()) {
        consumed = true;
        if valid {
            let anchor_cell_idx = ay as u32 * grid.cols as u32 + ax as u32;
            let target_idx = blockers.first().copied().unwrap_or(anchor_cell_idx);
            if Some(target_idx) != source_anchor_idx {
                *in_transit = Some(rift_ui_types::inventory::InTransitSource::from_drag(
                    drop.payload,
                    active_tab_u8,
                ));
                *in_transit_dest_rect =
                    Some([preview.x(), preview.y(), preview.width(), preview.height()]);
                handle_drop(
                    drop.payload,
                    target_for.target(target_idx),
                    stash_open,
                    active_tab_u8,
                    out_actions,
                );
            }
        }
    }
    consumed
}
