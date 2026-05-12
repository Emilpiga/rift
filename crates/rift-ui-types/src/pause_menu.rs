//! In-game pause menu (Escape) view model + action enum.
//!
//! Rendered as a modal by `rift_ui::pause_menu::frame_pause_menu`.
//! The host (`rift-client`) owns the boolean "is open" flag and
//! decides when to call the widget; the widget itself is
//! state-less.

/// One of the three player choices on the pause menu, plus the
/// implicit "close the menu and resume" return path.
///
/// `ExitToCharacterSelect` and `ExitGame` are intentionally
/// distinct even though both currently terminate the process —
/// the design intent is that `ExitToCharacterSelect` will later
/// gracefully tear down the net session and route back into the
/// roster screen without restarting the binary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseMenuAction {
    /// Close the menu and return to gameplay.
    Resume,
    /// Open the settings sub-screen.
    OpenSettings,
    /// Leave the running session and surface the character-
    /// select screen.
    ExitToCharacterSelect,
    /// Quit the entire client application.
    ExitGame,
}
