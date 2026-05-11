//! Non-combat HUD: minimap, interaction prompts, descend tooltip.
//! Everything in this module renders in screen space and is
//! agnostic of the active rift state — it just draws what the
//! caller passes in.

use glam::Vec3;
use rift_dungeon::NavGrid;
use rift_engine::ecs::components::{Boss, Enemy, LocalPlayer, Player, Transform};
use rift_engine::ui::im::{Banner, Color, Pos2, Rect, Ui};
use rift_ui_types::hud::{MinimapEnemy, MinimapPlayer, MinimapView};

/// Top-right minimap. Walks the hecs world to build a flat
/// [`MinimapView`], then delegates to the pure widget in
/// [`rift_ui::hud::frame_minimap`]. All visual layout +
/// drawing lives there; this shim is host glue only.
pub fn render_minimap(
    ui: &mut Ui<'_>,
    world: &hecs::World,
    nav: &NavGrid,
    player_facing: Vec3,
    portal_pos: Option<Vec3>,
) {
    // Walkable mask, row-major.
    let mut walkable = Vec::with_capacity(nav.width * nav.depth);
    for z in 0..nav.depth {
        for x in 0..nav.width {
            walkable.push(nav.is_walkable(x, z));
        }
    }

    // Enemy pips — separate non-boss / boss so the widget can
    // size them independently.
    let mut enemies: Vec<MinimapEnemy> = Vec::new();
    for (_id, (t, _e, boss)) in world
        .query::<(&Transform, &Enemy, Option<&Boss>)>()
        .iter()
    {
        enemies.push(MinimapEnemy {
            pos: (t.position.x, t.position.z),
            is_boss: boss.is_some(),
        });
    }

    // Local player + facing flattened to 2D nav-grid space.
    let player = world
        .query::<(&Transform, &Player, &LocalPlayer)>()
        .iter()
        .map(|(_, (t, _, _))| MinimapPlayer {
            pos: (t.position.x, t.position.z),
            facing: (player_facing.x, player_facing.z),
        })
        .next();

    let view = MinimapView {
        grid_width: nav.width as u32,
        grid_depth: nav.depth as u32,
        walkable: &walkable,
        portal: portal_pos.map(|p| (p.x, p.z)),
        enemies: &enemies,
        player,
    };
    rift_ui::hud::frame_minimap(ui, &view);
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
