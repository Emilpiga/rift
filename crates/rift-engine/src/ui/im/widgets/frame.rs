//! Panel / dialog frame.
//!
//! Replaces the `draw_panel` / `draw_panel_inner` / `draw_modal`
//! helpers that were copy-pasted across HUD, inventory, and
//! character-select. A `Frame` is pure data; call [`Frame::show`]
//! to draw it and run a body closure inside the padded interior
//! rect.
//!
//! ```ignore
//! Frame::panel(ui.theme()).show(ui, panel_rect, |ui, body_rect| {
//!     ui.draw_text(body_rect.min + Vec2::new(8.0, 8.0), "Hello", 16.0, ui.theme().colors.text);
//! });
//! ```

use super::super::color::{Color, Stroke};
use super::super::rect::{Pad, Rect, Vec2};
use super::super::theme::Theme;
use super::super::ui::Ui;

/// Style + layout for a panel-shaped surface.
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub fill: Color,
    pub stroke: Stroke,
    pub corner_radius: f32,
    pub padding: Pad,
}

impl Frame {
    /// Plain panel: dark fill, thin border, theme corner radius.
    pub fn panel(theme: &Theme) -> Self {
        Self {
            fill: theme.colors.bg_panel,
            stroke: theme.border_stroke(),
            corner_radius: theme.spacing.corner_radius,
            padding: theme.spacing.pad_md,
        }
    }

    /// Sub-panel inside another panel (text fields, list rows).
    /// Slightly different fill so it pops against `bg_panel`; thin
    /// border, no padding by default so callers can tile them.
    pub fn inset(theme: &Theme) -> Self {
        Self {
            fill: theme.colors.bg_panel_alt,
            stroke: theme.border_stroke(),
            corner_radius: (theme.spacing.corner_radius * 0.5).max(2.0),
            padding: theme.spacing.pad_sm,
        }
    }

    /// Tooltip styling: tighter padding, stronger border so it
    /// reads against arbitrary backgrounds.
    pub fn tooltip(theme: &Theme) -> Self {
        Self {
            fill: theme.colors.bg_panel,
            stroke: theme.border_strong_stroke(),
            corner_radius: theme.spacing.corner_radius,
            padding: theme.spacing.pad_sm,
        }
    }

    /// Override the fill colour. Builder-style for quick variants.
    pub fn with_fill(mut self, c: Color) -> Self {
        self.fill = c;
        self
    }

    /// Override the stroke. Pass [`Stroke::NONE`] to disable the border.
    pub fn with_stroke(mut self, s: Stroke) -> Self {
        self.stroke = s;
        self
    }

    /// Override padding.
    pub fn with_padding(mut self, p: Pad) -> Self {
        self.padding = p;
        self
    }

    /// Override corner radius. `0.0` disables rounding (cheaper draw).
    pub fn with_radius(mut self, r: f32) -> Self {
        self.corner_radius = r;
        self
    }

    /// Draw the frame at `rect` and call `body` with the padded
    /// interior rect. Returns whatever `body` returns so callers
    /// can propagate widget responses cleanly.
    pub fn show<R>(self, ui: &mut Ui<'_>, rect: Rect, body: impl FnOnce(&mut Ui<'_>, Rect) -> R) -> R {
        // Fill.
        if self.fill.0[3] > 0.0 {
            ui.draw_rounded_rect(rect, self.corner_radius, self.fill);
        }
        // Stroke.
        if self.stroke.thickness > 0.0 && self.stroke.color.0[3] > 0.0 {
            ui.draw_rounded_outline(
                rect,
                self.corner_radius,
                self.stroke.thickness,
                self.stroke.color,
            );
        }
        // Body inside the padded interior.
        let inner = rect.shrink2(self.padding);
        body(ui, inner)
    }

    /// Draw the frame at `rect` without invoking a body closure.
    /// Useful when the caller wants to draw children manually
    /// (e.g. a panel that contains absolutely-positioned widgets).
    /// Returns the padded interior rect.
    pub fn show_only(self, ui: &mut Ui<'_>, rect: Rect) -> Rect {
        self.show(ui, rect, |_, inner| inner)
    }

    /// Inflate `inner` by this frame's padding. Inverse of the
    /// "inner from outer" computation [`Self::show`] does \u2014 used
    /// when callers know the body size and want the outer rect.
    pub fn outer_from_inner(&self, inner: Rect) -> Rect {
        let p = self.padding;
        Rect::from_min_max(
            super::super::rect::Pos2::new(inner.min.x - p.left, inner.min.y - p.top),
            super::super::rect::Pos2::new(inner.max.x + p.right, inner.max.y + p.bottom),
        )
    }

    /// Convenience: padding-aware size hint when the body is known.
    pub fn outer_size(&self, inner: Vec2) -> Vec2 {
        Vec2::new(
            inner.x + self.padding.left + self.padding.right,
            inner.y + self.padding.top + self.padding.bottom,
        )
    }
}
