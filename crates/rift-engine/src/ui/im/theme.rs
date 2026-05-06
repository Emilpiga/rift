//! Centralized colors, paddings, and font sizes.
//!
//! Single source of truth so panels stop redefining "panel
//! background" or "tooltip border" in every file. Lives as a
//! `const Theme` for now; promote to a runtime field on
//! [`UiState`] later if hot-swap or per-screen palettes are
//! needed.

use super::color::{Color, Stroke};
use super::rect::Pad;

/// Static theme reference returned by [`Theme::default_ref`]. Use
/// this as the `theme` argument to `Ui::begin` until / unless we
/// add runtime themes.
pub const DEFAULT_THEME: Theme = Theme::DARK;

/// Aggregated style tokens. Pure data; cheap to copy.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub colors: Colors,
    pub spacing: Spacing,
    pub fonts: Fonts,
}

impl Theme {
    /// Default dark palette tuned to match the existing HUD /
    /// inventory aesthetic. Adjusted in Landing 5 once we audit
    /// the per-file `const COLOR_*` values they currently use.
    pub const DARK: Theme = Theme {
        colors: Colors {
            bg_panel:      Color::rgba8(18, 18, 24, 230),
            bg_panel_alt:  Color::rgba8(28, 28, 36, 230),
            bg_slot:       Color::rgba8(40, 40, 50, 200),
            bg_slot_hover: Color::rgba8(60, 60, 75, 220),
            border:        Color::rgba8(80, 80, 100, 255),
            border_strong: Color::rgba8(140, 140, 170, 255),
            text:          Color::rgba8(230, 230, 240, 255),
            text_dim:      Color::rgba8(160, 160, 175, 255),
            text_muted:    Color::rgba8(110, 110, 125, 255),
            accent:        Color::rgba8(110, 180, 255, 255),
            success:       Color::rgba8(110, 220, 130, 255),
            warning:       Color::rgba8(240, 180, 80,  255),
            danger:        Color::rgba8(240, 90,  90,  255),
            shadow:        Color::rgba8(0,   0,   0,   140),
        },
        spacing: Spacing {
            pad_sm: Pad::all(4.0),
            pad_md: Pad::all(8.0),
            pad_lg: Pad::all(12.0),
            gap_sm: 4.0,
            gap_md: 8.0,
            gap_lg: 12.0,
            border_thickness: 1.0,
            corner_radius: 4.0,
        },
        fonts: Fonts {
            size_sm: 12.0,
            size_md: 16.0,
            size_lg: 20.0,
            size_xl: 28.0,
        },
    };

    /// Stroke for ordinary panel borders.
    pub fn border_stroke(&self) -> Stroke {
        Stroke::new(self.spacing.border_thickness, self.colors.border)
    }

    /// Stroke for emphasized borders (focused field, hovered slot).
    pub fn border_strong_stroke(&self) -> Stroke {
        Stroke::new(self.spacing.border_thickness, self.colors.border_strong)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Colors {
    pub bg_panel: Color,
    pub bg_panel_alt: Color,
    pub bg_slot: Color,
    pub bg_slot_hover: Color,
    pub border: Color,
    pub border_strong: Color,
    pub text: Color,
    pub text_dim: Color,
    pub text_muted: Color,
    pub accent: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
    pub shadow: Color,
}

#[derive(Debug, Clone, Copy)]
pub struct Spacing {
    pub pad_sm: Pad,
    pub pad_md: Pad,
    pub pad_lg: Pad,
    pub gap_sm: f32,
    pub gap_md: f32,
    pub gap_lg: f32,
    pub border_thickness: f32,
    pub corner_radius: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct Fonts {
    pub size_sm: f32,
    pub size_md: f32,
    pub size_lg: f32,
    pub size_xl: f32,
}
