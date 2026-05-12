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

/// Per-line decoration. Most lines are plain `Text`; the rest
/// instruct [`Tooltip::show`] to paint a specific chrome
/// element (divider rule, legendary banner edge, …).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum TooltipLineDecor {
    /// Ordinary text line.
    #[default]
    Text,
    /// Gold horizontal rule. Text is ignored.
    Divider,
    /// Top edge of a legendary banner — gold gradient
    /// (transparent → opaque → transparent) rule and marks the
    /// start of a darker inset backdrop drawn behind every
    /// `BannerBody` line that follows until [`BannerEdgeBottom`].
    BannerEdgeTop,
    /// Bottom edge of a legendary banner. Same gradient rule;
    /// closes the inset backdrop.
    BannerEdgeBottom,
    /// Text line that lives between [`BannerEdgeTop`] and
    /// [`BannerEdgeBottom`]. The renderer paints a darker
    /// translucent fill behind these (with a horizontal
    /// 0 → full → 0 alpha mask so the plate fades into the
    /// surrounding stone background).
    BannerBody,
}

/// One row of a tooltip. Width measurement is automatic.
pub struct TooltipLine<'a> {
    pub text: &'a str,
    pub size: f32,
    pub color: Color,
    pub decor: TooltipLineDecor,
    /// When `Some`, the renderer splits the line text at the
    /// last `"  "` (two spaces) and draws the tail as a filled
    /// rounded pill ("badge") with this colour as the fill —
    /// used by item tooltips to surface roll-quality tiers
    /// (`▴ Fine`, `▴▴▴ Perfect`, …) as a visual chip rather
    /// than trailing text.
    pub badge: Option<Color>,
}

impl<'a> TooltipLine<'a> {
    pub fn new(text: &'a str, size: f32, color: Color) -> Self {
        Self {
            text,
            size,
            color,
            decor: TooltipLineDecor::Text,
            badge: None,
        }
    }

    /// Builder: set the per-line decoration. See
    /// [`TooltipLineDecor`].
    pub fn decor(mut self, d: TooltipLineDecor) -> Self {
        self.decor = d;
        self
    }

    /// Builder: tag this line with a trailing badge. See
    /// [`Self::badge`].
    pub fn badge(mut self, fill: Color) -> Self {
        self.badge = Some(fill);
        self
    }

    /// `true` if this line should render as a gold horizontal
    /// rule rather than text. Honours both the explicit
    /// [`TooltipLineDecor::Divider`] tag and the legacy `─`
    /// character-detection so callers that pre-date the decor
    /// field keep working.
    fn is_divider(&self) -> bool {
        if matches!(self.decor, TooltipLineDecor::Divider) {
            return true;
        }
        let t = self.text.trim();
        !t.is_empty() && t.chars().all(|c| c == '\u{2500}')
    }

    fn is_banner_edge_top(&self) -> bool {
        matches!(self.decor, TooltipLineDecor::BannerEdgeTop)
    }
    fn is_banner_edge_bottom(&self) -> bool {
        matches!(self.decor, TooltipLineDecor::BannerEdgeBottom)
    }
    #[allow(dead_code)]
    fn is_banner_body(&self) -> bool {
        matches!(self.decor, TooltipLineDecor::BannerBody)
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
        // Gradient gold rule slot used for legendary banner
        // edges — a touch taller than a regular divider so the
        // gradient reads, and with a bit more margin so the
        // banner reads as its own framed box.
        let banner_edge_h = 4.0 * theme.scale;
        let banner_edge_margin = 5.0 * theme.scale;
        // Inner horizontal padding inside a legendary banner so
        // the dark inset backdrop hugs the text instead of
        // running edge to edge — sells the "inset plate" look.
        let banner_inset = 4.0 * theme.scale;
        // Single uniform gap appended after every text row so
        // the whole tooltip has a consistent vertical rhythm.
        // Kept small (1px @ scale 1.0) — anything larger and
        // affix groups stop reading as a single block.
        let line_gap = 1.0 * theme.scale;

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
            } else if ln.is_banner_edge_top() || ln.is_banner_edge_bottom() {
                body_h += banner_edge_h + banner_edge_margin * 2.0;
            } else {
                // Width budget: head text + (optional) trailing
                // badge pill side-by-side. The renderer right-
                // aligns the badge against the tooltip edge, so
                // we measure both and reserve enough room for
                // them to coexist without overlapping the head.
                let (head, badge_text) = match (ln.badge, ln.text.rsplit_once("  ")) {
                    (Some(_), Some((h, t))) => {
                        let t = strip_band_glyph(t);
                        if t.is_empty() {
                            (ln.text, None)
                        } else {
                            (h.trim_end(), Some(t))
                        }
                    }
                    _ => (ln.text, None),
                };
                let head_w = ui.measure_text(head, ln.size);
                let badge_w = badge_text
                    .map(|t| {
                        let bf = ln.size * 0.82;
                        ui.measure_text(t, bf) + 4.0 * theme.scale * 2.0
                    })
                    .unwrap_or(0.0);
                let gap = if badge_w > 0.0 {
                    8.0 * theme.scale
                } else {
                    0.0
                };
                max_w = max_w.max(head_w + gap + badge_w);
                body_h += ln.size + line_gap;
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

            // Pre-scan to locate any legendary banner spans so
            // we can paint the dark inset backdrop **before**
            // the line texts go down on top of it. Tracks
            // matched (top_y, bot_y) pairs; unmatched edges are
            // tolerated (renderer never panics on malformed
            // input — worst case the backdrop is skipped).
            let mut banner_spans: Vec<(f32, f32)> = Vec::new();
            {
                let mut probe_y = cursor_y;
                let mut open_top: Option<f32> = None;
                for ln in lines {
                    if ln.is_divider() {
                        probe_y += divider_margin + divider_h + divider_margin;
                    } else if ln.is_banner_edge_top() {
                        let slot_top = probe_y;
                        probe_y += banner_edge_margin + banner_edge_h + banner_edge_margin;
                        // Banner body starts just inside the
                        // bottom of the edge slot.
                        open_top = Some(slot_top + banner_edge_margin + banner_edge_h * 0.5);
                    } else if ln.is_banner_edge_bottom() {
                        let slot_top = probe_y;
                        if let Some(t) = open_top.take() {
                            banner_spans
                                .push((t, slot_top + banner_edge_margin + banner_edge_h * 0.5));
                        }
                        probe_y += banner_edge_margin + banner_edge_h + banner_edge_margin;
                    } else {
                        probe_y += ln.size + line_gap;
                    }
                }
                // Drop any unclosed edge silently.
                let _ = open_top;
            }

            // Paint banner backdrops first so all subsequent
            // text and edge gradients draw on top. The plate is
            // a horizontal 0 → full → 0 alpha mask of a dark
            // translucent fill so it fades into the surrounding
            // stone background instead of reading as a hard
            // rectangle. Mirrors the gold edge rule's gradient
            // so the banner reads as one coherent ornament.
            for &(top_y, bot_y) in &banner_spans {
                let plate_l = rect.x() + pad - banner_inset;
                let plate_r = rect.max.x - pad + banner_inset;
                let plate_h = (bot_y - top_y).max(0.0);
                let mid_x = (plate_l + plate_r) * 0.5;
                let dark = Color::rgba(0.0, 0.0, 0.0, 0.55);
                let clear = Color::rgba(0.0, 0.0, 0.0, 0.0);
                let half_l = Rect::from_xywh(plate_l, top_y, mid_x - plate_l, plate_h);
                let half_r = Rect::from_xywh(mid_x, top_y, plate_r - mid_x, plate_h);
                // grad4 corners: top-left, top-right, bot-left, bot-right.
                ui.draw_grad4_rect(half_l, clear, dark, clear, dark);
                ui.draw_grad4_rect(half_r, dark, clear, dark, clear);
            }

            for ln in lines {
                if ln.is_divider() {
                    // Gold separator with the same horizontal
                    // 0 → full → 0 alpha mask as the legendary
                    // banner edges so every divider in the
                    // tooltip reads as one consistent ornament,
                    // just at different sizes.
                    cursor_y += divider_margin;
                    let mid_y = cursor_y + divider_h * 0.5 - 1.0;
                    let span_l = rect.x() + pad;
                    let span_r = rect.max.x - pad;
                    let mid_x = (span_l + span_r) * 0.5;
                    let gold = Color::rgba(0.78, 0.62, 0.30, 0.85);
                    let clear = Color::rgba(0.78, 0.62, 0.30, 0.0);
                    let row_l = Rect::from_xywh(span_l, mid_y, mid_x - span_l, 1.0);
                    let row_r = Rect::from_xywh(mid_x, mid_y, span_r - mid_x, 1.0);
                    ui.draw_grad4_rect(row_l, clear, gold, clear, gold);
                    ui.draw_grad4_rect(row_r, gold, clear, gold, clear);
                    cursor_y += divider_h + divider_margin;
                } else if ln.is_banner_edge_top() || ln.is_banner_edge_bottom() {
                    // Horizontal gold-gradient rule: transparent
                    // → opaque → transparent across the tooltip
                    // width. Drawn as two side-by-side gradient
                    // rects (left half transparent→opaque, right
                    // half opaque→transparent) using
                    // `draw_grad4_rect` since the primitive
                    // exposes 4-corner colours.
                    cursor_y += banner_edge_margin;
                    let mid_y = cursor_y + banner_edge_h * 0.5 - 1.0;
                    let span_l = rect.x() + pad - banner_inset;
                    let span_r = rect.max.x - pad + banner_inset;
                    let mid_x = (span_l + span_r) * 0.5;
                    let gold = Color::rgba(1.0, 0.82, 0.36, 0.95);
                    let clear = Color::rgba(1.0, 0.82, 0.36, 0.0);
                    let row_l = Rect::from_xywh(span_l, mid_y, mid_x - span_l, 2.0);
                    let row_r = Rect::from_xywh(mid_x, mid_y, span_r - mid_x, 2.0);
                    ui.draw_grad4_rect(row_l, clear, gold, clear, gold);
                    ui.draw_grad4_rect(row_r, gold, clear, gold, clear);
                    cursor_y += banner_edge_h + banner_edge_margin;
                } else {
                    // Optional trailing badge: split off the
                    // tail after the last `"  "` (two spaces)
                    // and render it inside a small filled
                    // rounded pill so roll-quality tiers
                    // (`▴ Fine`, `▴▴▴ Perfect`, …) read as a
                    // visual chip rather than trailing text.
                    // Both head and tail are `trim_end`/
                    // `trim_start`-ed so a stat formatter that
                    // emits an extra space around the
                    // delimiter doesn't push the badge text off
                    // its left padding edge.
                    let (head, badge_text) = match (ln.badge, ln.text.rsplit_once("  ")) {
                        (Some(_), Some((h, t))) => {
                            let t = strip_band_glyph(t);
                            if t.is_empty() {
                                (ln.text, None)
                            } else {
                                (h.trim_end(), Some(t))
                            }
                        }
                        _ => (ln.text, None),
                    };
                    ui.draw_text(Pos2::new(rect.x() + pad, cursor_y), head, ln.size, ln.color);
                    if let (Some(tail), Some(fill)) = (badge_text, ln.badge) {
                        // Badge sits aligned to the right edge of
                        // the tooltip body. Slightly smaller than
                        // the line size so the pill chrome reads
                        // as ornament instead of fighting the
                        // stat text for weight. Padding +
                        // rounded corners are scale-aware so the
                        // shape stays proportional across themes.
                        let badge_font = ln.size * 0.82;
                        let pad_x = 4.0 * theme.scale;
                        let pad_y = 1.0 * theme.scale;
                        let tail_w = ui.measure_text(tail, badge_font);
                        let badge_w = tail_w + pad_x * 2.0;
                        let badge_h = badge_font + pad_y * 2.0;
                        let badge_x = rect.max.x - pad - badge_w;
                        // Centre the pill vertically against the
                        // line baseline so it sits on the same
                        // optical row as the head text.
                        let badge_y = cursor_y + (ln.size - badge_h) * 0.5;
                        let badge_rect = Rect::from_xywh(badge_x, badge_y, badge_w, badge_h);
                        let radius = badge_h * 0.5;
                        // Soft-tinted fill so the pill reads as
                        // a chip without yelling. The text on
                        // top stays at the band's full saturated
                        // colour so the tier identity comes from
                        // the lettering, not the background.
                        let [r, g, b, _] = fill.0;
                        let chip_fill = Color::rgba(r, g, b, 0.18);
                        let chip_border = Color::rgba(r, g, b, 0.85);
                        ui.draw_rounded_rect(badge_rect, radius, chip_fill);
                        ui.draw_rounded_outline(badge_rect, radius, 1.0, chip_border);
                        ui.draw_text(
                            Pos2::new(badge_x + pad_x, badge_y + pad_y),
                            tail,
                            badge_font,
                            Color::rgba(r, g, b, 1.0),
                        );
                    }
                    cursor_y += ln.size + line_gap;
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

/// Strip the leading roll-band glyph(s) (`▾ ▸ ▴ ▴▴ ▴▴▴`) from a
/// badge tail like `"▴ Fine"`, returning just the band name
/// (`"Fine"`). The triangle glyphs carry large left side-bearing
/// in most fonts, which makes them sit visibly off-centre inside
/// the pill — and they're redundant anyway because the pill's
/// fill colour already encodes the band identity. Whitespace on
/// both ends is trimmed so `measure_text` matches the painted
/// glyph run exactly.
fn strip_band_glyph(tail: &str) -> &str {
    let trimmed = tail.trim();
    let rest = trimmed.trim_start_matches(|c| matches!(c, '\u{25B4}' | '\u{25B8}' | '\u{25BE}'));
    rest.trim_start()
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
            decor: TooltipLineDecor::Text,
            badge: None,
        })
        .collect()
}
