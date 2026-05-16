//! Standard rectangular button.
//!
//! Replaces the per-screen `draw_button` + `hit` + manual hover
//! id pattern. Returns a [`Response`] so callers do
//! `if ui.button("Save").clicked { ... }`.
//!
//! Visual state mapping (rounded void glass):
//!  - `Normal`    → violet-tint neutral slab + hover lift
//!  - `Primary`   → accent gradient
//!  - `Danger`    → danger gradient
//!  - `Active`    → accent gradient + strong border when idle
//!  - `Red`       → radial red CTA
//!  - disabled    → flat muted rounded rect
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
    /// Default neutral glass surface.
    Normal,
    /// Action button (Confirm, Play, Equip).
    Primary,
    /// Destructive action (Delete, Drop).
    Danger,
    /// Selected toggle — accent-tinted glass (tabs, gender).
    Active,
    /// Headline CTA — glossy radial red on violet chrome.
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
    /// When set, draws `left · atlas icon · right` centered instead of [`Self::label`].
    pub compound_icon_row: Option<(&'a str, &'a str, &'a str)>,
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
            compound_icon_row: None,
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

    /// Saturated red CTA; see [`ButtonVariant::Red`].
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

    /// Centered row: `left` text, registered atlas `icon_key`, then `right` text (e.g. cost).
    pub fn compound_icon_row(mut self, left: &'a str, icon_key: &'a str, right: &'a str) -> Self {
        self.compound_icon_row = Some((left, icon_key, right));
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

        // Rounded void-glass buttons: vertical gradient body,
        // optional radial for red CTA, cool gloss + depth bands,
        // violet chrome outlines — uniform with panel frames.
        let corner_r = (theme.spacing.corner_radius * 0.92)
            .min(rect.height() * 0.5)
            .max(2.5);

        if self.enabled {
            match self.variant {
                ButtonVariant::Red => {
                    let edge_base = theme.colors.red_smudge;
                    let centre_base = if hovered {
                        theme.colors.red_hover
                    } else {
                        theme.colors.red
                    };
                    let (edge, centre) = if pressed {
                        (
                            Color::rgba(
                                edge_base.0[0] * 0.58,
                                edge_base.0[1] * 0.58,
                                edge_base.0[2] * 0.58,
                                edge_base.0[3],
                            ),
                            Color::rgba(
                                centre_base.0[0] * 0.58,
                                centre_base.0[1] * 0.58,
                                centre_base.0[2] * 0.58,
                                centre_base.0[3],
                            ),
                        )
                    } else {
                        (edge_base, centre_base)
                    };
                    ui.draw_rounded_radial_rect(rect, corner_r, edge, centre);
                }
                ButtonVariant::Normal
                | ButtonVariant::Primary
                | ButtonVariant::Danger
                | ButtonVariant::Active => {
                    let (idle_top, idle_bot) = match self.variant {
                        ButtonVariant::Normal => {
                            let b = theme.colors.bg_panel_alt.0;
                            (
                                Color::rgba(
                                    (b[0] * 1.10).min(1.0),
                                    (b[1] * 1.06).min(1.0),
                                    (b[2] * 1.14).min(1.0),
                                    b[3],
                                ),
                                Color::rgba(b[0] * 0.50, b[1] * 0.48, b[2] * 0.58, b[3]),
                            )
                        }
                        ButtonVariant::Primary | ButtonVariant::Active => {
                            let a = theme.colors.accent.0;
                            (
                                Color::rgba(
                                    (a[0] * 1.05).min(1.0),
                                    (a[1] * 1.05).min(1.0),
                                    (a[2] * 1.08).min(1.0),
                                    1.0,
                                ),
                                Color::rgba(a[0] * 0.36, a[1] * 0.36, a[2] * 0.46, 1.0),
                            )
                        }
                        ButtonVariant::Danger => {
                            let d = theme.colors.danger.0;
                            (
                                Color::rgba(
                                    (d[0] * 1.02).min(1.0),
                                    (d[1] * 1.02).min(1.0),
                                    (d[2] * 1.04).min(1.0),
                                    1.0,
                                ),
                                Color::rgba(d[0] * 0.38, d[1] * 0.38, d[2] * 0.42, 1.0),
                            )
                        }
                        ButtonVariant::Red => unreachable!(),
                    };

                    let (mut top, mut bot) = (idle_top, idle_bot);
                    if hovered {
                        match self.variant {
                            ButtonVariant::Normal => {
                                let h = theme.colors.bg_slot_hover.0;
                                top = Color::rgba(
                                    (h[0] * 1.08).min(1.0),
                                    (h[1] * 1.06).min(1.0),
                                    (h[2] * 1.12).min(1.0),
                                    h[3],
                                );
                                bot = Color::rgba(h[0] * 0.55, h[1] * 0.52, h[2] * 0.62, h[3]);
                            }
                            ButtonVariant::Primary | ButtonVariant::Active => {
                                let a = theme.colors.accent.0;
                                top = theme.colors.accent;
                                bot = Color::rgba(a[0] * 0.42, a[1] * 0.42, a[2] * 0.52, 1.0);
                            }
                            ButtonVariant::Danger => {
                                let d = theme.colors.danger.0;
                                top = theme.colors.danger;
                                bot = Color::rgba(d[0] * 0.45, d[1] * 0.45, d[2] * 0.48, 1.0);
                            }
                            ButtonVariant::Red => unreachable!(),
                        }
                    }
                    if pressed {
                        top = Color::rgba(
                            top.0[0] * 0.58,
                            top.0[1] * 0.58,
                            top.0[2] * 0.58,
                            top.0[3],
                        );
                        bot = Color::rgba(
                            bot.0[0] * 0.58,
                            bot.0[1] * 0.58,
                            bot.0[2] * 0.58,
                            bot.0[3],
                        );
                    }
                    ui.draw_rounded_gradient_rect(rect, corner_r, top, bot);

                    let pressed_dim = if pressed { 0.42 } else { 1.0 };
                    let inset = 2.0_f32;
                    let inner_w = rect.width() - inset * 2.0;
                    if inner_w > 4.0 {
                        let gh = (rect.height() * 0.30).clamp(2.0, 12.0);
                        let gloss_r = (corner_r - 1.5).max(0.0);
                        ui.draw_rounded_gradient_rect(
                            Rect::from_xywh(rect.x() + inset, rect.y() + 1.0, inner_w, gh),
                            gloss_r,
                            Color::rgba(0.92, 0.90, 1.0, 0.11 * pressed_dim),
                            Color::rgba(0.92, 0.90, 1.0, 0.0),
                        );
                        let shadow_h = (rect.height() * 0.28).clamp(2.0, 11.0);
                        ui.draw_rounded_gradient_rect(
                            Rect::from_xywh(
                                rect.x() + inset,
                                rect.max.y - shadow_h - 1.0,
                                inner_w,
                                shadow_h,
                            ),
                            gloss_r,
                            Color::rgba(0.0, 0.0, 0.0, 0.0),
                            Color::rgba(0.0, 0.0, 0.0, if pressed { 0.48 } else { 0.38 }),
                        );
                    }
                }
            }
        } else {
            let p = theme.colors.bg_stone.0;
            let disabled_fill = Color::rgba(p[0] * 0.58, p[1] * 0.58, p[2] * 0.62, p[3]);
            ui.draw_rounded_rect(rect, corner_r, disabled_fill);
        }

        let (outline_color, outline_thickness) = match (self.enabled, hovered, self.variant) {
            (false, _, _) => (theme.colors.border, theme.spacing.border_thickness),
            (true, true, _) => (theme.colors.border_strong, 1.5),
            (true, false, ButtonVariant::Active) => (theme.colors.border_strong, 1.5),
            (true, false, _) => (theme.colors.border, 1.5),
        };
        ui.draw_outline(rect, outline_thickness, outline_color);
        if self.enabled {
            let inner_line_a = if pressed { 0.14 } else { 0.26 };
            ui.draw_outline(
                Rect::from_xywh(
                    rect.x() + 1.0,
                    rect.y() + 1.0,
                    (rect.width() - 2.0).max(0.0),
                    (rect.height() - 2.0).max(0.0),
                ),
                1.0,
                Color::rgba(0.72, 0.62, 0.98, inner_line_a),
            );
        }

        if self.enabled {
            let inner = Rect::from_xywh(
                rect.x() + 2.0,
                rect.y() + 2.0,
                (rect.width() - 4.0).max(0.0),
                (rect.height() - 4.0).max(0.0),
            );
            let hairline_a = if pressed { 0.10 } else { 0.20 };
            ui.draw_outline(inner, 1.0, Color::rgba(0.88, 0.84, 1.0, hairline_a));
        }

        // Label row — optional `[left · atlas icon · right]` centered layout,
        // otherwise classic single centered [`Self::label`].
        let base_size = self.size.font_size(&theme);
        let inset = (theme.spacing.gap_sm.max(6.0)) * 2.0;
        let avail_w = (rect.width() - inset).max(1.0);
        let text_color = if self.enabled {
            theme.colors.text
        } else {
            theme.colors.text_muted
        };

        if let Some((left, icon_key, right)) = self.compound_icon_row {
            let gap = theme.spacing.gap_sm.max(5.0);
            let shadow_label = self.enabled;
            let mut icon_px = (rect.height() * 0.50).clamp(15.0, rect.height() * 0.62);
            let mut ls = base_size;
            let mut rs = base_size;
            let measure_total = |ls: f32, rs: f32, ipx: f32| -> f32 {
                ui.measure_text(left, ls) + gap + ipx + gap + ui.measure_text(right, rs)
            };
            let total = measure_total(ls, rs, icon_px);
            if total > avail_w && total > 0.01 {
                let s = (avail_w / total).clamp(0.72, 1.0);
                ls = (ls * s).max(base_size * 0.72);
                rs = (rs * s).max(base_size * 0.72);
                icon_px *= s;
            }
            let wl = ui.measure_text(left, ls);
            let wr = ui.measure_text(right, rs);
            let total = wl + gap + icon_px + gap + wr;
            let x0 = rect.x() + (rect.width() - total).max(0.0) * 0.5;
            let ty_l = rect.y() + (rect.height() - ls) * 0.5;
            let ty_r = rect.y() + (rect.height() - rs) * 0.5;
            let ty_i = rect.y() + (rect.height() - icon_px) * 0.5;
            let lx = x0;
            let ix = x0 + wl + gap;
            let rx = ix + icon_px + gap;
            if shadow_label {
                ui.draw_text(
                    Pos2::new(lx + 1.0, ty_l + 1.0),
                    left,
                    ls,
                    Color::rgba(0.0, 0.0, 0.0, 0.55),
                );
                ui.draw_text(
                    Pos2::new(rx + 1.0, ty_r + 1.0),
                    right,
                    rs,
                    Color::rgba(0.0, 0.0, 0.0, 0.55),
                );
            }
            ui.draw_text(Pos2::new(lx, ty_l), left, ls, text_color);
            ui.draw_text(Pos2::new(rx, ty_r), right, rs, text_color);
            let icon_rect = Rect::from_xywh(ix, ty_i, icon_px, icon_px);
            ui.draw_icon(
                icon_rect,
                icon_key,
                Color::rgba(0.96, 0.90, 1.0, if self.enabled { 1.0 } else { 0.55 }),
            );
        } else {
            let natural_w = ui.measure_text(self.label, base_size);
            let text_size = if natural_w > avail_w {
                (base_size * (avail_w / natural_w)).max(base_size * 0.70)
            } else {
                base_size
            };
            let final_w = ui.measure_text(self.label, text_size);
            let ty = rect.y() + (rect.height() - text_size) * 0.5;
            let shadow_label = self.enabled;
            if final_w > avail_w {
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
