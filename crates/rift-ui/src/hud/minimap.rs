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
const RADIUS_BASE: f32 = 8.0;
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
    let radius = RADIUS_BASE * s;
    let screen = ui.screen_size();

    let map_x = screen.x - map_px - margin;
    let map_y = margin;
    let panel_rect = Rect::from_xywh(map_x, map_y, map_px, map_px);

    let frame = Frame::stone(&theme)
        .with_radius(radius)
        .with_padding(Pad::all(0.0));
    frame.show(ui, panel_rect, |ui, body| {
        draw_header(ui, body, header_h, s, &theme, view);
        draw_floor_and_pips(ui, body, header_h, inset, radius, s, view);
    });

    panel_rect
}

fn draw_header(
    ui: &mut Ui<'_>,
    body: Rect,
    header_h: f32,
    s: f32,
    theme: &rift_ui_im::Theme,
    view: &MinimapView<'_>,
) {
    let header = Rect::from_xywh(body.x(), body.y(), body.width(), header_h);
    PanelHeader::new(view.zone_title)
        .subtitle(view.zone_detail)
        .font_size(theme.fonts.size_md)
        .show(ui, header);
    let n_w = ui.measure_text("N", theme.fonts.size_sm);
    let n_x = header.max.x - n_w - 12.0 * s;
    ui.draw_rect(
        Rect::from_xywh(n_x - 5.0 * s, header.y() + 6.0 * s, 3.0 * s, 6.0 * s),
        Color::rgba(0.86, 0.66, 0.32, 0.82),
    );
    ui.draw_text(
        Pos2::new(n_x, header.y() + 7.0 * s),
        "N",
        theme.fonts.size_sm,
        Color::rgba(0.94, 0.84, 0.58, 0.95),
    );
}

fn draw_floor_and_pips(
    ui: &mut Ui<'_>,
    body: Rect,
    header_h: f32,
    inset: f32,
    radius: f32,
    s: f32,
    view: &MinimapView<'_>,
) {
    let inner_rect = Rect::from_xywh(
        body.x() + inset,
        body.y() + header_h + inset,
        body.width() - inset * 2.0,
        body.height() - header_h - inset * 2.0,
    );

    ui.draw_rounded_rect(
        inner_rect,
        (radius - 2.0 * s).max(1.0),
        Color::rgba(0.010, 0.011, 0.014, 1.0),
    );
    ui.draw_rounded_radial_rect_noisy(
        inner_rect,
        (radius - 2.0 * s).max(1.0),
        Color::rgba(0.010, 0.010, 0.013, 1.0),
        Color::rgba(0.060, 0.052, 0.044, 0.62),
    );

    let grid_max = view.grid_width.max(view.grid_depth) as f32;
    if grid_max < 1.0 {
        return;
    }
    let stride = view.grid_width as usize;
    let rich_cells = view.cells.len() == stride * view.grid_depth as usize;
    let view_tiles = if rich_cells {
        VIEW_TILES_BASE
    } else {
        grid_max
    };
    let cell = (inner_rect.width().min(inner_rect.height()) / view_tiles).max(1.0);
    let (focus_x, focus_z) = view
        .focus
        .unwrap_or((view.grid_width as f32 * 0.5, view.grid_depth as f32 * 0.5));
    let origin_x = if rich_cells {
        (focus_x - view_tiles * 0.5)
            .clamp(0.0, (view.grid_width as f32 - view_tiles).max(0.0))
            .floor()
    } else {
        0.0
    };
    let origin_z = if rich_cells {
        (focus_z - view_tiles * 0.5)
            .clamp(0.0, (view.grid_depth as f32 - view_tiles).max(0.0))
            .floor()
    } else {
        0.0
    };
    let inner_x = inner_rect.x() - origin_x * cell;
    let inner_y = inner_rect.y() - origin_z * cell;
    if rich_cells {
        draw_rich_floor(ui, view, inner_rect, inner_x, inner_y, cell, stride);
    } else {
        draw_walkable_floor(ui, view, inner_rect, inner_x, inner_y, cell, stride);
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

    ui.draw_rounded_outline(
        inner_rect,
        (radius - 2.0 * s).max(1.0),
        1.0,
        Color::rgba(0.18, 0.20, 0.26, 1.0),
    );

    // Mapping helpers — closures borrow `inner_x`/`inner_y`/`cell`.
    let to_map = |p: (f32, f32)| -> (f32, f32) {
        (inner_x + (p.0 + 0.5) * cell, inner_y + (p.1 + 0.5) * cell)
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
            let pip_size = (cell * 2.4).max(5.0);
            draw_pip(
                ui,
                mx,
                my,
                pip_size,
                Color::rgba(0.45, 0.85, 1.0, 1.0),
                Color::rgba(0.30, 0.75, 1.0, 0.35),
            );
        }
    }

    for member in view.party {
        let (mx, my) = to_map(member.pos);
        if !in_inner(mx, my) {
            continue;
        }
        let pip_size = (cell * 2.1).max(4.5);
        draw_pip(
            ui,
            mx,
            my,
            pip_size,
            Color::rgba(0.22, 0.86, 1.0, 1.0),
            Color::rgba(0.20, 0.76, 1.0, 0.42),
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
            let pip_size = (cell * 2.6).max(5.0);
            draw_pip(
                ui,
                mx,
                my,
                pip_size,
                Color::rgba(1.00, 0.60, 0.10, 1.0),
                Color::rgba(1.00, 0.55, 0.10, 0.40),
            );
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
            // Heading fan — 5 fading pips along the facing
            // vector, only drawn when the host passed a
            // non-zero direction.
            let len_sq = p.facing.0 * p.facing.0 + p.facing.1 * p.facing.1;
            if len_sq > 1e-4 {
                let len = len_sq.sqrt();
                let fx = p.facing.0 / len;
                let fz = p.facing.1 / len;
                let fan_len = (cell * 4.5).max(8.0);
                let dx = fx * fan_len;
                let dz = fz * fan_len;
                const STEPS: i32 = 5;
                for i in 1..=STEPS {
                    let t = i as f32 / STEPS as f32;
                    let size = (3.2 * (1.0 - t * 0.6)).max(1.4);
                    let alpha = (1.0 - t) * 0.85 + 0.15;
                    ui.draw_rounded_rect(
                        Rect::from_xywh(
                            mx + dx * t - size * 0.5,
                            my + dz * t - size * 0.5,
                            size,
                            size,
                        ),
                        size * 0.5,
                        Color::rgba(0.95, 0.97, 1.0, alpha),
                    );
                }
            }
            let pip_size = (cell * 2.0).max(4.5);
            draw_pip(
                ui,
                mx,
                my,
                pip_size,
                Color::rgba(0.98, 0.99, 1.0, 1.0),
                Color::rgba(0.55, 0.78, 1.0, 0.45),
            );
        }
    }
}

fn draw_walkable_floor(
    ui: &mut Ui<'_>,
    view: &MinimapView<'_>,
    clip: Rect,
    inner_x: f32,
    inner_y: f32,
    cell: f32,
    stride: usize,
) {
    let floor_a = Color::rgba(0.42, 0.36, 0.30, 0.95);
    let floor_b = Color::rgba(0.36, 0.30, 0.25, 0.95);
    for z in 0..view.grid_depth {
        for x in 0..view.grid_width {
            let idx = z as usize * stride + x as usize;
            if view.walkable.get(idx).copied().unwrap_or(false) {
                let rect = tile_rect(inner_x, inner_y, cell, x, z);
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
    cell: f32,
    stride: usize,
) {
    for z in 0..view.grid_depth {
        for x in 0..view.grid_width {
            let idx = z as usize * stride + x as usize;
            let Some(c) = view.cells.get(idx) else {
                continue;
            };
            if c.kind == MinimapTileKind::Wall {
                continue;
            }
            let rect = tile_rect(inner_x, inner_y, cell, x, z);
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
            let rect = tile_rect(inner_x, inner_y, cell, x, z);
            if !rect_intersects(rect, clip) {
                continue;
            }
            draw_unexplored_fog_frontier(ui, view, stride, x as i32, z as i32, rect);
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
            let rect = tile_rect(inner_x, inner_y, cell, x, z);
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

fn draw_unexplored_fog_frontier(
    ui: &mut Ui<'_>,
    view: &MinimapView<'_>,
    stride: usize,
    x: i32,
    z: i32,
    rect: Rect,
) {
    let haze = |explored: f32| {
        let alpha = 0.42 * explored.clamp(0.0, 1.0).powf(1.35);
        Color::rgba(0.030, 0.034, 0.044, alpha)
    };
    let tl = haze(smoothed_corner_explored(view, stride, x, z, -1, -1));
    let tr = haze(smoothed_corner_explored(view, stride, x, z, 1, -1));
    let bl = haze(smoothed_corner_explored(view, stride, x, z, -1, 1));
    let br = haze(smoothed_corner_explored(view, stride, x, z, 1, 1));
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
            let weight = smoothstep(1.0 - dist / 2.55);
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

fn tile_rect(inner_x: f32, inner_y: f32, cell: f32, x: u32, z: u32) -> Rect {
    Rect::from_xywh(
        inner_x + x as f32 * cell,
        inner_y + z as f32 * cell,
        cell,
        cell,
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
    let (size, col) = match kind {
        MinimapPropKind::Chest => ((cell * 2.0).max(5.0), Color::rgba(0.95, 0.72, 0.25, 0.95)),
        MinimapPropKind::Light => ((cell * 1.25).max(3.0), Color::rgba(1.0, 0.72, 0.28, 0.72)),
        MinimapPropKind::LargeSolid => {
            ((cell * 1.45).max(3.5), Color::rgba(0.12, 0.10, 0.08, 0.55))
        }
        MinimapPropKind::SmallSolid => {
            ((cell * 1.0).max(2.5), Color::rgba(0.10, 0.085, 0.07, 0.42))
        }
        MinimapPropKind::Decoration => return,
    };
    ui.draw_rounded_rect(
        Rect::from_xywh(mx - size * 0.5, my - size * 0.5, size, size),
        (size * 0.28).max(1.0),
        col,
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
