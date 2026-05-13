//! Lightweight rectangular button.
//!
//! Sits between [`super::Button`] (full themed chrome with
//! rounded corners, lit edge bands and outlines) and rolling
//! a flat rect by hand. Use [`MiniButton`] for compact UI
//! affordances that need:
//!
//! * Caller-supplied colours (so a "+ Buy" pill in the
//!   inventory can use the exact accent it wants without
//!   inventing a new theme variant).
//! * Independent armed / disabled states beyond the standard
//!   `enabled` flag (e.g. the 2-stage Salvage Trash button
//!   wants a red fill once primed).
//! * No rounded corners or lit-edge chrome — the inventory's
//!   tab strip and stats grid call dozens of these per frame
//!   and a flat rect blends into the panel chrome better than
//!   a pill that screams "click me".
//!
//! The widget intentionally does NOT auto-size — supply a
//! pre-computed `Rect` so the caller's layout math owns
//! placement.

use super::super::color::Color;
use super::super::id::Id;
use super::super::rect::{Pos2, Rect};
use super::super::ui::Ui;

/// Per-state fill colours. Build with [`MiniButtonFills::flat`]
/// for the simplest case (one colour, hover slightly brighter).
#[derive(Clone, Copy, Debug)]
pub struct MiniButtonFills {
    pub idle: Color,
    pub hover: Color,
    /// Drawn when `enabled` is false. Text colour switches to
    /// `text_dim` automatically.
    pub disabled: Color,
}

impl MiniButtonFills {
    /// Single fill colour with an auto-derived hover variant
    /// (lifted ~15% in luminance) and a desaturated disabled
    /// variant. Good enough for the majority of mini-buttons
    /// that just want "subtle, slightly brighter on hover".
    pub fn flat(base: Color) -> Self {
        let lift = |c: f32| (c * 1.18).min(1.0);
        let dim = |c: f32| c * 0.55;
        Self {
            idle: base,
            hover: Color::rgba(lift(base.0[0]), lift(base.0[1]), lift(base.0[2]), base.0[3]),
            disabled: Color::rgba(
                dim(base.0[0]),
                dim(base.0[1]),
                dim(base.0[2]),
                base.0[3] * 0.8,
            ),
        }
    }

    /// Fully explicit construction for callers that want
    /// armed-state colours that don't follow the lift/dim
    /// rules (e.g. the 2-stage Salvage Trash flips to red).
    pub fn explicit(idle: Color, hover: Color, disabled: Color) -> Self {
        Self {
            idle,
            hover,
            disabled,
        }
    }
}

/// Configurable mini-button. Cheap struct; build, configure,
/// `.show()`.
#[derive(Clone)]
pub struct MiniButton<'a> {
    label: &'a str,
    fills: MiniButtonFills,
    text_color: Option<Color>,
    text_size: Option<f32>,
    enabled: bool,
}

/// What [`MiniButton::show`] reports back. Callers usually
/// only care about `clicked`, but `hovered` is exposed so a
/// caller can drive its own tooltip without re-running
/// `interact_hover` against the same id.
#[derive(Clone, Copy, Debug, Default)]
pub struct MiniButtonResponse {
    pub hovered: bool,
    pub clicked: bool,
}

impl<'a> MiniButton<'a> {
    pub fn new(label: &'a str, fills: MiniButtonFills) -> Self {
        Self {
            label,
            fills,
            text_color: None,
            text_size: None,
            enabled: true,
        }
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// Override the text colour. Defaults to `theme.colors.text`
    /// when enabled and `theme.colors.text_dim` when disabled.
    pub fn text_color(mut self, c: Color) -> Self {
        self.text_color = Some(c);
        self
    }

    /// Override the text size. Defaults to `theme.fonts.size_md`.
    pub fn text_size(mut self, s: f32) -> Self {
        self.text_size = Some(s);
        self
    }

    /// Draw + interact. The `id` is required because
    /// mini-buttons typically appear inside loops (one per
    /// stash tab, one per stat row) where the rect alone
    /// can't disambiguate identity across frames.
    pub fn show(self, ui: &mut Ui<'_>, id: Id, rect: Rect) -> MiniButtonResponse {
        let theme = *ui.theme();
        let hovered = if self.enabled {
            ui.interact_hover(id, rect)
        } else {
            false
        };
        let clicked = self.enabled && hovered && ui.input().left_clicked();

        let fill = if !self.enabled {
            self.fills.disabled
        } else if hovered {
            self.fills.hover
        } else {
            self.fills.idle
        };
        ui.draw_rect(rect, fill);

        // Centred, ellipsized label.
        let text_size = self.text_size.unwrap_or(theme.fonts.size_md);
        let text_color = self.text_color.unwrap_or(if self.enabled {
            theme.colors.text
        } else {
            theme.colors.text_dim
        });
        let lw = ui.measure_text(self.label, text_size);
        let inner_pad = 4.0_f32;
        let max_lbl = (rect.width() - inner_pad * 2.0).max(1.0);
        let draw_w = lw.min(max_lbl);
        ui.draw_text_ellipsized(
            Pos2::new(
                rect.x() + (rect.width() - draw_w) * 0.5,
                rect.y() + (rect.height() - text_size) * 0.5,
            ),
            self.label,
            text_size,
            max_lbl,
            text_color,
        );

        MiniButtonResponse { hovered, clicked }
    }
}
