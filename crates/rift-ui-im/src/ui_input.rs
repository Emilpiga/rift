//! Input-reading trait used by the immediate-mode UI.
//!
//! `rift-engine::input::Input` is the real implementation;
//! widgets see only this trait so the UI crate doesn't need to
//! depend on `winit`. The key-event channel uses [`KeyEvent`]
//! / [`ImKey`] rather than `winit::keyboard::KeyCode` so the
//! crate has no winit-shaped surface.

use crate::im_key::ImKey;

/// A single non-character key event from this frame. Auto-repeat
/// presses are included so a held arrow key produces multiple
/// entries at the OS repeat rate — text-input widgets walk the
/// slice from [`UiInput::key_events`] in order.
pub type KeyEvent = ImKey;

/// What widgets read from. Implemented by
/// `rift-engine::input::Input`.
///
/// Methods come in two flavours:
///
/// * "gameplay" pollers ([`is_key_held`](Self::is_key_held),
///   [`key_just_pressed`](Self::key_just_pressed)) — suppressed
///   while text capture is on.
/// * "raw" pollers (`_raw` suffix) — bypass text capture,
///   intended for widget-internal use (modifier reads in a
///   text field).
pub trait UiInput {
    // ─── keyboard (gameplay-style, text-capture aware) ──────────
    fn is_key_held(&self, key: ImKey) -> bool;
    fn key_just_pressed(&self, key: ImKey) -> bool;

    // ─── keyboard (raw, bypasses text capture) ──────────────────
    fn is_key_held_raw(&self, key: ImKey) -> bool;
    fn key_just_pressed_raw(&self, key: ImKey) -> bool;

    // ─── text input (widget-style) ──────────────────────────────
    fn chars_typed(&self) -> &[char];
    fn backspace_count(&self) -> u32;
    fn delete_count(&self) -> u32;
    fn key_events(&self) -> &[KeyEvent];
    fn enter_just_pressed(&self) -> bool;
    /// Drop any text-input events buffered for this frame.
    fn discard_text_input(&self);

    // ─── mouse ──────────────────────────────────────────────────
    fn mouse_pos(&self) -> (f32, f32);
    fn left_mouse_held(&self) -> bool;
    fn left_just_pressed(&self) -> bool;
    fn left_just_released(&self) -> bool;
    /// Rising-edge left click. Consumes the click.
    fn left_clicked(&self) -> bool;
    /// Rising-edge right click. Consumes the click.
    fn right_clicked(&self) -> bool;
    /// Vertical scroll wheel delta (positive = scroll up /
    /// toward the user) accumulated this frame.
    fn scroll_delta(&self) -> f32;

    // ─── text-capture flag (set by the focus owner) ─────────────
    fn set_text_capture(&self, on: bool);
}
