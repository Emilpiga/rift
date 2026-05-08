//! Standard rectangular button.
//!
//! Replaces the per-screen `draw_button` + `hit` + manual hover
//! id pattern. Returns a [`Response`] so callers do
//! `if ui.button("Save").clicked { ... }`.
//!
//! Visual state mapping:
//!  - idle      → `theme.colors.bg_panel_alt`
//!  - hovered   → `theme.colors.bg_slot_hover`
//!  - pressed   → `theme.colors.accent` (mid-press tint)
//!  - disabled  → `theme.colors.bg_slot` with dimmed text
//!  - primary   → `theme.colors.accent` fill (or hovered variant)
//!  - danger    → `theme.colors.danger`
//!
//! Use the builder methods on [`Button`] to choose a variant and
//! enable/disable the widget; pass the constructed value to
//! [`Ui::add`](super::super::ui::Ui) — or call [`Button::show`] directly.

use super::super::color::Color;
use super::super::id::Id;
use super::super::rect::{Pad, Pos2, Rect};
use super::super::response::Response;
use super::super::ui::Ui;

/// Visual variant. Picked at construction; affects fill colour
/// only (text colour stays `theme.colors.text` for legibility).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonVariant {
    /// Default neutral surface.
    Normal,
    /// Action button (Confirm, Play, Equip).
    Primary,
    /// Destructive action (Delete, Drop).
    Danger,
    /// Pressed-toggle (gender selector showing the active option).
    Active,
}

/// Configurable button. Cheap struct; build, configure, `.show()`.
#[derive(Debug, Clone)]
pub struct Button<'a> {
    pub label: &'a str,
    pub variant: ButtonVariant,
    pub enabled: bool,
    pub min_size: (f32, f32),
    pub padding: Option<Pad>,
}

impl<'a> Button<'a> {
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            variant: ButtonVariant::Normal,
            enabled: true,
            min_size: (0.0, 0.0),
            padding: None,
        }
    }

    pub fn primary(label: &'a str) -> Self {
        Self { variant: ButtonVariant::Primary, ..Self::new(label) }
    }

    pub fn danger(label: &'a str) -> Self {
        Self { variant: ButtonVariant::Danger, ..Self::new(label) }
    }

    pub fn active(label: &'a str) -> Self {
        Self { variant: ButtonVariant::Active, ..Self::new(label) }
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// Minimum pixel size; the actual button grows to fit its label.
    pub fn min_size(mut self, w: f32, h: f32) -> Self {
        self.min_size = (w, h);
        self
    }

    pub fn padding(mut self, p: Pad) -> Self {
        self.padding = Some(p);
        self
    }

    /// Draw + interact at an explicit `rect`. Useful for layouts
    /// that pre-compute slot positions (the existing
    /// character-select grid). Most code should prefer
    /// [`Self::auto`] which sizes to fit.
    pub fn show(self, ui: &mut Ui<'_>, rect: Rect) -> Response {
        let id = Id::root("button").child((rect.x() as i32, rect.y() as i32, self.label));
        self.show_with_id(ui, id, rect)
    }

    /// Same as [`Self::show`] but the caller supplies the id,
    /// e.g. when the button is inside a loop and needs a stable
    /// id per iteration.
    pub fn show_with_id(self, ui: &mut Ui<'_>, id: Id, rect: Rect) -> Response {
        let theme = *ui.theme();
        let hovered = if self.enabled {
            ui.interact_hover(id, rect)
        } else {
            false
        };
        let pressed = hovered && ui.input().left_just_pressed();
        let clicked = self.enabled && hovered && ui.input().left_clicked();

        // Variant-aware fill.
        let fill = match (self.variant, self.enabled, hovered, pressed) {
            (_, false, _, _) => theme.colors.bg_slot,
            (ButtonVariant::Primary, true, true, _) => theme.colors.accent,
            (ButtonVariant::Primary, true, false, _) => Color::rgba(
                theme.colors.accent.0[0] * 0.7,
                theme.colors.accent.0[1] * 0.7,
                theme.colors.accent.0[2] * 0.7,
                1.0,
            ),
            (ButtonVariant::Danger, true, true, _) => theme.colors.danger,
            (ButtonVariant::Danger, true, false, _) => Color::rgba(
                theme.colors.danger.0[0] * 0.7,
                theme.colors.danger.0[1] * 0.7,
                theme.colors.danger.0[2] * 0.7,
                1.0,
            ),
            (ButtonVariant::Active, _, _, _) => theme.colors.accent,
            (ButtonVariant::Normal, true, true, _) => theme.colors.bg_slot_hover,
            (ButtonVariant::Normal, true, false, _) => theme.colors.bg_panel_alt,
        };
        ui.draw_rounded_rect(rect, theme.spacing.corner_radius, fill);

        // 1px lit edge along the top — same trick as Frame, scaled
        // down. Inset horizontally by `corner_radius` so the band
        // never pokes past the rounded corners. Skipped when
        // disabled (a flat slab reads as "not interactable").
        if self.enabled
            && rect.width() > theme.spacing.corner_radius * 2.0 + 2.0
        {
            let h_inset = theme.spacing.corner_radius.max(1.0);
            let top_band = Rect::from_xywh(
                rect.x() + h_inset,
                rect.y() + 1.0,
                rect.width() - h_inset * 2.0,
                1.0,
            );
            ui.draw_rect(top_band, Color::rgba(1.0, 1.0, 1.0, 0.10));
        }

        // Outline. Hover/active/primary/danger swap the border to
        // a stronger / accent-tinted stroke so the affordance
        // doesn't rely solely on the fill colour shift.
        let (outline_color, outline_thickness) = match (self.variant, self.enabled, hovered) {
            (_, false, _) => (theme.colors.border, theme.spacing.border_thickness),
            (ButtonVariant::Active, _, _) => (theme.colors.border_strong, 1.5),
            (ButtonVariant::Primary, _, _) | (ButtonVariant::Danger, _, _) => {
                (theme.colors.border_strong, 1.5)
            }
            (ButtonVariant::Normal, true, true) => (theme.colors.border_strong, 1.5),
            (ButtonVariant::Normal, true, false) => (theme.colors.border, theme.spacing.border_thickness),
        };
        ui.draw_rounded_outline(
            rect,
            theme.spacing.corner_radius,
            outline_thickness,
            outline_color,
        );

        // Centred label. Two-stage overflow guard so a button
        // can never render text outside its own rect:
        //   1. If the natural width at `size_md` would overflow,
        //      shrink the font proportionally, floored at 70%
        //      of base for legibility.
        //   2. If even at the floor size the text still
        //      overflows (very narrow rect / very long label),
        //      hard-ellipsize the string at that size.
        let base_size = theme.fonts.size_md;
        let inset = (theme.spacing.gap_sm.max(6.0)) * 2.0;
        let avail_w = (rect.width() - inset).max(1.0);
        let natural_w = ui.measure_text(self.label, base_size);
        let text_size = if natural_w > avail_w {
            (base_size * (avail_w / natural_w))
                .max(base_size * 0.70)
        } else {
            base_size
        };
        let text_color = if self.enabled {
            theme.colors.text
        } else {
            theme.colors.text_muted
        };
        // Final width with the (possibly shrunken) size.
        let final_w = ui.measure_text(self.label, text_size);
        let ty = rect.y() + (rect.height() - text_size) * 0.5;
        if final_w > avail_w {
            // Still too wide → ellipsize. Anchor to the inset
            // so the prefix is visible.
            let pos = Pos2::new(rect.x() + inset * 0.5, ty);
            ui.draw_text_ellipsized(pos, self.label, text_size, avail_w, text_color);
        } else {
            let tx = rect.x() + (rect.width() - final_w) * 0.5;
            ui.draw_text(Pos2::new(tx, ty), self.label, text_size, text_color);
        }

        Response {
            id,
            rect,
            hovered,
            pressed,
            clicked,
            drag_started: false,
            drag_released: false,
            focused: ui.state().focus == Some(id),
        }
    }
}
