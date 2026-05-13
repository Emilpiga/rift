//! Helpers for drawing UI anchored to world-space positions
//! (enemy health bars, floating combat numbers, world labels).
//!
//! Owns the projection math so callers don't repeat the
//! `view_proj * world_pos.extend(1.0)` / clip-space cull /
//! NDC → pixel ladder at every site.
//!
//! Construct one per frame from a `&mut Ui` plus the active
//! camera's `view_proj`, then call `world_at` / `bar_above` /
//! `text_above` for each anchor. Behind-camera and off-screen
//! anchors are silently skipped (returning `None`), matching
//! the previous hand-rolled code.
//!
//! Note: this is *not* a separate retained system. It's a thin
//! adapter over the existing IM stack so widgets like
//! [`ProgressBar`](super::ProgressBar) work in world space
//! without re-implementing their drawing logic.

use glam::{Mat4, Vec3};

use super::{Color, Pos2, ProgressBar, Rect, Ui};

/// World-space UI anchor adapter.
///
/// Lifetime borrows the underlying [`Ui`]; one of these per
/// frame, scoped to the world-render block of the HUD.
pub struct WorldUi<'u, 'a> {
    ui: &'u mut Ui<'a>,
    view_proj: Mat4,
    screen: (f32, f32),
}

impl<'u, 'a> WorldUi<'u, 'a> {
    pub fn new(ui: &'u mut Ui<'a>, view_proj: Mat4) -> Self {
        let s = ui.screen_size();
        Self {
            ui,
            view_proj,
            screen: (s.x, s.y),
        }
    }

    /// Project `world_pos` (in world space) to a pixel anchor.
    /// Returns `None` if behind the camera or outside the
    /// `(-1, 1)` clip cube on either axis.
    pub fn world_to_screen(&self, world_pos: Vec3) -> Option<Pos2> {
        let clip = self.view_proj * world_pos.extend(1.0);
        if clip.w <= 0.0 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        if ndc.x < -1.0 || ndc.x > 1.0 || ndc.y < -1.0 || ndc.y > 1.0 {
            return None;
        }
        let px = (ndc.x + 1.0) * 0.5 * self.screen.0;
        let py = (ndc.y + 1.0) * 0.5 * self.screen.1;
        Some(Pos2::new(px, py))
    }

    /// Direct mutable access to the underlying [`Ui`]. Useful
    /// when a caller needs an extra primitive (e.g. a debuff
    /// pip overlay) that doesn't have a dedicated helper yet.
    pub fn ui(&mut self) -> &mut Ui<'a> {
        self.ui
    }

    /// Draw a thin progress bar centred horizontally at `pos`,
    /// `y_offset_px` pixels above (negative = below) the
    /// projected anchor. No-op if the anchor is off-screen.
    /// Returns `Some(rect)` when drawn so callers can stack
    /// further widgets (debuff pips) against it.
    pub fn bar_above_world(
        &mut self,
        world_pos: Vec3,
        y_offset_px: f32,
        width: f32,
        height: f32,
        value: f32,
        fill: Color,
    ) -> Option<Rect> {
        let anchor = self.world_to_screen(world_pos)?;
        let rect = Rect::from_xywh(
            anchor.x - width * 0.5,
            anchor.y + y_offset_px,
            width,
            height,
        );
        ProgressBar::new(value)
            .fill(fill)
            .rounded(false)
            .show(self.ui, rect);
        Some(rect)
    }

    /// Draw text horizontally centred at the projected anchor.
    /// `y_offset_px` is added to the projected y (negative =
    /// further above).
    pub fn text_above_world(
        &mut self,
        world_pos: Vec3,
        y_offset_px: f32,
        text: &str,
        size: f32,
        color: Color,
    ) -> Option<Pos2> {
        let anchor = self.world_to_screen(world_pos)?;
        let tw = self.ui.measure_text(text, size);
        let pos = Pos2::new(anchor.x - tw * 0.5, anchor.y + y_offset_px);
        self.ui.draw_text(pos, text, size, color);
        Some(pos)
    }
}
