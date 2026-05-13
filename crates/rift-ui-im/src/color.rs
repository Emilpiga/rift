//! RGBA color type.
//!
//! Thin wrapper over `[f32; 4]` so widget signatures read
//! `Color` instead of an opaque tuple. Implicitly convertible to
//! the array form `OverlayBatch` already accepts.

/// Linear RGBA in `0.0..=1.0`. Alpha 0 is fully transparent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color(pub [f32; 4]);

impl Color {
    pub const TRANSPARENT: Color = Color([0.0, 0.0, 0.0, 0.0]);
    pub const WHITE: Color = Color([1.0, 1.0, 1.0, 1.0]);
    pub const BLACK: Color = Color([0.0, 0.0, 0.0, 1.0]);

    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self([r, g, b, a])
    }
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self([r, g, b, 1.0])
    }

    /// Build from `0..=255` channels. Cheaper to type for theme tables.
    pub const fn rgb8(r: u8, g: u8, b: u8) -> Self {
        Self([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0])
    }
    pub const fn rgba8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self([
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0,
            a as f32 / 255.0,
        ])
    }

    /// Replace the alpha channel.
    pub fn with_alpha(self, a: f32) -> Self {
        Self([self.0[0], self.0[1], self.0[2], a])
    }

    /// Multiply alpha (e.g. fade-in animations).
    pub fn fade(self, mul: f32) -> Self {
        Self([self.0[0], self.0[1], self.0[2], self.0[3] * mul])
    }

    pub fn to_array(self) -> [f32; 4] {
        self.0
    }
}

impl From<Color> for [f32; 4] {
    fn from(c: Color) -> Self {
        c.0
    }
}

impl From<[f32; 4]> for Color {
    fn from(a: [f32; 4]) -> Self {
        Color(a)
    }
}

/// Outline style for [`crate::ui::im::Frame`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stroke {
    pub thickness: f32,
    pub color: Color,
}

impl Stroke {
    pub const NONE: Stroke = Stroke {
        thickness: 0.0,
        color: Color::TRANSPARENT,
    };
    pub const fn new(thickness: f32, color: Color) -> Self {
        Self { thickness, color }
    }
}
