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
    /// UI scale multiplier baked into the theme by [`Ui::begin`]
    /// at frame start. `1.0` is the design-time reference
    /// (1080p). Already pre-multiplied into `spacing` / `fonts`
    /// fields so callers reading e.g. `theme.fonts.size_md`
    /// always get the resolution-appropriate value. Exposed
    /// here so screens that hold *raw* layout constants
    /// (panel widths, slot sizes, …) can scale them too via
    /// [`Ui::s`] or by multiplying directly.
    pub scale: f32,
}

impl Theme {
    /// Derive the auto UI scale from the active framebuffer
    /// dimensions. Tuned so 1080p renders at the design
    /// reference (`1.0`), 4K reads as a comfortable `1.5`, and
    /// tiny laptops scale below 1.0 instead of overflowing.
    /// Clamped so extreme values never produce illegible
    /// chrome.
    ///
    /// We take the **smaller** of the per-axis scale factors
    /// so an ultrawide / portrait window doesn't end up
    /// scaling the chrome larger than the short axis can
    /// fit. Falls back to height-only scaling when only one
    /// dimension is provided.
    pub fn auto_scale_for_size(screen_w: f32, screen_h: f32) -> f32 {
        // Reference resolution = 1920×1080. Square-rootish
        // curve on each axis so big panels don't blow past
        // the screen edges on 4K while phones / 720p still
        // stay readable.
        let raw_h = (screen_h / 1080.0).sqrt();
        let raw_w = (screen_w / 1920.0).sqrt();
        raw_h.min(raw_w).clamp(0.75, 2.0)
    }

    /// Backwards-compatible wrapper. Prefer
    /// [`Self::auto_scale_for_size`] which also clamps by the
    /// width axis (important on ultrawide / portrait
    /// resolutions).
    pub fn auto_scale_for_height(screen_h: f32) -> f32 {
        Self::auto_scale_for_size(screen_h * 16.0 / 9.0, screen_h)
    }

    /// Return a copy of `self` with every spacing / font
    /// dimension multiplied by `scale`. Colors are unchanged.
    /// Idempotent only relative to `1.0` — chaining
    /// `with_scale(a).with_scale(b)` yields `a * b`, not `b`.
    pub fn with_scale(mut self, scale: f32) -> Self {
        let s = scale.max(0.01);
        self.scale *= s;
        self.spacing.pad_sm = self.spacing.pad_sm.scaled(s);
        self.spacing.pad_md = self.spacing.pad_md.scaled(s);
        self.spacing.pad_lg = self.spacing.pad_lg.scaled(s);
        self.spacing.gap_sm *= s;
        self.spacing.gap_md *= s;
        self.spacing.gap_lg *= s;
        // Border thickness is left unscaled on purpose: the
        // 1px hairline reads identically on every panel and
        // scaling it makes selected-state outlines blur into
        // the surrounding rect at high scales.
        self.spacing.corner_radius *= s;
        self.fonts.size_sm *= s;
        self.fonts.size_md *= s;
        self.fonts.size_lg *= s;
        self.fonts.size_xl *= s;
        self
    }
}

impl Theme {
    /// Default dark palette. Deep void surfaces with violet
    /// chrome — sleek panels that read as crystal / rift glass
    /// rather than carved stone.
    pub const DARK: Theme = Theme {
        colors: Colors {
            // Surface stack: blue-violet blacks with a soft
            // amethyst lift on hover.
            bg_panel: Color::rgba8(14, 10, 22, 248),
            bg_panel_alt: Color::rgba8(22, 16, 34, 248),
            bg_slot: Color::rgba8(10, 8, 18, 242),
            bg_slot_hover: Color::rgba8(36, 26, 58, 245),
            border: Color::rgba8(72, 58, 112, 130),
            border_strong: Color::rgba8(196, 168, 255, 230),
            text: Color::rgba8(238, 234, 252, 255),
            text_dim: Color::rgba8(188, 178, 218, 255),
            text_muted: Color::rgba8(120, 108, 150, 255),
            // Character stats drawer: left-column names — high-chroma
            // magenta-violet so they never read as the same near-white as values.
            stat_label: Color::rgba8(255, 118, 252, 255),
            accent: Color::rgba8(186, 154, 255, 255),
            success: Color::rgba8(96, 224, 176, 255),
            warning: Color::rgba8(255, 196, 112, 255),
            danger: Color::rgba8(255, 112, 138, 255),
            shadow: Color::rgba8(8, 4, 18, 120),

            // Floating slabs (inventory, character select, modals).
            bg_stone: Color::rgba8(26, 20, 42, 248),
            bg_stone_alt: Color::rgba8(34, 26, 54, 248),
            border_stone: Color::rgba8(12, 8, 22, 255),

            // Primary action buttons — deep magenta-red that
            // still pops on violet panels.
            red: Color::rgba8(180, 36, 92, 255),
            red_hover: Color::rgba8(214, 56, 118, 255),
            red_smudge: Color::rgba8(48, 12, 36, 235),
            red_inset: Color::rgba8(255, 140, 188, 255),
        },
        spacing: Spacing {
            pad_sm: Pad::all(6.0),
            pad_md: Pad::all(10.0),
            pad_lg: Pad::all(14.0),
            gap_sm: 4.0,
            gap_md: 8.0,
            gap_lg: 14.0,
            border_thickness: 1.0,
            corner_radius: 6.0,
        },
        fonts: Fonts {
            size_sm: 12.0,
            size_md: 16.0,
            size_lg: 20.0,
            size_xl: 28.0,
        },
        scale: 1.0,
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
    /// Stat / affix names in the stats drawer (high-chroma; pairs with body-sized values).
    pub stat_label: Color,
    pub accent: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
    pub shadow: Color,

    // Heavy floating-panel tokens (slate / void slab).
    pub bg_stone: Color,
    pub bg_stone_alt: Color,
    pub border_stone: Color,

    // Red-action button tokens.
    pub red: Color,
    pub red_hover: Color,
    pub red_smudge: Color,
    pub red_inset: Color,
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

impl Spacing {
    /// Outside breathing room between a modal panel and the
    /// screen edges. Same value used by every full-screen panel
    /// (spellbook, inventory, character-select, …) so the chrome
    /// reads as deliberate margin rather than per-screen drift.
    /// Pre-scaled.
    pub fn panel_margin(&self) -> f32 {
        self.pad_lg.left
    }

    /// Distance from a panel's outer rect to its content. The
    /// `Frame::panel` body already inset by `pad_md`; screens
    /// that draw absolutely-positioned regions inside a panel
    /// should use this token to match.
    /// Pre-scaled.
    pub fn inner_pad(&self) -> f32 {
        self.pad_lg.left
    }

    /// Vertical gap between major sections inside a panel
    /// (header → body, body → footer, region → region).
    /// Pre-scaled.
    pub fn section_gap(&self) -> f32 {
        self.gap_lg
    }

    /// Vertical gap between sibling rows inside a section
    /// (label / row, row / row).
    /// Pre-scaled.
    pub fn row_gap(&self) -> f32 {
        self.gap_md
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Fonts {
    pub size_sm: f32,
    pub size_md: f32,
    pub size_lg: f32,
    pub size_xl: f32,
}
