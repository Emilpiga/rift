//! Surface-keyed audio sample banks.
//!
//! The single place that knows the *path* of every gameplay
//! sound asset. Gameplay code asks for a bank by
//! [`SurfaceKind`] (and event type) and gets back a slice
//! of asset paths to round-robin over.
//!
//! Adding a new surface or a new sample is a one-line edit
//! here — call sites that already use [`footstep_paths`]
//! pick the new files up automatically.
//!
//! Empty slices are returned for surfaces that don't yet
//! have authored samples; call sites must tolerate this
//! (the footstep tick falls back to silence).

use rift_engine::dungeon::SurfaceKind;

/// Footstep sample bank for the given surface. Returned
/// slice is round-robined over by the call site so two
/// consecutive plants on the same surface use different
/// files.
///
/// Returns an empty slice for surfaces that don't have
/// authored samples yet — the caller (footstep tick) skips
/// emission silently in that case rather than papering over
/// the gap with the wrong material's audio (the wrong-
/// material audio is far more jarring than a silent step
/// while the asset pipeline catches up).
pub fn footstep_paths(surface: SurfaceKind) -> &'static [&'static str] {
    match surface {
        SurfaceKind::Sand => &[
            "vfx/sand_footstep_1.wav",
            "vfx/sand_footstep_2.wav",
            "vfx/sand_footstep_3.wav",
        ],
        // Stone / Wood / Metal / Grass / Bone: no samples
        // authored yet. Drop files in `assets/audio/vfx/`
        // and add them here — no other code needs to
        // change.
        SurfaceKind::Stone => &[
            "vfx/stone_footstep_1.wav",
            "vfx/stone_footstep_2.wav",
            "vfx/stone_footstep_3.wav",
            "vfx/stone_footstep_4.wav",
        ],
        SurfaceKind::Wood => &[
            "vfx/wood_footstep_1.wav",
            "vfx/wood_footstep_2.wav",
            "vfx/wood_footstep_3.wav",
        ],
        SurfaceKind::Metal => &[],
        SurfaceKind::Grass => &[],
        SurfaceKind::Bone => &[],
    }
}
