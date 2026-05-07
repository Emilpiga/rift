//! Named effect builders, organised by gameplay domain.
//!
//! Each `pub fn` returns a fresh [`Effect`](super::spec::Effect)
//! ready to feed `VfxSystem::spawn`. Authoring a new ability
//! visual lives here — gameplay code just spawns a preset and
//! updates its endpoints / anchor. The presets themselves are
//! pure data with no mutable state, so they're cheap to build
//! at the call site.
//!
//! # Layout
//!
//! Presets are split by **category** (combat / environment) and
//! within each category by **theme** (fire, frost, arcane, …).
//! All leaf functions are flat-re-exported from this module so
//! existing call sites keep working with `vfx::presets::frost_ray`.
//!
//! When adding a new preset, drop it in the file that matches
//! its theme. If no file fits, prefer a new file under an
//! existing category over a new top-level category — the goal
//! is to keep this tree shallow and predictable.

pub mod combat;
pub mod environment;

pub use combat::*;
pub use environment::*;
