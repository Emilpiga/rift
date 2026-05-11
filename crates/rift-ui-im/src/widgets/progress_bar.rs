//! Filled horizontal bar with optional label and segmentation
//! pips. Used everywhere a `0..1` quantity needs to read at a
//! glance — HP, XP, rift progress, enemy health, cooldown rings,
//! etc.
//!
//! The widget is purely visual; it never claims input. Hover
//! interaction is the caller's job (e.g. via [`Ui::hover_only`])
//! so the same widget works for HUD, world-anchored bars, and
//! tooltips.
//!
//! Layered draw order:
//! 1. Track (rounded background fill).
//! 2. Fill (accent colour, clamped to `value`).
//! 3. Pips (vertical separators every `1/N` of the width) — used
//!    for the floor-count indicator and similar segmented gauges.
//! 4. Border (rounded outline; theme-driven).
//! 5. Centred label (optional).

use super::super::{
    color::Color, theme::Theme, ui::Ui,
};
use crate::rect::{Pos2, Rect};

/// One filled bar. All parameters except `value` and `rect`
/// have sensible defaults pulled from the active theme.
pub struct ProgressBar<'a> {
    value: f32,
    fill: Option<Color>,
    track: Option<Color>,
    border: Option<Color>,
    label: Option<&'a str>,
    label_color: Option<Color>,
    /// Number of pips drawn across the bar. `0` means none.
    pips: u32,
    /// When `true`, the bar is drawn with rounded ends matching
    /// `theme.spacing.corner_radius`. Set to `false` for the
    /// thin enemy-overhead bars where rounding looks fuzzy.
    rounded: bool,
}

impl<'a> ProgressBar<'a> {
    pub fn new(value: f32) -> Self {
        Self {
            value: value.clamp(0.0, 1.0),
            fill: None,
            track: None,
            border: None,
            label: None,
            label_color: None,
            pips: 0,
            rounded: true,
        }
    }

    pub fn fill(mut self, c: Color) -> Self {
        self.fill = Some(c);
        self
    }

    pub fn track(mut self, c: Color) -> Self {
        self.track = Some(c);
        self
    }

    pub fn border(mut self, c: Color) -> Self {
        self.border = Some(c);
        self
    }

    pub fn label(mut self, text: &'a str) -> Self {
        self.label = Some(text);
        self
    }

    pub fn label_color(mut self, c: Color) -> Self {
        self.label_color = Some(c);
        self
    }

    pub fn pips(mut self, n: u32) -> Self {
        self.pips = n;
        self
    }

    pub fn rounded(mut self, on: bool) -> Self {
        self.rounded = on;
        self
    }

    /// Draw the bar inside `rect`. Returns the filled-area rect
    /// so callers can anchor satellite UI (delta numerals, leech
    /// indicators) against it.
    pub fn show(self, ui: &mut Ui<'_>, rect: Rect) -> Rect {
        let theme = *ui.theme();
        let radius = if self.rounded {
            theme.spacing.corner_radius
        } else {
            0.0
        };

        let track = self.track.unwrap_or(Color::rgba(0.08, 0.08, 0.10, 0.85));
        let fill = self.fill.unwrap_or(theme.colors.accent);
        let border = self.border.unwrap_or(theme.colors.border);

        // Track.
        if radius > 0.0 {
            ui.draw_rounded_rect(rect, radius, track);
        } else {
            ui.draw_rect(rect, track);
        }

        // Fill — clipped horizontally to `value`.
        let fw = rect.width() * self.value;
        if fw > 0.0 {
            let fill_rect = Rect::from_xywh(rect.x(), rect.y(), fw, rect.height());
            // The filled portion uses the same radius as the
            // track so the leading edge looks glued to the
            // track when the bar is full. At low values the
            // far-right corners would otherwise be sharp; the
            // square edge there reads as a "drained" cap.
            if radius > 0.0 && self.value >= 0.999 {
                ui.draw_rounded_rect(fill_rect, radius, fill);
            } else {
                ui.draw_rect(fill_rect, fill);
            }
        }

        // Pips.
        if self.pips > 1 {
            let pip_w = (rect.height() * 0.10).max(1.0);
            let pip_color = Color::rgba(0.0, 0.0, 0.0, 0.55);
            for i in 1..self.pips {
                let x = rect.x() + rect.width() * (i as f32 / self.pips as f32);
                ui.draw_rect(
                    Rect::from_xywh(x - pip_w * 0.5, rect.y(), pip_w, rect.height()),
                    pip_color,
                );
            }
        }

        // Border.
        if radius > 0.0 {
            ui.draw_rounded_outline(
                rect,
                radius,
                theme.spacing.border_thickness,
                border,
            );
        } else {
            ui.draw_outline(rect, theme.spacing.border_thickness, border);
        }

        // Label.
        if let Some(text) = self.label {
            let size = pick_label_size(rect.height(), &theme);
            let lc = self.label_color.unwrap_or(theme.colors.text);
            let tw = ui.measure_text(text, size);
            ui.draw_text(
                Pos2::new(
                    rect.x() + (rect.width() - tw) * 0.5,
                    rect.y() + (rect.height() - size) * 0.5,
                ),
                text,
                size,
                lc,
            );
        }

        Rect::from_xywh(rect.x(), rect.y(), fw, rect.height())
    }
}

/// Pick a label font size that fits within the bar height.
/// Keeps the label proportional so HP (22 px tall) shows a 14 px
/// label while a 9 px XP bar shows an 11 px label sitting just
/// above its top edge.
fn pick_label_size(bar_h: f32, theme: &Theme) -> f32 {
    if bar_h >= 18.0 {
        theme.fonts.size_md
    } else if bar_h >= 12.0 {
        theme.fonts.size_sm
    } else {
        // For very thin bars the caller usually wants the label
        // outside the bar entirely; pick the smallest theme size
        // and let it overflow vertically — the label still reads.
        theme.fonts.size_sm
    }
}

/// HP-bar colour ramp. Returns the canonical green→amber→red
/// gradient stops used by every "remaining health" bar in the
/// game so UIs can stay in sync without each owning its own
/// thresholds.
pub fn hp_color(pct: f32) -> Color {
    let p = pct.clamp(0.0, 1.0);
    if p > 0.5 {
        Color::rgba(0.45, 0.78, 0.30, 0.95)
    } else if p > 0.25 {
        Color::rgba(0.90, 0.70, 0.05, 0.95)
    } else {
        Color::rgba(0.92, 0.18, 0.18, 0.95)
    }
}
