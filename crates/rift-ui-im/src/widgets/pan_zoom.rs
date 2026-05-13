//! Pan + zoom viewport helper.
//!
//! Generic widget for graph-shaped panels (talent tree, world
//! map, debug overlays) that need a draggable, scroll-zoomable
//! 2D canvas. The widget owns no persistent state — the host
//! threads a [`PanZoomState`] through every frame, mirroring
//! how `TextField` works.
//!
//! ## Coordinate spaces
//!
//! Two spaces are in play:
//!
//! - **World** — author-defined positions for nodes / edges
//!   (origin = wherever you want the canvas "centred").
//! - **Screen** — pixel coords on the host viewport (top-left
//!   origin), what `Ui::draw_*` and `Ui::input` use.
//!
//! [`PanZoomTransform`] is the value the widget returns; call
//! [`world_to_screen`](PanZoomTransform::world_to_screen) on
//! each point you want to draw and [`scale`](PanZoomTransform::scale)
//! on any pixel-length (radii, thicknesses) so they shrink
//! with the zoom-out.
//!
//! ## Interaction model
//!
//! - **Drag-pan** — left-mouse press inside [`PanZoom::viewport`]
//!   (and outside any sub-widget that has already claimed the
//!   mouse this frame) starts a drag; cursor delta is applied
//!   to `state.pan` until release.
//! - **Scroll-zoom** — vertical scroll while hovering the
//!   viewport multiplies `state.zoom` by `zoom_step` per wheel
//!   notch, anchored on the cursor so the point under the
//!   mouse stays put. Clamped to `[min_zoom, max_zoom]`.
//! - The widget claims neither the drag-source nor the
//!   keyboard channel — sub-widgets layered on top can still
//!   take clicks via [`Ui::interact_hover`] before the
//!   pan-drag falls through.

use crate::id::Id;
use crate::rect::{Pos2, Rect, Vec2};
use crate::ui::Ui;

/// Persistent state the host threads through each frame. Plain
/// data; serialise as you like.
#[derive(Clone, Debug)]
pub struct PanZoomState {
    /// Translation applied to the world origin, in screen
    /// pixels (before scaling). `(0, 0)` means the world
    /// origin sits at the viewport's centre.
    pub pan: Vec2,
    /// Multiplier applied to every world-space length. Clamped
    /// each frame to `[min_zoom, max_zoom]`.
    pub zoom: f32,
    /// True while a drag-pan is in progress (LMB held). Reset
    /// on release. Exposed so the host can suppress hover
    /// tooltips while scrubbing.
    pub dragging: bool,
    /// Last cursor position seen during a drag. Used internally
    /// to compute the per-frame delta; meaningless when
    /// `dragging == false`.
    pub last_cursor: Pos2,
}

impl Default for PanZoomState {
    fn default() -> Self {
        Self {
            pan: Vec2::ZERO,
            zoom: 1.0,
            dragging: false,
            last_cursor: Pos2::new(0.0, 0.0),
        }
    }
}

impl PanZoomState {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Configurable viewport. Cheap struct; build, configure, `.show()`.
#[derive(Clone, Debug)]
pub struct PanZoom {
    /// Pixel rect the canvas lives in. Mouse input outside
    /// this rect is ignored.
    pub viewport: Rect,
    /// Minimum zoom multiplier (clamped each frame).
    pub min_zoom: f32,
    /// Maximum zoom multiplier (clamped each frame).
    pub max_zoom: f32,
    /// Multiplier applied per scroll-wheel notch. Values
    /// above 1.0 mean "scroll up = zoom in".
    pub zoom_step: f32,
}

impl PanZoom {
    pub fn new(viewport: Rect) -> Self {
        Self {
            viewport,
            min_zoom: 0.5,
            max_zoom: 2.0,
            zoom_step: 1.1,
        }
    }

    pub fn zoom_range(mut self, min: f32, max: f32) -> Self {
        self.min_zoom = min;
        self.max_zoom = max;
        self
    }

    pub fn zoom_step(mut self, step: f32) -> Self {
        self.zoom_step = step;
        self
    }

    /// Update `state` from this frame's input and return the
    /// transform to use when drawing inside the viewport.
    ///
    /// `id` registers the viewport as a hover/drag claimant so
    /// nested widgets that ran first (and already claimed the
    /// mouse) keep priority. Pass a stable id derived from the
    /// host panel.
    pub fn show(self, ui: &mut Ui<'_>, id: Id, state: &mut PanZoomState) -> PanZoomTransform {
        // Clamp incoming zoom so a corrupt save can't render
        // a node a million pixels wide on the first frame.
        state.zoom = state.zoom.clamp(self.min_zoom, self.max_zoom);

        // Hit-test viewport. `hovered` is true only when the
        // cursor is over our rect *and* no overlapping widget
        // drawn later in the same frame claims it — we run
        // before sub-widgets, so we only steal when nothing
        // else has already taken the mouse.
        let hovered = ui.interact_hover(id, self.viewport);
        let input = ui.input();
        let (mx, my) = input.mouse_pos();
        let cursor = Pos2::new(mx, my);
        let left_held = input.left_mouse_held();
        let left_pressed = input.left_just_pressed();
        let scroll = input.scroll_delta();

        // ── Drag-pan ────────────────────────────────────────
        if state.dragging {
            if left_held {
                let dx = cursor.x - state.last_cursor.x;
                let dy = cursor.y - state.last_cursor.y;
                state.pan.x += dx;
                state.pan.y += dy;
                state.last_cursor = cursor;
            } else {
                state.dragging = false;
            }
        } else if hovered && left_pressed {
            state.dragging = true;
            state.last_cursor = cursor;
        }

        // ── Scroll-zoom ─────────────────────────────────────
        if hovered && scroll.abs() > f32::EPSILON {
            // Anchor on cursor: keep the world point currently
            // under the mouse stationary as zoom changes. This
            // is the standard image-viewer feel.
            let centre = self.viewport.center();
            let old_zoom = state.zoom;
            let factor = if scroll > 0.0 {
                self.zoom_step
            } else {
                1.0 / self.zoom_step
            };
            let new_zoom = (old_zoom * factor).clamp(self.min_zoom, self.max_zoom);
            if (new_zoom - old_zoom).abs() > f32::EPSILON {
                // World coord under cursor pre-zoom:
                //   (cursor - centre - pan) / old_zoom
                // To keep that world coord under the cursor at
                // new_zoom, we need:
                //   pan_new = cursor - centre - world * new_zoom
                let wx = (cursor.x - centre.x - state.pan.x) / old_zoom;
                let wy = (cursor.y - centre.y - state.pan.y) / old_zoom;
                state.pan.x = cursor.x - centre.x - wx * new_zoom;
                state.pan.y = cursor.y - centre.y - wy * new_zoom;
                state.zoom = new_zoom;
            }
        }

        PanZoomTransform {
            pan: state.pan,
            zoom: state.zoom,
            viewport_centre: self.viewport.center(),
            viewport: self.viewport,
        }
    }
}

/// Mapping between world-space and screen-space pixels for the
/// current frame. Returned by [`PanZoom::show`] — cheap to
/// copy.
#[derive(Copy, Clone, Debug)]
pub struct PanZoomTransform {
    pub pan: Vec2,
    pub zoom: f32,
    pub viewport_centre: Pos2,
    pub viewport: Rect,
}

impl PanZoomTransform {
    /// Map a world-space point to a screen-space pixel.
    pub fn world_to_screen(&self, p: Pos2) -> Pos2 {
        Pos2::new(
            self.viewport_centre.x + self.pan.x + p.x * self.zoom,
            self.viewport_centre.y + self.pan.y + p.y * self.zoom,
        )
    }

    /// Inverse of [`Self::world_to_screen`].
    pub fn screen_to_world(&self, p: Pos2) -> Pos2 {
        if self.zoom.abs() <= f32::EPSILON {
            return Pos2::new(0.0, 0.0);
        }
        Pos2::new(
            (p.x - self.viewport_centre.x - self.pan.x) / self.zoom,
            (p.y - self.viewport_centre.y - self.pan.y) / self.zoom,
        )
    }

    /// Scale a pixel-length (radius, thickness, …) by the
    /// current zoom so it shrinks with zoom-out.
    pub fn scale(&self, length: f32) -> f32 {
        length * self.zoom
    }
}
