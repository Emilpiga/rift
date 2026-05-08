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
    ///
    /// Visual layering, bottom → top:
    ///  1. Soft drop shadow (3 expanding copies of `theme.shadow`),
    ///     offset down a few pixels so the panel reads as a
    ///     floating card.
    ///  2. Fill (rounded).
    ///  3. 1px lit-edge highlight along the top quarter of the
    ///     border — lifts the panel off the background without
    ///     having to render a full gradient.
    ///  4. Hairline border outline.
    ///  5. Body, inside the padded interior.
    pub fn show<R>(self, ui: &mut Ui<'_>, rect: Rect, body: impl FnOnce(&mut Ui<'_>, Rect) -> R) -> R {
        // 1. Drop shadow. Three expanding copies with falling
        //    alpha approximate a Gaussian blur cheaply. Skip
        //    when fill is fully transparent (no panel ⇒ no
        //    floating shadow).
        if self.fill.0[3] > 0.05 {
            let shadow_color = ui.theme().colors.shadow;
            // Each pass: grow outward + drop down. Alpha
            // halves each pass so the closest copy is
            // strongest, mimicking ambient occlusion.
            for i in 0..3 {
                let grow = 2.0 + i as f32 * 4.0;
                let drop = 2.0 + i as f32 * 2.0;
                let a = shadow_color.0[3] * 0.55 / (i + 1) as f32;
                let s = Color::rgba(
                    shadow_color.0[0],
                    shadow_color.0[1],
                    shadow_color.0[2],
                    a,
                );
                let r = Rect::from_min_max(
                    super::super::rect::Pos2::new(rect.min.x - grow, rect.min.y - grow + drop),
                    super::super::rect::Pos2::new(rect.max.x + grow, rect.max.y + grow + drop),
                );
                ui.draw_rounded_rect(r, self.corner_radius + grow, s);
            }
        }

        // 2. Fill.
        if self.fill.0[3] > 0.0 {
            ui.draw_rounded_rect(rect, self.corner_radius, self.fill);
        }

        // 3. Lit top edge — sells the "lit from above" feel
        //    cheaply. The horizontal inset is `corner_radius`
        //    so the band sits entirely inside the straight
        //    section of the top edge — otherwise its flat
        //    ends would poke outside the rounded corners and
        //    render as visible nubs.
        if self.fill.0[3] > 0.05 && rect.width() > self.corner_radius * 2.0 + 2.0 {
            let h_inset = self.corner_radius.max(1.0);
            let top_band = Rect::from_xywh(
                rect.x() + h_inset,
                rect.y() + 1.0,
                rect.width() - h_inset * 2.0,
                1.0,
            );
            // Pure rect (radius 0) → no rounding artefacts.
            ui.draw_rect(top_band, Color::rgba(1.0, 1.0, 1.0, 0.07));

            // Soft falloff band a couple of pixels below.
            let band_h = (rect.height() * 0.20).min(20.0);
            if band_h > 4.0 {
                let soft = Rect::from_xywh(
                    rect.x() + h_inset,
                    rect.y() + 2.0,
                    rect.width() - h_inset * 2.0,
                    band_h * 0.5,
                );
                ui.draw_rect(soft, Color::rgba(1.0, 1.0, 1.0, 0.02));
            }
        }

        // 4. Stroke.
        if self.stroke.thickness > 0.0 && self.stroke.color.0[3] > 0.0 {
            ui.draw_rounded_outline(
                rect,
                self.corner_radius,
                self.stroke.thickness,
                self.stroke.color,
            );
        }

        // 5. Body inside the padded interior.
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
