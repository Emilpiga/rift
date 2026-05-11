//! Top-right minimap widget.
//!
//! Reads a flat [`MinimapView`] (walkable mask + pip lists) and
//! draws the framed mini-floorplan, vignette, player heading
//! fan, and enemy / portal pips. The widget is fully agnostic
//! of `hecs` / `rift_dungeon` — the host walks the world and
//! produces the view each frame.

use rift_ui_im::{Color, Frame, Pad, Pos2, Rect, Ui};
use rift_ui_types::hud::MinimapView;

const MAP_PX_BASE: f32 = 224.0;
const HEADER_H_BASE: f32 = 18.0;
const INSET_BASE: f32 = 6.0;
const MARGIN_BASE: f32 = 14.0;
const RADIUS_BASE: f32 = 6.0;

/// Render the minimap anchored to the top-right corner. Returns
/// the outer panel rect so callers can stack siblings (buff
/// strips, floor indicator) directly underneath.
pub fn frame_minimap(ui: &mut Ui<'_>, view: &MinimapView<'_>) -> Rect {
    let theme = *ui.theme();
    let s = theme.scale;
    let map_px = MAP_PX_BASE * s;
    let header_h = HEADER_H_BASE * s;
    let inset = INSET_BASE * s;
    let margin = MARGIN_BASE * s;
    let radius = RADIUS_BASE * s;
    let screen = ui.screen_size();

    let map_x = screen.x - map_px - margin;
    let map_y = margin;
    let panel_rect = Rect::from_xywh(map_x, map_y, map_px, map_px);

    // Drop shadow underneath the panel.
    ui.draw_rounded_rect(
        Rect::from_xywh(map_x + 2.0 * s, map_y + 3.0 * s, map_px, map_px),
        radius + 1.0 * s,
        Color::rgba(0.0, 0.0, 0.0, 0.32),
    );

    let frame = Frame::panel(&theme)
        .with_fill(Color::rgba(0.04, 0.05, 0.07, 0.94))
        .with_radius(radius)
        .with_padding(Pad::all(0.0));
    frame.show(ui, panel_rect, |ui, body| {
        draw_header(ui, body, header_h, s, &theme);
        draw_floor_and_pips(ui, body, header_h, inset, radius, s, view);
    });

    panel_rect
}

fn draw_header(ui: &mut Ui<'_>, body: Rect, header_h: f32, s: f32, theme: &rift_ui_im::Theme) {
    let header = Rect::from_xywh(body.x(), body.y(), body.width(), header_h);
    ui.draw_rect(
        Rect::from_xywh(header.x(), header.y(), header.width(), header.height()),
        Color::rgba(0.07, 0.09, 0.12, 1.0),
    );
    ui.draw_rect(
        Rect::from_xywh(header.x(), header.max.y - 1.0, header.width(), 1.0),
        Color::rgba(0.16, 0.18, 0.24, 1.0),
    );
    ui.draw_text(
        Pos2::new(header.x() + 8.0 * s, header.y() + 4.0 * s),
        "MAP",
        theme.fonts.size_sm,
        theme.colors.text_dim,
    );
    let n_w = ui.measure_text("N", theme.fonts.size_sm);
    let n_x = header.max.x - n_w - 12.0 * s;
    ui.draw_rect(
        Rect::from_xywh(n_x - 5.0 * s, header.y() + 6.0 * s, 3.0 * s, 6.0 * s),
        Color::rgba(0.55, 0.78, 1.0, 0.65),
    );
    ui.draw_text(
        Pos2::new(n_x, header.y() + 4.0 * s),
        "N",
        theme.fonts.size_sm,
        Color::rgba(0.85, 0.92, 1.0, 0.95),
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
        Color::rgba(0.025, 0.028, 0.035, 1.0),
    );

    let grid_max = view.grid_width.max(view.grid_depth) as f32;
    if grid_max < 1.0 {
        return;
    }
    let cell = (inner_rect.width().min(inner_rect.height()) / grid_max).max(1.0);
    let map_w = cell * view.grid_width as f32;
    let map_h = cell * view.grid_depth as f32;
    let inner_x = inner_rect.x() + (inner_rect.width() - map_w) * 0.5;
    let inner_y = inner_rect.y() + (inner_rect.height() - map_h) * 0.5;

    // Floor tiles (checker pattern over the walkable mask).
    let floor_a = Color::rgba(0.42, 0.36, 0.30, 0.95);
    let floor_b = Color::rgba(0.36, 0.30, 0.25, 0.95);
    let stride = view.grid_width as usize;
    for z in 0..view.grid_depth {
        for x in 0..view.grid_width {
            let idx = z as usize * stride + x as usize;
            if view.walkable.get(idx).copied().unwrap_or(false) {
                let col = if (x ^ z) & 1 == 0 { floor_a } else { floor_b };
                ui.draw_rect(
                    Rect::from_xywh(
                        inner_x + x as f32 * cell,
                        inner_y + z as f32 * cell,
                        cell,
                        cell,
                    ),
                    col,
                );
            }
        }
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
    let to_map = |p: (f32, f32)| -> (f32, f32) { (inner_x + p.0 * cell, inner_y + p.1 * cell) };
    let in_inner = |mx: f32, my: f32| -> bool {
        mx >= inner_rect.x()
            && mx <= inner_rect.max.x
            && my >= inner_rect.y()
            && my <= inner_rect.max.y
    };

    // Portal pip first so enemies / player layer over it.
    if let Some(p) = view.portal {
        let (mx, my) = to_map(p);
        if in_inner(mx, my) {
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

    for enemy in view.enemies {
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
