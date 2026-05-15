//! Top-right minimap widget.
//!
//! Reads a flat [`MinimapView`] (walkable mask + pip lists) and
//! draws the framed mini-floorplan, vignette, player heading
//! fan, and enemy / portal pips. The widget is fully agnostic
//! of `hecs` / `rift_dungeon` — the host walks the world and
//! produces the view each frame.

use rift_ui_im::{Color, Frame, Pad, PanelHeader, Pos2, Rect, Ui};
use rift_ui_types::hud::{
    MinimapCell, MinimapPropKind, MinimapRoomKind, MinimapStairDir, MinimapSurface,
    MinimapTileKind, MinimapView,
};

const MAP_PX_BASE: f32 = 292.0;
const HEADER_H_BASE: f32 = 30.0;
const INSET_BASE: f32 = 10.0;
const MARGIN_BASE: f32 = 14.0;
const VIEW_TILES_BASE: f32 = 58.0;

/// Render the minimap anchored to the top-right corner. Returns
/// the outer panel rect so callers can stack siblings (buff
/// strips, floor indicator) directly underneath.
pub fn frame_minimap(ui: &mut Ui<'_>, view: &MinimapView<'_>) -> Rect {
    let theme = *ui.theme();
    let s = theme.scale;
    let map_px = (MAP_PX_BASE * s).min(ui.screen_size().x * 0.36);
    let header_h = HEADER_H_BASE * s;
    let inset = INSET_BASE * s;
    let margin = MARGIN_BASE * s;
    let screen = ui.screen_size();

    let map_x = screen.x - map_px - margin;
    let map_y = margin;
    let panel_rect = Rect::from_xywh(map_x, map_y, map_px, map_px);

    let frame = Frame::stone(&theme)
        .with_radius(0.0)
        .with_padding(Pad::all(0.0));
    frame.show(ui, panel_rect, |ui, body| {
        draw_header(ui, body, header_h, &theme, view);
        draw_floor_and_pips(ui, body, header_h, inset, view);
    });

    panel_rect
}

fn draw_header(
    ui: &mut Ui<'_>,
    body: Rect,
    header_h: f32,
    theme: &rift_ui_im::Theme,
    view: &MinimapView<'_>,
) {
    let header = Rect::from_xywh(body.x(), body.y(), body.width(), header_h);
    PanelHeader::new(view.zone_title)
        .font_size(theme.fonts.size_md)
        .show(ui, header);
    draw_header_detail(ui, header, theme, view.zone_title, view.zone_detail);
}

fn draw_header_detail(
    ui: &mut Ui<'_>,
    header: Rect,
    theme: &rift_ui_im::Theme,
    title: &str,
    detail: &str,
) {
    let scale = theme.scale;
    let title_x = header.x() + 17.0 * scale;
    let title_w = ui.measure_text(title, theme.fonts.size_md);
    let left_guard = title_x + title_w + 14.0 * scale;
    let right_pad = 14.0 * scale;
    let available = (header.max.x - right_pad - left_guard).max(0.0);
    if available <= 8.0 * scale {
        return;
    }

    let mut size = theme.fonts.size_md;
    let mut width = ui.measure_text(detail, size);
    if width > available {
        size = (size * (available / width)).clamp(theme.fonts.size_sm * 0.78, theme.fonts.size_md);
        width = ui.measure_text(detail, size);
    }
    if width > available {
        size = theme.fonts.size_sm * 0.78;
        width = ui.measure_text(detail, size).min(available);
    }

    let x = header.max.x - right_pad - width;
    let y = header.y() + (header.height() - size) * 0.5 - 1.0 * scale;
    ui.draw_text(
        Pos2::new(x + 1.0 * scale, y + 1.0 * scale),
        detail,
        size,
        Color::rgba(0.0, 0.0, 0.0, 0.58),
    );
    ui.draw_text(
        Pos2::new(x, y),
        detail,
        size,
        Color::rgba(0.96, 0.84, 0.52, 1.0),
    );
}

fn draw_floor_and_pips(
    ui: &mut Ui<'_>,
    body: Rect,
    header_h: f32,
    inset: f32,
    view: &MinimapView<'_>,
) {
    let inner_rect = Rect::from_xywh(
        body.x() + inset,
        body.y() + header_h + inset,
        body.width() - inset * 2.0,
        body.height() - header_h - inset * 2.0,
    );

    ui.draw_rounded_rect(inner_rect, 0.0, Color::rgba(0.010, 0.011, 0.014, 1.0));
    ui.draw_rounded_radial_rect_noisy(
        inner_rect,
        0.0,
        Color::rgba(0.010, 0.010, 0.013, 1.0),
        Color::rgba(0.060, 0.052, 0.044, 0.62),
    );

    let grid_max = view.grid_width.max(view.grid_depth) as f32;
    if grid_max < 1.0 {
        return;
    }
    let stride = view.grid_width as usize;
    let rich_cells = view.cells.len() == stride * view.grid_depth as usize;
    let full_bounds = if view.show_full_extent {
        minimap_content_bounds(view, stride, rich_cells)
    } else {
        None
    };
    let (view_tiles_x, view_tiles_z) = if let Some((min_x, min_z, max_x, max_z)) = full_bounds {
        (
            (max_x - min_x + 1.0).max(1.0),
            (max_z - min_z + 1.0).max(1.0),
        )
    } else if rich_cells {
        let zoom = view.zoom.clamp(0.65, 1.75);
        let tiles = (VIEW_TILES_BASE / zoom).clamp(24.0, 92.0);
        (tiles, tiles)
    } else {
        (grid_max, grid_max)
    };
    let cell_x = (inner_rect.width() / view_tiles_x).max(1.0);
    let cell_z = (inner_rect.height() / view_tiles_z).max(1.0);
    let cell = cell_x.min(cell_z);
    let (focus_x, focus_z) = view
        .focus
        .unwrap_or((view.grid_width as f32 * 0.5, view.grid_depth as f32 * 0.5));
    let origin_x = if let Some((min_x, _min_z, _max_x, _max_z)) = full_bounds {
        min_x
    } else if rich_cells {
        (focus_x - view_tiles_x * 0.5).clamp(0.0, (view.grid_width as f32 - view_tiles_x).max(0.0))
    } else {
        0.0
    };
    let origin_z = if let Some((_min_x, min_z, _max_x, _max_z)) = full_bounds {
        min_z
    } else if rich_cells {
        (focus_z - view_tiles_z * 0.5).clamp(0.0, (view.grid_depth as f32 - view_tiles_z).max(0.0))
    } else {
        0.0
    };
    let inner_x = inner_rect.x() - origin_x * cell_x;
    let inner_y = inner_rect.y() - origin_z * cell_z;
    if rich_cells {
        draw_rich_floor(
            ui, view, inner_rect, inner_x, inner_y, cell_x, cell_z, stride,
        );
    } else {
        draw_walkable_floor(
            ui, view, inner_rect, inner_x, inner_y, cell_x, cell_z, stride,
        );
    }

    // Inner vignette — soft dark band on each edge so the
    // floorplan recedes into the panel chrome.
    const VIG_STEPS: i32 = 3;
    for i in 0..VIG_STEPS {
        let f = 1.0 - (i as f32 / VIG_STEPS as f32);
        let alpha = 0.28 * f;
        let band = (4.0 - i as f32 * 1.2).max(1.0);
        let col = Color::rgba(0.0, 0.0, 0.0, alpha);
        ui.draw_rect(
            Rect::from_xywh(
                inner_rect.x(),
                inner_rect.y() + i as f32,
                inner_rect.width(),
                band,
            ),
            col,
        );
        ui.draw_rect(
            Rect::from_xywh(
                inner_rect.x(),
                inner_rect.max.y - i as f32 - band,
                inner_rect.width(),
                band,
            ),
            col,
        );
        ui.draw_rect(
            Rect::from_xywh(
                inner_rect.x() + i as f32,
                inner_rect.y(),
                band,
                inner_rect.height(),
            ),
            col,
        );
        ui.draw_rect(
            Rect::from_xywh(
                inner_rect.max.x - i as f32 - band,
                inner_rect.y(),
                band,
                inner_rect.height(),
            ),
            col,
        );
    }

    ui.draw_rounded_outline(inner_rect, 0.0, 1.0, Color::rgba(0.18, 0.20, 0.26, 1.0));

    // Mapping helpers — closures borrow `inner_x`/`inner_y`/`cell`.
    let to_map = |p: (f32, f32)| -> (f32, f32) {
        (
            inner_x + (p.0 + 0.5) * cell_x,
            inner_y + (p.1 + 0.5) * cell_z,
        )
    };
    let in_inner = |mx: f32, my: f32| -> bool {
        mx >= inner_rect.x()
            && mx <= inner_rect.max.x
            && my >= inner_rect.y()
            && my <= inner_rect.max.y
    };

    for prop in view.props {
        if !pos_explored(view, stride, prop.pos) {
            continue;
        }
        let (mx, my) = to_map(prop.pos);
        if !in_inner(mx, my) {
            continue;
        }
        draw_prop_marker(ui, mx, my, cell, prop.kind);
    }

    // Portal pip first so enemies / party / player layer over it.
    if let Some(p) = view.portal {
        let (mx, my) = to_map(p);
        if pos_explored(view, stride, p) && in_inner(mx, my) {
            draw_portal_marker(ui, mx, my, (cell * 2.7).max(7.0));
        }
    }

    for member in view.party {
        let (mx, my) = to_map(member.pos);
        if !in_inner(mx, my) {
            continue;
        }
        draw_player_marker(
            ui,
            mx,
            my,
            cell,
            member.facing,
            Color::rgba(0.25, 0.78, 1.0, 1.0),
            Color::rgba(0.08, 0.18, 0.26, 0.58),
        );
    }

    for enemy in view.enemies {
        if !pos_visible(view, stride, enemy.pos) {
            continue;
        }
        let (mx, my) = to_map(enemy.pos);
        if !in_inner(mx, my) {
            continue;
        }
        if enemy.is_boss {
            draw_boss_marker(ui, mx, my, (cell * 3.0).max(8.0));
        } else {
            let pip_size = (cell * 1.7).max(3.0);
            draw_pip(
                ui,
                mx,
                my,
                pip_size,
                Color::rgba(0.94, 0.30, 0.26, 1.0),
                Color::rgba(0.92, 0.20, 0.18, 0.30),
            );
        }
    }

    if let Some(p) = view.player {
        let (mx, my) = to_map(p.pos);
        if in_inner(mx, my) {
            draw_player_marker(
                ui,
                mx,
                my,
                cell,
                p.facing,
                Color::rgba(0.98, 0.99, 1.0, 1.0),
                Color::rgba(0.55, 0.78, 1.0, 0.46),
            );
        }
    }
}

fn minimap_content_bounds(
    view: &MinimapView<'_>,
    stride: usize,
    rich_cells: bool,
) -> Option<(f32, f32, f32, f32)> {
    let mut min_x = view.grid_width;
    let mut min_z = view.grid_depth;
    let mut max_x = 0u32;
    let mut max_z = 0u32;
    let mut any = false;

    for z in 0..view.grid_depth {
        for x in 0..view.grid_width {
            let idx = z as usize * stride + x as usize;
            let occupied = if rich_cells {
                view.cells
                    .get(idx)
                    .map(|c| c.kind != MinimapTileKind::Wall && c.explored)
                    .unwrap_or(false)
            } else {
                view.walkable.get(idx).copied().unwrap_or(false)
            };
            if !occupied {
                continue;
            }
            any = true;
            min_x = min_x.min(x);
            min_z = min_z.min(z);
            max_x = max_x.max(x);
            max_z = max_z.max(z);
        }
    }

    any.then_some((min_x as f32, min_z as f32, max_x as f32, max_z as f32))
}

fn draw_walkable_floor(
    ui: &mut Ui<'_>,
    view: &MinimapView<'_>,
    clip: Rect,
    inner_x: f32,
    inner_y: f32,
    cell_x: f32,
    cell_z: f32,
    stride: usize,
) {
    let floor_a = Color::rgba(0.42, 0.36, 0.30, 0.95);
    let floor_b = Color::rgba(0.36, 0.30, 0.25, 0.95);
    for z in 0..view.grid_depth {
        for x in 0..view.grid_width {
            let idx = z as usize * stride + x as usize;
            if view.walkable.get(idx).copied().unwrap_or(false) {
                let rect = tile_rect(inner_x, inner_y, cell_x, cell_z, x, z);
                if !rect_intersects(rect, clip) {
                    continue;
                }
                let col = if (x ^ z) & 1 == 0 { floor_a } else { floor_b };
                ui.draw_rect(rect, col);
            }
        }
    }
}

fn draw_rich_floor(
    ui: &mut Ui<'_>,
    view: &MinimapView<'_>,
    clip: Rect,
    inner_x: f32,
    inner_y: f32,
    cell_x: f32,
    cell_z: f32,
    stride: usize,
) {
    let cell = cell_x.min(cell_z);
    for z in 0..view.grid_depth {
        for x in 0..view.grid_width {
            let idx = z as usize * stride + x as usize;
            let Some(c) = view.cells.get(idx) else {
                continue;
            };
            if c.kind == MinimapTileKind::Wall {
                continue;
            }
            let rect = tile_rect(inner_x, inner_y, cell_x, cell_z, x, z);
            if !rect_intersects(rect, clip) {
                continue;
            }
            if !c.explored {
                continue;
            }
            let fill_rect = Rect::from_xywh(
                rect.x() - 0.35,
                rect.y() - 0.35,
                rect.width() + 0.70,
                rect.height() + 0.70,
            );
            draw_tile_surface(ui, view, stride, *c, fill_rect, rect, cell, x, z);
            if c.room == MinimapRoomKind::Boss {
                ui.draw_rect(rect, Color::rgba(0.55, 0.18, 0.06, 0.13));
            } else if c.room == MinimapRoomKind::Portal {
                ui.draw_rect(rect, Color::rgba(0.08, 0.42, 0.55, 0.16));
            }
            draw_wall_contact_shadow(ui, view, stride, x as i32, z as i32, rect, cell);
            if c.kind == MinimapTileKind::Stair {
                draw_stair(ui, rect, c.stair_dir, cell);
            }
            draw_soft_fog(ui, view, stride, x as i32, z as i32, rect);
            draw_undiscovered_fog(ui, view, stride, x as i32, z as i32, rect);
            draw_undiscovered_edge_fade(ui, view, stride, x as i32, z as i32, rect, cell);
        }
    }

    for z in 0..view.grid_depth {
        for x in 0..view.grid_width {
            let idx = z as usize * stride + x as usize;
            let Some(c) = view.cells.get(idx) else {
                continue;
            };
            if c.kind == MinimapTileKind::Wall || c.explored {
                continue;
            }
            let rect = tile_rect(inner_x, inner_y, cell_x, cell_z, x, z);
            if !rect_intersects(rect, clip) {
                continue;
            }
            draw_undiscovered_fog(ui, view, stride, x as i32, z as i32, rect);
        }
    }

    for z in 0..view.grid_depth {
        for x in 0..view.grid_width {
            let idx = z as usize * stride + x as usize;
            let Some(c) = view.cells.get(idx) else {
                continue;
            };
            if c.kind == MinimapTileKind::Wall {
                continue;
            }
            let rect = tile_rect(inner_x, inner_y, cell_x, cell_z, x, z);
            if !c.explored || !rect_intersects(rect, clip) {
                continue;
            }
            let edge = Color::rgba(0.025, 0.022, 0.020, 0.92);
            let seam = Color::rgba(0.0, 0.0, 0.0, 0.055);
            let t = 1.0_f32.max(cell * 0.11);
            if is_wall_or_oob(view, stride, x as i32, z as i32 - 1) {
                ui.draw_line(
                    Pos2::new(rect.x(), rect.y()),
                    Pos2::new(rect.max.x, rect.y()),
                    t,
                    edge,
                );
            } else {
                if cell >= 4.0
                    && elevation_changes(view, stride, x as i32, z as i32, x as i32, z as i32 - 1)
                {
                    ui.draw_line(
                        Pos2::new(rect.x(), rect.y()),
                        Pos2::new(rect.max.x, rect.y()),
                        1.0,
                        seam,
                    );
                }
            }
            if is_wall_or_oob(view, stride, x as i32, z as i32 + 1) {
                ui.draw_line(
                    Pos2::new(rect.x(), rect.max.y),
                    Pos2::new(rect.max.x, rect.max.y),
                    t,
                    edge,
                );
            }
            if is_wall_or_oob(view, stride, x as i32 - 1, z as i32) {
                ui.draw_line(
                    Pos2::new(rect.x(), rect.y()),
                    Pos2::new(rect.x(), rect.max.y),
                    t,
                    edge,
                );
            } else {
                if cell >= 4.0
                    && elevation_changes(view, stride, x as i32, z as i32, x as i32 - 1, z as i32)
                {
                    ui.draw_line(
                        Pos2::new(rect.x(), rect.y()),
                        Pos2::new(rect.x(), rect.max.y),
                        1.0,
                        seam,
                    );
                }
            }
            if is_wall_or_oob(view, stride, x as i32 + 1, z as i32) {
                ui.draw_line(
                    Pos2::new(rect.max.x, rect.y()),
                    Pos2::new(rect.max.x, rect.max.y),
                    t,
                    edge,
                );
            }
        }
    }
}

fn draw_soft_fog(
    ui: &mut Ui<'_>,
    view: &MinimapView<'_>,
    stride: usize,
    x: i32,
    z: i32,
    rect: Rect,
) {
    let fog = |visibility: f32| {
        let hidden = (1.0 - visibility.clamp(0.0, 1.0)).powf(1.18);
        Color::rgba(0.0, 0.0, 0.0, 0.62 * hidden)
    };
    let tl = fog(smoothed_corner_visibility(view, stride, x, z, -1, -1));
    let tr = fog(smoothed_corner_visibility(view, stride, x, z, 1, -1));
    let bl = fog(smoothed_corner_visibility(view, stride, x, z, -1, 1));
    let br = fog(smoothed_corner_visibility(view, stride, x, z, 1, 1));
    let max_alpha = tl.0[3].max(tr.0[3]).max(bl.0[3]).max(br.0[3]);
    if max_alpha <= 0.01 {
        return;
    }
    let soft_rect = Rect::from_xywh(
        rect.x() - 0.75,
        rect.y() - 0.75,
        rect.width() + 1.50,
        rect.height() + 1.50,
    );
    ui.draw_grad4_rect(soft_rect, tl, tr, bl, br);
}

fn draw_undiscovered_fog(
    ui: &mut Ui<'_>,
    view: &MinimapView<'_>,
    stride: usize,
    x: i32,
    z: i32,
    rect: Rect,
) {
    let fog = |explored: f32| {
        let unknown = 1.0 - explored.clamp(0.0, 1.0);
        let alpha = 0.88 * smoothstep(unknown).powf(1.10);
        Color::rgba(0.0, 0.0, 0.0, alpha)
    };
    let tl = fog(smoothed_corner_explored(view, stride, x, z, -1, -1));
    let tr = fog(smoothed_corner_explored(view, stride, x, z, 1, -1));
    let bl = fog(smoothed_corner_explored(view, stride, x, z, -1, 1));
    let br = fog(smoothed_corner_explored(view, stride, x, z, 1, 1));
    let max_alpha = tl.0[3].max(tr.0[3]).max(bl.0[3]).max(br.0[3]);
    if max_alpha <= 0.01 {
        return;
    }
    let soft_rect = Rect::from_xywh(
        rect.x() - 1.25,
        rect.y() - 1.25,
        rect.width() + 2.50,
        rect.height() + 2.50,
    );
    ui.draw_grad4_rect(soft_rect, tl, tr, bl, br);
}

fn draw_undiscovered_edge_fade(
    ui: &mut Ui<'_>,
    view: &MinimapView<'_>,
    stride: usize,
    x: i32,
    z: i32,
    rect: Rect,
    cell: f32,
) {
    let band = (cell * 3.4).clamp(8.0, 22.0);
    let black = Color::rgba(0.0, 0.0, 0.0, 0.82);
    let soft = Color::rgba(0.0, 0.0, 0.0, 0.0);

    if is_undiscovered_open(view, stride, x, z - 1) {
        ui.draw_gradient_rect(
            Rect::from_xywh(rect.x(), rect.y(), rect.width(), band),
            black,
            soft,
        );
    }
    if is_undiscovered_open(view, stride, x, z + 1) {
        ui.draw_gradient_rect(
            Rect::from_xywh(rect.x(), rect.max.y - band, rect.width(), band),
            soft,
            black,
        );
    }
    if is_undiscovered_open(view, stride, x - 1, z) {
        ui.draw_grad4_rect(
            Rect::from_xywh(rect.x(), rect.y(), band, rect.height()),
            black,
            soft,
            black,
            soft,
        );
    }
    if is_undiscovered_open(view, stride, x + 1, z) {
        ui.draw_grad4_rect(
            Rect::from_xywh(rect.max.x - band, rect.y(), band, rect.height()),
            soft,
            black,
            soft,
            black,
        );
    }
}

fn smoothed_corner_visibility(
    view: &MinimapView<'_>,
    stride: usize,
    x: i32,
    z: i32,
    dx: i32,
    dz: i32,
) -> f32 {
    let corner_x = x as f32 + if dx < 0 { 0.0 } else { 1.0 };
    let corner_z = z as f32 + if dz < 0 { 0.0 } else { 1.0 };
    let base_x = corner_x.floor() as i32;
    let base_z = corner_z.floor() as i32;
    let mut visible_weight = 0.0;
    let mut total_weight = 0.0;
    for sz in -4..=4 {
        for sx in -4..=4 {
            let cx = base_x + sx;
            let cz = base_z + sz;
            let Some(cell) = minimap_cell(view, stride, cx, cz) else {
                continue;
            };
            if cell.kind == MinimapTileKind::Wall {
                continue;
            }
            let sample_x = cx as f32 + 0.5;
            let sample_z = cz as f32 + 0.5;
            let dist = ((sample_x - corner_x).powi(2) + (sample_z - corner_z).powi(2)).sqrt();
            let weight = smoothstep(1.0 - dist / 2.35);
            if weight <= 0.0 {
                continue;
            }
            total_weight += weight;
            if cell.explored && cell.visible {
                visible_weight += weight;
            }
        }
    }
    if total_weight <= 0.0 {
        0.0
    } else {
        visible_weight / total_weight
    }
}

fn smoothed_corner_explored(
    view: &MinimapView<'_>,
    stride: usize,
    x: i32,
    z: i32,
    dx: i32,
    dz: i32,
) -> f32 {
    let corner_x = x as f32 + if dx < 0 { 0.0 } else { 1.0 };
    let corner_z = z as f32 + if dz < 0 { 0.0 } else { 1.0 };
    let base_x = corner_x.floor() as i32;
    let base_z = corner_z.floor() as i32;
    let mut explored_weight = 0.0;
    let mut total_weight = 0.0;
    for sz in -2..=2 {
        for sx in -2..=2 {
            let cx = base_x + sx;
            let cz = base_z + sz;
            let Some(cell) = minimap_cell(view, stride, cx, cz) else {
                continue;
            };
            if cell.kind == MinimapTileKind::Wall {
                continue;
            }
            let sample_x = cx as f32 + 0.5;
            let sample_z = cz as f32 + 0.5;
            let dist = ((sample_x - corner_x).powi(2) + (sample_z - corner_z).powi(2)).sqrt();
            let weight = smoothstep(1.0 - dist / 4.15);
            if weight <= 0.0 {
                continue;
            }
            total_weight += weight;
            if cell.explored {
                explored_weight += weight;
            }
        }
    }
    if total_weight <= 0.0 {
        0.0
    } else {
        explored_weight / total_weight
    }
}

fn rect_intersects(a: Rect, b: Rect) -> bool {
    a.max.x >= b.min.x && a.min.x <= b.max.x && a.max.y >= b.min.y && a.min.y <= b.max.y
}

fn minimap_cell<'a>(
    view: &'a MinimapView<'_>,
    stride: usize,
    x: i32,
    z: i32,
) -> Option<&'a MinimapCell> {
    if x < 0 || z < 0 || x >= view.grid_width as i32 || z >= view.grid_depth as i32 {
        return None;
    }
    view.cells.get(z as usize * stride + x as usize)
}

fn pos_cell<'a>(
    view: &'a MinimapView<'_>,
    stride: usize,
    pos: (f32, f32),
) -> Option<&'a MinimapCell> {
    let x = (pos.0 + 0.5).floor() as i32;
    let z = (pos.1 + 0.5).floor() as i32;
    minimap_cell(view, stride, x, z)
}

fn pos_explored(view: &MinimapView<'_>, stride: usize, pos: (f32, f32)) -> bool {
    if view.cells.is_empty() {
        return true;
    }
    pos_cell(view, stride, pos)
        .map(|c| c.explored)
        .unwrap_or(false)
}

fn pos_visible(view: &MinimapView<'_>, stride: usize, pos: (f32, f32)) -> bool {
    if view.cells.is_empty() {
        return true;
    }
    pos_cell(view, stride, pos)
        .map(|c| c.visible)
        .unwrap_or(false)
}

fn tile_rect(inner_x: f32, inner_y: f32, cell_x: f32, cell_z: f32, x: u32, z: u32) -> Rect {
    Rect::from_xywh(
        inner_x + x as f32 * cell_x,
        inner_y + z as f32 * cell_z,
        cell_x,
        cell_z,
    )
}

fn is_wall_or_oob(view: &MinimapView<'_>, stride: usize, x: i32, z: i32) -> bool {
    if x < 0 || z < 0 || x >= view.grid_width as i32 || z >= view.grid_depth as i32 {
        return true;
    }
    let idx = z as usize * stride + x as usize;
    view.cells
        .get(idx)
        .map(|c| c.kind == MinimapTileKind::Wall)
        .unwrap_or(true)
}

fn is_undiscovered_open(view: &MinimapView<'_>, stride: usize, x: i32, z: i32) -> bool {
    let Some(cell) = minimap_cell(view, stride, x, z) else {
        return false;
    };
    cell.kind != MinimapTileKind::Wall && !cell.explored
}

fn elevation_changes(
    view: &MinimapView<'_>,
    stride: usize,
    ax: i32,
    az: i32,
    bx: i32,
    bz: i32,
) -> bool {
    if bx < 0 || bz < 0 || bx >= view.grid_width as i32 || bz >= view.grid_depth as i32 {
        return false;
    }
    let a = view.cells.get(az as usize * stride + ax as usize);
    let b = view.cells.get(bz as usize * stride + bx as usize);
    match (a, b) {
        (Some(a), Some(b)) => a.explored && b.explored && a.elevation != b.elevation,
        _ => false,
    }
}

fn tile_color(c: MinimapCell, x: u32, z: u32) -> Color {
    let (mut r, mut g, mut b) = match c.surface {
        MinimapSurface::Sand => (0.58, 0.47, 0.29),
        MinimapSurface::Stone => (0.46, 0.23, 0.21),
        MinimapSurface::Wood => (0.36, 0.26, 0.21),
        MinimapSurface::Metal => (0.28, 0.30, 0.31),
        MinimapSurface::Grass => (0.22, 0.32, 0.18),
        MinimapSurface::Bone => (0.50, 0.45, 0.33),
    };
    match c.room {
        MinimapRoomKind::Boss => {
            r += 0.10;
            g -= 0.02;
            b -= 0.05;
        }
        MinimapRoomKind::Portal => {
            r -= 0.04;
            g += 0.05;
            b += 0.08;
        }
        MinimapRoomKind::Corridor => {
            r *= 0.90;
            g *= 0.89;
            b *= 0.88;
        }
        MinimapRoomKind::None | MinimapRoomKind::Arena => {}
    }
    let grain = 0.985 + value_noise(x as f32 * 0.23, z as f32 * 0.23, 17) * 0.045;
    let elev = 1.0 + c.elevation as f32 * 0.024;
    Color::rgba(
        (r * grain * elev).clamp(0.02, 0.74),
        (g * grain * elev).clamp(0.02, 0.74),
        (b * grain * elev).clamp(0.02, 0.74),
        0.90,
    )
}

fn draw_tile_surface(
    ui: &mut Ui<'_>,
    view: &MinimapView<'_>,
    stride: usize,
    c: MinimapCell,
    fill_rect: Rect,
    rect: Rect,
    cell: f32,
    x: u32,
    z: u32,
) {
    let x = x as i32;
    let z = z as i32;
    ui.draw_grad4_rect(
        fill_rect,
        blended_corner_color(view, stride, x, z, -1, -1),
        blended_corner_color(view, stride, x, z, 1, -1),
        blended_corner_color(view, stride, x, z, -1, 1),
        blended_corner_color(view, stride, x, z, 1, 1),
    );
    draw_material_detail(ui, c.surface, rect, cell, x as u32, z as u32);
}

fn blended_corner_color(
    view: &MinimapView<'_>,
    stride: usize,
    x: i32,
    z: i32,
    dx: i32,
    dz: i32,
) -> Color {
    let samples = [(0, 0), (dx, 0), (0, dz), (dx, dz)];
    let mut sum = [0.0; 4];
    let mut count = 0.0;
    for (sx, sz) in samples {
        let cx = x + sx;
        let cz = z + sz;
        let Some(cell) = minimap_cell(view, stride, cx, cz) else {
            continue;
        };
        if cell.kind == MinimapTileKind::Wall || !cell.explored {
            continue;
        }
        let color = tile_color(*cell, cx as u32, cz as u32);
        sum[0] += color.0[0];
        sum[1] += color.0[1];
        sum[2] += color.0[2];
        sum[3] += color.0[3];
        count += 1.0;
    }
    if count <= 0.0 {
        return Color::rgba(0.0, 0.0, 0.0, 0.0);
    }
    Color::rgba(
        sum[0] / count,
        sum[1] / count,
        sum[2] / count,
        sum[3] / count,
    )
}

fn draw_material_detail(
    ui: &mut Ui<'_>,
    surface: MinimapSurface,
    rect: Rect,
    cell: f32,
    x: u32,
    z: u32,
) {
    if cell < 3.4 {
        return;
    }
    let h = tile_hash(x, z, 97);
    let phase = value_noise(x as f32 * 0.19, z as f32 * 0.19, 71);
    match surface {
        MinimapSurface::Stone => {
            if h % 9 == 0 {
                let y0 = rect.y() + cell * (0.15 + phase * 0.35);
                let y1 = y0 + cell * (0.16 + hash01(x, z, 43) * 0.18);
                ui.draw_line(
                    Pos2::new(rect.x() - cell * 0.30, y0),
                    Pos2::new(rect.max.x + cell * 1.45, y1),
                    1.0,
                    Color::rgba(0.10, 0.045, 0.035, 0.12),
                );
            }
        }
        MinimapSurface::Sand => {
            if h % 7 == 0 {
                let y = rect.y() + cell * (0.20 + phase * 0.46);
                ui.draw_line(
                    Pos2::new(rect.x() - cell * 0.45, y),
                    Pos2::new(rect.max.x + cell * 1.30, y + cell * 0.12),
                    1.0,
                    Color::rgba(0.88, 0.70, 0.40, 0.11),
                );
            }
        }
        MinimapSurface::Wood => {
            if h % 5 == 0 {
                let y = rect.y() + cell * (0.32 + phase * 0.36);
                ui.draw_line(
                    Pos2::new(rect.x() - cell * 0.35, y),
                    Pos2::new(rect.max.x + cell * 1.20, y + cell * 0.03),
                    1.0,
                    Color::rgba(0.12, 0.07, 0.04, 0.15),
                );
            }
        }
        MinimapSurface::Metal => {
            if h % 8 == 0 {
                let y = rect.y() + cell * (0.18 + phase * 0.52);
                ui.draw_line(
                    Pos2::new(rect.x() - cell * 0.20, y),
                    Pos2::new(rect.max.x + cell * 0.90, y),
                    1.0,
                    Color::rgba(0.80, 0.88, 0.88, 0.12),
                );
            }
        }
        MinimapSurface::Grass => {
            if h % 6 == 0 {
                let y = rect.y() + cell * (0.16 + phase * 0.58);
                ui.draw_line(
                    Pos2::new(rect.x() - cell * 0.15, y),
                    Pos2::new(rect.max.x + cell * 0.95, y + cell * 0.08),
                    1.0,
                    Color::rgba(0.08, 0.20, 0.06, 0.13),
                );
            }
        }
        MinimapSurface::Bone => {
            if h % 8 == 0 {
                ui.draw_line(
                    Pos2::new(
                        rect.x() - cell * 0.15,
                        rect.y() + cell * (0.25 + phase * 0.20),
                    ),
                    Pos2::new(
                        rect.max.x + cell * 1.10,
                        rect.max.y - cell * (0.25 + phase * 0.18),
                    ),
                    1.0,
                    Color::rgba(0.86, 0.80, 0.58, 0.12),
                );
            }
        }
    }
}

fn draw_wall_contact_shadow(
    ui: &mut Ui<'_>,
    view: &MinimapView<'_>,
    stride: usize,
    x: i32,
    z: i32,
    rect: Rect,
    cell: f32,
) {
    let band = (cell * 0.34).clamp(1.0, 2.4);
    let dark = Color::rgba(0.0, 0.0, 0.0, 0.20);
    let clear = Color::rgba(0.0, 0.0, 0.0, 0.0);
    if is_wall_or_oob(view, stride, x, z - 1) {
        ui.draw_gradient_rect(
            Rect::from_xywh(rect.x(), rect.y(), rect.width(), band),
            dark,
            clear,
        );
    }
    if is_wall_or_oob(view, stride, x, z + 1) {
        ui.draw_gradient_rect(
            Rect::from_xywh(rect.x(), rect.max.y - band, rect.width(), band),
            clear,
            dark,
        );
    }
    if is_wall_or_oob(view, stride, x - 1, z) {
        ui.draw_grad4_rect(
            Rect::from_xywh(rect.x(), rect.y(), band, rect.height()),
            dark,
            clear,
            dark,
            clear,
        );
    }
    if is_wall_or_oob(view, stride, x + 1, z) {
        ui.draw_grad4_rect(
            Rect::from_xywh(rect.max.x - band, rect.y(), band, rect.height()),
            clear,
            dark,
            clear,
            dark,
        );
    }
}

fn hash01(x: u32, z: u32, salt: u32) -> f32 {
    (tile_hash(x, z, salt) & 0xffff) as f32 / 65535.0
}

fn value_noise(x: f32, z: f32, salt: u32) -> f32 {
    let x0 = x.floor() as i32;
    let z0 = z.floor() as i32;
    let fx = smoothstep(x - x0 as f32);
    let fz = smoothstep(z - z0 as f32);
    let h00 = hash_signed_cell(x0, z0, salt);
    let h10 = hash_signed_cell(x0 + 1, z0, salt);
    let h01 = hash_signed_cell(x0, z0 + 1, salt);
    let h11 = hash_signed_cell(x0 + 1, z0 + 1, salt);
    let a = h00 + (h10 - h00) * fx;
    let b = h01 + (h11 - h01) * fx;
    a + (b - a) * fz
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn hash_signed_cell(x: i32, z: i32, salt: u32) -> f32 {
    let x = x as u32;
    let z = z as u32;
    hash01(x, z, salt) * 2.0 - 1.0
}

fn tile_hash(x: u32, z: u32, salt: u32) -> u32 {
    let mut n = x.wrapping_mul(0x9E37_79B9) ^ z.wrapping_mul(0x85EB_CA6B) ^ salt;
    n ^= n >> 16;
    n = n.wrapping_mul(0x7FEB_352D);
    n ^= n >> 15;
    n = n.wrapping_mul(0x846C_A68B);
    n ^ (n >> 16)
}

fn draw_stair(ui: &mut Ui<'_>, rect: Rect, dir: Option<MinimapStairDir>, cell: f32) {
    let col = Color::rgba(0.88, 0.82, 0.62, 0.58);
    let pad = (cell * 0.22).max(1.0);
    let mid_x = rect.x() + rect.width() * 0.5;
    let mid_y = rect.y() + rect.height() * 0.5;
    match dir.unwrap_or(MinimapStairDir::PosZ) {
        MinimapStairDir::PosX | MinimapStairDir::NegX => {
            ui.draw_line(
                Pos2::new(rect.x() + pad, mid_y),
                Pos2::new(rect.max.x - pad, mid_y),
                1.0,
                col,
            );
            ui.draw_line(
                Pos2::new(rect.x() + pad, rect.y() + pad),
                Pos2::new(rect.x() + pad, rect.max.y - pad),
                1.0,
                col,
            );
            ui.draw_line(
                Pos2::new(rect.max.x - pad, rect.y() + pad),
                Pos2::new(rect.max.x - pad, rect.max.y - pad),
                1.0,
                col,
            );
        }
        MinimapStairDir::PosZ | MinimapStairDir::NegZ => {
            ui.draw_line(
                Pos2::new(mid_x, rect.y() + pad),
                Pos2::new(mid_x, rect.max.y - pad),
                1.0,
                col,
            );
            ui.draw_line(
                Pos2::new(rect.x() + pad, rect.y() + pad),
                Pos2::new(rect.max.x - pad, rect.y() + pad),
                1.0,
                col,
            );
            ui.draw_line(
                Pos2::new(rect.x() + pad, rect.max.y - pad),
                Pos2::new(rect.max.x - pad, rect.max.y - pad),
                1.0,
                col,
            );
        }
    }
}

fn draw_prop_marker(ui: &mut Ui<'_>, mx: f32, my: f32, cell: f32, kind: MinimapPropKind) {
    match kind {
        MinimapPropKind::Chest => draw_chest_marker(ui, mx, my, (cell * 2.15).max(6.5)),
        MinimapPropKind::Light => draw_light_marker(ui, mx, my, (cell * 1.75).max(5.0)),
        MinimapPropKind::LargeSolid => draw_solid_marker(ui, mx, my, (cell * 1.55).max(4.0), 0.46),
        MinimapPropKind::SmallSolid => draw_solid_marker(ui, mx, my, (cell * 1.10).max(3.0), 0.32),
        MinimapPropKind::Decoration => {}
    }
}

fn draw_chest_marker(ui: &mut Ui<'_>, mx: f32, my: f32, size: f32) {
    let glow = size * 2.25;
    ui.draw_rounded_rect(
        Rect::from_xywh(mx - glow * 0.5, my - glow * 0.5, glow, glow),
        glow * 0.5,
        Color::rgba(1.0, 0.62, 0.16, 0.16),
    );
    let body = Rect::from_xywh(mx - size * 0.5, my - size * 0.38, size, size * 0.76);
    ui.draw_rounded_rect(
        body,
        (size * 0.16).max(1.0),
        Color::rgba(0.98, 0.66, 0.22, 0.96),
    );
    ui.draw_rect(
        Rect::from_xywh(
            body.x(),
            my - size * 0.05,
            body.width(),
            (size * 0.12).max(1.0),
        ),
        Color::rgba(0.38, 0.19, 0.07, 0.72),
    );
    ui.draw_rect(
        Rect::from_xywh(
            mx - size * 0.10,
            body.y(),
            (size * 0.20).max(1.0),
            body.height(),
        ),
        Color::rgba(1.0, 0.84, 0.42, 0.86),
    );
}

fn draw_light_marker(ui: &mut Ui<'_>, mx: f32, my: f32, size: f32) {
    let glow = size * 2.8;
    ui.draw_rounded_rect(
        Rect::from_xywh(mx - glow * 0.5, my - glow * 0.5, glow, glow),
        glow * 0.5,
        Color::rgba(1.0, 0.58, 0.20, 0.18),
    );
    ui.draw_rounded_rect(
        Rect::from_xywh(mx - size * 0.5, my - size * 0.5, size, size),
        size * 0.5,
        Color::rgba(1.0, 0.76, 0.28, 0.78),
    );
    ui.draw_rounded_rect(
        Rect::from_xywh(mx - size * 0.18, my - size * 0.18, size * 0.36, size * 0.36),
        size * 0.18,
        Color::rgba(1.0, 0.96, 0.62, 0.95),
    );
}

fn draw_portal_marker(ui: &mut Ui<'_>, mx: f32, my: f32, size: f32) {
    let halo = size * 2.5;
    ui.draw_rounded_rect(
        Rect::from_xywh(mx - halo * 0.5, my - halo * 0.5, halo, halo),
        halo * 0.5,
        Color::rgba(0.22, 0.70, 1.0, 0.22),
    );
    let outer = Rect::from_xywh(mx - size * 0.5, my - size * 0.5, size, size);
    ui.draw_rounded_outline(outer, size * 0.5, 2.0, Color::rgba(0.46, 0.88, 1.0, 0.96));
    let inner = size * 0.44;
    ui.draw_rounded_rect(
        Rect::from_xywh(mx - inner * 0.5, my - inner * 0.5, inner, inner),
        inner * 0.5,
        Color::rgba(0.78, 0.96, 1.0, 0.92),
    );
}

fn draw_boss_marker(ui: &mut Ui<'_>, mx: f32, my: f32, size: f32) {
    let halo = size * 2.25;
    ui.draw_rounded_rect(
        Rect::from_xywh(mx - halo * 0.5, my - halo * 0.5, halo, halo),
        halo * 0.5,
        Color::rgba(1.0, 0.28, 0.06, 0.26),
    );
    let half = size * 0.5;
    ui.draw_line(
        Pos2::new(mx, my - half),
        Pos2::new(mx + half, my),
        2.0,
        Color::rgba(1.0, 0.66, 0.18, 0.96),
    );
    ui.draw_line(
        Pos2::new(mx + half, my),
        Pos2::new(mx, my + half),
        2.0,
        Color::rgba(1.0, 0.66, 0.18, 0.96),
    );
    ui.draw_line(
        Pos2::new(mx, my + half),
        Pos2::new(mx - half, my),
        2.0,
        Color::rgba(1.0, 0.66, 0.18, 0.96),
    );
    ui.draw_line(
        Pos2::new(mx - half, my),
        Pos2::new(mx, my - half),
        2.0,
        Color::rgba(1.0, 0.66, 0.18, 0.96),
    );
    ui.draw_rounded_rect(
        Rect::from_xywh(mx - size * 0.18, my - size * 0.18, size * 0.36, size * 0.36),
        size * 0.18,
        Color::rgba(1.0, 0.86, 0.32, 0.98),
    );
}

fn draw_solid_marker(ui: &mut Ui<'_>, mx: f32, my: f32, size: f32, alpha: f32) {
    ui.draw_rounded_rect(
        Rect::from_xywh(mx - size * 0.5, my - size * 0.5, size, size),
        (size * 0.20).max(1.0),
        Color::rgba(0.04, 0.035, 0.030, alpha),
    );
    ui.draw_rounded_outline(
        Rect::from_xywh(mx - size * 0.5, my - size * 0.5, size, size),
        (size * 0.20).max(1.0),
        1.0,
        Color::rgba(0.18, 0.15, 0.11, alpha * 0.8),
    );
}

/// Soft-haloed pip: low-alpha halo behind the opaque core.
fn draw_pip(ui: &mut Ui<'_>, mx: f32, my: f32, core_size: f32, core_col: Color, halo_col: Color) {
    let halo = core_size * 2.4;
    ui.draw_rounded_rect(
        Rect::from_xywh(mx - halo * 0.5, my - halo * 0.5, halo, halo),
        halo * 0.5,
        halo_col,
    );
    ui.draw_rounded_rect(
        Rect::from_xywh(
            mx - core_size * 0.5,
            my - core_size * 0.5,
            core_size,
            core_size,
        ),
        core_size * 0.5,
        core_col,
    );
}

fn draw_player_marker(
    ui: &mut Ui<'_>,
    mx: f32,
    my: f32,
    cell: f32,
    facing: (f32, f32),
    fill: Color,
    halo: Color,
) {
    let len_sq = facing.0 * facing.0 + facing.1 * facing.1;
    let (dir_x, dir_y) = if len_sq > 1e-4 {
        let inv = 1.0 / len_sq.sqrt();
        (facing.0 * inv, facing.1 * inv)
    } else {
        (0.0, 1.0)
    };
    let side_x = -dir_y;
    let side_y = dir_x;
    let size = (cell * 2.55).clamp(10.0, 18.0);
    let outline = (size * 0.24).max(2.6);
    let inner = (outline - 1.45).max(1.15);

    let tip = Pos2::new(mx + dir_x * size * 0.62, my + dir_y * size * 0.62);
    let neck = Pos2::new(mx + dir_x * size * 0.06, my + dir_y * size * 0.06);
    let left = Pos2::new(
        mx - dir_x * size * 0.20 + side_x * size * 0.43,
        my - dir_y * size * 0.20 + side_y * size * 0.43,
    );
    let right = Pos2::new(
        mx - dir_x * size * 0.20 - side_x * size * 0.43,
        my - dir_y * size * 0.20 - side_y * size * 0.43,
    );
    let base = Pos2::new(mx - dir_x * size * 0.52, my - dir_y * size * 0.52);
    let diamond_tip = Pos2::new(base.x + dir_x * size * 0.30, base.y + dir_y * size * 0.30);
    let diamond_back = Pos2::new(base.x - dir_x * size * 0.30, base.y - dir_y * size * 0.30);
    let diamond_left = Pos2::new(base.x + side_x * size * 0.31, base.y + side_y * size * 0.31);
    let diamond_right = Pos2::new(base.x - side_x * size * 0.31, base.y - side_y * size * 0.31);

    let halo_size = size * 1.45;
    ui.draw_rounded_rect(
        Rect::from_xywh(
            mx - halo_size * 0.5,
            my - halo_size * 0.5,
            halo_size,
            halo_size,
        ),
        halo_size * 0.5,
        halo,
    );

    let outline_col = Color::rgba(0.0, 0.0, 0.0, 0.95);
    for thickness in [outline, inner] {
        let color = if thickness == outline {
            outline_col
        } else {
            fill
        };
        ui.draw_line(tip, left, thickness, color);
        ui.draw_line(tip, right, thickness, color);
        ui.draw_line(tip, neck, thickness, color);
        ui.draw_line(neck, base, thickness, color);
        ui.draw_line(diamond_tip, diamond_left, thickness, color);
        ui.draw_line(diamond_left, diamond_back, thickness, color);
        ui.draw_line(diamond_back, diamond_right, thickness, color);
        ui.draw_line(diamond_right, diamond_tip, thickness, color);
    }
}
