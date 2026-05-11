//! Per-frame fit-scaling for full panels.
//!
//! Every screen that lays out a fixed *unscaled* design at
//! 1080p baseline needs the same routine the inventory uses:
//!
//! ```text
//! fit = min(theme.scale, avail_w / total_w, avail_h / total_h)
//! ```
//!
//! …then multiply every dimension by `fit` before drawing.
//! [`FitScale`] packages that math so HUD chrome, spellbook,
//! channel-progress panels, etc. don't reinvent it. It is a
//! *value*, not a builder — call [`FitScale::compute`] once
//! per frame and read its fields.
//!
//! See [`mp_inventory_ui::Layout`](../../../../rift-client/src/game/mp_inventory_ui.rs)
//! for the canonical caller.

use super::rect::Vec2;
use super::ui::Ui;

/// Result of fitting an unscaled design rect into the current
/// screen. `factor` is the multiplier every literal at the
/// call site should be multiplied by — including font sizes
/// when the text is part of a sized layout. The associated
/// [`Self::s`] helper makes that ergonomic.
#[derive(Debug, Clone, Copy)]
pub struct FitScale {
    /// The chosen scale factor — already clamped against the
    /// global theme scale.
    pub factor: f32,
    /// Available screen size after subtracting the safe
    /// margin (kept around so callers can centre the panel).
    pub avail: Vec2,
    /// Pixel inset reserved on every edge of the screen.
    /// Same value passed to [`Self::compute`].
    pub margin: f32,
}

impl FitScale {
    /// Compute a fit scale for an unscaled design of size
    /// `total_unscaled`. `margin` is the reserved gutter on
    /// every screen edge (e.g. 24px). `min_factor` is the
    /// floor below which we'd rather let the design overflow
    /// than render unreadable text — typical value `0.4`.
    pub fn compute(
        ui: &Ui<'_>,
        total_unscaled: Vec2,
        margin: f32,
        min_factor: f32,
    ) -> Self {
        let theme = *ui.theme();
        let screen = ui.screen_size();
        let avail = Vec2::new(
            (screen.x - margin * 2.0).max(64.0),
            (screen.y - margin * 2.0).max(64.0),
        );
        let factor = theme
            .scale
            .min(avail.x / total_unscaled.x.max(1.0))
            .min(avail.y / total_unscaled.y.max(1.0))
            .max(min_factor);
        Self {
            factor,
            avail,
            margin,
        }
    }

    /// Multiply a baseline pixel value by the fit factor.
    /// Use everywhere literals appear inside the panel:
    /// `let pad = fit.s(8.0);`.
    #[inline]
    pub fn s(&self, v: f32) -> f32 {
        v * self.factor
    }

    /// Apply the fit factor to a 2-vector (e.g. a baseline
    /// `Vec2::new(24.0, 18.0)` offset).
    #[inline]
    pub fn v(&self, v: Vec2) -> Vec2 {
        Vec2::new(v.x * self.factor, v.y * self.factor)
    }
}
