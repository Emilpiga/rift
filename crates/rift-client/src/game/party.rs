//! Client-side party UI state + draw routines.
//!
//! Mirrors authoritative server state pushed via
//! [`rift_net::messages::ServerMsg::PartyState`] and surfaces:
//! - **Party frames** (top-left, fixed 4-slot column with
//!   leader on top). Each frame shows portrait class glyph,
//!   name + level, hp bar, and a small floor pip. Right-click
//!   opens a context menu (whisper / mute / kick / promote).
//! - **Incoming-invite toast + /accept and /decline commands**.
//! - **Portal modal**: opened by the local player when they
//!   walk up to the hub portal and press F. Lets them pick a
//!   start floor (clamped to `[1, deepest_cleared_floor + 1]`)
//!   and one of three modes (Solo, Party, Matchmade). Sends
//!   [`rift_net::messages::ClientMsg::ProposeRiftEntry`].
//! - **Per-member confirm modal**: shown to every other party
//!   member when the proposer fires the portal modal. Sends
//!   [`rift_net::messages::ClientMsg::PortalConfirm`].
//!
//! Ownership: the binary drains `pending_party_state`,
//! `pending_party_invites`, `pending_party_errors`,
//! `pending_portal_prompt`, and `pending_deepest_floor` from
//! [`crate::net::NetClient`] each frame and feeds them into
//! [`PartyUi::ingest_*`]. The UI emits outbound intents back
//! through [`crate::game::states::sub_state::NetState`] vecs
//! the binary forwards to the server.

use std::collections::VecDeque;
use std::time::Instant;

use rift_engine::ui::im::{
    widgets::Button, Color, Frame, Id, Layer, Pad, PanelHeader, Pos2, Rect, Stroke, Theme, Ui,
};
use rift_net::messages::{party_mode, ClientMsg, PartyMember, MAX_PARTY};
use rift_ui::icons::{draw_placeholder_icon, icon_rect_center, icon_rect_left, UiIcon};

use crate::game::chat::ChatUi;
use crate::game::states::frame_state::FrameState;
use crate::game::states::sub_state::NetState;
use crate::game::unit_frame::{
    apply_unit_context_action, draw_unit_context_menu, draw_unit_frame,
    unit_context_menu_should_close, UnitContextMenuState, UnitFrameBars, UnitFrameData,
};
use crate::net::PendingPortalPrompt;

/// How long an error toast remains on screen.
const ERROR_TOAST_TTL_SECS: f32 = 5.0;
/// How long an incoming-invite toast remains visible. Matches
/// the server-side `INVITE_TTL` so a player can /accept right
/// up to the moment the invite expires.
const INVITE_TOAST_TTL_SECS: f32 = 60.0;

/// Portal modal header band (single-line `PanelHeader`).
const PORTAL_MODAL_HEADER_H: f32 = 44.0;

/// Aggregate party UI state. One per `GameState`.
#[derive(Default)]
pub struct PartyUi {
    /// Authoritative party leader, mirrored from the latest
    /// `ServerMsg::PartyState`. `None` ⇔ solo.
    leader: Option<String>,
    /// Authoritative party roster, mirrored ditto. Empty ⇔ solo.
    members: Vec<PartyMember>,
    /// Our own character name, cached so we can identify
    /// "me" inside `members` for self-exclusion in the kick /
    /// promote menus and the leader-only checks.
    our_name: Option<String>,
    /// Pending invite toasts (FIFO, one displayed at a time).
    invite_toasts: VecDeque<InviteToast>,
    /// Pending error toasts (FIFO).
    error_toasts: VecDeque<ErrorToast>,
    /// Latest server-pushed deepest-cleared-floor watermark
    /// for the local player. Drives the portal modal stepper
    /// cap.
    pub(crate) deepest_floor: u32,
    /// `Some` while the player has the local portal modal open
    /// (after pressing F at the hub portal).
    portal_modal: Option<PortalModalState>,
    /// `Some` while the server has us answering a per-member
    /// portal-confirm modal.
    confirm_prompt: Option<ConfirmPromptState>,
    /// Right-click context menu anchored to a party frame.
    context_menu: Option<UnitContextMenuState>,
    /// Rects we drew this frame that should swallow gameplay
    /// mouse input (party frames, modals, context menu).
    /// Filled by [`Self::frame`]; queried by
    /// [`Self::consumes_mouse`] from `combat_phase` *next*
    /// frame to gate the basic-attack click.
    cached_consume_rects: Vec<Rect>,
}

#[derive(Clone, Debug)]
struct InviteToast {
    from: String,
    expires_at: Instant,
}

#[derive(Clone, Debug)]
struct ErrorToast {
    text: String,
    expires_at: Instant,
}

#[derive(Clone, Debug)]
struct PortalModalState {
    start_floor: u32,
    mode: u8,
}

#[derive(Clone, Debug)]
struct ConfirmPromptState {
    proposer: String,
    start_floor: u32,
    mode: u8,
    /// Wall-clock instant the prompt was opened; used to draw
    /// the per-member countdown. Server is authoritative on
    /// the timeout — if the modal closes early we still treat
    /// the user's reply as the source of truth.
    opened_at: Instant,
    seconds_remaining: u32,
}

impl PartyUi {
    pub fn new() -> Self {
        Self::default()
    }

    /// Cache our own character name so we can resolve "is the
    /// local player the leader?" without a network round-trip.
    /// Called by the binary when the Welcome arrives.
    pub fn set_our_name(&mut self, name: String) {
        self.our_name = Some(name);
    }

    /// Apply the latest authoritative party snapshot.
    pub fn ingest_state(&mut self, leader: Option<String>, members: Vec<PartyMember>) {
        self.leader = leader;
        self.members = members;
        // If our context menu's target left the party, drop the
        // menu so we don't act on a stale name.
        if let Some(menu) = &self.context_menu {
            let still_present = self.members.iter().any(|m| m.character_name == menu.target);
            if !still_present {
                self.context_menu = None;
            }
        }
    }

    pub fn ingest_invite(&mut self, from: String) {
        self.invite_toasts.push_back(InviteToast {
            from,
            expires_at: Instant::now() + std::time::Duration::from_secs_f32(INVITE_TOAST_TTL_SECS),
        });
    }

    pub fn ingest_error(&mut self, text: String) {
        self.error_toasts.push_back(ErrorToast {
            text,
            expires_at: Instant::now() + std::time::Duration::from_secs_f32(ERROR_TOAST_TTL_SECS),
        });
    }

    pub fn ingest_portal_prompt(&mut self, prompt: Option<PendingPortalPrompt>) {
        self.confirm_prompt = prompt.map(|p| ConfirmPromptState {
            proposer: p.proposer,
            start_floor: p.start_floor,
            mode: p.mode,
            opened_at: Instant::now(),
            seconds_remaining: p.seconds_remaining,
        });
    }

    pub fn ingest_deepest_floor(&mut self, value: u32) {
        self.deepest_floor = value;
    }

    /// Open the portal modal, defaulting the start floor to
    /// `deepest_floor + 1` (i.e. "the next floor I haven't
    /// cleared"), clamped to `>= 1`. Mode defaults to PARTY
    /// when the player is in a party, else SOLO.
    pub fn open_portal_modal(&mut self) {
        let default_floor = self.deepest_floor.saturating_add(1).max(1);
        let default_mode = if self.members.is_empty() {
            party_mode::SOLO
        } else {
            party_mode::PARTY
        };
        self.portal_modal = Some(PortalModalState {
            start_floor: default_floor,
            mode: default_mode,
        });
    }

    /// Convenience: are we currently the party leader? Used by
    /// chat slash-command parsing to early-reject `/kick` and
    /// `/promote`.
    fn we_are_leader(&self) -> bool {
        match (&self.leader, &self.our_name) {
            (Some(l), Some(o)) => l == o,
            _ => false,
        }
    }

    /// Parse a slash command head + body lifted by
    /// `chat::submit`. Returns `Some(Ok(ClientMsg))` to fire,
    /// `Some(Err(local_msg))` for client-side feedback (e.g.
    /// "/kick: not the leader"), or `None` if the head isn't
    /// a party command.
    pub fn try_handle_slash(&self, head: &str, body: &str) -> Option<Result<ClientMsg, String>> {
        let body = body.trim();
        let cmd: ClientMsg = match head {
            "invite" => {
                if body.is_empty() {
                    return Some(Err("/invite <character_name>".into()));
                }
                ClientMsg::PartyInvite {
                    name: body.to_string(),
                }
            }
            "accept" => ClientMsg::PartyAccept {
                from: if body.is_empty() {
                    None
                } else {
                    Some(body.to_string())
                },
            },
            "decline" => ClientMsg::PartyDecline {
                from: if body.is_empty() {
                    None
                } else {
                    Some(body.to_string())
                },
            },
            "leave" => ClientMsg::PartyLeave,
            "kick" => {
                if body.is_empty() {
                    return Some(Err("/kick <character_name>".into()));
                }
                if !self.we_are_leader() {
                    return Some(Err("Only the party leader can kick.".into()));
                }
                ClientMsg::PartyKick {
                    name: body.to_string(),
                }
            }
            "promote" => {
                if body.is_empty() {
                    return Some(Err("/promote <character_name>".into()));
                }
                if !self.we_are_leader() {
                    return Some(Err("Only the party leader can promote.".into()));
                }
                ClientMsg::PartyPromote {
                    name: body.to_string(),
                }
            }
            _ => return None,
        };
        Some(Ok(cmd))
    }

    /// One-frame UI tick. Renders party frames + any open
    /// modal/menu, drains expired toasts, and pushes any user
    /// intents into `net`.
    pub fn frame(
        &mut self,
        ui: &mut Ui<'_>,
        net: &mut NetState,
        chat: &mut ChatUi,
        frame_state: &mut FrameState,
    ) {
        // Drain expired toasts.
        let now = Instant::now();
        while self
            .invite_toasts
            .front()
            .map_or(false, |t| t.expires_at <= now)
        {
            self.invite_toasts.pop_front();
        }
        while self
            .error_toasts
            .front()
            .map_or(false, |t| t.expires_at <= now)
        {
            self.error_toasts.pop_front();
        }

        // Reset cached consume rects each frame; the draw
        // helpers below push their own rect when active.
        self.cached_consume_rects.clear();

        self.draw_party_frames(ui, frame_state);
        self.draw_invite_toast(ui, net);
        self.draw_error_toast(ui);
        self.draw_portal_modal(ui, net);
        self.draw_confirm_prompt(ui, net);
        self.draw_context_menu(ui, net, chat);
    }

    /// Whether a click at `(mx, my)` should be swallowed by
    /// the party UI rather than reaching the gameplay layer
    /// (basic-attack / cast-confirm). Mirrors the inventory's
    /// `consumes_mouse` contract: queried by `combat_phase`
    /// before it consumes `left_clicked()` / `right_clicked()`.
    /// Reads last-frame's drawn rects, which is what the
    /// player saw when they pressed the button — same 1-frame
    /// lag as inventory.
    pub fn consumes_mouse(&self, mx: f32, my: f32) -> bool {
        let mp = Pos2::new(mx, my);
        self.cached_consume_rects.iter().any(|r| r.contains(mp))
    }

    /// True while the portal proposal modal or the per-member
    /// confirm prompt is on screen — used by callers (chat /
    /// keybind layer) that want to gate Enter / Escape so a
    /// modal action doesn't double-fire.
    pub fn modal_open(&self) -> bool {
        self.portal_modal.is_some() || self.confirm_prompt.is_some()
    }

    // ---- party frames -----------------------------------------------------

    fn draw_party_frames(&mut self, ui: &mut Ui<'_>, frame_state: &mut FrameState) {
        if self.members.is_empty() {
            return;
        }
        let theme = *ui.theme();
        let s = theme.scale;
        let frame_w = 286.0 * s;
        let frame_h = 58.0 * s;
        let gap = 6.0 * s;
        // Top-left, leaving room for any future minimap badge.
        let origin = Pos2::new(12.0 * s, 12.0 * s);

        // Order leader-first, then by current `members` order
        // (server `promote` already reorders for us).
        let leader = self.leader.clone();
        let our_name = self.our_name.clone();
        let we_lead = self.we_are_leader();
        let mut ordered: Vec<PartyMember> = self.members.clone();
        if let Some(lead) = leader.as_ref() {
            ordered.sort_by_key(|m| if &m.character_name == lead { 0 } else { 1 });
        }

        let mut new_menu: Option<UnitContextMenuState> = None;
        let targeting_active = frame_state.entity_targeting.is_some();
        // Non-consuming read: peek at the LMB rising edge so
        // we don't steal the click from buttons drawn later
        // this frame (portal modal, context menu, chat input,
        // …). Consuming via `left_clicked()` here is what was
        // making every party-UI button look unclickable.
        let lmb = ui.input().left_just_pressed();
        for (i, member) in ordered.iter().enumerate().take(MAX_PARTY as usize) {
            let rect = Rect::from_xywh(
                origin.x,
                origin.y + (frame_h + gap) * i as f32,
                frame_w,
                frame_h,
            );
            self.cached_consume_rects.push(rect);
            draw_one_frame_static(ui, rect, member, leader.as_deref());
            if rect.contains(ui.mouse_pos()) {
                if ui.input().right_clicked() && Some(&member.character_name) != our_name.as_ref() {
                    new_menu = Some(UnitContextMenuState::party_member(
                        member.character_name.clone(),
                        ui.mouse_pos(),
                        we_lead,
                    ));
                }
                // Left-click while a friendly-target ability is
                // armed: route the cast through the party
                // frame. The combat tick consumes the intent
                // and resolves the name to a NetId via the
                // net session.
                if targeting_active && lmb {
                    frame_state.party_click_target_name = Some(member.character_name.clone());
                }
            }
        }
        if let Some(menu) = new_menu {
            self.context_menu = Some(menu);
        }
    }

    // ---- toasts -----------------------------------------------------------

    fn draw_invite_toast(&mut self, ui: &mut Ui<'_>, net: &mut NetState) {
        let Some(toast) = self.invite_toasts.front().cloned() else {
            return;
        };
        let theme = *ui.theme();
        let s = theme.scale;
        let screen = ui.screen_size();
        let w = 320.0 * s;
        let h = 92.0 * s;
        let rect = Rect::from_xywh((screen.x - w) * 0.5, screen.y * 0.18, w, h);
        Frame::panel(&theme).show(ui, rect, |ui, body| {
            let pad = 8.0 * s;
            let _ = ui.draw_text(
                Pos2::new(body.x() + pad, body.y() + pad),
                &format!("Party invite from {}", toast.from),
                theme.fonts.size_md,
                theme.colors.text,
            );
            let btn_w = 80.0 * s;
            let btn_h = 26.0 * s;
            let row_y = body.y() + body.height() - pad - btn_h;
            let accept_rect = Rect::from_xywh(
                body.x() + body.width() * 0.5 - btn_w - pad * 0.5,
                row_y,
                btn_w,
                btn_h,
            );
            let decline_rect = Rect::from_xywh(
                body.x() + body.width() * 0.5 + pad * 0.5,
                row_y,
                btn_w,
                btn_h,
            );
            if Button::primary("  Accept").show(ui, accept_rect).clicked {
                net.pending_party_msgs.push(ClientMsg::PartyAccept {
                    from: Some(toast.from.clone()),
                });
                self.invite_toasts.pop_front();
            }
            draw_placeholder_icon(
                ui,
                icon_rect_left(accept_rect, 14.0 * s, 7.0 * s),
                UiIcon::Check,
                theme.colors.text,
            );
            if Button::new("  Decline").show(ui, decline_rect).clicked {
                net.pending_party_msgs.push(ClientMsg::PartyDecline {
                    from: Some(toast.from.clone()),
                });
                self.invite_toasts.pop_front();
            }
            draw_placeholder_icon(
                ui,
                icon_rect_left(decline_rect, 14.0 * s, 7.0 * s),
                UiIcon::Cancel,
                theme.colors.text,
            );
        });
    }

    fn draw_error_toast(&mut self, ui: &mut Ui<'_>) {
        let Some(toast) = self.error_toasts.front().cloned() else {
            return;
        };
        let theme = *ui.theme();
        let s = theme.scale;
        let screen = ui.screen_size();
        let w = 360.0 * s;
        let h = 36.0 * s;
        let rect = Rect::from_xywh((screen.x - w) * 0.5, screen.y * 0.10, w, h);
        Frame::panel(&theme)
            .with_stroke(rift_engine::ui::im::Stroke {
                color: Color::rgba(0.95, 0.45, 0.40, 0.9),
                thickness: 1.5,
            })
            .show(ui, rect, |ui, body| {
                let pad = 8.0 * s;
                let _ = ui.draw_text(
                    Pos2::new(body.x() + pad, body.y() + pad),
                    &toast.text,
                    theme.fonts.size_md,
                    theme.colors.text,
                );
            });
    }

    // ---- portal modal -----------------------------------------------------

    fn draw_portal_modal(&mut self, ui: &mut Ui<'_>, net: &mut NetState) {
        let Some(modal) = self.portal_modal.clone() else {
            return;
        };
        if ui
            .input()
            .key_just_pressed(rift_engine::ui::im::ImKey::Escape)
        {
            self.portal_modal = None;
            return;
        }
        let theme = *ui.theme();
        let s = theme.scale;
        let screen = ui.screen_size();
        let w = 420.0 * s;
        let h = 248.0 * s;
        let rect = Rect::from_xywh((screen.x - w) * 0.5, (screen.y - h) * 0.5, w, h);

        let cap = self.deepest_floor.saturating_add(1).max(1);
        let mut new_modal = modal.clone();
        self.cached_consume_rects.push(rect);

        let mut close = false;
        let mut confirm_entry = false;
        ui.with_layer(Layer::Modal, |ui| {
            ui.draw_rect(
                Rect::from_xywh(0.0, 0.0, screen.x, screen.y),
                Color::rgba(0.0, 0.0, 0.0, 0.58),
            );

            Frame::stone(&theme)
                .with_radius(6.0 * s)
                .with_padding(Pad::all(0.0))
                .with_stroke(Stroke::new(2.0 * s, Color::rgba(0.52, 0.38, 0.92, 0.88)))
                .show(ui, rect, |ui, body| {
                    draw_rift_modal_backdrop(ui, body, &theme);

                    let pad = 18.0 * s;
                    let header_h = PORTAL_MODAL_HEADER_H * s;
                    let header = Rect::from_xywh(body.x(), body.y(), body.width(), header_h);
                    PanelHeader::new("RIFT GATE").show(ui, header);

                    let content_top = header.max.y + 12.0 * s;
                    let footer_h = 36.0 * s;
                    let footer_y = body.max.y - pad - footer_h;
                    let content = Rect::from_xywh(
                        body.x() + pad,
                        content_top,
                        body.width() - pad * 2.0,
                        (footer_y - content_top - 12.0 * s).max(0.0),
                    );

                    let depth_bottom = draw_centered_depth_picker(
                        ui,
                        content,
                        cap,
                        &mut new_modal.start_floor,
                        Id::root("portal_modal").child("depth"),
                    );

                    let mode_h = 32.0 * s;
                    let mode_gap = 8.0 * s;
                    let chip_w = ((content.width() - mode_gap * 2.0) / 3.0).max(0.0);
                    let modes_y = depth_bottom + 16.0 * s;
                    let in_party = !self.members.is_empty();
                    let solo = Rect::from_xywh(content.x(), modes_y, chip_w, mode_h);
                    let party = Rect::from_xywh(solo.max.x + mode_gap, modes_y, chip_w, mode_h);
                    let queue = Rect::from_xywh(party.max.x + mode_gap, modes_y, chip_w, mode_h);
                    if draw_mode_chip(
                        ui,
                        solo,
                        Id::root("portal_modal").child("solo"),
                        "SOLO",
                        new_modal.mode == party_mode::SOLO,
                        true,
                    ) {
                        new_modal.mode = party_mode::SOLO;
                    }
                    if draw_mode_chip(
                        ui,
                        party,
                        Id::root("portal_modal").child("party"),
                        "PARTY",
                        new_modal.mode == party_mode::PARTY,
                        in_party,
                    ) {
                        new_modal.mode = party_mode::PARTY;
                    }
                    if draw_mode_chip(
                        ui,
                        queue,
                        Id::root("portal_modal").child("matchmake"),
                        "QUEUE",
                        new_modal.mode == party_mode::MATCHMAKE,
                        true,
                    ) {
                        new_modal.mode = party_mode::MATCHMAKE;
                    }

                    let footer = Rect::from_xywh(
                        body.x() + pad,
                        footer_y,
                        body.width() - pad * 2.0,
                        footer_h,
                    );
                    let btn_gap = 10.0 * s;
                    let btn_w = (footer.width() - btn_gap) * 0.5;
                    let cancel = Rect::from_xywh(footer.x(), footer.y(), btn_w, footer.height());
                    let confirm =
                        Rect::from_xywh(cancel.max.x + btn_gap, footer.y(), btn_w, footer.height());
                    if Button::new("  Cancel").show(ui, cancel).clicked {
                        close = true;
                    }
                    draw_placeholder_icon(
                        ui,
                        icon_rect_left(cancel, 16.0 * s, 8.0 * s),
                        UiIcon::Cancel,
                        theme.colors.text,
                    );
                    if Button::primary("  Enter Rift").show(ui, confirm).clicked {
                        confirm_entry = true;
                    }
                    draw_placeholder_icon(
                        ui,
                        icon_rect_left(confirm, 16.0 * s, 8.0 * s),
                        UiIcon::Portal,
                        theme.colors.text,
                    );
                });
        });

        if close {
            self.portal_modal = None;
            return;
        }
        if confirm_entry {
            net.pending_propose_rift_entry = Some((new_modal.start_floor, new_modal.mode));
            self.portal_modal = None;
            return;
        }

        // The closure above moves `new_modal` only on the
        // confirm/cancel paths; the trailing assign keeps the
        // happy-path tweaks (stepper / radio clicks) in sync
        // with the next frame's draw.
        if let Some(existing) = self.portal_modal.as_mut() {
            *existing = new_modal;
        }
        let _ = rect;
    }

    // ---- per-member confirm prompt ----------------------------------------

    fn draw_confirm_prompt(&mut self, ui: &mut Ui<'_>, net: &mut NetState) {
        let Some(prompt) = self.confirm_prompt.clone() else {
            return;
        };
        let theme = *ui.theme();
        let s = theme.scale;
        let screen = ui.screen_size();
        let w = 400.0 * s;
        let h = 188.0 * s;
        let rect = Rect::from_xywh((screen.x - w) * 0.5, (screen.y - h) * 0.5, w, h);
        let elapsed = Instant::now()
            .saturating_duration_since(prompt.opened_at)
            .as_secs() as u32;
        let remaining = prompt.seconds_remaining.saturating_sub(elapsed);
        self.cached_consume_rects.push(rect);
        let mode_label = match prompt.mode {
            party_mode::SOLO => "Solo",
            party_mode::PARTY => "Party",
            party_mode::MATCHMAKE => "Matchmade",
            _ => "?",
        };
        let header_sub = format!(
            "{} proposes Floor {} ({})",
            prompt.proposer, prompt.start_floor, mode_label
        );
        let header_right = format!("{remaining}s");
        Frame::stone(&theme)
            .with_radius(6.0 * s)
            .with_padding(Pad::all(0.0))
            .with_stroke(Stroke::new(1.5 * s, Color::rgba(0.52, 0.38, 0.92, 0.75)))
            .show(ui, rect, |ui, body| {
                draw_rift_modal_backdrop(ui, body, &theme);
                let header_h = 44.0 * s;
                let header = Rect::from_xywh(body.x(), body.y(), body.width(), header_h);
                PanelHeader::new("RIFT INVITE")
                    .subtitle(header_sub.as_str())
                    .right_text(header_right.as_str())
                    .show(ui, header);

                let pad = 14.0 * s;
                let inner_top = header.max.y + 10.0 * s;
                let _ = ui.draw_text(
                    Pos2::new(body.x() + pad, inner_top),
                    "Same floor & mode as leader.",
                    theme.fonts.size_sm,
                    theme.colors.text_dim,
                );
                let action_y = body.y() + body.height() - pad - 32.0 * s;
                let aw = 110.0 * s;
                let ah = 32.0 * s;
                let no = Rect::from_xywh(
                    body.x() + body.width() * 0.5 - aw - 8.0 * s,
                    action_y,
                    aw,
                    ah,
                );
                let yes =
                    Rect::from_xywh(body.x() + body.width() * 0.5 + 8.0 * s, action_y, aw, ah);
                if Button::new("  Decline").show(ui, no).clicked {
                    net.pending_portal_confirm = Some(false);
                    self.confirm_prompt = None;
                }
                draw_placeholder_icon(
                    ui,
                    icon_rect_left(no, 16.0 * s, 8.0 * s),
                    UiIcon::Cancel,
                    theme.colors.text,
                );
                if Button::primary("  Accept").show(ui, yes).clicked {
                    net.pending_portal_confirm = Some(true);
                    self.confirm_prompt = None;
                }
                draw_placeholder_icon(
                    ui,
                    icon_rect_left(yes, 16.0 * s, 8.0 * s),
                    UiIcon::Check,
                    theme.colors.text,
                );
            });
    }

    // ---- right-click context menu -----------------------------------------

    fn draw_context_menu(&mut self, ui: &mut Ui<'_>, net: &mut NetState, chat: &mut ChatUi) {
        let Some(menu) = self.context_menu.clone() else {
            return;
        };
        if let Some(action) = draw_unit_context_menu(
            ui,
            &menu,
            Id::root("party::unit_context"),
            &mut self.cached_consume_rects,
        ) {
            apply_unit_context_action(action, &menu.target, ui, net, chat);
            self.context_menu = None;
        } else if unit_context_menu_should_close(ui, &menu) {
            self.context_menu = None;
        }
    }
}

fn draw_rift_modal_backdrop(ui: &mut Ui<'_>, body: Rect, theme: &Theme) {
    let s = ui.scale();
    ui.draw_rounded_radial_rect_noisy(
        body,
        6.0 * s,
        Color::rgba(0.10, 0.06, 0.22, 0.82),
        Color::rgba(0.018, 0.014, 0.032, 0.98),
    );
    ui.draw_grad4_rect(
        body,
        Color::rgba(0.42, 0.22, 0.72, 0.14),
        Color::rgba(0.12, 0.08, 0.22, 0.10),
        Color::rgba(0.0, 0.0, 0.0, 0.18),
        Color::rgba(0.0, 0.0, 0.0, 0.26),
    );
    ui.draw_rounded_outline(body, 6.0 * s, 1.0 * s, Color::rgba(0.62, 0.48, 0.95, 0.28));
    let _ = theme;
}

/// Centered floor picker: `MAX` hint, large number, chevron steppers on each side.
/// Returns the bottom y of the depth block for laying out the mode row beneath.
fn draw_centered_depth_picker(
    ui: &mut Ui<'_>,
    content: Rect,
    cap: u32,
    floor: &mut u32,
    base_id: Id,
) -> f32 {
    let theme = *ui.theme();
    let s = theme.scale;

    let cap_text = format!("MAX {cap}");
    let cap_size = 10.0 * s;
    let cap_w = ui.measure_text(&cap_text, cap_size);
    let cap_y = content.y() + 4.0 * s;
    ui.draw_text(
        Pos2::new(content.x() + (content.width() - cap_w) * 0.5, cap_y),
        &cap_text,
        cap_size,
        theme.colors.text_dim,
    );

    let value = floor.to_string();
    let value_size = 52.0 * s;
    let value_w = ui.measure_text(&value, value_size);
    let chev_w = 40.0 * s;
    let chev_h = 48.0 * s;
    let row_gap = 10.0 * s;
    let row_w = chev_w + row_gap + value_w + row_gap + chev_w;
    let row_x = content.x() + (content.width() - row_w) * 0.5;
    let row_y = cap_y + cap_size + 10.0 * s;

    let can_dec = *floor > 1;
    let can_inc = *floor < cap;
    let chev_col = |active: bool| {
        if active {
            theme.colors.text
        } else {
            Color::rgba(
                theme.colors.text_dim.0[0],
                theme.colors.text_dim.0[1],
                theme.colors.text_dim.0[2],
                theme.colors.text_dim.0[3] * 0.45,
            )
        }
    };

    let left = Rect::from_xywh(row_x, row_y + (value_size - chev_h) * 0.5, chev_w, chev_h);
    let right = Rect::from_xywh(
        row_x + chev_w + row_gap + value_w + row_gap,
        row_y + (value_size - chev_h) * 0.5,
        chev_w,
        chev_h,
    );
    if can_dec && ui.interact_hover(base_id.child("dec"), left) && ui.input().left_just_pressed() {
        *floor = floor.saturating_sub(1);
    }
    if can_inc && ui.interact_hover(base_id.child("inc"), right) && ui.input().left_just_pressed() {
        *floor = (*floor + 1).min(cap);
    }
    let chev_icon = 20.0 * s;
    draw_placeholder_icon(
        ui,
        icon_rect_center(left, chev_icon),
        UiIcon::CaretLeft,
        chev_col(can_dec),
    );
    draw_placeholder_icon(
        ui,
        icon_rect_center(right, chev_icon),
        UiIcon::CaretRight,
        chev_col(can_inc),
    );

    ui.draw_text(
        Pos2::new(row_x + chev_w + row_gap, row_y),
        &value,
        value_size,
        theme.colors.text,
    );

    row_y + value_size
}

fn draw_mode_chip(
    ui: &mut Ui<'_>,
    rect: Rect,
    id: Id,
    label: &str,
    selected: bool,
    enabled: bool,
) -> bool {
    let theme = *ui.theme();
    let s = theme.scale;
    let hovered = enabled && ui.interact_hover(id, rect);
    let clicked = hovered && ui.input().left_just_pressed();
    let fill = if selected {
        Color::rgba(0.22, 0.14, 0.38, 0.92)
    } else if hovered {
        Color::rgba(0.14, 0.11, 0.24, 0.92)
    } else {
        Color::rgba(0.09, 0.07, 0.16, 0.86)
    };
    draw_inset_plate(ui, rect, fill);
    if selected {
        ui.draw_outline(rect, 1.5 * s, theme.colors.border_strong);
    }
    let text_alpha = if enabled { 1.0 } else { 0.42 };
    let label_size = 12.0 * s;
    let label_w = ui.measure_text(label, label_size);
    ui.draw_text(
        Pos2::new(
            rect.x() + (rect.width() - label_w) * 0.5,
            rect.y() + (rect.height() - label_size) * 0.5 - 1.0 * s,
        ),
        label,
        label_size,
        Color::rgba(
            theme.colors.text.0[0],
            theme.colors.text.0[1],
            theme.colors.text.0[2],
            theme.colors.text.0[3] * text_alpha,
        ),
    );
    clicked
}

fn draw_inset_plate(ui: &mut Ui<'_>, rect: Rect, fill: Color) {
    let s = ui.scale();
    ui.draw_gradient_rect(rect, scale_rgb(fill, 1.20), scale_rgb(fill, 0.62));
    ui.draw_outline(rect, 1.0 * s, Color::rgba(0.58, 0.48, 0.82, 0.42));
    ui.draw_outline(
        Rect::from_xywh(
            rect.x() + 2.0 * s,
            rect.y() + 2.0 * s,
            rect.width() - 4.0 * s,
            rect.height() - 4.0 * s,
        ),
        1.0,
        Color::rgba(0.82, 0.78, 1.0, 0.08),
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

/// Free helper so the borrow on `self.members` stays alive for
/// the duration of the loop in `draw_party_frames`. The closure
/// captures only the inputs it needs and never re-borrows
/// `PartyUi`.
fn draw_one_frame_static(ui: &mut Ui<'_>, rect: Rect, member: &PartyMember, leader: Option<&str>) {
    let leader_marker = if leader == Some(member.character_name.as_str()) {
        "* "
    } else {
        ""
    };
    let name = format!("{leader_marker}{}", member.character_name);
    let detail = format!("Lv {} / F{}", member.level, member.floor);
    let pct = if member.hp_max > 0.001 {
        (member.hp / member.hp_max).clamp(0.0, 1.0)
    } else {
        0.0
    };
    draw_unit_frame(
        ui,
        rect,
        UnitFrameData {
            name: &name,
            detail: Some(&detail),
            bars: UnitFrameBars {
                health_displayed: pct,
                health_trail: pct,
                health_pulse: 0.0,
                resource_displayed: Some(member.resource_pct.clamp(0.0, 1.0)),
                resource_trail: member.resource_pct.clamp(0.0, 1.0),
                resource_pulse: 0.0,
            },
        },
    );
}
