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

use super::super::{color::Color, layer::Layer, theme::Theme, ui::Ui};
use crate::rect::{Pos2, Rect};

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

    /// `true` if this line should render as a gold horizontal
    /// rule rather than text. Detected by leading `─` (the box-
    /// drawing char the item-tooltip builder uses as a section
    /// separator). Keeps the API surface flat — callers just
    /// push divider lines like any other.
    fn is_divider(&self) -> bool {
        let t = self.text.trim();
        !t.is_empty() && t.chars().all(|c| c == '\u{2500}')
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
    /// When `anchor_to` is set, flip the side preference so
    /// the tooltip is placed to the LEFT of the anchor first.
    /// Used by the inventory's "equipped" compare panel so it
    /// chains leftward off the "hovered" tooltip instead of
    /// overlapping it.
    prefer_left: bool,
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
            prefer_left: false,
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

    /// Flip the [`Self::anchor_to`] side preference so the
    /// tooltip is placed to the LEFT of the anchor first
    /// (falling back to the right if it doesn't fit). Used
    /// by chained tooltips (compare panes) that need to
    /// stack leftward instead of overlapping their sibling.
    pub fn prefer_left(mut self, yes: bool) -> Self {
        self.prefer_left = yes;
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
        // Height of a rendered divider line (replaces text).
        let divider_h = 6.0 * theme.scale;
        // Extra vertical breathing room around each divider
        // so affix groups read as distinct sections.
        let divider_margin = 4.0 * theme.scale;

        // Per-line measurement so we don't have to assume a
        // fixed glyph width. Dividers contribute no width but
        // get a slightly taller slot than a normal line.
        let mut max_w = self
            .header
            .map(|h| ui.measure_text(h, theme.fonts.size_sm))
            .unwrap_or(0.0);
        let mut body_h = 0.0_f32;
        for ln in lines {
            if ln.is_divider() {
                body_h += divider_h + divider_margin * 2.0;
            } else {
                max_w = max_w.max(ui.measure_text(ln.text, ln.size));
                body_h += ln.size + 2.0 * theme.scale;
            }
        }
        let header_h = if self.header.is_some() {
            theme.fonts.size_sm + 4.0 * theme.scale
        } else {
            0.0
        };
        let w = (max_w + pad * 2.0).max(min_width);
        let h = body_h + header_h + pad * 2.0;

        let screen = ui.screen_size();
        // Anchor-aware positioning. With `anchor_to`, pick the
        // preferred side first (configurable via
        // `prefer_left`) and fall back to the other side if
        // the first doesn't fit. Otherwise use the raw `pos`.
        let (mut x, mut y) = if let Some(anchor) = self.anchor_to {
            let right_x = anchor.max.x + gap_anchor;
            let left_x = anchor.min.x - gap_anchor - w;
            let chosen_x = if self.prefer_left {
                if left_x >= gap_screen {
                    left_x
                } else if right_x + w <= screen.x - gap_screen {
                    right_x
                } else {
                    left_x.max(gap_screen)
                }
            } else if right_x + w <= screen.x - gap_screen {
                right_x
            } else if left_x >= gap_screen {
                left_x
            } else {
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
            // Carved-stone background — same noisy radial
            // gradient as `Frame::stone`, rendered inline so
            // we control the corner radius (0 — flush rect
            // reads as engraved plaque) and stroke ourselves.
            let f = theme.colors.bg_stone.0;
            let centre = Color::rgba(
                (f[0] * 1.18 + 0.04).min(1.0),
                (f[1] * 1.18 + 0.04).min(1.0),
                (f[2] * 1.18 + 0.04).min(1.0),
                f[3].max(0.96),
            );
            let edge = Color::rgba(f[0] * 0.55, f[1] * 0.55, f[2] * 0.55, f[3].max(0.96));
            ui.draw_rounded_radial_rect_noisy(rect, 0.0, edge, centre);

            // Heavy outer + inset hairline — the doubled rule
            // sells the carved-stone bevel that single-color
            // borders can't.
            ui.draw_rounded_outline(rect, 0.0, 2.0, theme.colors.border_stone);
            let inset = Rect::from_xywh(
                rect.x() + 2.0,
                rect.y() + 2.0,
                (rect.width() - 4.0).max(0.0),
                (rect.height() - 4.0).max(0.0),
            );
            ui.draw_rounded_outline(inset, 0.0, 1.0, Color::rgba(1.0, 0.92, 0.84, 0.12));

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
                if ln.is_divider() {
                    // Gold separator — same honey tint used by
                    // the inventory's cell outlines so the two
                    // surfaces feel like one set. Two stacked
                    // hairlines (gold + soft highlight) read
                    // as etched metal.
                    cursor_y += divider_margin;
                    let mid_y = cursor_y + divider_h * 0.5 - 1.0;
                    let line_l = rect.x() + pad;
                    let line_r = rect.max.x - pad;
                    let gold = Color::rgba(0.78, 0.62, 0.30, 0.85);
                    let hi = Color::rgba(1.0, 0.95, 0.82, 0.18);
                    ui.draw_rect(Rect::from_xywh(line_l, mid_y, line_r - line_l, 1.0), gold);
                    ui.draw_rect(
                        Rect::from_xywh(line_l, mid_y + 1.0, line_r - line_l, 1.0),
                        hi,
                    );
                    cursor_y += divider_h + divider_margin;
                } else {
                    // Detect a trailing roll-quality token like
                    // `… +5 Intellect  [42%]`. The item-tooltip
                    // builder appends it with two leading spaces;
                    // when present we split the line so the
                    // `[NN%]` chunk renders at a dimmed alpha —
                    // information, not a primary stat read.
                    let (head, tail) = match ln.text.rsplit_once("  [") {
                        Some((h, t)) if t.ends_with("%]") => (h, format!("  [{}", t)),
                        _ => (ln.text, String::new()),
                    };
                    ui.draw_text(Pos2::new(rect.x() + pad, cursor_y), head, ln.size, ln.color);
                    if !tail.is_empty() {
                        let head_w = ui.measure_text(head, ln.size);
                        let [r, g, b, _] = ln.color.0;
                        // Mute saturation toward neutral and drop
                        // alpha so the bracketed roll-quality
                        // reads as secondary metadata next to
                        // the main stat text.
                        let dim_r = r * 0.55 + 0.18;
                        let dim_g = g * 0.55 + 0.18;
                        let dim_b = b * 0.55 + 0.18;
                        let dim = Color::rgba(dim_r, dim_g, dim_b, 0.55);
                        ui.draw_text(
                            Pos2::new(rect.x() + pad + head_w, cursor_y),
                            &tail,
                            ln.size,
                            dim,
                        );
                    }
                    cursor_y += ln.size + 2.0 * theme.scale;
                }
            }
        });

        rect
    }
}

/// Convenience wrapper that anchors the tooltip just to the
/// right of the cursor, with screen clamping.
pub fn tooltip_at_mouse(ui: &mut Ui<'_>, header: Option<&str>, lines: &[TooltipLine<'_>]) -> Rect {
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
