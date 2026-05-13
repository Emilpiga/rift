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
    widgets::{Button, ProgressBar},
    Color, Frame, Id, Layer, Pad, Pos2, Rect, Stroke, Ui,
};
use rift_net::messages::{party_mode, ClientMsg, PartyMember, MAX_PARTY};

use crate::game::chat::ChatUi;
use crate::game::states::frame_state::FrameState;
use crate::game::states::sub_state::NetState;
use crate::net::PendingPortalPrompt;

/// How long an error toast remains on screen.
const ERROR_TOAST_TTL_SECS: f32 = 5.0;
/// How long an incoming-invite toast remains visible. Matches
/// the server-side `INVITE_TTL` so a player can /accept right
/// up to the moment the invite expires.
const INVITE_TOAST_TTL_SECS: f32 = 60.0;

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
    context_menu: Option<ContextMenuState>,
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

#[derive(Clone, Debug)]
struct ContextMenuState {
    target: String,
    pos: Pos2,
    /// `true` when the local player is the leader (kick /
    /// promote rows enabled).
    is_leader: bool,
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
        let frame_w = 220.0 * s;
        // Slightly taller than the natural "name + bar"
        // minimum so there's clear vertical breathing room
        // between the player label / level and the HP bar.
        let frame_h = 64.0 * s;
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

        let mut new_menu: Option<ContextMenuState> = None;
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
                    new_menu = Some(ContextMenuState {
                        target: member.character_name.clone(),
                        pos: ui.mouse_pos(),
                        is_leader: we_lead,
                    });
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
            if Button::primary("Accept").show(ui, accept_rect).clicked {
                net.pending_party_msgs.push(ClientMsg::PartyAccept {
                    from: Some(toast.from.clone()),
                });
                self.invite_toasts.pop_front();
            }
            if Button::new("Decline").show(ui, decline_rect).clicked {
                net.pending_party_msgs.push(ClientMsg::PartyDecline {
                    from: Some(toast.from.clone()),
                });
                self.invite_toasts.pop_front();
            }
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
        let w = 520.0 * s;
        let h = 352.0 * s;
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
                .with_stroke(Stroke::new(2.0 * s, Color::rgba(0.78, 0.44, 0.24, 0.92)))
                .show(ui, rect, |ui, body| {
                    draw_rift_modal_backdrop(ui, body);

                    let pad = 18.0 * s;
                    let header_h = 72.0 * s;
                    let header = Rect::from_xywh(body.x(), body.y(), body.width(), header_h);
                    ui.draw_grad4_rect(
                        header,
                        Color::rgba(0.22, 0.050, 0.040, 0.54),
                        Color::rgba(0.08, 0.025, 0.035, 0.26),
                        Color::rgba(0.04, 0.030, 0.030, 0.00),
                        Color::rgba(0.04, 0.030, 0.030, 0.00),
                    );
                    ui.draw_rect(
                        Rect::from_xywh(
                            header.x() + pad,
                            header.max.y - 1.0,
                            header.width() - pad * 2.0,
                            1.0,
                        ),
                        Color::rgba(0.86, 0.55, 0.30, 0.42),
                    );

                    ui.draw_text(
                        Pos2::new(body.x() + pad, body.y() + 13.0 * s),
                        "RIFT GATE",
                        12.0 * s,
                        Color::rgba(0.94, 0.72, 0.46, 0.92),
                    );
                    ui.draw_text(
                        Pos2::new(body.x() + pad, body.y() + 28.0 * s),
                        "Enter the Rift",
                        27.0 * s,
                        Color::rgba(0.98, 0.91, 0.78, 1.0),
                    );
                    ui.draw_text(
                        Pos2::new(body.x() + pad, body.y() + 55.0 * s),
                        "Choose your depth and formation before the gate tears open.",
                        12.0 * s,
                        Color::rgba(0.78, 0.72, 0.66, 0.86),
                    );

                    let sigil_c = Pos2::new(body.max.x - 60.0 * s, body.y() + 37.0 * s);
                    draw_rift_sigil(ui, sigil_c, 34.0 * s);

                    let floor_panel = Rect::from_xywh(
                        body.x() + pad,
                        body.y() + header_h + 18.0 * s,
                        190.0 * s,
                        146.0 * s,
                    );
                    draw_floor_selector(ui, floor_panel, cap, &mut new_modal.start_floor);

                    let modes_x = floor_panel.max.x + 14.0 * s;
                    let mode_y = floor_panel.y();
                    let mode_w = body.max.x - pad - modes_x;
                    ui.draw_text(
                        Pos2::new(modes_x, mode_y - 3.0 * s),
                        "FORMATION",
                        11.0 * s,
                        Color::rgba(0.76, 0.64, 0.48, 0.88),
                    );
                    let card_h = 38.0 * s;
                    let gap = 8.0 * s;
                    let in_party = !self.members.is_empty();
                    let solo = Rect::from_xywh(modes_x, mode_y + 17.0 * s, mode_w, card_h);
                    let party = Rect::from_xywh(modes_x, solo.max.y + gap, mode_w, card_h);
                    let mm = Rect::from_xywh(modes_x, party.max.y + gap, mode_w, card_h);
                    if draw_mode_card(
                        ui,
                        solo,
                        Id::root("portal_modal").child("solo"),
                        "SOLO",
                        "Private rift instance",
                        new_modal.mode == party_mode::SOLO,
                        true,
                    ) {
                        new_modal.mode = party_mode::SOLO;
                    }
                    if draw_mode_card(
                        ui,
                        party,
                        Id::root("portal_modal").child("party"),
                        "PARTY",
                        "Bring your current party",
                        new_modal.mode == party_mode::PARTY,
                        in_party,
                    ) {
                        new_modal.mode = party_mode::PARTY;
                    }
                    if draw_mode_card(
                        ui,
                        mm,
                        Id::root("portal_modal").child("matchmake"),
                        "MATCHMAKE",
                        "Open a public queue",
                        new_modal.mode == party_mode::MATCHMAKE,
                        true,
                    ) {
                        new_modal.mode = party_mode::MATCHMAKE;
                    }

                    let summary = Rect::from_xywh(
                        body.x() + pad,
                        floor_panel.max.y + 14.0 * s,
                        body.width() - pad * 2.0,
                        42.0 * s,
                    );
                    draw_entry_summary(ui, summary, new_modal.start_floor, new_modal.mode, cap);

                    let action_y = body.max.y - pad - 36.0 * s;
                    let cancel = Rect::from_xywh(
                        body.max.x - pad - 238.0 * s,
                        action_y,
                        104.0 * s,
                        36.0 * s,
                    );
                    let confirm = Rect::from_xywh(
                        body.max.x - pad - 124.0 * s,
                        action_y,
                        124.0 * s,
                        36.0 * s,
                    );
                    if Button::new("Cancel").show(ui, cancel).clicked {
                        close = true;
                    }
                    if Button::red("Enter Rift").show(ui, confirm).clicked {
                        confirm_entry = true;
                    }
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
        let w = 360.0 * s;
        let h = 160.0 * s;
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
        Frame::panel(&theme).show(ui, rect, |ui, body| {
            let pad = 12.0 * s;
            let _ = ui.draw_text(
                Pos2::new(body.x() + pad, body.y() + pad),
                &format!(
                    "{} wants to enter Floor {} ({mode_label})",
                    prompt.proposer, prompt.start_floor
                ),
                theme.fonts.size_md,
                theme.colors.text,
            );
            let _ = ui.draw_text(
                Pos2::new(body.x() + pad, body.y() + pad + 22.0 * s),
                &format!("Reply within {remaining}s"),
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
            let yes = Rect::from_xywh(body.x() + body.width() * 0.5 + 8.0 * s, action_y, aw, ah);
            if Button::new("Decline").show(ui, no).clicked {
                net.pending_portal_confirm = Some(false);
                self.confirm_prompt = None;
            }
            if Button::primary("Accept").show(ui, yes).clicked {
                net.pending_portal_confirm = Some(true);
                self.confirm_prompt = None;
            }
        });
    }

    // ---- right-click context menu -----------------------------------------

    fn draw_context_menu(&mut self, ui: &mut Ui<'_>, net: &mut NetState, chat: &mut ChatUi) {
        let Some(menu) = self.context_menu.clone() else {
            return;
        };
        let theme = *ui.theme();
        let s = theme.scale;
        // Dismiss on any outside click. We test before drawing
        // so the menu's own buttons get their click first.
        let mp = ui.mouse_pos();
        let w = 160.0 * s;
        let row_h = 24.0 * s;
        let rows: &[(&str, ContextAction, bool)] = &[
            ("Whisper", ContextAction::Whisper, true),
            ("Mute", ContextAction::Mute, true),
            ("Promote", ContextAction::Promote, menu.is_leader),
            ("Kick", ContextAction::Kick, menu.is_leader),
        ];
        let h = row_h * rows.len() as f32 + 8.0 * s;
        let rect = Rect::from_xywh(menu.pos.x, menu.pos.y, w, h);
        self.cached_consume_rects.push(rect);
        let mut chosen: Option<ContextAction> = None;
        Frame::panel(&theme).show(ui, rect, |ui, body| {
            let pad = 4.0 * s;
            for (i, (label, action, enabled)) in rows.iter().enumerate() {
                let row = Rect::from_xywh(
                    body.x() + pad,
                    body.y() + pad + i as f32 * row_h,
                    body.width() - pad * 2.0,
                    row_h - 2.0 * s,
                );
                let btn = Button::new(label).enabled(*enabled);
                if btn.show(ui, row).clicked && *enabled {
                    chosen = Some(*action);
                }
            }
        });

        if let Some(action) = chosen {
            match action {
                ContextAction::Whisper => {
                    // Open the chat input prefilled with the
                    // /w form so the player just has to type
                    // their message and hit Enter.
                    chat.open_with_draft(ui, format!("/w {} ", menu.target));
                }
                ContextAction::Mute => {
                    chat.toggle_mute(&menu.target);
                }
                ContextAction::Promote => {
                    net.pending_party_msgs.push(ClientMsg::PartyPromote {
                        name: menu.target.clone(),
                    });
                }
                ContextAction::Kick => {
                    net.pending_party_msgs.push(ClientMsg::PartyKick {
                        name: menu.target.clone(),
                    });
                }
            }
            self.context_menu = None;
        } else if !rect.contains(mp)
            && (ui.input().left_just_pressed() || ui.input().right_clicked())
        {
            // Any outside click closes. We use right-click
            // (already consumed by the frame that opened the
            // menu, so a fresh right-click is needed) and a
            // best-effort left-click sentinel.
            self.context_menu = None;
        }
        // Pressing Escape also closes.
        if ui
            .input()
            .key_just_pressed(rift_engine::ui::im::ImKey::Escape)
        {
            self.context_menu = None;
        }
    }
}

fn draw_rift_modal_backdrop(ui: &mut Ui<'_>, body: Rect) {
    let s = ui.scale();
    ui.draw_rounded_radial_rect_noisy(
        body,
        6.0 * s,
        Color::rgba(0.20, 0.045, 0.045, 0.78),
        Color::rgba(0.018, 0.016, 0.018, 0.98),
    );
    ui.draw_grad4_rect(
        body,
        Color::rgba(0.55, 0.18, 0.08, 0.16),
        Color::rgba(0.14, 0.05, 0.10, 0.10),
        Color::rgba(0.0, 0.0, 0.0, 0.18),
        Color::rgba(0.0, 0.0, 0.0, 0.26),
    );
    ui.draw_rounded_outline(body, 6.0 * s, 1.0 * s, Color::rgba(1.0, 0.78, 0.42, 0.20));
}

fn draw_rift_sigil(ui: &mut Ui<'_>, centre: Pos2, radius: f32) {
    let hot = Color::rgba(1.0, 0.42, 0.16, 0.90);
    let ember = Color::rgba(0.95, 0.18, 0.10, 0.42);
    ui.draw_circle(centre, radius, Color::rgba(0.80, 0.12, 0.08, 0.10));
    ui.draw_circle(centre, radius * 0.72, Color::rgba(0.95, 0.24, 0.10, 0.12));
    ui.draw_circle(centre, radius * 0.38, Color::rgba(1.0, 0.60, 0.18, 0.16));
    for i in 0..10 {
        let a = i as f32 * std::f32::consts::TAU / 10.0;
        let inner = radius * if i % 2 == 0 { 0.28 } else { 0.43 };
        let outer = radius * if i % 2 == 0 { 0.95 } else { 0.78 };
        let (sin, cos) = a.sin_cos();
        ui.draw_line(
            Pos2::new(centre.x + cos * inner, centre.y + sin * inner),
            Pos2::new(centre.x + cos * outer, centre.y + sin * outer),
            1.0,
            if i % 2 == 0 { hot } else { ember },
        );
    }
    ui.draw_rounded_outline(
        Rect::from_xywh(
            centre.x - radius * 0.74,
            centre.y - radius * 0.74,
            radius * 1.48,
            radius * 1.48,
        ),
        radius,
        1.0,
        Color::rgba(1.0, 0.68, 0.28, 0.42),
    );
}

fn draw_floor_selector(ui: &mut Ui<'_>, rect: Rect, cap: u32, floor: &mut u32) {
    let theme = *ui.theme();
    let s = theme.scale;
    draw_inset_plate(ui, rect, Color::rgba(0.18, 0.09, 0.065, 0.78));
    ui.draw_text(
        Pos2::new(rect.x() + 12.0 * s, rect.y() + 10.0 * s),
        "START FLOOR",
        11.0 * s,
        Color::rgba(0.76, 0.64, 0.48, 0.90),
    );

    let value = floor.to_string();
    let value_size = 48.0 * s;
    let value_w = ui.measure_text(&value, value_size);
    ui.draw_text(
        Pos2::new(
            rect.x() + (rect.width() - value_w) * 0.5,
            rect.y() + 36.0 * s,
        ),
        &value,
        value_size,
        Color::rgba(1.0, 0.82, 0.48, 1.0),
    );

    let cap_text = format!("DEEPEST UNLOCKED: {cap}");
    let cap_w = ui.measure_text(&cap_text, 10.0 * s);
    ui.draw_text(
        Pos2::new(rect.x() + (rect.width() - cap_w) * 0.5, rect.y() + 91.0 * s),
        &cap_text,
        10.0 * s,
        Color::rgba(0.68, 0.63, 0.58, 0.86),
    );

    let button_y = rect.max.y - 34.0 * s;
    let minus = Rect::from_xywh(rect.x() + 18.0 * s, button_y, 52.0 * s, 26.0 * s);
    let plus = Rect::from_xywh(rect.max.x - 70.0 * s, button_y, 52.0 * s, 26.0 * s);
    if Button::red("-").show(ui, minus).clicked {
        *floor = floor.saturating_sub(1).max(1);
    }
    if Button::red("+").show(ui, plus).clicked {
        *floor = (*floor + 1).min(cap);
    }
}

fn draw_mode_card(
    ui: &mut Ui<'_>,
    rect: Rect,
    id: Id,
    label: &str,
    detail: &str,
    selected: bool,
    enabled: bool,
) -> bool {
    let s = ui.scale();
    let hovered = enabled && ui.interact_hover(id, rect);
    let clicked = hovered && ui.input().left_just_pressed();
    let fill = if selected {
        Color::rgba(0.24, 0.11, 0.06, 0.92)
    } else if hovered {
        Color::rgba(0.14, 0.09, 0.075, 0.92)
    } else {
        Color::rgba(0.075, 0.065, 0.060, 0.86)
    };
    draw_inset_plate(ui, rect, fill);
    if selected {
        ui.draw_grad4_rect(
            rect,
            Color::rgba(1.0, 0.48, 0.16, 0.16),
            Color::rgba(1.0, 0.32, 0.12, 0.08),
            Color::rgba(0.0, 0.0, 0.0, 0.0),
            Color::rgba(0.0, 0.0, 0.0, 0.0),
        );
        ui.draw_outline(rect, 1.5 * s, Color::rgba(1.0, 0.62, 0.24, 0.76));
    }
    let text_alpha = if enabled { 1.0 } else { 0.42 };
    ui.draw_text(
        Pos2::new(rect.x() + 12.0 * s, rect.y() + 6.0 * s),
        label,
        13.0 * s,
        Color::rgba(0.95, 0.84, 0.64, text_alpha),
    );
    ui.draw_text(
        Pos2::new(rect.x() + 12.0 * s, rect.y() + 22.0 * s),
        detail,
        10.5 * s,
        Color::rgba(0.70, 0.66, 0.60, text_alpha * 0.82),
    );
    let pip = Pos2::new(rect.max.x - 17.0 * s, rect.y() + rect.height() * 0.5);
    ui.draw_circle(
        pip,
        5.0 * s,
        if selected {
            Color::rgba(1.0, 0.54, 0.18, 0.90)
        } else {
            Color::rgba(0.24, 0.22, 0.20, if enabled { 0.82 } else { 0.42 })
        },
    );
    clicked
}

fn draw_entry_summary(ui: &mut Ui<'_>, rect: Rect, floor: u32, mode: u8, cap: u32) {
    let s = ui.scale();
    draw_inset_plate(ui, rect, Color::rgba(0.055, 0.050, 0.050, 0.74));
    let label = format!("Floor {floor} / {cap}");
    ui.draw_text(
        Pos2::new(rect.x() + 12.0 * s, rect.y() + 7.0 * s),
        &label,
        12.0 * s,
        Color::rgba(0.92, 0.82, 0.64, 0.96),
    );
    ui.draw_text(
        Pos2::new(rect.x() + 12.0 * s, rect.y() + 23.0 * s),
        mode_label(mode),
        11.0 * s,
        Color::rgba(0.70, 0.66, 0.60, 0.88),
    );
    let gate = Rect::from_xywh(
        rect.max.x - 74.0 * s,
        rect.y() + 9.0 * s,
        52.0 * s,
        24.0 * s,
    );
    ui.draw_gradient_rect(
        gate,
        Color::rgba(0.48, 0.12, 0.06, 0.72),
        Color::rgba(0.13, 0.040, 0.035, 0.88),
    );
    ui.draw_outline(gate, 1.0, Color::rgba(0.95, 0.48, 0.20, 0.48));
}

fn draw_inset_plate(ui: &mut Ui<'_>, rect: Rect, fill: Color) {
    let s = ui.scale();
    ui.draw_gradient_rect(rect, scale_rgb(fill, 1.20), scale_rgb(fill, 0.62));
    ui.draw_outline(rect, 1.0 * s, Color::rgba(0.72, 0.52, 0.30, 0.38));
    ui.draw_outline(
        Rect::from_xywh(
            rect.x() + 2.0 * s,
            rect.y() + 2.0 * s,
            rect.width() - 4.0 * s,
            rect.height() - 4.0 * s,
        ),
        1.0,
        Color::rgba(1.0, 0.92, 0.72, 0.07),
    );
}

fn mode_label(mode: u8) -> &'static str {
    match mode {
        party_mode::SOLO => "Solo expedition",
        party_mode::PARTY => "Party expedition",
        party_mode::MATCHMAKE => "Matchmade expedition",
        _ => "Unknown expedition",
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

#[derive(Clone, Copy, Debug)]
enum ContextAction {
    Whisper,
    Mute,
    Promote,
    Kick,
}

/// Free helper so the borrow on `self.members` stays alive for
/// the duration of the loop in `draw_party_frames`. The closure
/// captures only the inputs it needs and never re-borrows
/// `PartyUi`.
fn draw_one_frame_static(ui: &mut Ui<'_>, rect: Rect, member: &PartyMember, leader: Option<&str>) {
    let theme = *ui.theme();
    let pad = 6.0 * theme.scale;
    Frame::panel(&theme).show(ui, rect, |ui, body| {
        let leader_marker = if leader == Some(member.character_name.as_str()) {
            "* "
        } else {
            ""
        };
        let label = format!(
            "{leader_marker}{} (Lv {})",
            member.character_name, member.level
        );
        // Top text row: full `pad` above so the label
        // doesn't kiss the frame border.
        let _ = ui.draw_text(
            Pos2::new(body.x() + pad, body.y() + pad),
            &label,
            theme.fonts.size_md,
            theme.colors.text,
        );
        let floor_label = format!("F{}", member.floor);
        let fw = ui.measure_text(&floor_label, theme.fonts.size_sm);
        let _ = ui.draw_text(
            Pos2::new(body.x() + body.width() - pad - fw, body.y() + pad),
            &floor_label,
            theme.fonts.size_sm,
            theme.colors.text_dim,
        );
        let bar_h = 14.0 * theme.scale;
        // Pin the HP bar to the bottom edge with the same
        // `pad` margin used at the top; the extra `frame_h`
        // (vs the previous 56*s) becomes the gap between the
        // label row above and the bar.
        let bar = Rect::from_xywh(
            body.x() + pad,
            body.y() + body.height() - pad - bar_h,
            body.width() - pad * 2.0,
            bar_h,
        );
        let pct = if member.hp_max > 0.001 {
            (member.hp / member.hp_max).clamp(0.0, 1.0)
        } else {
            0.0
        };
        ProgressBar::new(pct)
            .fill(rift_engine::ui::im::widgets::hp_color(pct))
            .label(&format!("{:.0}/{:.0}", member.hp, member.hp_max))
            .show(ui, bar);
    });
}
