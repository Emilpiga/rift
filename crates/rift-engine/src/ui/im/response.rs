//! Per-frame return value from interactive widgets.
//!
//! Replaces the ad-hoc `bool` returns sprinkled across the
//! current UI surfaces. Composite widgets compose by returning
//! their inner-most response (or aggregating when they wrap
//! multiple interactive children).
//!
//! ```ignore
//! if ui.button("Save").clicked {
//!     persist();
//! }
//! ```

use super::id::Id;
use super::rect::Rect;

/// Outcome of a single interactive widget for this frame.
#[derive(Debug, Clone, Copy)]
pub struct Response {
    /// Stable id used for focus / hover bookkeeping. May be
    /// [`Id::NONE`] for non-interactive returns (e.g. a panel
    /// frame's body rect, exposed for layout chaining).
    pub id: Id,
    /// Pixel rect actually allocated by the widget.
    pub rect: Rect,
    /// Mouse is over the widget rect this frame, and no higher
    /// layer (modal, tooltip) is intercepting the cursor.
    pub hovered: bool,
    /// Mouse button was pressed this frame inside the widget.
    pub pressed: bool,
    /// Mouse button was released inside the widget after a
    /// matching press inside it ("clean click"). False if the
    /// press started elsewhere.
    pub clicked: bool,
    /// Drag started this frame on this widget (press inside it
    /// followed by movement past the threshold).
    pub drag_started: bool,
    /// Mouse was released this frame while a drag was in progress.
    /// Note: fires on the *source* widget, not the destination.
    pub drag_released: bool,
    /// `true` if the widget currently owns keyboard focus.
    pub focused: bool,
}

impl Response {
    /// Convenience for non-interactive surfaces (panel bodies,
    /// spacers). Sets every flag to `false`.
    pub fn inert(rect: Rect) -> Self {
        Self {
            id: Id::NONE,
            rect,
            hovered: false,
            pressed: false,
            clicked: false,
            drag_started: false,
            drag_released: false,
            focused: false,
        }
    }
}
