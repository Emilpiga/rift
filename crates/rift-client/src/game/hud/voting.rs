//! Group-decision HUD: revive-shrine progress and the exit-vote
//! panel + cooldown banner. These views surface multi-player
//! state — both render top-center and animate over short
//! windows during which input handling and other HUD overlays
//! intentionally yield space.

use rift_engine::ui::im::{Banner, Color, Pos2, Rect, Ui};

/// Revive-shrine channel progress panel. Shown whenever a
/// shrine on the floor has any active channelers (so even
/// remote players see the progress when their teammate is
/// alone on the shrine). Draws a slim horizontal bar with the
/// "N / M CHANNELING" caption underneath the prompt.
pub fn render_shrine_progress(ui: &mut Ui<'_>, progress: f32, channelers: u8, required: u8) {
    use rift_engine::ui::im::{Frame, Vec2};
    let theme = *ui.theme();
    let screen = ui.screen_size();
    let s = theme.scale;
    let bar_w: f32 = 320.0 * s;
    let bar_h: f32 = 14.0 * s;
    let label = format!("REVIVE SHRINE  -  {} / {} CHANNELING", channelers, required);
    let label_size = 11.0 * s;
    let text_w = ui.measure_text(&label, label_size);
    let inner = Vec2::new(bar_w.max(text_w), bar_h + theme.spacing.gap_sm + label_size);
    let pad = theme.spacing.pad_md;
    let outer_w = inner.x + pad.left + pad.right;
    let outer_h = inner.y + pad.top + pad.bottom;
    let rect = Rect::from_xywh(
        (screen.x - outer_w) / 2.0,
        screen.y * 0.55,
        outer_w,
        outer_h,
    );
    let frame = Frame::panel(&theme)
        .with_fill(Color::rgba(0.05, 0.10, 0.18, 0.92))
        .with_padding(pad);
    frame.show(ui, rect, |ui, body| {
        ui.draw_text(
            Pos2::new(body.x() + (inner.x - text_w) * 0.5, body.y()),
            &label,
            label_size,
            Color::rgba(0.65, 0.88, 1.05, 1.0),
        );
        let bar_y = body.y() + label_size + theme.spacing.gap_sm;
        let bar_x = body.x() + (inner.x - bar_w) * 0.5;
        ui.draw_rect(
            Rect::from_xywh(bar_x, bar_y, bar_w, bar_h),
            Color::rgba(0.05, 0.08, 0.13, 1.0),
        );
        let p = progress.clamp(0.0, 1.0);
        if p > 0.0 {
            ui.draw_rect(
                Rect::from_xywh(bar_x, bar_y, bar_w * p, bar_h),
                Color::rgba(0.45, 0.85, 1.10, 1.0),
            );
        }
        ui.draw_outline(
            Rect::from_xywh(bar_x, bar_y, bar_w, bar_h),
            1.0,
            Color::rgba(0.35, 0.55, 0.85, 1.0),
        );
    });
}

/// Rift exit-vote panel. Top-center HUD card showing the
/// countdown, per-voter Yes/No/Pending status, and the local
/// Y/N hint when our own choice is still Pending. While the vote
/// is inactive but `cooldown_remaining > 0`, a slim cooldown
/// banner is shown instead so the player knows why F at the
/// rift-spawn portal is currently a no-op.
pub fn render_exit_vote(
    ui: &mut Ui<'_>,
    vote: &rift_net::messages::VoteState,
    our_net_id: Option<rift_net::NetId>,
) {
    use rift_engine::ui::im::{Frame, Vec2};

    let theme = *ui.theme();
    let screen = ui.screen_size();
    let s = theme.scale;

    if !vote.active {
        if vote.cooldown_remaining <= 0.0 {
            return;
        }
        let label = format!("VOTE COOLDOWN  {:.0}s", vote.cooldown_remaining.ceil());
        Banner::new(&label)
            .text_size(12.0 * s)
            .text_color(Color::rgba(0.95, 0.55, 0.35, 1.0))
            .fill(Color::rgba(0.10, 0.06, 0.04, 0.88))
            .pad(theme.spacing.pad_sm)
            .y_factor(0.08)
            .show(ui);
        return;
    }

    let title = match vote.kind {
        rift_net::messages::VoteKind::Exit => "LEAVE THE RIFT?",
        rift_net::messages::VoteKind::Descend => "DESCEND TO NEXT FLOOR?",
    };
    let title_size = 16.0 * s;
    let row_size = 12.0 * s;
    let row_h = row_size + theme.spacing.gap_sm;
    let n_rows = vote.voters.len();
    let pad = theme.spacing.pad_lg;
    let body_w: f32 = 280.0 * s;
    let countdown_h = 18.0 * s;
    let footer_h = 16.0 * s;
    let inner_h = title_size
        + theme.spacing.gap_md
        + countdown_h
        + theme.spacing.gap_md
        + (n_rows as f32) * row_h
        + theme.spacing.gap_md
        + footer_h;
    let outer_w = body_w + pad.left + pad.right;
    let outer_h = inner_h + pad.top + pad.bottom;
    let rect = Rect::from_xywh(
        (screen.x - outer_w) / 2.0,
        screen.y * 0.10,
        outer_w,
        outer_h,
    );
    let frame = Frame::panel(&theme)
        .with_fill(Color::rgba(0.04, 0.06, 0.10, 0.94))
        .with_padding(pad);

    let voters = vote.voters.clone();
    let time_remaining = vote.time_remaining.max(0.0);
    let we_pending = our_net_id
        .and_then(|nid| {
            vote.voters
                .iter()
                .find(|(id, _)| *id == nid)
                .map(|(_, c)| *c)
        })
        .map(|c| matches!(c, rift_net::messages::VoteChoice::Pending))
        .unwrap_or(false);

    frame.show(ui, rect, |ui, body| {
        let title_w = ui.measure_header_text(title, title_size);
        ui.draw_header_text(
            Pos2::new(body.x() + (body.width() - title_w) / 2.0, body.y()),
            title,
            title_size,
            Color::rgba(0.95, 0.85, 0.55, 1.0),
        );

        let bar_y = body.y() + title_size + theme.spacing.gap_md;
        let bar_rect = Rect::from_xywh(body.x(), bar_y, body.width(), countdown_h);
        let frac = (time_remaining / 15.0).clamp(0.0, 1.0);
        ui.draw_rect(bar_rect, Color::rgba(0.10, 0.12, 0.16, 1.0));
        let fill_w = bar_rect.width() * frac;
        let fill_rect = Rect::from_xywh(bar_rect.x(), bar_rect.y(), fill_w, bar_rect.height());
        let fill_col = if frac > 0.5 {
            Color::rgba(0.45, 0.85, 0.55, 1.0)
        } else if frac > 0.25 {
            Color::rgba(0.95, 0.80, 0.40, 1.0)
        } else {
            Color::rgba(0.95, 0.40, 0.30, 1.0)
        };
        ui.draw_rect(fill_rect, fill_col);
        let timer_label = format!("{:.0}s", time_remaining.ceil());
        let timer_w = ui.measure_text(&timer_label, row_size);
        ui.draw_text(
            Pos2::new(
                bar_rect.x() + (bar_rect.width() - timer_w) / 2.0,
                bar_rect.y() + (countdown_h - row_size) / 2.0,
            ),
            &timer_label,
            row_size,
            Color::rgba(0.05, 0.07, 0.10, 1.0),
        );

        let mut row_y = bar_y + countdown_h + 8.0 * s;
        for (nid, choice) in voters.iter() {
            let (mark, mark_col) = match choice {
                rift_net::messages::VoteChoice::Yes => ("YES", Color::rgba(0.45, 0.85, 0.55, 1.0)),
                rift_net::messages::VoteChoice::No => ("NO", Color::rgba(0.95, 0.40, 0.30, 1.0)),
                rift_net::messages::VoteChoice::Pending => {
                    ("...", Color::rgba(0.65, 0.65, 0.70, 1.0))
                }
            };
            let is_us = our_net_id == Some(*nid);
            let name = if is_us {
                format!("you (#{})", nid.0)
            } else {
                format!("player #{}", nid.0)
            };
            let name_col = if is_us {
                Color::rgba(0.85, 0.92, 1.0, 1.0)
            } else {
                Color::rgba(0.65, 0.72, 0.82, 1.0)
            };
            ui.draw_text(Pos2::new(body.x(), row_y), &name, row_size, name_col);
            let mark_w = ui.measure_text(mark, row_size);
            ui.draw_text(
                Pos2::new(body.x() + body.width() - mark_w, row_y),
                mark,
                row_size,
                mark_col,
            );
            row_y += row_h;
        }

        let footer_y = body.y() + body.height() - footer_h;
        let hint = if we_pending {
            "PRESS [Y] YES   [N] NO"
        } else {
            "WAITING FOR PARTY..."
        };
        let hint_col = if we_pending {
            Color::rgba(0.95, 0.85, 0.55, 1.0)
        } else {
            Color::rgba(0.55, 0.62, 0.72, 1.0)
        };
        let hint_w = ui.measure_text(hint, row_size);
        ui.draw_text(
            Pos2::new(body.x() + (body.width() - hint_w) / 2.0, footer_y),
            hint,
            row_size,
            hint_col,
        );
        let _ = Vec2::ZERO;
    });
}
