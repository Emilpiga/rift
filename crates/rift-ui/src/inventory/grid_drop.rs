//! Shared snap-anchor drag preview + drop resolver used by
//! both the bag and the stash panels. Given a `cols × rows`
//! grid laid out at a screen rect, and a `cell_owner` map
//! from per-cell index → owning anchor index, computes where
//! the in-flight ghost will land, draws the green/red
//! footprint preview, and consumes the drop via
//! `take_drop_at` so all per-cell `take_drop` calls inside
//! `ItemSlot::interact` can't intercept the release at the
//! wrong sub-cell.

use rift_ui_im::{Color, Pos2, Rect, Ui};
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

/// Drop-target factory: given the resolved anchor cell index,
/// return the `DropTarget` to feed into `handle_drop`.
pub trait TargetFor {
    fn target(&self, anchor_idx: u32) -> DropTarget;
}

impl<F: Fn(u32) -> DropTarget> TargetFor for F {
    fn target(&self, idx: u32) -> DropTarget {
        (self)(idx)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct SnapResolution {
    anchor_cell_idx: u32,
    blockers: Vec<u32>,
    preview: Rect,
}

impl SnapResolution {
    fn is_legal(
        &self,
        grid: &GridSpec<'_>,
        src_w: u8,
        src_h: u8,
        source_anchor_idx: Option<u32>,
    ) -> bool {
        if self.blockers.len() > 1 {
            return false;
        }
        let (Some(source), Some(&blocker)) = (source_anchor_idx, self.blockers.first()) else {
            return true;
        };
        displaced_fits_source_after_move(grid, source, blocker, self.anchor_cell_idx, src_w, src_h)
    }
}

fn resolve_snap_anchor(
    grid: &GridSpec<'_>,
    src_w: u8,
    src_h: u8,
    source_anchor_idx: Option<u32>,
    mouse_pos: Pos2,
    grab_frac: (f32, f32),
) -> Option<SnapResolution> {
    if src_w as usize > grid.cols as usize || src_h as usize > grid.rows as usize {
        return None;
    }
    if !grid.rect.contains(mouse_pos) {
        return None;
    }

    let (fx, fy) = grab_frac;
    let gx = (fx * src_w as f32).floor() as i32;
    let gy = (fy * src_h as f32).floor() as i32;
    let cur_cx = ((mouse_pos.x - grid.rect.x()) / grid.cell_px).floor() as i32;
    let cur_cy = ((mouse_pos.y - grid.rect.y()) / grid.cell_px).floor() as i32;
    let max_x = grid.cols as i32 - src_w as i32;
    let max_y = grid.rows as i32 - src_h as i32;
    let ax = (cur_cx - gx).clamp(0, max_x) as u8;
    let ay = (cur_cy - gy).clamp(0, max_y) as u8;

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

    Some(SnapResolution {
        anchor_cell_idx: ay as u32 * grid.cols as u32 + ax as u32,
        blockers,
        preview: Rect::from_xywh(
            grid.rect.x() + ax as f32 * grid.cell_px,
            grid.rect.y() + ay as f32 * grid.cell_px,
            src_w as f32 * grid.cell_px,
            src_h as f32 * grid.cell_px,
        ),
    })
}

fn displaced_fits_source_after_move(
    grid: &GridSpec<'_>,
    source: u32,
    blocker: u32,
    target: u32,
    moved_w: u8,
    moved_h: u8,
) -> bool {
    let Some((blocker_w, blocker_h)) = owner_footprint(grid, blocker) else {
        return false;
    };
    let cols = grid.cols as usize;
    let rows = grid.rows as usize;
    let source = source as usize;
    let blocker = blocker as usize;
    let target = target as usize;
    let sx = source % cols;
    let sy = source / cols;
    if sx + blocker_w as usize > cols || sy + blocker_h as usize > rows {
        return false;
    }

    for dy in 0..blocker_h as usize {
        for dx in 0..blocker_w as usize {
            let cell = (sy + dy) * cols + (sx + dx);
            if moved_covers(target, moved_w, moved_h, cell, cols) {
                return false;
            }
            if let Some(owner) = grid.cell_owner[cell] {
                let owner = owner as usize;
                if owner != source && owner != blocker {
                    return false;
                }
            }
        }
    }
    true
}

fn owner_footprint(grid: &GridSpec<'_>, owner: u32) -> Option<(u8, u8)> {
    let cols = grid.cols as usize;
    let mut min_x = usize::MAX;
    let mut min_y = usize::MAX;
    let mut max_x = 0usize;
    let mut max_y = 0usize;
    let mut found = false;
    for (idx, cell_owner) in grid.cell_owner.iter().enumerate() {
        if *cell_owner != Some(owner) {
            continue;
        }
        found = true;
        let x = idx % cols;
        let y = idx / cols;
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    found.then_some(((max_x - min_x + 1) as u8, (max_y - min_y + 1) as u8))
}

fn moved_covers(anchor: usize, w: u8, h: u8, cell: usize, cols: usize) -> bool {
    let ax = anchor % cols;
    let ay = anchor / cols;
    let cx = cell % cols;
    let cy = cell / cols;
    cx >= ax && cx < ax + w as usize && cy >= ay && cy < ay + h as usize
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
    let Some(resolved) = resolve_snap_anchor(
        grid,
        src_w,
        src_h,
        source_anchor_idx,
        ui.mouse_pos(),
        ui.drag_grab_frac().unwrap_or((0.5, 0.5)),
    ) else {
        return false;
    };
    let valid = resolved.is_legal(grid, src_w, src_h, source_anchor_idx);
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
    ui.draw_rect(resolved.preview, fill);
    ui.draw_outline(resolved.preview, 2.0, stroke);

    let mut consumed = false;
    if let Some(drop) = ui.take_drop_at::<DragSource>(grid.rect, resolved.preview.center()) {
        consumed = true;
        if valid {
            let target_idx = resolved.anchor_cell_idx;
            if Some(target_idx) != source_anchor_idx {
                *in_transit = Some(rift_ui_types::inventory::InTransitSource::from_drag(
                    drop.payload,
                    active_tab_u8,
                ));
                *in_transit_dest_rect = Some([
                    resolved.preview.x(),
                    resolved.preview.y(),
                    resolved.preview.width(),
                    resolved.preview.height(),
                ]);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn grid<'a>(owners: &'a [Option<u32>]) -> GridSpec<'a> {
        GridSpec {
            rect: Rect::from_xywh(100.0, 50.0, 40.0, 40.0),
            cell_px: 10.0,
            cols: 4,
            rows: 4,
            cell_owner: owners,
        }
    }

    #[test]
    fn snap_targets_preview_anchor_not_overlapped_owner() {
        let mut owners = vec![None; 16];
        owners[5] = Some(5);
        owners[6] = Some(5);
        owners[9] = Some(5);
        owners[10] = Some(5);
        let resolved = resolve_snap_anchor(
            &grid(&owners),
            1,
            1,
            None,
            Pos2::new(125.0, 75.0),
            (0.5, 0.5),
        )
        .unwrap();
        assert_eq!(resolved.anchor_cell_idx, 10);
        assert_eq!(resolved.blockers, vec![5]);
    }

    #[test]
    fn snap_ignores_source_item_footprint() {
        let mut owners = vec![None; 16];
        owners[0] = Some(0);
        owners[1] = Some(0);
        owners[4] = Some(0);
        owners[5] = Some(0);
        let resolved = resolve_snap_anchor(
            &grid(&owners),
            2,
            2,
            Some(0),
            Pos2::new(115.0, 65.0),
            (0.5, 0.5),
        )
        .unwrap();
        assert_eq!(resolved.anchor_cell_idx, 0);
        assert!(resolved.blockers.is_empty());
    }

    #[test]
    fn snap_reports_multiple_blockers() {
        let mut owners = vec![None; 16];
        owners[5] = Some(5);
        owners[6] = Some(6);
        let resolved = resolve_snap_anchor(
            &grid(&owners),
            2,
            1,
            None,
            Pos2::new(115.0, 65.0),
            (0.0, 0.0),
        )
        .unwrap();
        assert_eq!(resolved.anchor_cell_idx, 5);
        assert_eq!(resolved.blockers, vec![5, 6]);
    }

    #[test]
    fn adjacent_large_items_cannot_partially_overlap_swap() {
        let mut owners = vec![None; 24];
        for row in 0..3 {
            owners[row * 6] = Some(0);
            owners[row * 6 + 1] = Some(0);
            owners[row * 6 + 2] = Some(2);
            owners[row * 6 + 3] = Some(2);
        }
        let grid = GridSpec {
            rect: Rect::from_xywh(0.0, 0.0, 60.0, 40.0),
            cell_px: 10.0,
            cols: 6,
            rows: 4,
            cell_owner: &owners,
        };
        let resolved =
            resolve_snap_anchor(&grid, 2, 3, Some(0), Pos2::new(10.0, 0.0), (0.0, 0.0)).unwrap();
        assert_eq!(resolved.anchor_cell_idx, 1);
        assert_eq!(resolved.blockers, vec![2]);
        assert!(!resolved.is_legal(&grid, 2, 3, Some(0)));
    }
}
