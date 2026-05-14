//! Host-side state for the in-game pause menu + settings panel.
//!
//! Owned by [`super::state::GameState`] so the open/closed flag
//! survives across `ui_phase` ticks and the `request_quit` bit
//! is reachable from `main.rs` after `GameState::update`
//! returns.

/// Cross-frame pause-menu state.
#[derive(Debug)]
pub struct PauseState {
    /// Pause-menu modal is visible.
    pub menu_open: bool,
    /// Settings sub-modal is visible (replaces the pause menu
    /// while open). Always implies `menu_open == false`.
    pub settings_open: bool,
    /// Master output gain, linear 0..=1. Mirrored into
    /// `AudioSystem::set_master_volume` whenever the player
    /// drags the slider. Persists across menu open/close.
    pub master_volume: f32,
    /// Session graphics toggle for directional and point-light
    /// shadow maps. Persists across menu open/close.
    pub shadows_enabled: bool,
    /// Experimental session graphics toggle for height-map-aware
    /// shadow receiver lookups on PBR materials.
    pub height_shadows_enabled: bool,
    /// Set to `true` when the player picks "Exit Game".
    /// `main.rs` polls this after `GameState::update` and
    /// terminates the process.
    pub request_quit: bool,
    /// Set to `true` when the player picks "Exit to Character
    /// Select". The top-level app tears down the active net
    /// session, reconnects, and returns to the roster screen.
    pub request_character_select: bool,
}

impl PauseState {
    /// `true` when either modal is currently obscuring the
    /// world — gameplay input should be suppressed.
    pub fn is_obscuring(&self) -> bool {
        self.menu_open || self.settings_open
    }
}

/// Sensible default starting volume (full source level). The
/// game has no settings-persistence layer yet, so this is what
/// every fresh launch lands on.
const DEFAULT_MASTER_VOLUME: f32 = 1.0;

impl Default for PauseState {
    fn default() -> Self {
        Self {
            menu_open: false,
            settings_open: false,
            master_volume: DEFAULT_MASTER_VOLUME,
            shadows_enabled: true,
            height_shadows_enabled: false,
            request_quit: false,
            request_character_select: false,
        }
    }
}
