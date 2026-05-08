//! Read-only character sheet panel rendered to the right of
//! the bag. Mirrors the inventory chrome and shows the
//! resolved stats from [`PlayerState`].

use rift_engine::ui::im::{Color, Frame, Pad, Pos2, Rect, Ui};

use crate::game::PlayerState;

use super::layout::Layout;

/// Render the resolved character sheet (level, class, name +
/// every CharacterStats field) in a panel that mirrors the
/// inventory chrome. Read-only — no interaction.
pub fn render_stats_panel(
    ui: &mut Ui<'_>,
    rect: Rect,
    ps: &PlayerState,
    layout: &Layout,
) {
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
