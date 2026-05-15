use rift_engine::ui::im::{Button, Color, Frame, Id, Pad, Pos2, Rect, Ui};
use rift_net::messages::ClientMsg;
use rift_ui::icons::{draw_placeholder_icon, icon_rect_left, UiIcon};

use crate::game::chat::ChatUi;
use crate::game::states::sub_state::NetState;

#[derive(Clone, Copy)]
pub struct UnitBarStyle {
    pub base: Color,
    pub hot: Color,
    pub chip: Color,
    pub glow: Color,
}

#[derive(Clone, Copy)]
pub struct UnitFrameBars {
    pub health_displayed: f32,
    pub health_trail: f32,
    pub health_pulse: f32,
    pub resource_displayed: Option<f32>,
    pub resource_trail: f32,
    pub resource_pulse: f32,
}

pub struct UnitFrameData<'a> {
    pub name: &'a str,
    pub detail: Option<&'a str>,
    pub bars: UnitFrameBars,
}

#[derive(Clone, Debug)]
pub struct UnitContextMenuState {
    pub target: String,
    pub pos: Pos2,
    pub can_invite: bool,
    pub can_mute: bool,
    pub can_promote: bool,
    pub can_kick: bool,
}

impl UnitContextMenuState {
    pub fn friendly_target(target: String, pos: Pos2) -> Self {
        Self {
            target,
            pos,
            can_invite: true,
            can_mute: false,
            can_promote: false,
            can_kick: false,
        }
    }

    pub fn party_member(target: String, pos: Pos2, is_leader: bool) -> Self {
        Self {
            target,
            pos,
            can_invite: false,
            can_mute: true,
            can_promote: is_leader,
            can_kick: is_leader,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum UnitContextAction {
    Whisper,
    Invite,
    Mute,
    Promote,
    Kick,
}

pub fn draw_unit_frame(ui: &mut Ui<'_>, rect: Rect, data: UnitFrameData<'_>) {
    let theme = *ui.theme();
    let s = theme.scale;
    Frame::stone(&theme)
        .with_padding(Pad::symmetric(5.0 * s, 5.0 * s))
        .with_radius(2.0 * s)
        .show(ui, rect, |ui, body| {
            let detail_w = data
                .detail
                .map(|detail| ui.measure_text(detail, 11.0 * s))
                .unwrap_or(0.0);
            let text_w = (body.width() - detail_w - 8.0 * s).max(40.0 * s);
            ui.draw_text_ellipsized(
                Pos2::new(body.x(), body.y() + 3.0 * s),
                data.name,
                13.0 * s,
                text_w,
                Color::rgba(0.98, 0.91, 0.82, 0.98),
            );
            if let Some(detail) = data.detail {
                ui.draw_text(
                    Pos2::new(body.max.x - detail_w, body.y() + 4.0 * s),
                    detail,
                    11.0 * s,
                    Color::rgba(0.70, 0.66, 0.60, 0.88),
                );
            }

            let hp_rect = Rect::from_xywh(body.x(), body.y() + 21.0 * s, body.width(), 14.0 * s);
            draw_unit_bar(
                ui,
                hp_rect,
                data.bars.health_displayed,
                data.bars.health_trail,
                data.bars.health_pulse,
                health_bar_style(),
                theme.colors.border_stone,
            );
            if let Some(resource_displayed) = data.bars.resource_displayed {
                let res_rect =
                    Rect::from_xywh(body.x(), body.y() + 38.0 * s, body.width(), 7.0 * s);
                draw_unit_bar(
                    ui,
                    res_rect,
                    resource_displayed,
                    data.bars.resource_trail,
                    data.bars.resource_pulse,
                    resource_bar_style(),
                    theme.colors.border_stone,
                );
            }
        });
}

pub fn draw_unit_context_menu(
    ui: &mut Ui<'_>,
    menu: &UnitContextMenuState,
    id: Id,
    consume_rects: &mut Vec<Rect>,
) -> Option<UnitContextAction> {
    let theme = *ui.theme();
    let s = theme.scale;
    let rows = context_rows(menu);
    let w = 160.0 * s;
    let row_h = 24.0 * s;
    let pad = 4.0 * s;
    let rect = Rect::from_xywh(
        menu.pos.x,
        menu.pos.y,
        w,
        row_h * rows.len() as f32 + pad * 2.0,
    );
    consume_rects.push(rect);

    let mut chosen = None;
    Frame::panel(&theme)
        .with_padding(Pad::all(pad))
        .show(ui, rect, |ui, body| {
            for (i, (label, action, enabled)) in rows.iter().enumerate() {
                let row = Rect::from_xywh(
                    body.x(),
                    body.y() + i as f32 * row_h,
                    body.width(),
                    row_h - 2.0 * s,
                );
                if Button::new(&format!("  {label}"))
                    .enabled(*enabled)
                    .show_with_id(ui, id.child(*label), row)
                    .clicked
                    && *enabled
                {
                    chosen = Some(*action);
                }
                draw_placeholder_icon(
                    ui,
                    icon_rect_left(row, 14.0 * s, 8.0 * s),
                    context_icon(*action),
                    theme.colors.text,
                );
            }
        });

    chosen
}

fn context_icon(action: UnitContextAction) -> UiIcon {
    match action {
        UnitContextAction::Whisper => UiIcon::Whisper,
        UnitContextAction::Invite => UiIcon::Invite,
        UnitContextAction::Mute => UiIcon::Cancel,
        UnitContextAction::Promote => UiIcon::Stats,
        UnitContextAction::Kick => UiIcon::Exit,
    }
}

pub fn unit_context_menu_should_close(ui: &Ui<'_>, menu: &UnitContextMenuState) -> bool {
    let s = ui.scale();
    let rows = context_rows(menu);
    let rect = Rect::from_xywh(
        menu.pos.x,
        menu.pos.y,
        160.0 * s,
        24.0 * s * rows.len() as f32 + 8.0 * s,
    );
    (!rect.contains(ui.mouse_pos())
        && (ui.input().left_just_pressed() || ui.input().right_clicked()))
        || ui
            .input()
            .key_just_pressed(rift_engine::ui::im::ImKey::Escape)
}

pub fn apply_unit_context_action(
    action: UnitContextAction,
    target: &str,
    ui: &mut Ui<'_>,
    net: &mut NetState,
    chat: &mut ChatUi,
) {
    match action {
        UnitContextAction::Whisper => {
            chat.open_with_draft(ui, format!("/w {target} "));
        }
        UnitContextAction::Invite => {
            net.pending_party_msgs.push(ClientMsg::PartyInvite {
                name: target.to_string(),
            });
        }
        UnitContextAction::Mute => {
            chat.toggle_mute(target);
        }
        UnitContextAction::Promote => {
            net.pending_party_msgs.push(ClientMsg::PartyPromote {
                name: target.to_string(),
            });
        }
        UnitContextAction::Kick => {
            net.pending_party_msgs.push(ClientMsg::PartyKick {
                name: target.to_string(),
            });
        }
    }
}

fn context_rows(menu: &UnitContextMenuState) -> Vec<(&'static str, UnitContextAction, bool)> {
    let mut rows = vec![("Whisper", UnitContextAction::Whisper, true)];
    if menu.can_invite {
        rows.push(("Invite", UnitContextAction::Invite, true));
    }
    if menu.can_mute {
        rows.push(("Mute", UnitContextAction::Mute, true));
    }
    if menu.can_promote {
        rows.push(("Promote", UnitContextAction::Promote, true));
    }
    if menu.can_kick {
        rows.push(("Kick", UnitContextAction::Kick, true));
    }
    rows
}

fn draw_unit_bar(
    ui: &mut Ui<'_>,
    rect: Rect,
    displayed: f32,
    trail: f32,
    pulse: f32,
    style: UnitBarStyle,
    border: Color,
) {
    let displayed = displayed.clamp(0.0, 1.0);
    let trail = trail.clamp(displayed, 1.0);
    let pulse = pulse.clamp(0.0, 1.0);
    let flow = ui.state_mut().rift_progress.flow;

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
        let chip = Rect::from_xywh(rect.x() + fill_w, rect.y(), trail_w - fill_w, rect.height());
        ui.draw_grad4_rect(
            chip,
            style.chip,
            style.chip.fade(0.52),
            Color::rgba(0.0, 0.0, 0.0, 0.18),
            style.chip.fade(0.20),
        );
    }

    if fill_w > 0.5 {
        let fill = Rect::from_xywh(rect.x(), rect.y(), fill_w, rect.height());
        let lift = 1.0 + pulse * 0.22;
        ui.draw_grad4_rect(
            fill,
            scale_rgb(style.hot, lift),
            scale_rgb(style.base, 1.04 + pulse * 0.16),
            scale_rgb(style.base, 0.64),
            scale_rgb(style.base, 0.78 + pulse * 0.10),
        );
        ui.draw_gradient_rect(
            fill,
            Color::rgba(1.0, 1.0, 1.0, 0.16 + pulse * 0.08),
            Color::rgba(0.0, 0.0, 0.0, 0.22),
        );
        let sweep_w = (rect.height() * 1.9).clamp(12.0, 28.0).min(fill.width());
        let sweep_x = fill.x() - sweep_w + (fill.width() + sweep_w * 2.0) * flow;
        let sweep_left = sweep_x.max(fill.x());
        let sweep_right = (sweep_x + sweep_w).min(fill.x() + fill.width());
        if sweep_right > sweep_left + 0.5 {
            let sweep = Rect::from_xywh(
                sweep_left,
                fill.y() + 1.0,
                sweep_right - sweep_left,
                fill.height() - 2.0,
            );
            ui.draw_grad4_rect(
                sweep,
                Color::rgba(1.0, 1.0, 1.0, 0.0),
                Color::rgba(1.0, 1.0, 1.0, 0.17 + pulse * 0.12),
                Color::rgba(1.0, 1.0, 1.0, 0.0),
                Color::rgba(1.0, 1.0, 1.0, 0.04 + pulse * 0.04),
            );
        }
        draw_unit_bar_cursor(ui, rect, fill_w, style.glow, pulse);
    }

    ui.draw_outline(rect, 1.0, border);
}

fn draw_unit_bar_cursor(ui: &mut Ui<'_>, rect: Rect, fill_w: f32, glow: Color, pulse: f32) {
    if fill_w <= 1.0 || fill_w >= rect.width() - 0.5 {
        return;
    }
    let x = rect.x() + fill_w;
    let halo_w = (rect.height() * 0.90).clamp(8.0, 18.0);
    ui.draw_grad4_rect(
        Rect::from_xywh(x - halo_w * 0.55, rect.y(), halo_w, rect.height()),
        Color::rgba(glow.0[0], glow.0[1], glow.0[2], 0.0),
        glow.fade(0.34 + pulse * 0.18),
        Color::rgba(glow.0[0], glow.0[1], glow.0[2], 0.0),
        glow.fade(0.12 + pulse * 0.12),
    );
    ui.draw_rect(
        Rect::from_xywh(x - 1.0, rect.y() + 1.0, 2.0, rect.height() - 2.0),
        Color::rgba(1.0, 0.96, 0.82, 0.40 + pulse * 0.26),
    );
}

fn health_bar_style() -> UnitBarStyle {
    UnitBarStyle {
        base: Color::rgba(0.16, 0.62, 0.28, 0.98),
        hot: Color::rgba(0.48, 0.92, 0.36, 1.0),
        chip: Color::rgba(1.0, 0.96, 0.90, 0.26),
        glow: Color::rgba(0.74, 1.0, 0.62, 1.0),
    }
}

fn resource_bar_style() -> UnitBarStyle {
    UnitBarStyle {
        base: Color::rgba(0.24, 0.48, 0.96, 0.97),
        hot: Color::rgba(0.42, 0.78, 1.0, 1.0),
        chip: Color::rgba(0.78, 0.92, 1.0, 0.32),
        glow: Color::rgba(0.34, 0.72, 1.0, 1.0),
    }
}

fn scale_rgb(color: Color, mul: f32) -> Color {
    Color::rgba(
        (color.0[0] * mul).clamp(0.0, 1.0),
        (color.0[1] * mul).clamp(0.0, 1.0),
        (color.0[2] * mul).clamp(0.0, 1.0),
        color.0[3],
    )
}
