//! 2D geometry primitives shared across the immediate-mode UI.
//!
//! Pixel-space (top-left origin), `f32`. Kept tiny on purpose:
//! widgets only need a handful of operations (point-in-rect,
//! shrink-by-padding, split-from-edge) so we don't pull in a
//! full 2D math crate.

/// 2D point in pixels. Top-left origin to match `OverlayBatch`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pos2 {
    pub x: f32,
    pub y: f32,
}

impl Pos2 {
    pub const ZERO: Pos2 = Pos2 { x: 0.0, y: 0.0 };
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// 2D vector (size or offset) in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
    pub const fn splat(v: f32) -> Self {
        Self { x: v, y: v }
    }
}

impl std::ops::Add<Vec2> for Pos2 {
    type Output = Pos2;
    fn add(self, rhs: Vec2) -> Pos2 {
        Pos2 {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
        }
    }
}

impl std::ops::Sub<Pos2> for Pos2 {
    type Output = Vec2;
    fn sub(self, rhs: Pos2) -> Vec2 {
        Vec2 {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
        }
    }
}

/// Axis-aligned rectangle in pixel space.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub min: Pos2,
    pub max: Pos2,
}

impl Rect {
    pub const ZERO: Rect = Rect {
        min: Pos2::ZERO,
        max: Pos2::ZERO,
    };

    pub const fn from_min_max(min: Pos2, max: Pos2) -> Self {
        Self { min, max }
    }

    pub fn from_min_size(min: Pos2, size: Vec2) -> Self {
        Self {
            min,
            max: Pos2::new(min.x + size.x, min.y + size.y),
        }
    }

    pub fn from_xywh(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self {
            min: Pos2::new(x, y),
            max: Pos2::new(x + w, y + h),
        }
    }

    /// Rect spanning the entire screen at `(screen_w, screen_h)`.
    pub fn from_screen(screen_w: f32, screen_h: f32) -> Self {
        Self::from_xywh(0.0, 0.0, screen_w, screen_h)
    }

    pub fn x(&self) -> f32 {
        self.min.x
    }
    pub fn y(&self) -> f32 {
        self.min.y
    }
    pub fn width(&self) -> f32 {
        self.max.x - self.min.x
    }
    pub fn height(&self) -> f32 {
        self.max.y - self.min.y
    }
    pub fn size(&self) -> Vec2 {
        Vec2::new(self.width(), self.height())
    }
    pub fn center(&self) -> Pos2 {
        Pos2::new(
            (self.min.x + self.max.x) * 0.5,
            (self.min.y + self.max.y) * 0.5,
        )
    }

    /// `true` if `p` lies inside (inclusive of `min`, exclusive of `max`).
    pub fn contains(&self, p: Pos2) -> bool {
        p.x >= self.min.x && p.x < self.max.x && p.y >= self.min.y && p.y < self.max.y
    }

    /// Shrink uniformly on all sides by `pad`. Negative values expand.
    pub fn shrink(&self, pad: f32) -> Rect {
        Rect {
            min: Pos2::new(self.min.x + pad, self.min.y + pad),
            max: Pos2::new(self.max.x - pad, self.max.y - pad),
        }
    }

    /// Shrink by per-side padding.
    pub fn shrink2(&self, pad: Pad) -> Rect {
        Rect {
            min: Pos2::new(self.min.x + pad.left, self.min.y + pad.top),
            max: Pos2::new(self.max.x - pad.right, self.max.y - pad.bottom),
        }
    }

    /// Center a child rect of `size` inside `self`.
    pub fn align_center(&self, size: Vec2) -> Rect {
        let cx = self.center().x - size.x * 0.5;
        let cy = self.center().y - size.y * 0.5;
        Rect::from_min_size(Pos2::new(cx, cy), size)
    }

    /// Translate by `offset`.
    pub fn translate(&self, offset: Vec2) -> Rect {
        Rect {
            min: self.min + offset,
            max: self.max + offset,
        }
    }

    /// Intersection with `other`. Returns a zero-sized rect if disjoint.
    pub fn intersect(&self, other: Rect) -> Rect {
        let min = Pos2::new(self.min.x.max(other.min.x), self.min.y.max(other.min.y));
        let max = Pos2::new(self.max.x.min(other.max.x), self.max.y.min(other.max.y));
        if min.x >= max.x || min.y >= max.y {
            Rect { min, max: min }
        } else {
            Rect { min, max }
        }
    }

    /// Carve a `h`-pixel-tall strip off the top of `self` and
    /// return it. The receiver is unchanged — pair with
    /// [`Self::shrink_top`] when you also want to consume the
    /// space, or use [`Self::split_off_top`] to do both in one
    /// call. Used together with [`crate::Row`] / [`crate::Column`]
    /// to carve a panel into header / body / footer strips
    /// without arithmetic.
    pub fn top(&self, h: f32) -> Rect {
        Rect::from_xywh(self.x(), self.y(), self.width(), h.min(self.height()))
    }
    /// Carve a `h`-pixel-tall strip off the bottom.
    pub fn bottom(&self, h: f32) -> Rect {
        let h = h.min(self.height());
        Rect::from_xywh(self.x(), self.max.y - h, self.width(), h)
    }
    /// Carve a `w`-pixel-wide strip off the left.
    pub fn left(&self, w: f32) -> Rect {
        Rect::from_xywh(self.x(), self.y(), w.min(self.width()), self.height())
    }
    /// Carve a `w`-pixel-wide strip off the right.
    pub fn right(&self, w: f32) -> Rect {
        let w = w.min(self.width());
        Rect::from_xywh(self.max.x - w, self.y(), w, self.height())
    }

    /// Split `(strip, remainder)` off the top by `h` pixels.
    /// Convenient for laying out a vertical stack:
    ///
    /// ```ignore
    /// let (header, rest) = body.split_off_top(48.0);
    /// let (footer, content) = rest.split_off_bottom(56.0);
    /// ```
    pub fn split_off_top(&self, h: f32) -> (Rect, Rect) {
        let h = h.clamp(0.0, self.height());
        let top = Rect::from_xywh(self.x(), self.y(), self.width(), h);
        let rest = Rect::from_xywh(self.x(), self.y() + h, self.width(), self.height() - h);
        (top, rest)
    }
    /// Split `(remainder, strip)` off the bottom by `h` pixels.
    pub fn split_off_bottom(&self, h: f32) -> (Rect, Rect) {
        let h = h.clamp(0.0, self.height());
        let rest = Rect::from_xywh(self.x(), self.y(), self.width(), self.height() - h);
        let bot = Rect::from_xywh(self.x(), self.max.y - h, self.width(), h);
        (rest, bot)
    }
    /// Split `(strip, remainder)` off the left by `w` pixels.
    pub fn split_off_left(&self, w: f32) -> (Rect, Rect) {
        let w = w.clamp(0.0, self.width());
        let left = Rect::from_xywh(self.x(), self.y(), w, self.height());
        let rest = Rect::from_xywh(self.x() + w, self.y(), self.width() - w, self.height());
        (left, rest)
    }
    /// Split `(remainder, strip)` off the right by `w` pixels.
    pub fn split_off_right(&self, w: f32) -> (Rect, Rect) {
        let w = w.clamp(0.0, self.width());
        let rest = Rect::from_xywh(self.x(), self.y(), self.width() - w, self.height());
        let right = Rect::from_xywh(self.max.x - w, self.y(), w, self.height());
        (rest, right)
    }
}

/// Per-side padding in pixels. Used by [`Rect::shrink2`] and the
/// `Frame` widget.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pad {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

impl Pad {
    pub const ZERO: Pad = Pad {
        left: 0.0,
        right: 0.0,
        top: 0.0,
        bottom: 0.0,
    };
    pub const fn all(v: f32) -> Self {
        Self {
            left: v,
            right: v,
            top: v,
            bottom: v,
        }
    }
    pub const fn symmetric(h: f32, v: f32) -> Self {
        Self {
            left: h,
            right: h,
            top: v,
            bottom: v,
        }
    }

    /// Multiply every side by `s`. Used by `Theme::with_scale`
    /// to bake the per-frame UI scale into spacing tokens.
    pub fn scaled(self, s: f32) -> Self {
        Self {
            left: self.left * s,
            right: self.right * s,
            top: self.top * s,
            bottom: self.bottom * s,
        }
    }
}
