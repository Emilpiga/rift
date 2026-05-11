//! Bottom-left chat HUD widget.
//!
//! Hot-reloadable. The host (`rift_client::game::chat::ChatUi`)
//! flattens its state into a [`ChatView`] every frame and
//! passes a `&mut String` draft buffer for the text input.
//! Returned [`ChatAction`]s tell the host how the player
//! interacted with the channel button / picker dropdown so the
//! host can update its own state in-place.
//!
//! The input field + channel selector are rendered every
//! frame regardless of whether the player is "actively
//! typing" — the chat surface is always visible so the
//! player can glance at it without first pressing T.
//! `view.open` only controls auto-focus (so opening the chat
//! still snaps the cursor to the field) and the placeholder
//! wording.

use rift_ui_im::{
    widgets::TextField, Button, ButtonSize, Color, Frame, Id, Pad, Pos2, Rect, Theme, Ui,
};
use rift_ui_types::chat::{ChatAction, ChatView};

/// Render the chat scrollback + input field + channel
/// selector. Returns a list of host-actionable events; multiple
/// may be returned per frame (e.g. picker open + outside-click
/// dismiss both fire on the same press).
///
/// The `draft` buffer is mutated in place by the text field;
/// the host owns the string and is free to read / clear it
/// outside of this widget.
pub fn frame_chat(
    ui: &mut Ui<'_>,
    view: &ChatView<'_>,
    draft: &mut String,
    time: f32,
) -> Vec<ChatAction> {
    let theme = *ui.theme();
    let screen = ui.screen_size();
    let scale = theme.scale;
    let mut actions: Vec<ChatAction> = Vec::new();

    // Container geometry. A touch wider + taller than the
    // legacy version so larger text fits comfortably and the
    // panel reads as a deliberate UI region rather than a few
    // transparent lines floating in the corner.
    let panel_w = (460.0 * scale).min(screen.x * 0.45);
    let panel_h = 240.0 * scale;
    let margin = 16.0 * scale;
    let input_h = 42.0 * scale;
    let input_gap = 6.0 * scale;

    let scrollback_rect = Rect::from_xywh(
        margin,
        screen.y - margin - input_h - input_gap - panel_h,
        panel_w,
        panel_h,
    );

    Frame::panel(&theme)
        .with_fill(Color::rgba(0.05, 0.06, 0.09, 0.62))
        .with_padding(Pad::all(8.0 * scale))
        .show(ui, scrollback_rect, |ui, body| {
            draw_scrollback(ui, body, view.messages, &theme);
        });

    // ── Input row (always present) ──
    let input_rect = Rect::from_xywh(margin, screen.y - margin - input_h, panel_w, input_h);

    // Channel button drawn to the left of the input field in
    // the standard Button styling used everywhere else in the
    // app. The label combines the channel short-name with a
    // small chevron so the click affordance reads.
    let button_label = format!(
        "{}  {}",
        view.channel_short,
        if view.picker_open {
            "\u{25B2}"
        } else {
            "\u{25BC}"
        }
    );
    let button_w =
        (ui.measure_text(&button_label, theme.fonts.size_md) + 28.0 * scale).max(76.0 * scale);
    let button_rect = Rect::from_xywh(input_rect.x(), input_rect.y(), button_w, input_h);
    let button_id = Id::root("chat").child("channel_btn");
    let btn_resp = Button::new(&button_label)
        .size(ButtonSize::Medium)
        .show_with_id(ui, button_id, button_rect);

    // Channel-colour chip pinned to the left edge of the
    // button so the player can identify the active channel
    // without parsing the letter.
    let chip_w = 5.0 * scale;
    let chip_inset = 6.0 * scale;
    let chip = Rect::from_xywh(
        button_rect.x() + chip_inset,
        button_rect.y() + chip_inset,
        chip_w,
        button_rect.height() - chip_inset * 2.0,
    );
    ui.draw_rounded_rect(
        chip,
        chip_w * 0.5,
        Color::rgba(
            view.channel_pip_color[0],
            view.channel_pip_color[1],
            view.channel_pip_color[2],
            view.channel_pip_color[3].max(0.9),
        ),
    );

    let field_x = button_rect.max.x + 6.0 * scale;
    let field_rect = Rect::from_xywh(
        field_x,
        input_rect.y(),
        (input_rect.max.x - field_x).max(0.0),
        input_rect.height(),
    );
    let field_id = Id::root("chat").child("input");
    let placeholder = if view.open {
        "Enter to send  ·  Esc to close"
    } else {
        "Press T to chat"
    };
    // Note: no `auto_focus` here. The host explicitly seeds
    // focus via `ui.state_mut().focus = Some(field_id)` when
    // the player presses T (see `ChatUi::focus_field`). If
    // we also asked the field to auto-focus while `view.open`
    // is true, a click outside the field would blur it and
    // then the very next line of `TextField::show` would
    // immediately re-grab focus, making outside-clicks fail
    // to close the chat.
    let _ = TextField::new(field_id)
        .max_chars(view.max_chars)
        .placeholder(placeholder)
        .show(ui, field_rect, draft, time);

    if btn_resp.clicked {
        actions.push(ChatAction::TogglePicker);
        // Restore field focus immediately if the player is
        // already typing — clicking the button would
        // otherwise blur the field due to the outside-press
        // rule baked into TextField.
        if view.open {
            ui.state_mut().focus = Some(field_id);
        }
    }

    if view.picker_open {
        let any_row_hovered = draw_picker(
            ui,
            &theme,
            button_rect,
            view,
            field_id,
            view.open,
            &mut actions,
        );
        // Outside click anywhere except the button closes the
        // dropdown. The button's own toggle already fired
        // above; closing here would only undo that flip.
        if ui.input().left_clicked() && !any_row_hovered {
            let (mx, my) = ui.input().mouse_pos();
            if !button_rect.contains(Pos2::new(mx, my)) {
                actions.push(ChatAction::ClosePicker);
            }
        }
    }

    actions
}

/// Render the channel-picker dropdown above the channel button.
/// Returns whether the cursor is over any row this frame so
/// the caller can decide whether an outside-click should
/// dismiss the picker.
fn draw_picker(
    ui: &mut Ui<'_>,
    theme: &Theme,
    button_rect: Rect,
    view: &ChatView<'_>,
    field_id: Id,
    chat_open: bool,
    actions: &mut Vec<ChatAction>,
) -> bool {
    let scale = theme.scale;
    let row_h = 30.0 * scale;
    let pad = 4.0 * scale;
    let picker_w = 180.0 * scale;
    let picker_h = row_h * view.picker_options.len() as f32 + pad * 2.0;
    let picker_rect = Rect::from_xywh(
        button_rect.x(),
        button_rect.y() - picker_h - 4.0 * scale,
        picker_w,
        picker_h,
    );
    Frame::panel(theme)
        .with_fill(Color::rgba(0.05, 0.06, 0.09, 0.95))
        .show_only(ui, picker_rect);

    let mut any_row_hovered = false;
    for (i, opt) in view.picker_options.iter().enumerate() {
        let row_rect = Rect::from_xywh(
            picker_rect.x() + pad,
            picker_rect.y() + pad + row_h * i as f32,
            picker_rect.width() - pad * 2.0,
            row_h,
        );
        let row_id = Id::root("chat").child(("picker_row", opt.id));
        let hovered = ui.interact_hover(row_id, row_rect);
        let clicked = hovered && ui.input().left_clicked();
        if hovered {
            any_row_hovered = true;
        }
        let active = opt.id == view.channel;
        let fill = if hovered {
            Color::rgba(0.20, 0.25, 0.35, 0.90)
        } else if active {
            Color::rgba(0.15, 0.18, 0.26, 0.85)
        } else {
            Color::rgba(0.0, 0.0, 0.0, 0.0)
        };
        if fill.0[3] > 0.0 {
            ui.draw_rounded_rect(row_rect, 4.0, fill);
        }
        let pip = Rect::from_xywh(
            row_rect.x() + 6.0 * scale,
            row_rect.y() + (row_rect.height() - 14.0 * scale) * 0.5,
            14.0 * scale,
            14.0 * scale,
        );
        ui.draw_rounded_rect(
            pip,
            3.0,
            Color::rgba(
                opt.pip_color[0],
                opt.pip_color[1],
                opt.pip_color[2],
                opt.pip_color[3],
            ),
        );
        draw_text_shadow(
            ui,
            Pos2::new(
                pip.x() + pip.width() + 10.0 * scale,
                row_rect.y() + (row_rect.height() - theme.fonts.size_md) * 0.5,
            ),
            opt.label,
            theme.fonts.size_md,
            Color::rgb(0.95, 0.95, 0.95),
        );
        if clicked {
            actions.push(ChatAction::SelectChannel(opt.id));
            // Snap focus back to the field if the chat is
            // actually active; forcing focus while chat is
            // closed would steal it from gameplay.
            if chat_open {
                ui.state_mut().focus = Some(field_id);
            }
        }
    }
    any_row_hovered
}

/// Render the most recent lines that fit inside `body`,
/// bottom-aligned so the newest line is closest to the input
/// field. Every row is drawn with a 1 px black shadow under
/// the foreground colour so chat stays readable against the
/// busiest world backgrounds.
fn draw_scrollback(
    ui: &mut Ui<'_>,
    body: Rect,
    messages: &[rift_ui_types::chat::ChatLineView<'_>],
    theme: &Theme,
) {
    // size_md — the previous size_sm was unreadable at a
    // glance; size_md matches the inventory / tooltip body
    // text so the eye doesn't switch typographic scale when
    // moving between surfaces.
    let line_size = theme.fonts.size_md;
    let line_pitch = line_size + 4.0 * theme.scale;
    if line_pitch <= 0.0 {
        return;
    }

    let mut y = body.y() + body.height() - line_pitch;
    'outer: for line in messages.iter().rev() {
        let color = Color::rgba(line.color[0], line.color[1], line.color[2], line.color[3]);
        let rows = wrap_text(ui, line.text, line_size, body.width());
        for row in rows.iter().rev() {
            if y < body.y() {
                break 'outer;
            }
            draw_text_shadow(ui, Pos2::new(body.x(), y), row, line_size, color);
            y -= line_pitch;
        }
    }
}

/// 1 px offset black-alpha shadow + foreground draw. Same
/// recipe used by the vitals widget so chat / vitals text
/// reads identically against any backdrop.
fn draw_text_shadow(ui: &mut Ui<'_>, pos: Pos2, text: &str, size: f32, color: Color) {
    let shadow = Color::rgba(0.0, 0.0, 0.0, 0.75);
    ui.draw_text(Pos2::new(pos.x + 1.0, pos.y + 1.0), text, size, shadow);
    ui.draw_text(pos, text, size, color);
}

/// Break `text` into rows no wider than `max_width`. Prefers
/// whitespace breaks; falls back to per-character splits for
/// un-breakable runs (long URLs, etc.) so the caller never
/// sees an overflowing row.
fn wrap_text(ui: &mut Ui<'_>, text: &str, size: f32, max_width: f32) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    if max_width <= 0.0 {
        rows.push(text.to_string());
        return rows;
    }
    let mut current = String::new();
    for word in text.split_inclusive(char::is_whitespace) {
        let trial = format!("{current}{word}");
        if ui.measure_text(trial.trim_end(), size) <= max_width {
            current = trial;
            continue;
        }
        if !current.is_empty() {
            rows.push(current.trim_end().to_string());
        }
        if ui.measure_text(word.trim_end(), size) <= max_width {
            current = word.to_string();
        } else {
            let mut buf = String::new();
            for ch in word.chars() {
                let trial = format!("{buf}{ch}");
                if ui.measure_text(&trial, size) <= max_width {
                    buf = trial;
                } else {
                    if !buf.is_empty() {
                        rows.push(buf.clone());
                    }
                    buf = ch.to_string();
                }
            }
            current = buf;
        }
    }
    if !current.trim_end().is_empty() {
        rows.push(current.trim_end().to_string());
    }
    if rows.is_empty() {
        rows.push(String::new());
    }
    rows
}
