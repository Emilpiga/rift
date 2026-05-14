//! Settings screen view model + action enum.
//!
//! Exposes session settings for audio and graphics. Structured
//! so future settings (key bindings, display, etc.) can join the
//! same view/action pair without breaking the widget-host
//! contract.

/// One display resolution exposed by the current monitor / window host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayResolution {
    pub width: u32,
    pub height: u32,
}

/// Snapshot of the player-configurable settings, built fresh by
/// the host each frame from the live config and rendered by
/// `rift_ui::settings::frame_settings`.
#[derive(Clone, Copy, Debug)]
pub struct SettingsView<'a> {
    /// Master output gain, linear 0..=1. `1.0` = source level.
    pub master_volume: f32,
    /// Whether directional and point-light shadow maps are rendered
    /// and sampled this session.
    pub shadows_enabled: bool,
    /// Whether PBR materials use their height maps to perturb
    /// shadow receiver lookups and add subtle self-shadowing.
    pub height_shadows_enabled: bool,
    /// Whether the half-resolution bright/blur bloom stack is
    /// recorded and composited.
    pub bloom_enabled: bool,
    /// Whether the screen-space ambient occlusion graph node is
    /// recorded and applied in the final composite.
    pub ssao_enabled: bool,
    /// Whether the post-process volumetric ray graph node is
    /// recorded and added in the final composite.
    pub volumetrics_enabled: bool,
    /// Whether presentation should use FIFO/vsync instead of
    /// preferring uncapped low-latency modes.
    pub vsync_enabled: bool,
    /// Resolutions exposed by the current monitor / window host.
    pub display_resolutions: &'a [DisplayResolution],
    /// Currently active window or fullscreen resolution.
    pub selected_resolution: DisplayResolution,
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
    /// Player toggled bloom post-processing.
    SetBloomEnabled(bool),
    /// Player toggled screen-space ambient occlusion.
    SetSsaoEnabled(bool),
    /// Player toggled post-process volumetric rays.
    SetVolumetricsEnabled(bool),
    /// Player toggled FIFO/vsync presentation on/off.
    SetVsyncEnabled(bool),
    /// Player selected a display resolution.
    SetDisplayResolution(DisplayResolution),
    /// Player asked to close the settings sub-screen (Escape or
    /// Back button).
    Close,
}
