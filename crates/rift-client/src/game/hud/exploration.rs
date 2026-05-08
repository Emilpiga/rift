//! Non-combat HUD: minimap, interaction prompts, descend tooltip.
//! Everything in this module renders in screen space and is
//! agnostic of the active rift state — it just draws what the
//! caller passes in.

use glam::Vec3;
use rift_dungeon::NavGrid;
use rift_engine::ecs::components::{Boss, Enemy, Health, LocalPlayer, Player, Transform};
use rift_engine::ui::im::{Banner, Color, Pos2, Rect, Ui};

/// Soft-haloed minimap pip: draws a low-alpha halo rounded-rect
/// then the opaque core rounded-rect on top.
fn draw_pip(
    ui: &mut Ui<'_>,
    mx: f32,
    my: f32,
    core_size: f32,
    core_col: Color,
    halo_col: Color,
) {
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

/// Top-right minimap. Shows walkable tiles, the player (white pip with a
/// short heading fan), nearby enemies (red), the boss (orange), and the
/// active rift / hub portal (cyan).
pub fn render_minimap(
    ui: &mut Ui<'_>,
    world: &hecs::World,
    nav: &NavGrid,
    player_facing: Vec3,
    portal_pos: Option<Vec3>,
) {
    use rift_engine::ui::im::{Frame, Pad};

    const MAP_PX_BASE: f32 = 224.0;
    const HEADER_H_BASE: f32 = 18.0;
    const INSET_BASE: f32 = 6.0;
    const MARGIN_BASE: f32 = 14.0;
    const RADIUS_BASE: f32 = 6.0;

    let theme = *ui.theme();
    let s = theme.scale;
    let map_px = MAP_PX_BASE * s;
    let header_h = HEADER_H_BASE * s;
    let inset = INSET_BASE * s;
    let margin = MARGIN_BASE * s;
    let radius = RADIUS_BASE * s;
    let screen = ui.screen_size();
    let sw = screen.x;

    let map_x = sw - map_px - margin;
    let map_y = margin;
    let panel_rect = Rect::from_xywh(map_x, map_y, map_px, map_px);

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

        let cell = (inner_rect.width().min(inner_rect.height())
            / nav.width.max(nav.depth) as f32)
            .max(1.0);
        let map_w = cell * nav.width as f32;
        let map_h = cell * nav.depth as f32;
        let inner_x = inner_rect.x() + (inner_rect.width() - map_w) * 0.5;
        let inner_y = inner_rect.y() + (inner_rect.height() - map_h) * 0.5;

        let floor_a = Color::rgba(0.42, 0.36, 0.30, 0.95);
        let floor_b = Color::rgba(0.36, 0.30, 0.25, 0.95);
        for z in 0..nav.depth {
            for x in 0..nav.width {
                if nav.is_walkable(x, z) {
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

        let to_map = |p: Vec3| -> (f32, f32) { (inner_x + p.x * cell, inner_y + p.z * cell) };
        let in_inner = |mx: f32, my: f32| -> bool {
            mx >= inner_rect.x()
                && mx <= inner_rect.max.x
                && my >= inner_rect.y()
                && my <= inner_rect.max.y
        };

        if let Some(p) = portal_pos {
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

        for (_id, (t, _e, boss, _)) in world
            .query::<(&Transform, &Enemy, Option<&Boss>, Option<&Health>)>()
            .iter()
        {
            let (mx, my) = to_map(t.position);
            if !in_inner(mx, my) {
                continue;
            }
            if boss.is_some() {
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

        if let Some((pp, _)) = world
            .query::<(&Transform, &Player, &LocalPlayer)>()
            .iter()
            .map(|(_, (t, p, _))| (t.position, p.aim_dir))
            .next()
        {
            let (mx, my) = to_map(pp);
            if in_inner(mx, my) {
                let f = Vec3::new(player_facing.x, 0.0, player_facing.z);
                if f.length_squared() > 1e-4 {
                    let f = f.normalize();
                    let len = (cell * 4.5).max(8.0);
                    let dx = f.x * len;
                    let dz = f.z * len;
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
    });
}

/// Generic interaction prompt centred just below mid-screen, used by
/// the rift / hub portals. `text` is the message body (e.g.
/// "PRESS [F] TO ENTER THE RIFT").
pub fn render_hud_prompt(ui: &mut Ui<'_>, text: &str) {
    let theme = *ui.theme();
    let s = theme.scale;
    Banner::new(text)
        .text_size(12.0 * s)
        .text_color(Color::rgba(0.55, 0.78, 1.0, 1.0))
        .fill(Color::rgba(0.05, 0.08, 0.14, 0.92))
        .y_factor(0.62)
        .show(ui);
}

/// Difficulty step-up tooltip drawn just above the descend
/// F-prompt. Computes the deltas between the current floor's
/// `FloorConfig` and the next floor's so the player can read
/// what they're walking into before pressing F.
pub fn render_descend_tooltip(ui: &mut Ui<'_>, current_floor: u32) {
    use rift_dungeon::FloorConfig;
    use rift_engine::ui::im::{Frame, Pad, Vec2};

    if current_floor == 0 {
        return;
    }
    let next = current_floor + 1;
    let cur_cfg = FloorConfig::for_floor(current_floor);
    let next_cfg = FloorConfig::for_floor(next);

    let title = format!("DESCEND TO FLOOR {next}");
    let cur_count = cur_cfg.enemy_count();
    let next_count = next_cfg.enemy_count();
    let count_pct = if cur_count > 0 {
        ((next_count as f32 / cur_count as f32) - 1.0) * 100.0
    } else {
        0.0
    };
    let hp_pct = (next_cfg.enemy_health / cur_cfg.enemy_health - 1.0) * 100.0;
    let dmg_pct = (next_cfg.enemy_damage_mult / cur_cfg.enemy_damage_mult - 1.0) * 100.0;
    let speed_pct = (next_cfg.enemy_speed / cur_cfg.enemy_speed - 1.0) * 100.0;

    let lines: [(&str, String); 4] = [
        (
            "Enemies",
            format!("{} \u{2192} {}  (+{:.0}%)", cur_count, next_count, count_pct),
        ),
        (
            "Enemy HP",
            format!(
                "{:.0} \u{2192} {:.0}  (+{:.0}%)",
                cur_cfg.enemy_health, next_cfg.enemy_health, hp_pct
            ),
        ),
        (
            "Enemy DMG",
            format!(
                "{:.2}\u{00d7} \u{2192} {:.2}\u{00d7}  (+{:.0}%)",
                cur_cfg.enemy_damage_mult, next_cfg.enemy_damage_mult, dmg_pct
            ),
        ),
        (
            "Enemy speed",
            format!(
                "{:.1} \u{2192} {:.1}  (+{:.0}%)",
                cur_cfg.enemy_speed, next_cfg.enemy_speed, speed_pct
            ),
        ),
    ];

    let theme = *ui.theme();
    let screen = ui.screen_size();
    let s = theme.scale;
    let title_size = 13.0 * s;
    let row_size = 11.0 * s;
    let row_gap = 3.0 * s;
    let key_w_max = lines
        .iter()
        .map(|(k, _)| ui.measure_text(k, row_size))
        .fold(0.0_f32, f32::max);
    let val_w_max = lines
        .iter()
        .map(|(_, v)| ui.measure_text(v, row_size))
        .fold(0.0_f32, f32::max);
    let col_gap = 18.0 * s;
    let inner_w = ui
        .measure_text(&title, title_size)
        .max(key_w_max + col_gap + val_w_max);
    let inner_h = title_size
        + 6.0 * s
        + (lines.len() as f32) * (row_size + row_gap)
        - row_gap;
    let pad = Pad::symmetric(18.0 * s, 8.0 * s);
    let outer_w = inner_w + pad.left + pad.right;
    let outer_h = inner_h + pad.top + pad.bottom;
    let portal_prompt_y = screen.y * 0.62;
    let rect = Rect::from_xywh(
        (screen.x - outer_w) / 2.0,
        portal_prompt_y - outer_h - 8.0 * s,
        outer_w,
        outer_h,
    );
    let frame = Frame::panel(&theme)
        .with_fill(Color::rgba(0.06, 0.04, 0.10, 0.92))
        .with_padding(pad);
    frame.show(ui, rect, |ui, body| {
        let title_w = ui.measure_text(&title, title_size);
        ui.draw_text(
            Pos2::new(body.x() + (inner_w - title_w) * 0.5, body.y()),
            &title,
            title_size,
            Color::rgba(0.95, 0.75, 0.55, 1.0),
        );
        let mut row_y = body.y() + title_size + 6.0 * s;
        for (key, val) in &lines {
            ui.draw_text(
                Pos2::new(body.x(), row_y),
                key,
                row_size,
                Color::rgba(0.65, 0.72, 0.82, 1.0),
            );
            let val_w = ui.measure_text(val, row_size);
            ui.draw_text(
                Pos2::new(body.x() + inner_w - val_w, row_y),
                val,
                row_size,
                Color::rgba(0.95, 0.55, 0.45, 1.0),
            );
            row_y += row_size + row_gap;
        }
        let _ = Vec2::ZERO;
    });
}
