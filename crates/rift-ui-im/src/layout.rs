//! Linear layout primitives: [`Row`] and [`Column`].
//!
//! Replaces the per-screen pattern of arithmetic on `Rect`s
//! and pixel literals (`body.x() + 150.0 * s`,
//! `body.width() * 0.55`, etc.) with a tiny, allocation-free
//! main-axis splitter. Use it like:
//!
//! ```ignore
//! use rift_ui_im::{Row, Sized, Frame, Button, ButtonSize};
//!
//! Frame::stone(theme).show(ui, panel, |ui, body| {
//!     // Footer: PLAY (red, fills) | DELETE (fixed) | QUIT (fixed)
//!     let footer = body.split_bottom(56.0);
//!     let cells = Row::new(footer)
//!         .gap(12.0)
//!         .item(Sized::flex(1.0))
//!         .item(Sized::fixed(140.0))
//!         .item(Sized::fixed(140.0))
//!         .layout();
//!
//!     if Button::red("PLAY").size(ButtonSize::Large)
//!         .show_with_id(ui, id.child("play"), cells[0]).clicked { /* … */ }
//!     // …
//! });
//! ```
//!
//! Design choices:
//!
//! * **Returns rects, doesn't draw.** The caller still owns
//!   widget choice and ids — Row/Column just hand back
//!   `Vec<Rect>` (one per item), in order. This matches the
//!   rest of the immediate-mode crate (no parent/child
//!   widget tree, no push/pop balance bugs) and means the
//!   tooling cost of adopting it incrementally is "replace
//!   the arithmetic, keep the widget calls".
//! * **No allocation past a small inline buffer.** The hot
//!   path stores up to 8 items inline; layouts beyond that
//!   spill to a `Vec`. UI screens rarely exceed 4–6 cells in
//!   one row.
//! * **Cross-axis is the rect itself.** A `Row` lays out
//!   along x and every cell inherits the row's full height.
//!   A `Column` is the transpose. For mixed alignment within
//!   a cell, nest layouts or call `Rect::align_center` /
//!   `shrink2` on the returned cell.

use crate::rect::{Pos2, Rect};

/// Sizing for one item along a [`Row`] / [`Column`]'s main
/// axis. `Fixed(px)` reserves a literal pixel size; `Flex(w)`
/// shares the remaining space proportionally to its weight.
///
/// Common cases:
/// * `Sized::fixed(120.0)` — a 120 px wide button.
/// * `Sized::flex(1.0)`    — fill the rest (most common).
/// * Two `Sized::flex(1.0)` cells split the remainder 50/50.
/// * `Sized::flex(2.0)` + `Sized::flex(1.0)` → 2/3 + 1/3.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sized {
    Fixed(f32),
    Flex(f32),
}

impl Sized {
    pub const fn fixed(px: f32) -> Self {
        Self::Fixed(px)
    }
    pub const fn flex(weight: f32) -> Self {
        Self::Flex(weight)
    }
}

/// Convert raw pixel/weight values into [`Sized`] without
/// going through the constructor. Lets callers write
/// `.item(120.0)` for fixed and `.item(Sized::flex(1.0))`
/// for flex without losing intent.
impl From<f32> for Sized {
    fn from(px: f32) -> Self {
        Sized::Fixed(px)
    }
}

/// Cross-axis alignment within a cell. Defaults to [`Stretch`]
/// (cell fills the layout's cross axis) which is the common
/// case for button rows and form fields.
///
/// For text labels you usually want to leave the layout in
/// stretch mode and call `Rect::align_center` / a manual
/// y-offset on the returned cell, since text rendering
/// already has its own baseline rules.
///
/// [`Stretch`]: CrossAlign::Stretch
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CrossAlign {
    #[default]
    Stretch,
    Start,
    Center,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Horizontal,
    Vertical,
}

/// Linear main-axis splitter. Use [`Row`] / [`Column`] type
/// aliases below — `Linear` itself is private surface area.
#[derive(Debug)]
struct Linear {
    rect: Rect,
    axis: Axis,
    gap: f32,
    cross: CrossAlign,
    cross_size: Option<f32>,
    items: SmallVec<Sized>,
}

impl Linear {
    fn new(rect: Rect, axis: Axis) -> Self {
        Self {
            rect,
            axis,
            gap: 0.0,
            cross: CrossAlign::Stretch,
            cross_size: None,
            items: SmallVec::new(),
        }
    }

    fn item(mut self, s: impl Into<Sized>) -> Self {
        self.items.push(s.into());
        self
    }

    fn gap(mut self, gap: f32) -> Self {
        self.gap = gap;
        self
    }

    fn cross_align(mut self, a: CrossAlign) -> Self {
        self.cross = a;
        self
    }

    fn cross_size(mut self, px: f32) -> Self {
        self.cross_size = Some(px);
        self
    }

    fn layout(self) -> Vec<Rect> {
        let n = self.items.len();
        if n == 0 {
            return Vec::new();
        }
        let (main_extent, cross_extent) = match self.axis {
            Axis::Horizontal => (self.rect.width(), self.rect.height()),
            Axis::Vertical => (self.rect.height(), self.rect.width()),
        };
        let total_gap = self.gap * (n as f32 - 1.0).max(0.0);
        let mut fixed_total = 0.0;
        let mut weight_total = 0.0;
        for s in self.items.iter() {
            match s {
                Sized::Fixed(px) => fixed_total += px.max(0.0),
                Sized::Flex(w) => weight_total += w.max(0.0),
            }
        }
        let flex_pool = (main_extent - total_gap - fixed_total).max(0.0);
        let per_weight = if weight_total > 0.0 {
            flex_pool / weight_total
        } else {
            0.0
        };

        // Cross-axis size: cell either fills the layout rect
        // (Stretch) or uses the explicit cross_size; alignment
        // then shifts the cell along the cross axis.
        let cell_cross = match (self.cross, self.cross_size) {
            (CrossAlign::Stretch, _) => cross_extent,
            (_, Some(c)) => c.min(cross_extent),
            // Without an explicit cross_size, non-stretch
            // alignments still need a cross extent — fall back
            // to filling the rect (i.e. Stretch). Callers that
            // want a tighter cell should set `cross_size`.
            (_, None) => cross_extent,
        };
        let cross_offset = match self.cross {
            CrossAlign::Stretch | CrossAlign::Start => 0.0,
            CrossAlign::Center => (cross_extent - cell_cross) * 0.5,
            CrossAlign::End => cross_extent - cell_cross,
        };

        let mut out = Vec::with_capacity(n);
        let mut cursor = 0.0_f32;
        for (i, s) in self.items.iter().enumerate() {
            let main = match s {
                Sized::Fixed(px) => px.max(0.0),
                Sized::Flex(w) => w.max(0.0) * per_weight,
            };
            let cell = match self.axis {
                Axis::Horizontal => Rect::from_min_size(
                    Pos2::new(self.rect.x() + cursor, self.rect.y() + cross_offset),
                    crate::rect::Vec2::new(main, cell_cross),
                ),
                Axis::Vertical => Rect::from_min_size(
                    Pos2::new(self.rect.x() + cross_offset, self.rect.y() + cursor),
                    crate::rect::Vec2::new(cell_cross, main),
                ),
            };
            out.push(cell);
            cursor += main;
            if i + 1 < n {
                cursor += self.gap;
            }
        }
        out
    }
}

/// Horizontal splitter. Items lay out left-to-right, every
/// cell inherits the row's full height (or the configured
/// cross size). See module docs for usage.
pub struct Row(Linear);

impl Row {
    pub fn new(rect: Rect) -> Self {
        Self(Linear::new(rect, Axis::Horizontal))
    }
    pub fn item(mut self, s: impl Into<Sized>) -> Self {
        self.0 = self.0.item(s);
        self
    }
    /// Append `n` flex(1.0) items in one call. Convenience for
    /// the common "split into N equal cells" pattern.
    pub fn equal(mut self, n: usize) -> Self {
        for _ in 0..n {
            self.0 = self.0.item(Sized::flex(1.0));
        }
        self
    }
    pub fn gap(mut self, gap: f32) -> Self {
        self.0 = self.0.gap(gap);
        self
    }
    pub fn cross_align(mut self, a: CrossAlign) -> Self {
        self.0 = self.0.cross_align(a);
        self
    }
    pub fn cross_size(mut self, px: f32) -> Self {
        self.0 = self.0.cross_size(px);
        self
    }
    pub fn layout(self) -> Vec<Rect> {
        self.0.layout()
    }
}

/// Vertical splitter. Items lay out top-to-bottom, every cell
/// inherits the column's full width. See module docs.
pub struct Column(Linear);

impl Column {
    pub fn new(rect: Rect) -> Self {
        Self(Linear::new(rect, Axis::Vertical))
    }
    pub fn item(mut self, s: impl Into<Sized>) -> Self {
        self.0 = self.0.item(s);
        self
    }
    pub fn equal(mut self, n: usize) -> Self {
        for _ in 0..n {
            self.0 = self.0.item(Sized::flex(1.0));
        }
        self
    }
    pub fn gap(mut self, gap: f32) -> Self {
        self.0 = self.0.gap(gap);
        self
    }
    pub fn cross_align(mut self, a: CrossAlign) -> Self {
        self.0 = self.0.cross_align(a);
        self
    }
    pub fn cross_size(mut self, px: f32) -> Self {
        self.0 = self.0.cross_size(px);
        self
    }
    pub fn layout(self) -> Vec<Rect> {
        self.0.layout()
    }
}

// ─── inline small-vector ───────────────────────────────────────
//
// Keeps small layouts (≤ INLINE) allocation-free. The standard
// `smallvec` crate would fit, but we ship a 30-line specialised
// version to avoid pulling in a new dependency for one type.

const INLINE: usize = 8;

#[derive(Debug)]
struct SmallVec<T: Copy> {
    inline: [std::mem::MaybeUninit<T>; INLINE],
    len: usize,
    spill: Vec<T>,
}

impl<T: Copy> SmallVec<T> {
    fn new() -> Self {
        Self {
            inline: [std::mem::MaybeUninit::uninit(); INLINE],
            len: 0,
            spill: Vec::new(),
        }
    }
    fn push(&mut self, v: T) {
        if !self.spill.is_empty() {
            self.spill.push(v);
        } else if self.len < INLINE {
            self.inline[self.len].write(v);
            self.len += 1;
        } else {
            // Promote to spill.
            let mut moved: Vec<T> = (0..self.len)
                .map(|i| unsafe { self.inline[i].assume_init() })
                .collect();
            moved.push(v);
            self.spill = moved;
        }
    }
    fn len(&self) -> usize {
        if self.spill.is_empty() {
            self.len
        } else {
            self.spill.len()
        }
    }
    fn iter(&self) -> SmallVecIter<'_, T> {
        SmallVecIter { v: self, i: 0 }
    }
}

impl<T: Copy> Drop for SmallVec<T> {
    fn drop(&mut self) {
        // T is Copy → no destructors to run; nothing to do.
    }
}

struct SmallVecIter<'a, T: Copy> {
    v: &'a SmallVec<T>,
    i: usize,
}

impl<'a, T: Copy> Iterator for SmallVecIter<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<&'a T> {
        if !self.v.spill.is_empty() {
            let r = self.v.spill.get(self.i)?;
            self.i += 1;
            Some(r)
        } else if self.i < self.v.len {
            // Safety: indices < len are initialised.
            let r = unsafe { &*self.v.inline[self.i].as_ptr() };
            self.i += 1;
            Some(r)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_xywh(x, y, w, h)
    }

    #[test]
    fn row_two_equal_no_gap() {
        let cells = Row::new(r(0.0, 0.0, 100.0, 40.0)).equal(2).layout();
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0], r(0.0, 0.0, 50.0, 40.0));
        assert_eq!(cells[1], r(50.0, 0.0, 50.0, 40.0));
    }

    #[test]
    fn row_fixed_then_flex() {
        let cells = Row::new(r(10.0, 5.0, 200.0, 30.0))
            .gap(10.0)
            .item(Sized::fixed(60.0))
            .item(Sized::flex(1.0))
            .layout();
        assert_eq!(cells[0], r(10.0, 5.0, 60.0, 30.0));
        // 200 - 60 - 10 = 130
        assert_eq!(cells[1], r(80.0, 5.0, 130.0, 30.0));
    }

    #[test]
    fn column_three_equal_with_gap() {
        let cells = Column::new(r(0.0, 0.0, 50.0, 100.0))
            .gap(10.0)
            .equal(3)
            .layout();
        // (100 - 20) / 3 = 26.666...
        let h = (100.0 - 20.0) / 3.0;
        assert_eq!(cells.len(), 3);
        assert!((cells[0].height() - h).abs() < 1e-3);
        assert!((cells[1].y() - (h + 10.0)).abs() < 1e-3);
        assert!((cells[2].y() - (h * 2.0 + 20.0)).abs() < 1e-3);
    }

    #[test]
    fn weighted_flex() {
        let cells = Row::new(r(0.0, 0.0, 90.0, 10.0))
            .item(Sized::flex(2.0))
            .item(Sized::flex(1.0))
            .layout();
        assert_eq!(cells[0].width(), 60.0);
        assert_eq!(cells[1].width(), 30.0);
    }

    #[test]
    fn cross_center_with_size() {
        let cells = Row::new(r(0.0, 0.0, 100.0, 50.0))
            .cross_align(CrossAlign::Center)
            .cross_size(20.0)
            .equal(1)
            .layout();
        assert_eq!(cells[0], r(0.0, 15.0, 100.0, 20.0));
    }
}
