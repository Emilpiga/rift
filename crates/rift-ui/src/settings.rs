//! Settings sub-screen. Reached from the pause menu's "Settings"
//! button; closes via the on-modal Back button or by pressing Escape.
//!
//! Returns a `Vec<SettingsAction>` so a single frame can both
//! drag the slider (emitting `SetMasterVolume`) AND click Back
//! (emitting `Close`) without losing either edge.

use rift_ui_im::{
    widgets::{label, title},
    Button, ButtonSize, Color, Frame, Id, Layer, Pad, Rect, Ui, Vec2,
};
use rift_ui_types::settings::{SettingsAction, SettingsView};

/// One frame of the settings panel.
pub fn frame_settings(ui: &mut Ui<'_>, view: &SettingsView) -> Vec<SettingsAction> {
    let mut actions = Vec::new();

    let screen = ui.screen_rect();
    ui.with_layer(Layer::Modal, |ui| {
        ui.draw_rect(screen, Color::rgba(0.0, 0.0, 0.0, 0.55));
    });
    ui.claim_mouse();

    let theme = *ui.theme();
    let sc = theme.scale;
    let s = ui.screen_size();
    let mw = 460.0 * sc;
    let mh = 320.0 * sc;
    let modal_rect = Rect::from_xywh((s.x - mw) * 0.5, (s.y - mh) * 0.5, mw, mh);

    ui.with_layer(Layer::Modal, |ui| {
        Frame::stone(&theme)
            .with_padding(Pad::all(20.0 * sc))
            .show(ui, modal_rect, |ui, body| {
                let _ = title(ui, body.min, "Settings");

                // Volume row: label on the left, slider filling
                // the rest of the row, percent readout at the
                // right.
                let row_y = body.min.y + 56.0 * sc;
                let row_h = 28.0 * sc;
                label(
                    ui,
                    body.min + Vec2::new(0.0, 56.0 * sc + 4.0 * sc),
                    "Master Volume",
                );

                let value_w = 56.0 * sc;
                let slider_left = body.min.x + 160.0 * sc;
                let slider_right = body.max.x - value_w - 8.0 * sc;
                let slider_rect = Rect::from_xywh(
                    slider_left,
                    row_y + (row_h - 10.0 * sc) * 0.5,
                    (slider_right - slider_left).max(40.0 * sc),
                    10.0 * sc,
                );

                if let Some(new_v) = volume_slider(
                    ui,
                    Id::root("settings").child("master_vol"),
                    slider_rect,
                    view.master_volume,
                ) {
                    actions.push(SettingsAction::SetMasterVolume(new_v));
                }

                let pct = (view.master_volume.clamp(0.0, 1.0) * 100.0).round() as i32;
                let pct_text = format!("{pct} %");
                ui.draw_text(
                    rift_ui_im::Pos2::new(slider_right + 8.0 * sc, row_y + 4.0 * sc),
                    &pct_text,
                    14.0 * sc,
                    theme.colors.text,
                );

                label(ui, body.min + Vec2::new(0.0, 118.0 * sc), "Graphics");

                let shadows_y = body.min.y + 148.0 * sc;
                label(
                    ui,
                    rift_ui_im::Pos2::new(body.min.x, shadows_y + 10.0 * sc),
                    "Realtime Shadows",
                );
                let toggle_w = 116.0 * sc;
                let toggle_h = 34.0 * sc;
                let toggle_rect =
                    Rect::from_xywh(body.max.x - toggle_w, shadows_y, toggle_w, toggle_h);
                let toggle_label = if view.shadows_enabled { "On" } else { "Off" };
                let toggle_resp = if view.shadows_enabled {
                    Button::active(toggle_label)
                } else {
                    Button::new(toggle_label)
                }
                .show_with_id(
                    ui,
                    Id::root("settings").child("shadows"),
                    toggle_rect,
                );
                if toggle_resp.clicked {
                    actions.push(SettingsAction::SetShadowsEnabled(!view.shadows_enabled));
                }

                let height_y = body.min.y + 190.0 * sc;
                label(
                    ui,
                    rift_ui_im::Pos2::new(body.min.x, height_y + 10.0 * sc),
                    "Texture Height Shadows",
                );
                let height_rect =
                    Rect::from_xywh(body.max.x - toggle_w, height_y, toggle_w, toggle_h);
                let height_label = if view.height_shadows_enabled {
                    "On"
                } else {
                    "Off"
                };
                let height_resp = if view.height_shadows_enabled {
                    Button::active(height_label)
                } else {
                    Button::new(height_label)
                }
                .show_with_id(
                    ui,
                    Id::root("settings").child("height_shadows"),
                    height_rect,
                );
                if height_resp.clicked {
                    actions.push(SettingsAction::SetHeightShadowsEnabled(
                        !view.height_shadows_enabled,
                    ));
                }

                // Back button at the bottom-right.
                let btn_h = 44.0 * sc;
                let btn_w = 140.0 * sc;
                let back_rect =
                    Rect::from_xywh(body.max.x - btn_w, body.max.y - btn_h, btn_w, btn_h);
                if Button::new("Back")
                    .size(ButtonSize::Large)
                    .show_with_id(ui, Id::root("settings").child("back"), back_rect)
                    .clicked
                {
                    actions.push(SettingsAction::Close);
                }
            });
    });

    // Escape is intentionally handled by the caller. See the
    // matching note in `pause_menu.rs`.

    actions
}

/// Horizontal volume slider. Returns the new value when the
/// player is dragging (or just-clicked) over the track, `None`
/// otherwise. Pure-immediate: `state.dragging` is implicit in
/// `left_mouse_held` + hover.
///
/// Visual: a slim filled track from 0..value, an empty tail
/// from value..1, and a square thumb centred on the value
/// position.
fn volume_slider(ui: &mut Ui<'_>, id: Id, rect: Rect, value: f32) -> Option<f32> {
    let theme = *ui.theme();
    let sc = theme.scale;
    let v = value.clamp(0.0, 1.0);

    // Slightly enlarged hit-rect so the player can grab the
    // thumb even when it's at the edges of the track.
    let pad = 8.0 * sc;
    let hit = Rect::from_xywh(
        rect.min.x - pad,
        rect.min.y - pad,
        rect.size().x + pad * 2.0,
        rect.size().y + pad * 2.0,
    );
    let hover = ui.interact_hover(id, hit);
    let mouse = ui.mouse_pos();
    let dragging = hover && ui.input().left_mouse_held();

    // Track background.
    let track_color = Color::rgba(0.10, 0.10, 0.12, 0.85);
    let fill_color = if dragging {
        Color::rgba(0.95, 0.75, 0.30, 1.0)
    } else if hover {
        Color::rgba(0.85, 0.65, 0.25, 1.0)
    } else {
        Color::rgba(0.75, 0.55, 0.20, 1.0)
    };
    ui.draw_rect(rect, track_color);

    let fill_w = rect.size().x * v;
    if fill_w > 0.5 {
        let fill_rect = Rect::from_xywh(rect.min.x, rect.min.y, fill_w, rect.size().y);
        ui.draw_rect(fill_rect, fill_color);
    }

    // Thumb.
    let thumb_w = 14.0 * sc;
    let thumb_h = rect.size().y + 8.0 * sc;
    let thumb_x = rect.min.x + fill_w - thumb_w * 0.5;
    let thumb_y = rect.center().y - thumb_h * 0.5;
    let thumb_rect = Rect::from_xywh(thumb_x, thumb_y, thumb_w, thumb_h);
    let thumb_color = if dragging {
        Color::rgba(1.0, 0.95, 0.85, 1.0)
    } else {
        Color::rgba(0.95, 0.90, 0.80, 0.95)
    };
    ui.draw_rect(thumb_rect, thumb_color);

    if dragging || (hover && ui.input().left_just_pressed()) {
        let w = rect.size().x.max(1.0);
        let new_v = ((mouse.x - rect.min.x) / w).clamp(0.0, 1.0);
        if (new_v - value).abs() > 1e-4 {
            return Some(new_v);
        }
    }
    None
}
