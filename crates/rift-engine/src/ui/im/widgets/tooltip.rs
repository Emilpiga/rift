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
    /// Optional rect the tooltip should be anchored adjacent
    /// to (e.g. the hovered inventory slot). When present,
    /// [`Self::show`] tries the right side of `anchor_to`
    /// first and flips to the left if the right side would
    /// overflow the screen — matching the "tooltip never
    /// occludes the thing you're hovering" rule.
    anchor_to: Option<Rect>,
}

impl<'a> Default for Tooltip<'a> {
    fn default() -> Self {
        Self {
            header: None,
            header_color: Color::rgba(0.55, 0.65, 0.78, 1.0),
            fill: Color::rgba(0.02, 0.03, 0.05, 0.96),
            pad: 8.0,
            min_width: 180.0,
            anchor_to: None,
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

    /// Anchor this tooltip adjacent to `r`. The renderer
    /// prefers the right side of `r`; if the tooltip wouldn't
    /// fit there, it flips to the left side. Vertically the
    /// tooltip aligns to `r.top()` and clamps to the screen.
    /// When set, the `pos` argument to [`Self::show`] is
    /// ignored.
    pub fn anchor_to(mut self, r: Rect) -> Self {
        self.anchor_to = Some(r);
        self
    }

    /// Render the tooltip with its top-left at `pos` and
    /// return the actual rect drawn (after screen clamping)
    /// so callers can lay siblings next to it.
    pub fn show(self, ui: &mut Ui<'_>, pos: Pos2, lines: &[TooltipLine<'_>]) -> Rect {
        let theme = *ui.theme();
        // Apply the active UI scale to the tooltip chrome so
        // a 1.5\u00d7 theme doesn't draw 8px padding around 24px
        // text (and a 0.6\u00d7 theme doesn't waste a 180px floor
        // on 9px text). `theme.scale` is already baked at
        // `Ui::begin`; we just multiply through.
        let pad = self.pad * theme.scale;
        let min_width = self.min_width * theme.scale;
        let gap_anchor = 6.0 * theme.scale;
        let gap_screen = 4.0 * theme.scale;

        // Per-line measurement so we don't have to assume a
        // fixed glyph width.
        let mut max_w = self
            .header
            .map(|h| ui.measure_text(h, theme.fonts.size_sm))
            .unwrap_or(0.0);
        let mut body_h = 0.0_f32;
        for ln in lines {
            max_w = max_w.max(ui.measure_text(ln.text, ln.size));
            body_h += ln.size + 2.0 * theme.scale;
        }
        let header_h = if self.header.is_some() {
            theme.fonts.size_sm + 4.0 * theme.scale
        } else {
            0.0
        };
        let w = (max_w + pad * 2.0).max(min_width);
        let h = body_h + header_h + pad * 2.0;

        let screen = ui.screen_size();
        // Anchor-aware positioning. If the caller supplied a
        // `Rect` to anchor against, pick the side that fits
        // (right-of-rect first, fall back to left-of-rect)
        // and align vertically to its top. Otherwise use the
        // raw `pos` the caller passed in.
        let (mut x, mut y) = if let Some(anchor) = self.anchor_to {
            let right_x = anchor.max.x + gap_anchor;
            let left_x = anchor.min.x - gap_anchor - w;
            let chosen_x = if right_x + w <= screen.x - gap_screen {
                right_x
            } else if left_x >= gap_screen {
                left_x
            } else {
                // Neither side fits — fall back to plain
                // screen-clamping below the cursor's anchor.
                right_x
            };
            (chosen_x, anchor.min.y)
        } else {
            (pos.x, pos.y)
        };
        // Final hard clamp to the screen rect so a too-wide
        // tooltip never bleeds past the edge.
        x = x.min(screen.x - w - gap_screen).max(gap_screen);
        y = y.min(screen.y - h - gap_screen).max(gap_screen);
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

            let mut cursor_y = rect.y() + pad;
            if let Some(h) = self.header {
                ui.draw_text(
                    Pos2::new(rect.x() + pad, cursor_y),
                    h,
                    theme.fonts.size_sm,
                    self.header_color,
                );
                cursor_y += theme.fonts.size_sm + 4.0 * theme.scale;
            }
            for ln in lines {
                ui.draw_text(
                    Pos2::new(rect.x() + pad, cursor_y),
                    ln.text,
                    ln.size,
                    ln.color,
                );
                cursor_y += ln.size + 2.0 * theme.scale;
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
    t.show(ui, Pos2::new(mp.x + 18.0 * ui.theme().scale, mp.y), lines)
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
