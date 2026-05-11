//! Lightweight anchor / layout helpers.
//!
//! Sits on top of the immediate-mode primitives without
//! introducing a retained widget tree. Two patterns:
//!
//! - [`Anchor`] + [`Ui::anchor`]: resolve a screen-relative
//!   position from a corner / edge plus a pixel offset.
//!   Replaces hand-written `(sw - bar_w) / 2.0` math at HUD
//!   call sites so resizing the window no longer leaves the
//!   action bar floating.
//!
//! - [`Ui::column`] / [`Ui::row`]: walk a [`Rect`] cursor
//!   left→right or top→bottom while drawing children, with a
//!   gap between siblings. Each child returns its consumed
//!   extent; the container reports the bounding rect of the
//!   block. No allocation, no recursion through trait objects
//!   — just a moving cursor.
//!
//! The two helpers compose: anchor a panel's *origin* to a
//! corner, then `column` / `row` the contents inside it.

use super::rect::{Pos2, Rect};
use super::ui::Ui;

/// Where a logical UI element pins to the screen. Combined
/// with a per-axis pixel offset, this produces the absolute
/// `Pos2` used by `draw_*` calls. Negative offsets shift
/// inward from the chosen edge — the convention used by the
/// existing HUD code (e.g. top-right HUD stamps sit at
/// `Anchor::TopRight, (-pad, +pad)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    TopLeft,
    TopCenter,
    TopRight,
    CenterLeft,
    Center,
    CenterRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl Anchor {
    /// Resolve to a screen-space position given the screen
    /// rect and an optional `(dx, dy)` offset. The offset is
    /// applied *after* the anchor — so `TopLeft + (8, 8)`
    /// produces `(8, 8)` while `BottomRight + (-8, -8)`
    /// produces `(screen_w - 8, screen_h - 8)`.
    pub fn resolve(self, screen: Rect, dx: f32, dy: f32) -> Pos2 {
        let (ax, ay) = match self {
            Anchor::TopLeft => (screen.min.x, screen.min.y),
            Anchor::TopCenter => ((screen.min.x + screen.max.x) * 0.5, screen.min.y),
            Anchor::TopRight => (screen.max.x, screen.min.y),
            Anchor::CenterLeft => (screen.min.x, (screen.min.y + screen.max.y) * 0.5),
            Anchor::Center => (
                (screen.min.x + screen.max.x) * 0.5,
                (screen.min.y + screen.max.y) * 0.5,
            ),
            Anchor::CenterRight => (screen.max.x, (screen.min.y + screen.max.y) * 0.5),
            Anchor::BottomLeft => (screen.min.x, screen.max.y),
            Anchor::BottomCenter => ((screen.min.x + screen.max.x) * 0.5, screen.max.y),
            Anchor::BottomRight => (screen.max.x, screen.max.y),
        };
        Pos2::new(ax + dx, ay + dy)
    }

    /// Resolve a *rect* anchored to the screen with the given
    /// `width` × `height`. Centres / right-edges shift the
    /// origin so the visible rect lines up with the chosen
    /// anchor — i.e. `TopRight, (-pad, pad), w, h` produces a
    /// `w × h` rect whose right edge sits `pad` from the
    /// screen's right edge.
    pub fn resolve_rect(
        self,
        screen: Rect,
        dx: f32,
        dy: f32,
        width: f32,
        height: f32,
    ) -> Rect {
        let p = self.resolve(screen, dx, dy);
        let (xshift, yshift) = match self {
            Anchor::TopLeft | Anchor::CenterLeft | Anchor::BottomLeft => (0.0, 0.0),
            Anchor::TopCenter | Anchor::Center | Anchor::BottomCenter => (-width * 0.5, 0.0),
            Anchor::TopRight | Anchor::CenterRight | Anchor::BottomRight => (-width, 0.0),
        };
        let yshift = yshift + match self {
            Anchor::TopLeft | Anchor::TopCenter | Anchor::TopRight => 0.0,
            Anchor::CenterLeft | Anchor::Center | Anchor::CenterRight => -height * 0.5,
            Anchor::BottomLeft | Anchor::BottomCenter | Anchor::BottomRight => -height,
        };
        Rect::from_xywh(p.x + xshift, p.y + yshift, width, height)
    }
}

impl<'a> Ui<'a> {
    /// Resolve `anchor + (dx, dy)` against the current screen.
    /// Use at HUD / chrome sites that today hand-roll
    /// `(sw - bar_w) / 2.0` style math.
    pub fn anchor(&self, anchor: Anchor, dx: f32, dy: f32) -> Pos2 {
        anchor.resolve(self.screen_rect(), dx, dy)
    }

    /// Resolve an anchored *rect* (origin shifted so the rect
    /// itself sits flush with the chosen edge / corner).
    pub fn anchor_rect(
        &self,
        anchor: Anchor,
        dx: f32,
        dy: f32,
        width: f32,
        height: f32,
    ) -> Rect {
        anchor.resolve_rect(self.screen_rect(), dx, dy, width, height)
    }

    /// Stack children top-to-bottom inside `bounds` with `gap`
    /// pixels between siblings. The closure receives the
    /// `&mut Ui`, the *child rect* it should draw into (full
    /// width of `bounds`, top-aligned at the cursor), and the
    /// child's index, and returns the actual height it
    /// consumed. The cursor advances by that height plus
    /// `gap` between children. Returns the total consumed
    /// rect (for borders / hit-tests).
    ///
    /// `count` is provided up front because immediate-mode
    /// children typically come from a fixed-size collection;
    /// keeping it explicit means callers can break out of
    /// long lists without surprises.
    pub fn column<F>(&mut self, bounds: Rect, gap: f32, count: usize, mut child: F) -> Rect
    where
        F: FnMut(&mut Ui<'_>, Rect, usize) -> f32,
    {
        let mut y = bounds.min.y;
        for i in 0..count {
            let child_rect = Rect::from_xywh(
                bounds.min.x,
                y,
                bounds.width(),
                (bounds.max.y - y).max(0.0),
            );
            let consumed = child(self, child_rect, i);
            y += consumed;
            if i + 1 < count {
                y += gap;
            }
            if y >= bounds.max.y {
                break;
            }
        }
        Rect::from_xywh(
            bounds.min.x,
            bounds.min.y,
            bounds.width(),
            (y - bounds.min.y).max(0.0),
        )
    }

    /// Lay out `count` children left-to-right inside `bounds`
    /// with `gap` pixels between siblings. The closure
    /// receives the `&mut Ui`, the child rect, and the
    /// child index; it returns the width it consumed. See
    /// [`Self::column`] for the symmetrical vertical
    /// counterpart.
    pub fn row<F>(&mut self, bounds: Rect, gap: f32, count: usize, mut child: F) -> Rect
    where
        F: FnMut(&mut Ui<'_>, Rect, usize) -> f32,
    {
        let mut x = bounds.min.x;
        for i in 0..count {
            let child_rect = Rect::from_xywh(
                x,
                bounds.min.y,
                (bounds.max.x - x).max(0.0),
                bounds.height(),
            );
            let consumed = child(self, child_rect, i);
            x += consumed;
            if i + 1 < count {
                x += gap;
            }
            if x >= bounds.max.x {
                break;
            }
        }
        Rect::from_xywh(
            bounds.min.x,
            bounds.min.y,
            (x - bounds.min.x).max(0.0),
            bounds.height(),
        )
    }
}
