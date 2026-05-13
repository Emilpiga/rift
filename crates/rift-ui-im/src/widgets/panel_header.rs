//! Shared forged panel header chrome.

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
    let edge = Color::rgba(0.060, 0.044, 0.030, 0.96);
    let centre = Color::rgba(0.190, 0.116, 0.052, 0.96);
    let mid = Color::rgba(0.250, 0.165, 0.070, 0.96);
    let left = Rect::from_xywh(rect.x(), rect.y(), rect.width() * 0.5, rect.height());
    let right = Rect::from_xywh(
        rect.x() + rect.width() * 0.5,
        rect.y(),
        rect.width() * 0.5,
        rect.height(),
    );
    ui.draw_grad4_rect(left, edge, mid, edge, centre);
    ui.draw_grad4_rect(right, mid, edge, centre, edge);
    ui.draw_gradient_rect(
        Rect::from_xywh(
            rect.x() + 1.0,
            rect.y() + 1.0,
            (rect.width() - 2.0).max(0.0),
            rect.height() * 0.42,
        ),
        Color::rgba(1.0, 0.86, 0.52, 0.15),
        Color::rgba(1.0, 0.86, 0.52, 0.0),
    );
    ui.draw_outline(rect, 1.0, Color::rgba(0.88, 0.62, 0.28, 0.74));
    ui.draw_outline(
        Rect::from_xywh(
            rect.x() + 1.0,
            rect.y() + 1.0,
            (rect.width() - 2.0).max(0.0),
            (rect.height() - 2.0).max(0.0),
        ),
        1.0,
        Color::rgba(1.0, 0.92, 0.72, 0.10),
    );

    draw_header_rule(ui, rect, scale);
    draw_header_studs(ui, rect, scale);
    draw_top_brackets(ui, rect, scale);

    let title_size = header.font_size.unwrap_or_else(|| {
        if rect.height() >= 36.0 * scale {
            theme.fonts.size_lg
        } else {
            theme.fonts.size_md
        }
    });
    let title_color = header
        .title_color
        .unwrap_or(Color::rgba(0.98, 0.86, 0.58, 1.0));
    let text_x = rect.x() + 14.0 * scale;
    let title_y = if header.subtitle.is_some() {
        rect.y() + 6.0 * scale
    } else {
        rect.y() + (rect.height() - title_size) * 0.5 - 1.0 * scale
    };
    draw_shadowed_text(ui, Pos2::new(text_x, title_y), header.title, title_size, title_color);

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
        let right_w = ui.measure_text(right_text, right_size);
        let right_x = rect.max.x - right_w - 14.0 * scale;
        let right_y = rect.y() + (rect.height() - right_size) * 0.5 - 1.0 * scale;
        draw_shadowed_text(
            ui,
            Pos2::new(right_x.max(text_x), right_y),
            right_text,
            right_size,
            Color::rgba(0.96, 0.84, 0.52, 1.0),
        );
    }
}

fn draw_header_rule(ui: &mut Ui<'_>, rect: Rect, scale: f32) {
    let y = rect.max.y - 2.0 * scale;
    let clear = Color::rgba(0.78, 0.52, 0.20, 0.0);
    let bronze = Color::rgba(0.86, 0.58, 0.25, 0.82);
    let mid_x = rect.x() + rect.width() * 0.5;
    ui.draw_grad4_rect(
        Rect::from_xywh(rect.x() + 7.0 * scale, y, mid_x - rect.x() - 7.0 * scale, 1.0),
        clear,
        bronze,
        clear,
        bronze,
    );
    ui.draw_grad4_rect(
        Rect::from_xywh(mid_x, y, rect.max.x - mid_x - 7.0 * scale, 1.0),
        bronze,
        clear,
        bronze,
        clear,
    );
}

fn draw_header_studs(ui: &mut Ui<'_>, rect: Rect, scale: f32) {
    let stud = 3.0 * scale;
    let y = rect.y() + (rect.height() - stud) * 0.5;
    for x in [rect.x() + 7.0 * scale, rect.max.x - 7.0 * scale - stud] {
        let r = Rect::from_xywh(x, y, stud, stud);
        ui.draw_rect(r, Color::rgba(0.98, 0.70, 0.30, 0.70));
        ui.draw_outline(r, 1.0, Color::rgba(0.20, 0.10, 0.04, 0.76));
    }
}

fn draw_top_brackets(ui: &mut Ui<'_>, rect: Rect, scale: f32) {
    let w = 10.0 * scale;
    let h = 8.0 * scale;
    let col = Color::rgba(0.94, 0.66, 0.28, 0.58);
    ui.draw_line(
        Pos2::new(rect.x() + 2.0 * scale, rect.y() + 2.0 * scale),
        Pos2::new(rect.x() + w, rect.y() + 2.0 * scale),
        1.0,
        col,
    );
    ui.draw_line(
        Pos2::new(rect.x() + 2.0 * scale, rect.y() + 2.0 * scale),
        Pos2::new(rect.x() + 2.0 * scale, rect.y() + h),
        1.0,
        col,
    );
    ui.draw_line(
        Pos2::new(rect.max.x - 2.0 * scale, rect.y() + 2.0 * scale),
        Pos2::new(rect.max.x - w, rect.y() + 2.0 * scale),
        1.0,
        col,
    );
    ui.draw_line(
        Pos2::new(rect.max.x - 2.0 * scale, rect.y() + 2.0 * scale),
        Pos2::new(rect.max.x - 2.0 * scale, rect.y() + h),
        1.0,
        col,
    );
}

fn draw_shadowed_text(ui: &mut Ui<'_>, pos: Pos2, text: &str, size: f32, color: Color) {
    ui.draw_text(
        Pos2::new(pos.x + 1.0, pos.y + 1.0),
        text,
        size,
        Color::rgba(0.0, 0.0, 0.0, 0.68),
    );
    ui.draw_text(pos, text, size, color);
}