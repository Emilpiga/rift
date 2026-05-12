//! Small, winit-free key enum for the immediate-mode UI.
//!
//! Only the variants the widget code actually reads are listed.
//! `rift-engine::input::Input::on_key_event` translates from
//! `winit::keyboard::KeyCode` to this enum at the engine
//! boundary; widgets never see winit.

/// Subset of `winit::keyboard::KeyCode` that the immediate-mode
/// widgets read. New variants get added here on demand, then
/// mapped in the engine's `from_winit_key` adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImKey {
    // Modifiers
    ShiftLeft,
    ShiftRight,
    ControlLeft,
    ControlRight,
    AltLeft,
    AltRight,

    // Navigation / editing
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Home,
    End,
    Escape,
    Enter,
    Tab,
    Backspace,
    Delete,

    // Letter keys (only the ones widgets / screens actually bind to)
    KeyA,
    KeyB,
    KeyC,
    KeyR,
    KeyT,
    KeyV,
    KeyX,
    KeyZ,

    // Punctuation
    Slash,
}
