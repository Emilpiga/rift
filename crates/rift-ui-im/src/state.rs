//! Cross-frame UI state.
//!
//! Lives in `GameState` (or wherever the screen owner sits) and
//! is borrowed by [`Ui::begin`](super::ui::Ui::begin) each frame.
//! Holds the *minimum* state that has to survive between frames:
//! focus, hover from the previous frame (this frame consults it
//! and writes the next-frame value), drag-in-progress, and the
//! modal stack.

use super::id::Id;
use super::rect::{Pos2, Rect};
use std::collections::HashMap;

/// Tracks active drag-and-drop. The payload is type-erased so
/// the destination widget can downcast to whatever the source
/// pushed (item-row index, ability-slot index, etc.).
#[derive(Debug)]
pub struct DragState {
    /// Widget that initiated the drag.
    pub source: Id,
    /// Rect of the source widget at press time. Used to derive
    /// the *grab offset* (where inside the item the user
    /// clicked) so the in-flight ghost can preserve the
    /// cursor's relative position within the item rather than
    /// snapping to its centre.
    pub source_rect: Rect,
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
    pub fn new<T: 'static + Send + Sync>(
        source: Id,
        source_rect: Rect,
        press_pos: Pos2,
        payload: T,
    ) -> Self {
        Self {
            source,
            source_rect,
            press_pos,
            active: false,
            payload: Box::new(payload),
        }
    }
}

/// Editor state for whichever text field currently owns
/// keyboard focus. Holds the selection range as byte offsets
/// into the field's `&mut String`. `anchor == caret` means no
/// active selection (just a caret); `anchor != caret` means a
/// range is selected. Reset whenever focus moves to a different
/// id so the new field starts with a fresh, end-anchored caret.
#[derive(Debug, Clone, Copy, Default)]
pub struct TextSelection {
    /// Byte offset where a click / Shift-anchor was placed.
    pub anchor: usize,
    /// Byte offset where the caret currently sits. Always lies
    /// on a UTF-8 char boundary of the underlying string.
    pub caret: usize,
}

impl TextSelection {
    /// `true` iff `anchor != caret` — i.e. a non-empty span is
    /// currently selected. Editing operations branch on this.
    pub fn has_range(&self) -> bool {
        self.anchor != self.caret
    }

    /// `(min, max)` byte range, ordered. Empty when `has_range`
    /// is false.
    pub fn range(&self) -> (usize, usize) {
        if self.anchor <= self.caret {
            (self.anchor, self.caret)
        } else {
            (self.caret, self.anchor)
        }
    }
}

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
    /// Selection / caret state for the focused text field.
    /// Reset to default whenever `focus` changes id (the
    /// `TextField` widget detects the focus transition and
    /// re-seeds this with caret-at-end on first focus). For
    /// non-text-field focus owners (e.g. a button) it sits
    /// idle.
    pub text_selection: TextSelection,
    /// `true` while the left mouse button is held *and* the
    /// initial press happened inside the focused text field —
    /// drives mouse-drag selection. Cleared on release.
    pub text_drag: bool,
    /// Widget the cursor was over at the *end* of the previous
    /// frame. Read at the start of the next frame to decide
    /// whether widgets should render in their hover style.
    pub hovered_last_frame: Option<Id>,
    /// Hover candidate computed this frame — promoted to
    /// `hovered_last_frame` at end-of-frame.
    pub(super) hovered_this_frame: Option<Id>,
    /// Active drag, if any.
    pub drag: Option<DragState>,
    /// Button id currently being held down (mouse pressed
    /// inside its rect, not yet released). Buttons fire their
    /// `clicked` flag on *release inside the same rect*, not
    /// on the down-edge — this matches platform UX (cancel a
    /// click by dragging off before releasing). Cleared
    /// unconditionally on every `left_just_released`.
    pub pressed_button: Option<Id>,
    /// Modal stack; topmost intercepts input.
    pub modals: Vec<Modal>,
    /// Visual-only animation state for the bottom HUD vitals.
    pub vitals: VitalsAnimState,
    /// Visual-only animation state for the top-center rift
    /// progress meter.
    pub rift_progress: RiftProgressAnimState,
    /// Visual-only animation state for health / essence bars
    /// anchored to world entities. Keys are supplied by the
    /// caller so this crate stays independent of the ECS type.
    pub world_vitals: HashMap<u64, WorldVitalsAnimState>,
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

#[derive(Debug, Clone, Copy, Default)]
pub struct VitalsAnimState {
    pub hp: ResourceBarAnim,
    pub essence: ResourceBarAnim,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WorldVitalsAnimState {
    pub hp: ResourceBarAnim,
    pub essence: ResourceBarAnim,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RiftProgressAnimState {
    pub bar: ResourceBarAnim,
    pub flow: f32,
}

impl RiftProgressAnimState {
    pub fn tick(&mut self, target: f32, dt: f32) {
        let dt = dt.clamp(1.0 / 240.0, 1.0 / 20.0);
        self.bar.tick(target, dt);
        self.flow = (self.flow + dt * (0.42 + self.bar.pulse * 0.55)).fract();
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ResourceBarAnim {
    pub displayed: f32,
    pub trail: f32,
    pub last_target: f32,
    pub pulse: f32,
    pub initialized: bool,
}

impl Default for ResourceBarAnim {
    fn default() -> Self {
        Self {
            displayed: 1.0,
            trail: 1.0,
            last_target: 1.0,
            pulse: 0.0,
            initialized: false,
        }
    }
}

impl ResourceBarAnim {
    pub fn tick(&mut self, target: f32, dt: f32) {
        let target = target.clamp(0.0, 1.0);
        if !self.initialized {
            self.displayed = target;
            self.trail = target;
            self.last_target = target;
            self.initialized = true;
            return;
        }

        let dt = dt.clamp(1.0 / 240.0, 1.0 / 20.0);
        let lost = target < self.last_target - 0.002;
        let gained = target > self.last_target + 0.002;
        if lost {
            self.trail = self.trail.max(self.displayed).max(self.last_target);
            self.pulse = 1.0;
        } else if gained {
            self.trail = self.trail.max(target);
            self.pulse = self.pulse.max(0.35);
        }

        let fill_rate = if target < self.displayed { 16.0 } else { 22.0 };
        let trail_rate = if target < self.trail { 5.5 } else { 18.0 };
        self.displayed = approach_exp(self.displayed, target, fill_rate, dt);
        self.trail = approach_exp(self.trail, target.max(self.displayed), trail_rate, dt);
        self.pulse = approach_exp(self.pulse, 0.0, 4.8, dt);
        self.last_target = target;
    }
}

fn approach_exp(current: f32, target: f32, rate: f32, dt: f32) -> f32 {
    let alpha = 1.0 - (-rate * dt).exp();
    current + (target - current) * alpha
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
