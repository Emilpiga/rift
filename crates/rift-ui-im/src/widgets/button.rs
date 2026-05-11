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
    /// Forge-iron red surface: red fill with darker smudges
    /// painted inside, a brighter inset highlight along the
    /// top, and a near-black border that matches the stone
    /// panel chrome to create a sunken-into-the-panel feel.
    /// Used for the headline action on a stone-panel screen
    /// (Play, Enter World).
    Red,
}

/// Coarse size preset. Each value pins a minimum height and a
/// font size token; the actual rect width is whatever the
/// caller passes to `show`. Default is `Medium`, which
/// matches the original 40-px tall button so existing call
/// sites are unaffected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonSize {
    /// 28-px tall, `size_sm` text. Inline / secondary actions
    /// (Cancel next to a primary, footer chips).
    Small,
    /// 40-px tall, `size_md` text. Default.
    Medium,
    /// 56-px tall, `size_lg` text. Hero call-to-action
    /// (Enter World on the character-select screen).
    Large,
}

impl ButtonSize {
    /// Suggested min height in pixels for this size, scaled.
    /// Returned for callers that pre-compute layout rows; the
    /// button itself renders into whatever rect the caller
    /// passes to `show`, so this is informational only.
    pub fn min_height(self, theme: &super::super::theme::Theme) -> f32 {
        let base = match self {
            ButtonSize::Small => 28.0,
            ButtonSize::Medium => 40.0,
            ButtonSize::Large => 56.0,
        };
        base * theme.scale
    }

    /// Font size token for this button size.
    fn font_size(self, theme: &super::super::theme::Theme) -> f32 {
        match self {
            ButtonSize::Small => theme.fonts.size_sm,
            ButtonSize::Medium => theme.fonts.size_md,
            ButtonSize::Large => theme.fonts.size_lg,
        }
    }
}

/// Configurable button. Cheap struct; build, configure, `.show()`.
#[derive(Debug, Clone)]
pub struct Button<'a> {
    pub label: &'a str,
    pub variant: ButtonVariant,
    pub size: ButtonSize,
    pub enabled: bool,
    pub min_size: (f32, f32),
    pub padding: Option<Pad>,
}

impl<'a> Button<'a> {
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            variant: ButtonVariant::Normal,
            size: ButtonSize::Medium,
            enabled: true,
            min_size: (0.0, 0.0),
            padding: None,
        }
    }

    pub fn primary(label: &'a str) -> Self {
        Self {
            variant: ButtonVariant::Primary,
            ..Self::new(label)
        }
    }

    pub fn danger(label: &'a str) -> Self {
        Self {
            variant: ButtonVariant::Danger,
            ..Self::new(label)
        }
    }

    pub fn active(label: &'a str) -> Self {
        Self {
            variant: ButtonVariant::Active,
            ..Self::new(label)
        }
    }

    /// Forge-iron red surface; see [`ButtonVariant::Red`].
    pub fn red(label: &'a str) -> Self {
        Self {
            variant: ButtonVariant::Red,
            ..Self::new(label)
        }
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// Pick a coarse size preset (height + font). Default is `Medium`.
    pub fn size(mut self, s: ButtonSize) -> Self {
        self.size = s;
        self
    }

    /// Shortcut for `.size(ButtonSize::Small)`.
    pub fn small(self) -> Self {
        self.size(ButtonSize::Small)
    }
    /// Shortcut for `.size(ButtonSize::Large)`.
    pub fn large(self) -> Self {
        self.size(ButtonSize::Large)
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
        // Down-edge: latch this button as the pressed one.
        // Release-edge: fire `clicked` only if the release
        // happens inside *this same* button rect — matches
        // platform UX (drag the cursor off before releasing
        // to cancel the click). The latch is cleared on
        // every release so the next press starts fresh.
        if self.enabled && hovered && ui.input().left_just_pressed() {
            ui.state_mut().pressed_button = Some(id);
        }
        let was_pressed_here = ui.state().pressed_button == Some(id);
        let pressed = self.enabled && hovered && was_pressed_here;
        let released = ui.input().left_just_released();
        let clicked = self.enabled && hovered && was_pressed_here && released;
        if released && was_pressed_here {
            ui.state_mut().pressed_button = None;
        }

        // Every enabled button uses the same surface recipe:
        //   1. Radial-elliptical fill (dark edge → bright
        //      centre, smoothstep falloff) — gives the
        //      "polished oval hotspot" look in one primitive.
        //      The hotspot is inscribed inside the rounded
        //      rect (see `rounded_rect_px_radial`) so every
        //      perimeter sample evaluates to the edge colour
        //      and the rounded corners blend cleanly into
        //      the long edges instead of forming visible
        //      wedges.
        //   2. Top-bevel sheen that fades to transparent at
        //      the horizontal edges (4-corner gradients).
        //   3. Bottom inner shadow, also fading at the edges.
        //   4. Dark outer border + lighter hairline 1 px in
        //      → forged-bevel framing.
        // Disabled buttons stay flat so the affordance reads
        // immediately as "not interactable".
        if self.enabled {
            // Variant-specific (edge, centre, label-tone)
            // palette. Edge is the darker base, centre is the
            // brighter hotspot. Hover lifts the centre, pressed
            // inverts edge↔centre so the button reads recessed.
            let (base_edge, base_centre, hover_centre) = match self.variant {
                ButtonVariant::Red => (
                    theme.colors.red_smudge,
                    theme.colors.red,
                    theme.colors.red_hover,
                ),
                ButtonVariant::Primary => {
                    let a = theme.colors.accent;
                    (
                        Color::rgba(a.0[0] * 0.45, a.0[1] * 0.45, a.0[2] * 0.45, 1.0),
                        Color::rgba(a.0[0] * 0.85, a.0[1] * 0.85, a.0[2] * 0.85, 1.0),
                        a,
                    )
                }
                ButtonVariant::Danger => {
                    let d = theme.colors.danger;
                    (
                        Color::rgba(d.0[0] * 0.45, d.0[1] * 0.45, d.0[2] * 0.45, 1.0),
                        Color::rgba(d.0[0] * 0.85, d.0[1] * 0.85, d.0[2] * 0.85, 1.0),
                        d,
                    )
                }
                ButtonVariant::Active => {
                    // Toggle-on state (gender picker, tab
                    // headers): mirror the Red CTA palette so
                    // the "this option is selected" affordance
                    // matches the screen's headline action,
                    // never the accent-blue used for hover
                    // links. Sharing the recipe also means the
                    // active toggle and the Confirm/Play CTA
                    // visually rhyme — the same forge-iron
                    // chrome on both reads as "these belong
                    // together" instead of two different
                    // styles fighting for attention.
                    (
                        theme.colors.red_smudge,
                        theme.colors.red,
                        theme.colors.red_hover,
                    )
                }
                ButtonVariant::Normal => (
                    theme.colors.bg_panel,
                    theme.colors.bg_panel_alt,
                    theme.colors.bg_slot_hover,
                ),
            };
            let (edge, centre) = if pressed {
                // Pressed: uniformly darker than the rest
                // state so the button reads as pushed-in
                // without inverting the rim (which produced
                // a bright halo) or stripping the gradient
                // (which produced a flat plate).
                let de = Color::rgba(
                    base_edge.0[0] * 0.55,
                    base_edge.0[1] * 0.55,
                    base_edge.0[2] * 0.55,
                    base_edge.0[3],
                );
                let dc = Color::rgba(
                    base_centre.0[0] * 0.55,
                    base_centre.0[1] * 0.55,
                    base_centre.0[2] * 0.55,
                    base_centre.0[3],
                );
                (de, dc)
            } else if hovered {
                (base_edge, hover_centre)
            } else {
                (base_edge, base_centre)
            };
            ui.draw_rounded_radial_rect_noisy(rect, theme.spacing.corner_radius, edge, centre);

            // Top + bottom bevel bands. Pressed keeps a
            // softer version of both (alpha halved) so the
            // surface still reads as forged metal instead of
            // a flat plate, but the bands clearly recede.
            let pressed_dim = if pressed { 0.45 } else { 1.0 };
            {
                let r = theme.spacing.corner_radius;
                let inset = r.max(2.0);
                let inner_x = rect.x() + inset;
                let inner_w = rect.width() - inset * 2.0;
                if inner_w > 4.0 {
                    let band_h = (rect.height() * 0.35).clamp(4.0, 14.0);
                    let band_y = rect.y() + 1.0;
                    let half_w = inner_w * 0.5;
                    let opaque_mid = Color::rgba(1.0, 0.96, 0.90, 0.32 * pressed_dim);
                    let transparent = Color::rgba(1.0, 0.96, 0.90, 0.0);
                    ui.draw_grad4_rect(
                        Rect::from_xywh(inner_x, band_y, half_w, band_h),
                        transparent,
                        opaque_mid,
                        transparent,
                        transparent,
                    );
                    ui.draw_grad4_rect(
                        Rect::from_xywh(inner_x + half_w, band_y, half_w, band_h),
                        opaque_mid,
                        transparent,
                        transparent,
                        transparent,
                    );

                    let shadow_h = (rect.height() * 0.30).clamp(3.0, 12.0);
                    let shadow_y = rect.max.y - shadow_h - 1.0;
                    // Pressed gets a *stronger* bottom
                    // shadow band so the recess reads even
                    // though the top sheen is muted.
                    let bottom_alpha = if pressed { 0.55 } else { 0.45 };
                    let opaque_dark = Color::rgba(0.0, 0.0, 0.0, bottom_alpha);
                    let trans_dark = Color::rgba(0.0, 0.0, 0.0, 0.0);
                    ui.draw_grad4_rect(
                        Rect::from_xywh(inner_x, shadow_y, half_w, shadow_h),
                        trans_dark,
                        trans_dark,
                        trans_dark,
                        opaque_dark,
                    );
                    ui.draw_grad4_rect(
                        Rect::from_xywh(inner_x + half_w, shadow_y, half_w, shadow_h),
                        trans_dark,
                        trans_dark,
                        opaque_dark,
                        trans_dark,
                    );
                }
            }
        } else {
            // Disabled: flat, muted, noticeably darker than
            // any active state so the affordance reads as
            // "not interactable" against any panel colour.
            // Pulls from `bg_panel` (warm stone) instead of
            // `bg_slot` (cool slate) so disabled buttons
            // don't pop as a bright chip on the carved-stone
            // surfaces the inventory + character-select use.
            let p = theme.colors.bg_panel.0;
            let disabled_fill = Color::rgba(p[0] * 0.60, p[1] * 0.60, p[2] * 0.60, p[3]);
            ui.draw_rounded_rect(rect, theme.spacing.corner_radius, disabled_fill);
        }

        // Outline. Every enabled button gets the dark stone
        // border (so all chrome reads as forged into the
        // panel); hover/active swap to the brighter strong
        // border to telegraph state.
        let (outline_color, outline_thickness) = match (self.enabled, hovered, self.variant) {
            (false, _, _) => (theme.colors.border, theme.spacing.border_thickness),
            (true, _, ButtonVariant::Active) => (theme.colors.border_strong, 1.5),
            (true, true, _) => (theme.colors.border_strong, 1.5),
            (true, false, _) => (theme.colors.border_stone, 1.5),
        };
        ui.draw_rounded_outline(
            rect,
            theme.spacing.corner_radius,
            outline_thickness,
            outline_color,
        );

        // Inner hairline 1 px inside the outer border. Reads
        // as a forged-bevel framing line. Stays on for
        // pressed (alpha halved) so the chrome is consistent
        // across rest / hover / pressed.
        if self.enabled {
            let inner = Rect::from_xywh(
                rect.x() + 2.0,
                rect.y() + 2.0,
                (rect.width() - 4.0).max(0.0),
                (rect.height() - 4.0).max(0.0),
            );
            // Keep the inner radius positive so the corner
            // arcs are visible — going to 0 would degrade
            // to a sharp inset rectangle.
            let inner_r = (theme.spacing.corner_radius - 2.0).max(2.0);
            let hairline_a = if pressed { 0.09 } else { 0.18 };
            ui.draw_rounded_outline(
                inner,
                inner_r,
                1.0,
                Color::rgba(1.0, 0.92, 0.84, hairline_a),
            );
        }

        // Centred label. Two-stage overflow guard so a button
        // can never render text outside its own rect:
        //   1. If the natural width at the size-preset font
        //      would overflow, shrink the font proportionally,
        //      floored at 70% of base for legibility.
        //   2. If even at the floor size the text still
        //      overflows (very narrow rect / very long label),
        //      hard-ellipsize the string at that size.
        let base_size = self.size.font_size(&theme);
        let inset = (theme.spacing.gap_sm.max(6.0)) * 2.0;
        let avail_w = (rect.width() - inset).max(1.0);
        let natural_w = ui.measure_text(self.label, base_size);
        let text_size = if natural_w > avail_w {
            (base_size * (avail_w / natural_w)).max(base_size * 0.70)
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
        // 1 px drop-shadow under the label so it reads
        // cleanly against the bright bevel hotspot. Skipped
        // on disabled (already low-contrast and the shadow
        // would clutter it).
        let shadow_label = self.enabled;
        if final_w > avail_w {
            // Still too wide → ellipsize. Anchor to the inset
            // so the prefix is visible.
            let pos = Pos2::new(rect.x() + inset * 0.5, ty);
            if shadow_label {
                ui.draw_text_ellipsized(
                    Pos2::new(pos.x + 1.0, pos.y + 1.0),
                    self.label,
                    text_size,
                    avail_w,
                    Color::rgba(0.0, 0.0, 0.0, 0.55),
                );
            }
            ui.draw_text_ellipsized(pos, self.label, text_size, avail_w, text_color);
        } else {
            let tx = rect.x() + (rect.width() - final_w) * 0.5;
            if shadow_label {
                ui.draw_text(
                    Pos2::new(tx + 1.0, ty + 1.0),
                    self.label,
                    text_size,
                    Color::rgba(0.0, 0.0, 0.0, 0.55),
                );
            }
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
