//! Settings screen view model + action enum.
//!
//! Exposes session settings for audio and graphics. Structured
//! so future settings (key bindings, display, etc.) can join the
//! same view/action pair without breaking the widget-host
//! contract.

/// Snapshot of the player-configurable settings, built fresh by
/// the host each frame from the live config and rendered by
/// `rift_ui::settings::frame_settings`.
#[derive(Clone, Copy, Debug)]
pub struct SettingsView {
    /// Master output gain, linear 0..=1. `1.0` = source level.
    pub master_volume: f32,
    /// Whether directional and point-light shadow maps are rendered
    /// and sampled this session.
    pub shadows_enabled: bool,
    /// Whether PBR materials use their height maps to perturb
    /// shadow receiver lookups and add subtle self-shadowing.
    pub height_shadows_enabled: bool,
}

/// Player intent emitted by the settings widget for the host to
/// apply.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SettingsAction {
    /// Master-volume slider moved — value is the new linear gain.
    SetMasterVolume(f32),
    /// Player toggled realtime shadows on/off.
    SetShadowsEnabled(bool),
    /// Player toggled experimental texture-height-aware shadows.
    SetHeightShadowsEnabled(bool),
    /// Player asked to close the settings sub-screen (Escape or
    /// Back button).
    Close,
}
