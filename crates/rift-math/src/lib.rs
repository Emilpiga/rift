//! Math primitives for rift. Re-exports glam plus a small set
//! of scalar helpers that show up enough times across the
//! workspace to deserve a single home.

pub use glam::*;

pub mod physics;

/// Linear interpolation between `a` and `b` by `t`. `t` is not
/// clamped — passing values outside `[0, 1]` extrapolates.
#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Cubic Hermite smoothstep (`3t² − 2t³`) on `t ∈ [0, 1]`. The
/// caller is responsible for clamping `t` to that range; the
/// raw form is preferred so callers that already clamped don't
/// pay for it twice.
#[inline]
pub fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}
