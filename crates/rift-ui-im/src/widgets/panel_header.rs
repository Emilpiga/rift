//! Shared panel header chrome — void glass bar aligned with [`Theme::DARK`].

use super::super::color::Color;
use super::super::rect::{Pos2, Rect};
use super::super::theme::Theme;
use super::super::ui::Ui;

#[derive(Clone, Copy, Debug)]
pub struct PanelHeader<'a> {
    title: &'a str,
    subtitle: Option<&'a str>,
    right_text: Option<&'a str>,
    title_color: Option<Color>,
    subtitle_color: Option<Color>,
    font_size: Option<f32>,
}

impl<'a> PanelHeader<'a> {
    pub fn new(title: &'a str) -> Self {
        Self {
            title,
            subtitle: None,
            right_text: None,
            title_color: None,
            subtitle_color: None,
            font_size: None,
        }
    }

    pub fn subtitle(mut self, subtitle: &'a str) -> Self {
        self.subtitle = Some(subtitle);
        self
    }

    pub fn right_text(mut self, text: &'a str) -> Self {
        self.right_text = Some(text);
        self
    }

    pub fn title_color(mut self, color: Color) -> Self {
        self.title_color = Some(color);
        self
    }

    pub fn subtitle_color(mut self, color: Color) -> Self {
        self.subtitle_color = Some(color);
        self
    }

    pub fn font_size(mut self, size: f32) -> Self {
        self.font_size = Some(size);
        self
    }

    pub fn show(self, ui: &mut Ui<'_>, rect: Rect) {
        let theme = *ui.theme();
        draw_panel_header(ui, rect, self, &theme);
    }
}

fn draw_panel_header(ui: &mut Ui<'_>, rect: Rect, header: PanelHeader<'_>, theme: &Theme) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }

    let scale = theme.scale;
    let s = theme.colors.bg_stone.0;
    // Cool violet slab — slightly lifted centre, deeper corners.
    let edge = Color::rgba(s[0] * 0.38, s[1] * 0.34, s[2] * 0.52, 0.96);
    let centre = Color::rgba(
        (s[0] * 1.12).min(1.0),
        (s[1] * 1.08).min(1.0),
        (s[2] * 1.22).min(1.0),
        0.94,
    );
    let mid = Color::rgba(
        (s[0] * 0.78).min(1.0),
        (s[1] * 0.72).min(1.0),
        (s[2] * 0.95).min(1.0),
        0.95,
    );
    let left = Rect::from_xywh(rect.x(), rect.y(), rect.width() * 0.5, rect.height());
    let right = Rect::from_xywh(
        rect.x() + rect.width() * 0.5,
        rect.y(),
        rect.width() * 0.5,
        rect.height(),
    );
    ui.draw_grad4_rect(left, edge, mid, edge, centre);
    ui.draw_grad4_rect(right, mid, edge, centre, edge);

    let sheen_top = Color::rgba(0.78, 0.74, 1.0, 0.14);
    let sheen_bot = Color::rgba(0.78, 0.74, 1.0, 0.0);
    ui.draw_gradient_rect(
        Rect::from_xywh(
            rect.x() + 1.0,
            rect.y() + 1.0,
            (rect.width() - 2.0).max(0.0),
            rect.height() * 0.40,
        ),
        sheen_top,
        sheen_bot,
    );

    let b = theme.colors.border.0;
    ui.draw_outline(
        rect,
        1.0,
        Color::rgba(b[0], b[1], b[2], (b[3] * 1.15).min(1.0)),
    );
    let bs = theme.colors.border_strong.0;
    ui.draw_outline(
        Rect::from_xywh(
            rect.x() + 1.0,
            rect.y() + 1.0,
            (rect.width() - 2.0).max(0.0),
            (rect.height() - 2.0).max(0.0),
        ),
        1.0,
        Color::rgba(bs[0], bs[1], bs[2], 0.11),
    );

    draw_header_rule(ui, rect, scale, theme);
    draw_glass_corners(ui, rect, scale, theme);

    let title_size = header.font_size.unwrap_or_else(|| {
        if rect.height() >= 36.0 * scale {
            theme.fonts.size_lg
        } else {
            theme.fonts.size_md
        }
    });
    let title_color = header.title_color.unwrap_or(theme.colors.text);
    let text_x = rect.x() + 17.0 * scale;
    let title_y = if header.subtitle.is_some() {
        rect.y() + 6.0 * scale
    } else {
        rect.y() + (rect.height() - title_size) * 0.5 - 1.0 * scale
    };
    draw_shadowed_text(
        ui,
        Pos2::new(text_x, title_y),
        header.title,
        title_size,
        title_color,
    );

    if let Some(subtitle) = header.subtitle {
        let subtitle_size = theme.fonts.size_sm;
        let subtitle_color = header.subtitle_color.unwrap_or(theme.colors.text_dim);
        draw_shadowed_text(
            ui,
            Pos2::new(text_x, title_y + title_size + 3.0 * scale),
            subtitle,
            subtitle_size,
            subtitle_color,
        );
    }

    if let Some(right_text) = header.right_text {
        let right_size = theme.fonts.size_md;
        let right_w = ui.measure_header_text(right_text, right_size);
        let right_x = rect.max.x - right_w - 14.0 * scale;
        let right_y = rect.y() + (rect.height() - right_size) * 0.5 - 1.0 * scale;
        draw_shadowed_text(
            ui,
            Pos2::new(right_x.max(text_x), right_y),
            right_text,
            right_size,
            theme.colors.accent,
        );
    }
}

fn draw_header_rule(ui: &mut Ui<'_>, rect: Rect, scale: f32, theme: &Theme) {
    let y = rect.max.y - 2.0 * scale;
    let clear = Color::rgba(
        theme.colors.accent.0[0],
        theme.colors.accent.0[1],
        theme.colors.accent.0[2],
        0.0,
    );
    let line = Color::rgba(
        theme.colors.accent.0[0],
        theme.colors.accent.0[1],
        theme.colors.accent.0[2],
        0.45,
    );
    let mid_x = rect.x() + rect.width() * 0.5;
    ui.draw_grad4_rect(
        Rect::from_xywh(
            rect.x() + 7.0 * scale,
            y,
            mid_x - rect.x() - 7.0 * scale,
            1.0,
        ),
        clear,
        line,
        clear,
        line,
    );
    ui.draw_grad4_rect(
        Rect::from_xywh(mid_x, y, rect.max.x - mid_x - 7.0 * scale, 1.0),
        line,
        clear,
        line,
        clear,
    );
}

/// Minimal corner ticks — thin violet glass highlights, no rivets.
fn draw_glass_corners(ui: &mut Ui<'_>, rect: Rect, scale: f32, theme: &Theme) {
    let w = 9.0 * scale;
    let h = 7.0 * scale;
    let c = Color::rgba(
        theme.colors.accent.0[0],
        theme.colors.accent.0[1],
        theme.colors.accent.0[2],
        0.42,
    );
    ui.draw_line(
        Pos2::new(rect.x() + 2.0 * scale, rect.y() + 2.0 * scale),
        Pos2::new(rect.x() + w, rect.y() + 2.0 * scale),
        1.0,
        c,
    );
    ui.draw_line(
        Pos2::new(rect.x() + 2.0 * scale, rect.y() + 2.0 * scale),
        Pos2::new(rect.x() + 2.0 * scale, rect.y() + h),
        1.0,
        c,
    );
    ui.draw_line(
        Pos2::new(rect.max.x - 2.0 * scale, rect.y() + 2.0 * scale),
        Pos2::new(rect.max.x - w, rect.y() + 2.0 * scale),
        1.0,
        c,
    );
    ui.draw_line(
        Pos2::new(rect.max.x - 2.0 * scale, rect.y() + 2.0 * scale),
        Pos2::new(rect.max.x - 2.0 * scale, rect.y() + h),
        1.0,
        c,
    );
}

fn draw_shadowed_text(ui: &mut Ui<'_>, pos: Pos2, text: &str, size: f32, color: Color) {
    ui.draw_header_text(
        Pos2::new(pos.x + 1.0, pos.y + 1.0),
        text,
        size,
        Color::rgba(0.0, 0.0, 0.0, 0.55),
    );
    ui.draw_header_text(pos, text, size, color);
}
