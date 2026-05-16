//! Settings sub-screen. Reached from the pause menu's "Settings"
//! button; closes via the on-modal Back button or by pressing Escape.
//!
//! Returns a `Vec<SettingsAction>` so a single frame can both
//! drag the slider (emitting `SetMasterVolume`) AND click Back
//! (emitting `Close`) without losing either edge.

use rift_ui_im::{
    widgets::{label, title, tooltip_at_mouse, TooltipLine},
    Button, ButtonSize, Color, Frame, Id, Layer, Pad, Rect, Ui, Vec2,
};
use rift_ui_types::settings::{DisplayResolution, SettingsAction, SettingsView};

use crate::icons::{draw_placeholder_icon, icon_rect_left, UiIcon};

/// One frame of the settings panel.
pub fn frame_settings(ui: &mut Ui<'_>, view: &SettingsView<'_>) -> Vec<SettingsAction> {
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
    let mh = 580.0 * sc;
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
                draw_placeholder_icon(
                    ui,
                    Rect::from_xywh(body.min.x, row_y + 4.0 * sc, 20.0 * sc, 20.0 * sc),
                    UiIcon::Volume,
                    theme.colors.text_dim,
                );
                label(
                    ui,
                    body.min + Vec2::new(28.0 * sc, 56.0 * sc + 4.0 * sc),
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
                setting_tooltip(
                    ui,
                    body,
                    "master_vol_tip",
                    Rect::from_xywh(body.min.x, row_y - 4.0 * sc, body.size().x, 40.0 * sc),
                    "Master Volume",
                    "Adjusts the overall game audio level for this session.",
                );

                let pct = (view.master_volume.clamp(0.0, 1.0) * 100.0).round() as i32;
                let pct_text = format!("{pct} %");
                ui.draw_text(
                    rift_ui_im::Pos2::new(slider_right + 8.0 * sc, row_y + 4.0 * sc),
                    &pct_text,
                    14.0 * sc,
                    theme.colors.text,
                );

                label(ui, body.min + Vec2::new(0.0, 118.0 * sc), "Graphics");

                let toggle_w = 116.0 * sc;
                let toggle_h = 34.0 * sc;
                if toggle_row(
                    ui,
                    body,
                    "shadows",
                    "Realtime Shadows",
                    "Renders dynamic directional and point-light shadows. Turn off for a large GPU performance win.",
                    view.shadows_enabled,
                    UiIcon::Shield,
                    body.min.y + 148.0 * sc,
                    toggle_w,
                    toggle_h,
                ) {
                    actions.push(SettingsAction::SetShadowsEnabled(!view.shadows_enabled));
                }

                if toggle_row(
                    ui,
                    body,
                    "height_shadows",
                    "Texture Height Shadows",
                    "Uses material height maps to add subtle receiver self-shadowing. Costs extra shading work.",
                    view.height_shadows_enabled,
                    UiIcon::Shield,
                    body.min.y + 190.0 * sc,
                    toggle_w,
                    toggle_h,
                ) {
                    actions.push(SettingsAction::SetHeightShadowsEnabled(
                        !view.height_shadows_enabled,
                    ));
                }

                if toggle_row(
                    ui,
                    body,
                    "bloom",
                    "Bloom",
                    "Adds glow from bright pixels through the post-process blur stack. Disable to reduce post-processing cost.",
                    view.bloom_enabled,
                    UiIcon::Damage,
                    body.min.y + 232.0 * sc,
                    toggle_w,
                    toggle_h,
                ) {
                    actions.push(SettingsAction::SetBloomEnabled(!view.bloom_enabled));
                }

                if toggle_row(
                    ui,
                    body,
                    "ssao",
                    "Ambient Occlusion",
                    "Darkens creases and contact areas with screen-space ambient occlusion. Costs an extra post pass.",
                    view.ssao_enabled,
                    UiIcon::Filter,
                    body.min.y + 274.0 * sc,
                    toggle_w,
                    toggle_h,
                ) {
                    actions.push(SettingsAction::SetSsaoEnabled(!view.ssao_enabled));
                }

                if toggle_row(
                    ui,
                    body,
                    "volumetrics",
                    "Volumetric Rays",
                    "Adds foggy light shafts in post-processing. Disable if heavy scenes hitch near bright lights.",
                    view.volumetrics_enabled,
                    UiIcon::Portal,
                    body.min.y + 316.0 * sc,
                    toggle_w,
                    toggle_h,
                ) {
                    actions.push(SettingsAction::SetVolumetricsEnabled(
                        !view.volumetrics_enabled,
                    ));
                }

                if toggle_row(
                    ui,
                    body,
                    "vsync",
                    "VSync",
                    "Uses the guaranteed FIFO present mode when available, reducing tearing by syncing presents to the display.",
                    view.vsync_enabled,
                    UiIcon::Monitor,
                    body.min.y + 358.0 * sc,
                    toggle_w,
                    toggle_h,
                ) {
                    actions.push(SettingsAction::SetVsyncEnabled(!view.vsync_enabled));
                }

                if let Some(resolution) = resolution_row(
                    ui,
                    body,
                    view.display_resolutions,
                    view.selected_resolution,
                    body.min.y + 400.0 * sc,
                ) {
                    actions.push(SettingsAction::SetDisplayResolution(resolution));
                }

                // Back button at the bottom-right.
                let btn_h = 44.0 * sc;
                let btn_w = 140.0 * sc;
                let back_rect =
                    Rect::from_xywh(body.max.x - btn_w, body.max.y - btn_h, btn_w, btn_h);
                if Button::new("  Back")
                    .size(ButtonSize::Large)
                    .show_with_id(ui, Id::root("settings").child("back"), back_rect)
                    .clicked
                {
                    actions.push(SettingsAction::Close);
                }
                draw_placeholder_icon(
                    ui,
                    icon_rect_left(back_rect, 20.0 * sc, 14.0 * sc),
                    UiIcon::Back,
                    theme.colors.text,
                );
            });
    });

    // Escape is intentionally handled by the caller. See the
    // matching note in `pause_menu.rs`.

    actions
}

fn toggle_row(
    ui: &mut Ui<'_>,
    body: Rect,
    id: &'static str,
    text: &'static str,
    tooltip: &'static str,
    enabled: bool,
    icon: UiIcon,
    y: f32,
    toggle_w: f32,
    toggle_h: f32,
) -> bool {
    let theme = *ui.theme();
    let sc = theme.scale;
    draw_placeholder_icon(
        ui,
        Rect::from_xywh(body.min.x, y + 7.0 * sc, 20.0 * sc, 20.0 * sc),
        icon,
        theme.colors.text_dim,
    );
    label(
        ui,
        rift_ui_im::Pos2::new(body.min.x + 28.0 * sc, y + 10.0 * sc),
        text,
    );
    let toggle_rect = Rect::from_xywh(body.max.x - toggle_w, y, toggle_w, toggle_h);
    let toggle_label = if enabled { "On" } else { "Off" };
    let toggle_resp = if enabled {
        Button::active(toggle_label)
    } else {
        Button::new(toggle_label)
    }
    .show_with_id(ui, Id::root("settings").child(id), toggle_rect);

    setting_tooltip(
        ui,
        body,
        id,
        Rect::from_xywh(body.min.x, y, body.size().x, toggle_h),
        text,
        tooltip,
    );
    toggle_resp.clicked
}

fn resolution_row(
    ui: &mut Ui<'_>,
    body: Rect,
    resolutions: &[DisplayResolution],
    selected: DisplayResolution,
    y: f32,
) -> Option<DisplayResolution> {
    let theme = *ui.theme();
    let sc = theme.scale;
    let row_h = 34.0 * sc;
    let value_w = 184.0 * sc;
    let dropdown_id = Id::root("settings").child("resolution_dropdown");

    draw_placeholder_icon(
        ui,
        Rect::from_xywh(body.min.x, y + 7.0 * sc, 20.0 * sc, 20.0 * sc),
        UiIcon::Monitor,
        theme.colors.text_dim,
    );
    label(
        ui,
        rift_ui_im::Pos2::new(body.min.x + 28.0 * sc, y + 10.0 * sc),
        "Resolution",
    );

    let selected_text = resolution_label(selected);
    let value_rect = Rect::from_xywh(body.max.x - value_w, y, value_w, row_h);
    let open = ui.state().focus == Some(dropdown_id);
    let button_label = if open {
        format!("{} ^", selected_text)
    } else {
        format!("{} v", selected_text)
    };

    let mut picked = None;
    let button_resp = Button::new(&button_label)
        .enabled(!resolutions.is_empty())
        .show_with_id(ui, dropdown_id, value_rect);
    if button_resp.clicked {
        ui.state_mut().focus = if open { None } else { Some(dropdown_id) };
    }

    let mut hovered_dropdown = button_resp.hovered;
    if open {
        let option_h = 30.0 * sc;
        let list_len = resolutions.len().min(8);
        let list_rect = Rect::from_xywh(
            value_rect.min.x,
            value_rect.max.y + 4.0 * sc,
            value_rect.size().x,
            option_h * list_len as f32,
        );

        ui.with_layer(Layer::Tooltip, |ui| {
            ui.draw_rect(list_rect, Color::rgba(0.04, 0.032, 0.068, 0.98));
            ui.draw_line(
                list_rect.min,
                rift_ui_im::Pos2::new(list_rect.max.x, list_rect.min.y),
                1.0 * sc,
                Color::rgba(0.68, 0.52, 0.92, 0.78),
            );

            for (i, resolution) in resolutions.iter().take(list_len).enumerate() {
                let option_rect = Rect::from_xywh(
                    list_rect.min.x,
                    list_rect.min.y + i as f32 * option_h,
                    list_rect.size().x,
                    option_h,
                );
                let option_id = Id::root("settings").child(("resolution_option", i as u32));
                let hovered = ui.interact_hover(option_id, option_rect);
                let selected_option = *resolution == selected;
                hovered_dropdown |= hovered;

                let fill = if selected_option {
                    Color::rgba(0.22, 0.12, 0.36, 0.98)
                } else if hovered {
                    Color::rgba(0.16, 0.10, 0.28, 0.98)
                } else if i % 2 == 0 {
                    Color::rgba(0.08, 0.06, 0.14, 0.98)
                } else {
                    Color::rgba(0.06, 0.048, 0.10, 0.98)
                };
                ui.draw_rect(option_rect, fill);

                let label_text = resolution_label(*resolution);
                ui.draw_text(
                    rift_ui_im::Pos2::new(
                        option_rect.min.x + 12.0 * sc,
                        option_rect.min.y + 8.0 * sc,
                    ),
                    &label_text,
                    13.0 * sc,
                    theme.colors.text,
                );

                if hovered && ui.input().left_clicked() {
                    picked = Some(*resolution);
                    ui.state_mut().focus = None;
                }
            }
        });

        if ui.input().left_just_pressed() && !hovered_dropdown {
            ui.state_mut().focus = None;
        }
    }

    if !open {
        setting_tooltip(
            ui,
            body,
            "resolution_tip",
            Rect::from_xywh(body.min.x, y, body.size().x, row_h),
            "Resolution",
            "Changes the display mode to a resolution reported by the current monitor. Lower values reduce GPU pixel work.",
        );
    }

    picked
}

fn resolution_label(resolution: DisplayResolution) -> String {
    if resolution.width == 0 || resolution.height == 0 {
        "Unknown".to_string()
    } else {
        format!("{} x {}", resolution.width, resolution.height)
    }
}

fn setting_tooltip(
    ui: &mut Ui<'_>,
    body: Rect,
    id: &'static str,
    rect: Rect,
    header: &'static str,
    text: &'static str,
) {
    let hover = ui.interact_hover(Id::root("settings_tip").child(id), rect);
    if !hover {
        return;
    }

    let theme = *ui.theme();
    let lines = [TooltipLine::new(
        text,
        theme.fonts.size_sm,
        theme.colors.text_muted,
    )];
    let _ = body;
    tooltip_at_mouse(ui, Some(header), &lines);
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
        Color::rgba(0.82, 0.62, 1.0, 1.0)
    } else if hover {
        Color::rgba(0.72, 0.52, 0.92, 1.0)
    } else {
        Color::rgba(0.62, 0.44, 0.86, 1.0)
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
        Color::rgba(0.94, 0.90, 1.0, 1.0)
    } else {
        Color::rgba(0.88, 0.84, 0.98, 0.95)
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
