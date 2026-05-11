//! Collapsible character-sheet subsection inside the
//! inventory drawer.

use rift_ui_im::{Color, Pos2, Rect, Ui};
use rift_ui_types::inventory::StatsView;

pub fn render_stats_panel(ui: &mut Ui<'_>, rect: Rect, view: &StatsView<'_>, fit: f32) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let theme = *ui.theme();

    // No internal background — the outer drawer paints stone
    // chrome. Title sits at the top with a divider, mirroring
    // the inventory header.
    let _ = view.name;
    let _ = view.level;
    let _ = view.class_name;
    let body = rect;

    ui.draw_text(
        Pos2::new(body.x(), body.y()),
        "STATS",
        theme.fonts.size_lg,
        theme.colors.text,
    );
    let div_y = body.y() + theme.fonts.size_lg + 6.0 * fit;
    ui.draw_rect(
        Rect::from_xywh(body.x(), div_y, body.width(), 1.0),
        theme.colors.border_stone,
    );

    let mut y = div_y + 10.0 * fit;
    let text_size = theme.fonts.size_md;
    let row_h = text_size + 6.0 * fit;
    let header_col = Color::rgba(0.95, 0.85, 0.55, 1.0);

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
            y += row_h;
        }
        y += 6.0 * fit;
    }
}
