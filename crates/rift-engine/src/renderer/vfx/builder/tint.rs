//! Colour scaling helpers for reusable gradient templates.

use crate::renderer::vfx::spec::{Gradient, GradientStop};

/// Multiply RGB channels of every stop by `tint`; alpha is unchanged.
pub fn tint_gradient(g: &Gradient, tint: [f32; 3]) -> Gradient {
    Gradient {
        stops: g
            .stops
            .iter()
            .map(|s| GradientStop {
                t: s.t,
                color: [
                    s.color[0] * tint[0],
                    s.color[1] * tint[1],
                    s.color[2] * tint[2],
                    s.color[3],
                ],
            })
            .collect(),
    }
}

/// Build a gradient from `(t, rgba)` stops with per-channel RGB tint.
pub fn gradient_from_tinted_stops(
    stops: &[(f32, [f32; 4])],
    tint: [f32; 3],
) -> Gradient {
    Gradient::from_stops(stops.iter().copied().map(|(t, c)| {
        (
            t,
            [
                c[0] * tint[0],
                c[1] * tint[1],
                c[2] * tint[2],
                c[3],
            ],
        )
    }))
}
