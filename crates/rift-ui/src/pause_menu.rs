//! In-game pause menu (Escape). Renders a centred modal with
//! Resume / Settings / optional Exit-to-Hub / Exit-to-Character-Select / Exit-Game
//! buttons over a dimmed backdrop.
//!
//! State-less widget — the host owns the "menu open" flag and
//! decides each frame whether to call us. Returns `None` when
//! no input occurred, `Some(action)` when the player clicked a
//! button or pressed Escape (which maps to `Resume`).

use rift_ui_im::{
    widgets::{label, title},
    Button, ButtonSize, Color, Frame, Id, Layer, Pad, Rect, Ui, Vec2,
};
use rift_ui_types::pause_menu::PauseMenuAction;

use crate::icons::{draw_placeholder_icon, icon_rect_left, UiIcon};

/// One frame of the pause menu. Call inside `Ui::begin`/`end`
/// scope when the menu is open.
pub fn frame_pause_menu(ui: &mut Ui<'_>, in_rift: bool) -> Option<PauseMenuAction> {
    let screen = ui.screen_rect();
    ui.with_layer(Layer::Modal, |ui| {
        ui.draw_rect(screen, Color::rgba(0.0, 0.0, 0.0, 0.55));
    });

    // Eat the click underneath so dismiss-on-backdrop doesn't
    // also fire the spell on the world.
    ui.claim_mouse();

    let theme = *ui.theme();
    let sc = theme.scale;
    let s = ui.screen_size();
    let mw = 360.0 * sc;
    let mh = if in_rift { 398.0 * sc } else { 340.0 * sc };
    let modal_rect = Rect::from_xywh((s.x - mw) * 0.5, (s.y - mh) * 0.5, mw, mh);

    let mut action: Option<PauseMenuAction> = None;
    ui.with_layer(Layer::Modal, |ui| {
        Frame::stone(&theme)
            .with_padding(Pad::all(20.0 * sc))
            .show(ui, modal_rect, |ui, body| {
                let _ = title(ui, body.min, "Paused");
                label(ui, body.min + Vec2::new(0.0, 36.0 * sc), "");

                let id = Id::root("pause_menu");
                let btn_h = 48.0 * sc;
                let gap = 10.0 * sc;
                let mut y = body.min.y + 60.0 * sc;
                let bw = body.size().x;

                let resume_rect = Rect::from_xywh(body.min.x, y, bw, btn_h);
                if Button::primary("Resume")
                    .size(ButtonSize::Large)
                    .show_with_id(ui, id.child("resume"), resume_rect)
                    .clicked
                {
                    action = Some(PauseMenuAction::Resume);
                }
                draw_placeholder_icon(
                    ui,
                    icon_rect_left(resume_rect, 22.0 * sc, 14.0 * sc),
                    UiIcon::Play,
                    theme.colors.text,
                );
                y += btn_h + gap;

                let settings_rect = Rect::from_xywh(body.min.x, y, bw, btn_h);
                if Button::new("  Settings")
                    .size(ButtonSize::Large)
                    .show_with_id(ui, id.child("settings"), settings_rect)
                    .clicked
                {
                    action = Some(PauseMenuAction::OpenSettings);
                }
                draw_placeholder_icon(
                    ui,
                    icon_rect_left(settings_rect, 22.0 * sc, 14.0 * sc),
                    UiIcon::Gear,
                    theme.colors.text,
                );
                y += btn_h + gap;

                if in_rift {
                    let hub_rect = Rect::from_xywh(body.min.x, y, bw, btn_h);
                    if Button::danger("  Exit to Hub")
                        .size(ButtonSize::Large)
                        .show_with_id(ui, id.child("hub"), hub_rect)
                        .clicked
                    {
                        action = Some(PauseMenuAction::ExitToHub);
                    }
                    draw_placeholder_icon(
                        ui,
                        icon_rect_left(hub_rect, 22.0 * sc, 14.0 * sc),
                        UiIcon::Portal,
                        theme.colors.text,
                    );
                    y += btn_h + gap;
                }

                let chsel_rect = Rect::from_xywh(body.min.x, y, bw, btn_h);
                if Button::new("  Exit to Character Select")
                    .size(ButtonSize::Large)
                    .show_with_id(ui, id.child("chsel"), chsel_rect)
                    .clicked
                {
                    action = Some(PauseMenuAction::ExitToCharacterSelect);
                }
                draw_placeholder_icon(
                    ui,
                    icon_rect_left(chsel_rect, 22.0 * sc, 14.0 * sc),
                    UiIcon::Character,
                    theme.colors.text,
                );
                y += btn_h + gap;

                let quit_rect = Rect::from_xywh(body.min.x, y, bw, btn_h);
                if Button::danger("  Exit Game")
                    .size(ButtonSize::Large)
                    .show_with_id(ui, id.child("quit"), quit_rect)
                    .clicked
                {
                    action = Some(PauseMenuAction::ExitGame);
                }
                draw_placeholder_icon(
                    ui,
                    icon_rect_left(quit_rect, 22.0 * sc, 14.0 * sc),
                    UiIcon::Exit,
                    theme.colors.text,
                );
            });
    });

    // Note: Escape is intentionally handled by the caller
    // (`tick_pause_menu` in rift-client). Handling it here too
    // would race against the host's own "open menu on Escape"
    // edge, slamming the menu shut on the very frame it opens.

    action
}
