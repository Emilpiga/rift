//! Settings screen view model + action enum.
//!
//! Today exposes only the master audio volume; structured this
//! way so future settings (graphics, key bindings, etc.) can
//! join the same view/action pair without breaking the
//! widget-host contract.

/// Snapshot of the player-configurable settings, built fresh by
/// the host each frame from the live config and rendered by
/// `rift_ui::settings::frame_settings`.
#[derive(Clone, Copy, Debug)]
pub struct SettingsView {
    /// Master output gain, linear 0..=1. `1.0` = source level.
    pub master_volume: f32,
}

/// Player intent emitted by the settings widget for the host to
/// apply.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SettingsAction {
    /// Master-volume slider moved — value is the new linear gain.
    SetMasterVolume(f32),
    /// Player asked to close the settings sub-screen (Escape or
    /// Back button).
    Close,
}
