//! Cross-frame UI state.
//!
//! Lives in `GameState` (or wherever the screen owner sits) and
//! is borrowed by [`Ui::begin`](super::ui::Ui::begin) each frame.
//! Holds the *minimum* state that has to survive between frames:
//! focus, hover from the previous frame (this frame consults it
//! and writes the next-frame value), drag-in-progress, and the
//! modal stack.

use super::id::Id;
use super::rect::Pos2;

/// Tracks active drag-and-drop. The payload is type-erased so
/// the destination widget can downcast to whatever the source
/// pushed (item-row index, ability-slot index, etc.).
#[derive(Debug)]
pub struct DragState {
    /// Widget that initiated the drag.
    pub source: Id,
    /// Mouse position when the press happened, used to enforce a
    /// minimum movement threshold before `drag_started` fires.
    pub press_pos: Pos2,
    /// Whether the mouse has crossed the threshold (transition
    /// `false -> true` is what surfaces `drag_started`).
    pub active: bool,
    /// Type-erased payload. Destinations call `downcast_ref::<T>()`.
    pub payload: Box<dyn std::any::Any + Send + Sync>,
}

impl DragState {
    /// Build a fresh latent drag (`active = false`).
    pub fn new<T: 'static + Send + Sync>(source: Id, press_pos: Pos2, payload: T) -> Self {
        Self {
            source,
            press_pos,
            active: false,
            payload: Box::new(payload),
        }
    }
}

/// One entry on the modal stack. The top entry intercepts mouse
/// + keyboard above its layer; everything below is greyed out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Modal {
    pub id: Id,
}

/// Persistent UI state. Owned by the game state; passed by
/// `&mut` to `Ui::begin` once per frame.
#[derive(Default)]
pub struct UiState {
    /// Widget that currently owns keyboard focus (text field,
    /// pressed button about to fire on release, …).
    pub focus: Option<Id>,
    /// Widget the cursor was over at the *end* of the previous
    /// frame. Read at the start of the next frame to decide
    /// whether widgets should render in their hover style.
    pub hovered_last_frame: Option<Id>,
    /// Hover candidate computed this frame — promoted to
    /// `hovered_last_frame` at end-of-frame.
    pub(super) hovered_this_frame: Option<Id>,
    /// Active drag, if any.
    pub drag: Option<DragState>,
    /// Modal stack; topmost intercepts input.
    pub modals: Vec<Modal>,
    /// Set this frame by any widget that consumed the mouse
    /// (button press absorbed, drag started, hover inside a
    /// panel rect). Game code reads this via `Ui::end()` to
    /// decide whether to forward the click to the world (cast
    /// abilities, target picker, …).
    pub(super) mouse_claimed: bool,
    /// Same idea for keyboard: a focused text field claims
    /// keystrokes so WASD movement doesn't fire while typing.
    pub(super) keyboard_claimed: bool,
}

impl UiState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push `id` onto the modal stack. Use the `RAII` guard
    /// returned by `Ui::modal` once that exists; this is the
    /// raw plumbing.
    pub fn push_modal(&mut self, id: Id) {
        if !self.modals.iter().any(|m| m.id == id) {
            self.modals.push(Modal { id });
        }
    }

    /// Pop the top modal. No-op if `id` isn't the topmost (so
    /// widgets can defensively `pop` without worrying about
    /// stack corruption when another modal opened on top).
    pub fn pop_modal(&mut self, id: Id) {
        if self.modals.last().map(|m| m.id) == Some(id) {
            self.modals.pop();
        }
    }

    /// Topmost modal's id, if any.
    pub fn top_modal(&self) -> Option<Id> {
        self.modals.last().map(|m| m.id)
    }
}
