//! Bottom-center HP / Essence / XP vitals stack.
//!
//! Wraps three progress bars in a `Frame::stone` plaque so the
//! cluster reads as one floating void-glass surface. Bars are
//! flat-edged (no rounded radius) so they read as tiles in the
//! plaque, then receive a top-down highlight → bottom-shadow
//! gradient overlay for a slight beveled look. All labels are
//! drawn through `draw_text_shadow` so they pop against the
//! bar fill without needing a separate stroke pass.

use rift_ui_im::{Color, Frame, Pad, Pos2, ProgressBar, Rect, Ui};
use rift_ui_types::hud::HudVitalsView;

use super::ability_bar;

#[derive(Clone, Copy)]
struct ResourceAnimSnapshot {
    displayed: f32,
    trail: f32,
    pulse: f32,
}

#[derive(Clone, Copy)]
struct ResourceBarStyle {
    base: Color,
    hot: Color,
    chip: Color,
    glow: Color,
}

/// Render the vitals stack centered horizontally, anchored
/// `bottom_offset_px` above the screen's bottom edge (baseline
/// pixels; scaled internally by `theme.scale`). Returns the
/// outer plaque rect so callers can place buff strips above it.
pub fn frame_vitals(ui: &mut Ui<'_>, view: &HudVitalsView<'_>, bottom_offset_px: f32) -> Rect {
    let theme = *ui.theme();
    let s = theme.scale;
    let screen = ui.screen_size();

    // Plaque matches the ability bar's width exactly so the
    // two HUD surfaces stack into one cohesive column.
    let plaque_pad_x = 6.0 * s;
    let plaque_pad_y = 6.0 * s;
    let plaque_w = ability_bar::PLAQUE_W_BASE * s;
    let level_w = 52.0 * s;
    let level_gap = 6.0 * s;
    let bar_w = plaque_w - plaque_pad_x * 2.0 - level_w - level_gap;

    // Bigger bars: the previous 22 / 12 / 9 px heights left
    // the inline labels too small to read against a moving
    // background. Bumping to 30 / 18 / 12 fills the wider
    // plaque and gives every bar's text breathing room.
    let hp_h = 30.0 * s;
    let ess_h = 18.0 * s;
    let xp_h = 12.0 * s;
    let row_gap = 2.0 * s;
    let body_h = hp_h + row_gap + ess_h + row_gap + xp_h;

    let plaque_h = body_h + plaque_pad_y * 2.0;
    let plaque_x = (screen.x - plaque_w) * 0.5;
    let plaque_y = screen.y - bottom_offset_px * s - plaque_h;
    let plaque_rect = Rect::from_xywh(plaque_x, plaque_y, plaque_w, plaque_h);

    Frame::stone(&theme)
        .with_padding(Pad::symmetric(plaque_pad_x, plaque_pad_y))
        .with_radius(2.0 * s)
        .show_only(ui, plaque_rect);

    // Inner stack origin.
    let level_rect = Rect::from_xywh(
        plaque_x + plaque_pad_x,
        plaque_y + plaque_pad_y,
        level_w,
        body_h,
    );
    draw_level_badge(ui, level_rect, view.level);

    let bar_x = level_rect.max.x + level_gap;
    let mut y = plaque_y + plaque_pad_y;

    // ── HP bar ──
    let hp_pct = view.hp_fraction.clamp(0.0, 1.0);
    let hp_rect = Rect::from_xywh(bar_x, y, bar_w, hp_h);
    let hp_anim = {
        let anim = &mut ui.state_mut().vitals.hp;
        anim.tick(hp_pct, view.dt);
        ResourceAnimSnapshot {
            displayed: anim.displayed,
            trail: anim.trail,
            pulse: anim.pulse,
        }
    };
    draw_resource_bar(
        ui,
        hp_rect,
        hp_anim,
        ResourceBarStyle {
            base: Color::rgba(0.16, 0.62, 0.28, 0.98),
            hot: Color::rgba(0.48, 0.92, 0.36, 1.0),
            chip: Color::rgba(1.0, 0.96, 0.90, 0.26),
            glow: Color::rgba(0.74, 1.0, 0.62, 1.0),
        },
        theme.colors.border_stone,
    );
    draw_text_centered_shadow(ui, view.hp_label, 16.0 * s, hp_rect, 0.95);
    y += hp_h + row_gap;

    // ── Essence bar ──
    let ess_pct = view.essence_fraction.clamp(0.0, 1.0);
    let ess_rect = Rect::from_xywh(bar_x, y, bar_w, ess_h);
    let ess_anim = {
        let anim = &mut ui.state_mut().vitals.essence;
        anim.tick(ess_pct, view.dt);
        ResourceAnimSnapshot {
            displayed: anim.displayed,
            trail: anim.trail,
            pulse: anim.pulse,
        }
    };
    draw_resource_bar(
        ui,
        ess_rect,
        ess_anim,
        ResourceBarStyle {
            base: Color::rgba(0.24, 0.48, 0.96, 0.97),
            hot: Color::rgba(0.42, 0.78, 1.0, 1.0),
            chip: Color::rgba(0.78, 0.92, 1.0, 0.32),
            glow: Color::rgba(0.34, 0.72, 1.0, 1.0),
        },
        theme.colors.border_stone,
    );
    draw_text_centered_shadow(ui, view.essence_label, 13.0 * s, ess_rect, 0.95);
    y += ess_h + row_gap;

    // ── XP bar ──
    let xp_pct = view.xp_fraction.clamp(0.0, 1.0);
    let xp_rect = Rect::from_xywh(bar_x, y, bar_w, xp_h);
    let xp_fill = ProgressBar::new(xp_pct)
        .fill(Color::rgba(0.45, 0.30, 0.85, 0.95))
        .border(theme.colors.border_stone)
        .rounded(false)
        .show(ui, xp_rect);
    apply_bar_gradient(ui, xp_fill);
    // Label drawn centered inside the bar now that it's tall
    // enough to host text legibly.
    draw_text_centered_shadow(ui, view.xp_label, 11.0 * s, xp_rect, 0.95);

    plaque_rect
}

fn draw_level_badge(ui: &mut Ui<'_>, rect: Rect, level: u32) {
    let s = ui.scale();
    ui.draw_gradient_rect(
        rect,
        Color::rgba(0.14, 0.10, 0.22, 0.98),
        Color::rgba(0.04, 0.03, 0.08, 0.99),
    );
    ui.draw_rect(
        Rect::from_xywh(
            rect.x() + 2.0 * s,
            rect.y() + 2.0 * s,
            rect.width() - 4.0 * s,
            1.0,
        ),
        Color::rgba(0.78, 0.72, 1.0, 0.14),
    );
    ui.draw_outline(rect, 1.0 * s, Color::rgba(0.58, 0.48, 0.88, 0.82));
    ui.draw_outline(
        Rect::from_xywh(
            rect.x() + 2.0 * s,
            rect.y() + 2.0 * s,
            rect.width() - 4.0 * s,
            rect.height() - 4.0 * s,
        ),
        1.0 * s,
        Color::rgba(0.72, 0.65, 0.98, 0.12),
    );
    ui.draw_gradient_rect(
        Rect::from_xywh(
            rect.max.x - 2.0 * s,
            rect.y() + 3.0 * s,
            1.0 * s,
            rect.height() - 6.0 * s,
        ),
        Color::rgba(0.72, 0.58, 1.0, 0.18),
        Color::rgba(0.0, 0.0, 0.0, 0.18),
    );

    let label = "LV";
    let label_size = 9.0 * s;
    let label_w = ui.measure_text(label, label_size);
    ui.draw_text(
        Pos2::new(
            rect.x() + (rect.width() - label_w) * 0.5,
            rect.y() + 8.0 * s,
        ),
        label,
        label_size,
        Color::rgba(0.76, 0.72, 0.92, 0.92),
    );

    let value = level.to_string();
    let value_size = 20.0 * s;
    let value_w = ui.measure_text(&value, value_size);
    draw_text_at_shadow(
        ui,
        &value,
        Pos2::new(
            rect.x() + (rect.width() - value_w) * 0.5,
            rect.y() + (rect.height() - value_size) * 0.5 + 7.0 * s,
        ),
        value_size,
        Color::rgba(0.92, 0.88, 0.98, 1.0),
    );
}

fn draw_resource_bar(
    ui: &mut Ui<'_>,
    rect: Rect,
    anim: ResourceAnimSnapshot,
    style: ResourceBarStyle,
    border: Color,
) {
    let displayed = anim.displayed.clamp(0.0, 1.0);
    let trail = anim.trail.clamp(displayed, 1.0);
    let pulse = anim.pulse.clamp(0.0, 1.0);

    ui.draw_gradient_rect(
        rect,
        Color::rgba(0.045, 0.042, 0.046, 0.96),
        Color::rgba(0.010, 0.010, 0.013, 0.98),
    );
    ui.draw_rect(
        Rect::from_xywh(rect.x(), rect.y(), rect.width(), 1.0),
        Color::rgba(1.0, 1.0, 1.0, 0.08),
    );

    let trail_w = rect.width() * trail;
    let fill_w = rect.width() * displayed;
    if trail_w > fill_w + 0.5 {
        let chip_rect =
            Rect::from_xywh(rect.x() + fill_w, rect.y(), trail_w - fill_w, rect.height());
        ui.draw_grad4_rect(
            chip_rect,
            style.chip,
            style.chip.fade(0.52),
            Color::rgba(0.0, 0.0, 0.0, 0.18),
            style.chip.fade(0.20),
        );
    }

    if fill_w > 0.5 {
        let fill = Rect::from_xywh(rect.x(), rect.y(), fill_w, rect.height());
        let pulse_lift = 1.0 + pulse * 0.22;
        ui.draw_grad4_rect(
            fill,
            scale_rgb(style.hot, pulse_lift),
            scale_rgb(style.base, 1.04 + pulse * 0.16),
            scale_rgb(style.base, 0.64),
            scale_rgb(style.base, 0.78 + pulse * 0.10),
        );
        apply_bar_gradient(ui, fill);
        draw_bar_sheen(ui, fill, pulse);
        draw_bar_cursor(ui, rect, fill_w, style.glow, pulse);
    }

    if pulse > 0.01 {
        let glow_w = rect.width() * displayed;
        if glow_w > 1.0 {
            let glow = Rect::from_xywh(
                rect.x() - 2.0,
                rect.y() - 2.0,
                glow_w + 4.0,
                rect.height() + 4.0,
            );
            ui.draw_grad4_rect(
                glow,
                style.glow.fade(0.08 * pulse),
                style.glow.fade(0.02 * pulse),
                style.glow.fade(0.02 * pulse),
                style.glow.fade(0.01 * pulse),
            );
        }
    }

    ui.draw_outline(rect, 1.0, border);
}

fn draw_bar_sheen(ui: &mut Ui<'_>, fill: Rect, pulse: f32) {
    let top = Rect::from_xywh(
        fill.x(),
        fill.y() + 1.0,
        fill.width(),
        (fill.height() * 0.38).max(2.0),
    );
    ui.draw_gradient_rect(
        top,
        Color::rgba(1.0, 1.0, 1.0, 0.18 + pulse * 0.08),
        Color::rgba(1.0, 1.0, 1.0, 0.015),
    );
    let streak_w = (fill.height() * 1.8).clamp(14.0, 34.0).min(fill.width());
    if streak_w > 2.0 {
        let streak_x = (fill.max.x - streak_w * 1.08).max(fill.x());
        ui.draw_grad4_rect(
            Rect::from_xywh(streak_x, fill.y() + 1.0, streak_w, fill.height() - 2.0),
            Color::rgba(1.0, 1.0, 1.0, 0.00),
            Color::rgba(1.0, 1.0, 1.0, 0.18 + pulse * 0.12),
            Color::rgba(1.0, 1.0, 1.0, 0.00),
            Color::rgba(1.0, 1.0, 1.0, 0.04 + pulse * 0.04),
        );
    }
}

fn draw_bar_cursor(ui: &mut Ui<'_>, rect: Rect, fill_w: f32, glow: Color, pulse: f32) {
    if fill_w <= 1.0 || fill_w >= rect.width() - 0.5 {
        return;
    }
    let x = rect.x() + fill_w;
    let halo_w = (rect.height() * 0.90).clamp(8.0, 20.0);
    ui.draw_grad4_rect(
        Rect::from_xywh(x - halo_w * 0.55, rect.y(), halo_w, rect.height()),
        Color::rgba(1.0, 1.0, 1.0, 0.0),
        glow.fade(0.20 + pulse * 0.22),
        Color::rgba(1.0, 1.0, 1.0, 0.0),
        glow.fade(0.05 + pulse * 0.08),
    );
    ui.draw_gradient_rect(
        Rect::from_xywh(x - 1.0, rect.y() + 1.0, 2.0, rect.height() - 2.0),
        Color::rgba(1.0, 1.0, 1.0, 0.72 + pulse * 0.18),
        glow.fade(0.40 + pulse * 0.22),
    );
}

fn scale_rgb(color: Color, mul: f32) -> Color {
    Color::rgba(
        (color.0[0] * mul).clamp(0.0, 1.0),
        (color.0[1] * mul).clamp(0.0, 1.0),
        (color.0[2] * mul).clamp(0.0, 1.0),
        color.0[3],
    )
}

/// Overlay a vertical highlight → shadow gradient on the filled
/// portion of a bar. The top is a soft white sheen, the bottom
/// a dark wash; both at low alpha so the underlying fill colour
/// still drives the bar's identity (red HP, blue essence, …).
fn apply_bar_gradient(ui: &mut Ui<'_>, fill_rect: Rect) {
    if fill_rect.width() <= 0.0 || fill_rect.height() <= 0.0 {
        return;
    }
    ui.draw_gradient_rect(
        fill_rect,
        Color::rgba(1.0, 1.0, 1.0, 0.22),
        Color::rgba(0.0, 0.0, 0.0, 0.28),
    );
}

/// Draw `text` centered inside `rect`, with a 1 px offset black
/// shadow for legibility against any bar fill.
fn draw_text_centered_shadow(ui: &mut Ui<'_>, text: &str, size: f32, rect: Rect, alpha: f32) {
    let tw = ui.measure_text(text, size);
    let pos = Pos2::new(
        rect.x() + (rect.width() - tw) * 0.5,
        rect.y() + (rect.height() - size) * 0.5,
    );
    draw_text_at_shadow(ui, text, pos, size, Color::rgba(0.96, 0.96, 0.98, alpha));
}

/// Like `draw_text_centered_shadow` but takes a pre-computed
/// origin. Both helpers funnel through the same shadow recipe
/// so any future tweak (offset, alpha, double-shadow) lands in
/// one place.
fn draw_text_at_shadow(ui: &mut Ui<'_>, text: &str, pos: Pos2, size: f32, color: Color) {
    let shadow = Color::rgba(0.0, 0.0, 0.0, 0.75);
    ui.draw_text(Pos2::new(pos.x + 1.0, pos.y + 1.0), text, size, shadow);
    ui.draw_text(pos, text, size, color);
}
