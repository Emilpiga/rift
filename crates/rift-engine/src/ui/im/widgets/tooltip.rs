//! Multi-line tooltip rendered on the [`Layer::Tooltip`]
//! sort layer so it always sits above panels and drag ghosts.
//!
//! Two flavours are exposed:
//!
//! * [`Tooltip::show`] — builder for arbitrary line lists,
//!   useful when a caller already has the lines in hand
//!   (item tooltips, ability tooltips).
//! * [`tooltip_at_mouse`] — convenience wrapper that anchors
//!   to the cursor and clamps within the screen.
//!
//! All tooltip rendering is automatic-position-clamped so the
//! tip never escapes the screen on the right or bottom edges,
//! which means callers can pass `mouse_pos + offset` blindly.

use super::super::{
    color::Color, layer::Layer, theme::Theme, ui::Ui,
};
use crate::ui::im::rect::{Pos2, Rect};

/// One row of a tooltip. Width measurement is automatic.
pub struct TooltipLine<'a> {
    pub text: &'a str,
    pub size: f32,
    pub color: Color,
}

impl<'a> TooltipLine<'a> {
    pub fn new(text: &'a str, size: f32, color: Color) -> Self {
        Self { text, size, color }
    }
}

/// Builder. Construct, configure, then [`Tooltip::show`].
pub struct Tooltip<'a> {
    header: Option<&'a str>,
    header_color: Color,
    fill: Color,
    pad: f32,
    /// Minimum total width — handy when callers want adjacent
    /// tooltips (compare panes) to line up visually.
    min_width: f32,
}

impl<'a> Default for Tooltip<'a> {
    fn default() -> Self {
        Self {
            header: None,
            header_color: Color::rgba(0.55, 0.65, 0.78, 1.0),
            fill: Color::rgba(0.02, 0.03, 0.05, 0.96),
            pad: 8.0,
            min_width: 180.0,
        }
    }
}

impl<'a> Tooltip<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn header(mut self, header: &'a str) -> Self {
        self.header = Some(header);
        self
    }

    pub fn header_color(mut self, c: Color) -> Self {
        self.header_color = c;
        self
    }

    pub fn fill(mut self, c: Color) -> Self {
        self.fill = c;
        self
    }

    pub fn pad(mut self, p: f32) -> Self {
        self.pad = p;
        self
    }

    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = w;
        self
    }

    /// Render the tooltip with its top-left at `pos` and
    /// return the actual rect drawn (after screen clamping)
    /// so callers can lay siblings next to it.
    pub fn show(self, ui: &mut Ui<'_>, pos: Pos2, lines: &[TooltipLine<'_>]) -> Rect {
        let theme = *ui.theme();
        // Per-line measurement so we don't have to assume a
        // fixed glyph width.
        let mut max_w = self
            .header
            .map(|h| ui.measure_text(h, theme.fonts.size_sm))
            .unwrap_or(0.0);
        let mut body_h = 0.0_f32;
        for ln in lines {
            max_w = max_w.max(ui.measure_text(ln.text, ln.size));
            body_h += ln.size + 2.0;
        }
        let header_h = if self.header.is_some() {
            theme.fonts.size_sm + 4.0
        } else {
            0.0
        };
        let w = (max_w + self.pad * 2.0).max(self.min_width);
        let h = body_h + header_h + self.pad * 2.0;

        let screen = ui.screen_size();
        let x = pos.x.min(screen.x - w - 4.0).max(0.0);
        let y = pos.y.min(screen.y - h - 4.0).max(0.0);
        let rect = Rect::from_xywh(x, y, w, h);

        // Always render on the tooltip layer so callers don't
        // have to worry about z-order with the surrounding panel.
        ui.with_layer(Layer::Tooltip, |ui| {
            ui.draw_rounded_rect(rect, theme.spacing.corner_radius, self.fill);
            ui.draw_rounded_outline(
                rect,
                theme.spacing.corner_radius,
                theme.spacing.border_thickness,
                theme.colors.border,
            );

            let mut cursor_y = rect.y() + self.pad;
            if let Some(h) = self.header {
                ui.draw_text(
                    Pos2::new(rect.x() + self.pad, cursor_y),
                    h,
                    theme.fonts.size_sm,
                    self.header_color,
                );
                cursor_y += theme.fonts.size_sm + 4.0;
            }
            for ln in lines {
                ui.draw_text(
                    Pos2::new(rect.x() + self.pad, cursor_y),
                    ln.text,
                    ln.size,
                    ln.color,
                );
                cursor_y += ln.size + 2.0;
            }
        });

        rect
    }
}

/// Convenience wrapper that anchors the tooltip just to the
/// right of the cursor, with screen clamping.
pub fn tooltip_at_mouse(
    ui: &mut Ui<'_>,
    header: Option<&str>,
    lines: &[TooltipLine<'_>],
) -> Rect {
    let mp = ui.mouse_pos();
    let mut t = Tooltip::new();
    if let Some(h) = header {
        t = t.header(h);
    }
    t.show(ui, Pos2::new(mp.x + 18.0, mp.y), lines)
}

/// Convenience used by the inventory: build a list of lines
/// out of an item's `tooltip()` `Vec<String>` plus a rarity
/// color for the first (name) line.
pub fn item_tooltip_lines<'a>(
    lines_in: &'a [String],
    rarity_color: [f32; 3],
    theme: &Theme,
) -> Vec<TooltipLine<'a>> {
    let rarity = Color::rgba(rarity_color[0], rarity_color[1], rarity_color[2], 1.0);
    lines_in
        .iter()
        .enumerate()
        .map(|(i, s)| TooltipLine {
            text: s.as_str(),
            size: if i == 0 {
                theme.fonts.size_md
            } else {
                theme.fonts.size_sm
            },
            color: if i == 0 { rarity } else { theme.colors.text },
        })
        .collect()
}
