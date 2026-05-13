//! Collapsible character-sheet subsection inside the
//! inventory drawer.

use rift_ui_im::widgets::tooltip::{tooltip_at_mouse, TooltipLine};
use rift_ui_im::{Color, Pos2, Rect, Ui};
use rift_ui_types::inventory::StatsView;

pub fn render_stats_panel(ui: &mut Ui<'_>, rect: Rect, view: &StatsView<'_>, fit: f32) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let theme = *ui.theme();

    // No internal background — the outer drawer paints stone
    // chrome and the caller paints the shared panel header.
    let _ = view.name;
    let _ = view.level;
    let _ = view.class_name;
    let body = rect;

    let mut y = body.y();
    let text_size = theme.fonts.size_md;
    let row_h = text_size + 6.0 * fit;
    let header_col = Color::rgba(0.95, 0.85, 0.55, 1.0);
    let mp = ui.mouse_pos();
    // Tooltip string + anchor label captured during the
    // section pass so we can defer rendering until after the
    // panel paints — keeps tooltip layer ordering correct.
    let mut hovered_tip: Option<(&str, &str)> = None;

    for section in view.sections {
        if y + row_h > body.max.y {
            break;
        }
        ui.draw_text(
            Pos2::new(body.x(), y),
            section.header,
            text_size,
            header_col,
        );
        y += row_h;

        for row in section.rows {
            if y + row_h > body.max.y {
                break;
            }
            let value_color = row
                .value_color
                .map(|[r, g, b, a]| Color::rgba(r, g, b, a))
                .unwrap_or(theme.colors.text);
            let vw = ui.measure_text(row.value, text_size);
            let gap = 8.0_f32 * fit;
            let label_max = (body.width() - vw - gap).max(0.0);
            ui.draw_text_ellipsized(
                Pos2::new(body.x(), y),
                row.label,
                text_size,
                label_max,
                theme.colors.text_dim,
            );
            ui.draw_text(
                Pos2::new(body.max.x - vw, y),
                row.value,
                text_size,
                value_color,
            );
            // Row hit-rect spans the panel width so hovering
            // anywhere on the line — label or value — surfaces
            // the explanation.
            let row_rect = Rect::from_xywh(body.x(), y, body.width(), text_size);
            if let Some(tip) = row.tooltip {
                if row_rect.contains(mp) {
                    hovered_tip = Some((row.label, tip));
                }
            }
            y += row_h;
        }
        y += 6.0 * fit;
    }

    if let Some((label, tip)) = hovered_tip {
        let lines = [
            TooltipLine::new(label, theme.fonts.size_md, theme.colors.text),
            TooltipLine::new(tip, theme.fonts.size_sm, theme.colors.text_dim),
        ];
        tooltip_at_mouse(ui, None, &lines);
    }
}
