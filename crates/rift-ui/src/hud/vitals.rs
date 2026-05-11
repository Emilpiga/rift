//! Bottom-center HP / Essence / XP vitals stack.
//!
//! Wraps three progress bars in a `Frame::stone` plaque so the
//! cluster reads as a single carved-stone surface. Bars are
//! flat-edged (no rounded radius) so they read as tiles in the
//! plaque, then receive a top-down highlight → bottom-shadow
//! gradient overlay for a slight beveled look. All labels are
//! drawn through `draw_text_shadow` so they pop against the
//! bar fill without needing a separate stroke pass.

use rift_ui_im::{hp_color, Color, Frame, Pad, Pos2, ProgressBar, Rect, Ui};
use rift_ui_types::hud::HudVitalsView;

use super::ability_bar;

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
    let bar_w = plaque_w - plaque_pad_x * 2.0;

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
    let bar_x = plaque_x + plaque_pad_x;
    let mut y = plaque_y + plaque_pad_y;

    // ── HP bar ──
    let hp_pct = view.hp_fraction.clamp(0.0, 1.0);
    let hp_rect = Rect::from_xywh(bar_x, y, bar_w, hp_h);
    let hp_fill = ProgressBar::new(hp_pct)
        .fill(hp_color(hp_pct))
        .border(theme.colors.border_stone)
        .rounded(false)
        .show(ui, hp_rect);
    apply_bar_gradient(ui, hp_fill);
    draw_text_centered_shadow(ui, view.hp_label, 16.0 * s, hp_rect, 0.95);
    y += hp_h + row_gap;

    // ── Essence bar ──
    let ess_pct = view.essence_fraction.clamp(0.0, 1.0);
    let ess_rect = Rect::from_xywh(bar_x, y, bar_w, ess_h);
    let ess_fill = ProgressBar::new(ess_pct)
        .fill(Color::rgba(0.32, 0.55, 0.95, 0.95))
        .border(theme.colors.border_stone)
        .rounded(false)
        .show(ui, ess_rect);
    apply_bar_gradient(ui, ess_fill);
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

    // ── Level pip — floats just left of the plaque so the
    //    eye groups "player identity" with the vital pools.
    let level_text = format!("Lv.{}", view.level);
    let level_pos = Pos2::new(plaque_x - 54.0 * s, plaque_y + plaque_pad_y + 6.0 * s);
    draw_text_at_shadow(
        ui,
        &level_text,
        level_pos,
        17.0 * s,
        Color::rgba(0.92, 0.92, 0.92, 1.0),
    );

    plaque_rect
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
