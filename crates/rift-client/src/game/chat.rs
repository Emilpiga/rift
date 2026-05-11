//! Client-side chat HUD: scrollback panel + input field +
//! command parsing + per-player mute list.
//!
//! The panel always renders the recent scrollback in the
//! bottom-left corner so system events (joins, deaths, boss
//! kills) are visible during play. Pressing `T` opens an
//! input field anchored just above the scrollback, claims the
//! keyboard so WASD movement is gated, and accepts a single
//! line of chat. `Enter` sends, `Escape` closes without
//! sending, `Tab` cycles the active outbound channel, `R`
//! pre-fills a `/w <last_dm_from> ` reply.
//!
//! Slash commands route to specific channels regardless of the
//! current selection: `/g`, `/h`, `/f`, `/p`, `/w name text`,
//! `/r text` (reply DM), `/mute name`, `/unmute name`. No
//! prefix uses the current channel.
//!
//! The struct lives in [`crate::game::state::GameState`] under
//! the `chat` field. Inbound lines arrive via the binary
//! draining `NetClient::take_pending_chats` into
//! [`ChatUi::push`]; outbound lines are pushed into
//! `state.net.pending_chats_out` for the binary to forward as
//! `ClientMsg::ChatSend`.

use std::collections::{HashSet, VecDeque};

use rift_engine::ui::im::{
    Color, Frame, Id, Pad, Pos2, Rect, Ui,
};
use rift_net::messages::{chat_channel, CHAT_MAX_LEN};

/// One stored chat line on the client side. Retained in
/// [`ChatUi::messages`] for the scrollback panel.
#[derive(Clone, Debug)]
pub struct ChatLine {
    pub channel: u8,
    pub sender: Option<String>,
    pub target: Option<String>,
    pub text: String,
}

/// Client-side chat state. Owns scrollback, the open/closed
/// flag, the draft buffer, the active outbound channel, the
/// per-name mute list, and the last whisper sender (for
/// /r-reply).
pub struct ChatUi {
    /// Scrollback. Bounded — older entries fall off the front.
    messages: VecDeque<ChatLine>,
    /// Visible / typing state. Drives whether the input field
    /// renders and whether key handling claims the keyboard.
    open: bool,
    /// Active outbound channel for non-prefixed sends.
    /// Cycles via Tab while open.
    channel: u8,
    /// Buffer the input field mutates while open.
    draft: String,
    /// Last whisper *sender* — drives `/r` reply behaviour.
    last_whisper_from: Option<String>,
    /// Local per-name mute set. Filters inbound non-system
    /// lines whose sender is in the set. Wiped on game exit
    /// (no persistence yet).
    mutes: HashSet<String>,
    /// `true` once the player has explicitly chosen the
    /// active channel (Tab cycle, dropdown click, or `/g`-style
    /// command). Floor transitions only auto-switch the
    /// channel while this is `false`, so a player who picked
    /// GLOBAL keeps GLOBAL when descending into a rift.
    manual_channel: bool,
    /// Channel-picker dropdown visibility. Toggled by clicking
    /// the channel pip; auto-closes after a selection or when
    /// the chat closes.
    picker_open: bool,
    /// Slash commands the chat parser didn't recognise — e.g.
    /// `/invite`, `/accept`. Drained by the UI phase and
    /// routed to specialised modules (currently the party UI).
    /// `(head_lowercased, body_trimmed)`.
    pub pending_slash: Vec<(String, String)>,
    /// Bottom-left input field rect cached during the most
    /// recent [`Self::frame`] when `open` was true. Queried by
    /// `combat_phase::consumes_mouse` so a click on the chat
    /// input doesn't double-fire as a basic attack.
    cached_input_rect: Option<Rect>,
}

const SCROLLBACK_CAP: usize = 200;

impl Default for ChatUi {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatUi {
    pub fn new() -> Self {
        Self {
            messages: VecDeque::new(),
            open: false,
            channel: chat_channel::GLOBAL,
            draft: String::new(),
            last_whisper_from: None,
            mutes: HashSet::new(),
            manual_channel: false,
            picker_open: false,
            pending_slash: Vec::new(),
            cached_input_rect: None,
        }
    }

    /// Toggle a name on the local mute list. Returns `true`
    /// if the name was newly muted, `false` if it was already
    /// muted (and was therefore unmuted). Used by the party
    /// frame's right-click context menu so the same UI does
    /// double duty for mute / unmute.
    pub fn toggle_mute(&mut self, name: &str) -> bool {
        if self.mutes.remove(name) {
            self.push_local_system(&format!("Unmuted '{name}'."));
            false
        } else {
            self.mutes.insert(name.to_string());
            self.push_local_system(&format!("Muted '{name}'."));
            true
        }
    }

    /// Open the chat input pre-filled with `draft`. Caller is
    /// responsible for adding any trailing space; we leave the
    /// caret at the end of the supplied string. The next-frame
    /// text-input is discarded so the keystroke that triggered
    /// the open (e.g. a context-menu click) doesn't also land
    /// in the field.
    pub fn open_with_draft(&mut self, ui: &mut Ui<'_>, draft: String) {
        self.open = true;
        self.draft = draft;
        ui.input().discard_text_input();
    }

    /// Called by the binary when a `LoadFloor` lands. Auto-
    /// switches the active outbound channel to FLOOR (rift)
    /// or HUB (hub) so casual chat lands in the most useful
    /// scope by default. Skipped once the player has manually
    /// chosen a channel — manual choice persists across
    /// transitions.
    pub fn on_floor_changed(&mut self, is_hub: bool) {
        if self.manual_channel {
            return;
        }
        self.channel = if is_hub {
            chat_channel::HUB
        } else {
            chat_channel::FLOOR
        };
    }

    /// `true` while the input field is focused / typing. The
    /// gameplay phase reads this to gate movement / hotkeys
    /// just like any other modal text-input.
    pub fn is_typing(&self) -> bool {
        self.open
    }

    /// Whether a click at `(mx, my)` lands on the chat input
    /// field (only meaningful while the field is open).
    /// Mirrors `mp_inventory_ui::consumes_mouse` — gates the
    /// gameplay basic-attack click so typing into chat or
    /// dropping the cursor on the input doesn't fire an
    /// ability.
    pub fn consumes_mouse(&self, mx: f32, my: f32) -> bool {
        match self.cached_input_rect {
            Some(r) => r.contains(Pos2::new(mx, my)),
            None => false,
        }
    }

    /// Append an inbound line. System lines (`sender == None`)
    /// always pass; non-system lines from a muted sender are
    /// dropped silently. Whispers from another player update
    /// `last_whisper_from` so `/r` works.
    ///
    /// `our_name` is the local player's character name; it
    /// lets us tell our own whisper *echoes* (which carry the
    /// same `target` as inbound whispers to us) apart from
    /// genuine inbound whispers, so `/r` only ever fills with
    /// names that have actually messaged us.
    pub fn push(&mut self, line: ChatLine, our_name: Option<&str>) {
        // Mute check — system events always show through.
        if let Some(sender) = line.sender.as_deref() {
            if self.mutes.contains(sender) {
                return;
            }
        }
        if line.channel == chat_channel::WHISPER {
            if let Some(sender) = line.sender.as_deref() {
                // Inbound whisper to us: sender is the *other*
                // player. Echo of our own outbound whisper:
                // sender is us. Distinguish by name so /r only
                // remembers the other end.
                let is_our_echo = our_name
                    .map(|n| n.eq_ignore_ascii_case(sender))
                    .unwrap_or(false);
                if !is_our_echo {
                    self.last_whisper_from = Some(sender.to_string());
                }
            }
        }
        if self.messages.len() == SCROLLBACK_CAP {
            self.messages.pop_front();
        }
        self.messages.push_back(line);
    }

    /// Process key bindings each frame. Must be called *before*
    /// [`ChatUi::frame`] so the open/close edge picked up by
    /// `T` / `Esc` lands in the same frame the panel renders.
    /// Returns `true` if the chat consumed an event the
    /// gameplay layer should ignore (e.g. an Enter that
    /// submitted a line).
    pub fn handle_keys(
        &mut self,
        ui: &mut Ui<'_>,
        out: &mut Vec<(u8, Option<String>, String)>,
    ) {
        // Ignore key bindings entirely if some other UI
        // surface is already capturing the keyboard (avoids
        // T-to-open firing while the user is typing in the
        // account field). Focus is the canonical signal —
        // any focused widget owns text input this frame.
        let already_typing_elsewhere = ui.state().focus.is_some()
            && ui.state().focus != Some(Id::root("chat").child("input"));

        if !self.open {
            // Open with T (or Slash) when no other surface
            // owns the keyboard.
            if !already_typing_elsewhere
                && (ui.input().key_just_pressed(rift_engine::ui::im::ImKey::KeyT)
                    || ui.input().key_just_pressed(rift_engine::ui::im::ImKey::Slash))
            {
                self.open = true;
                self.draft.clear();
                // Swallow this frame's typed-char buffer so
                // the very `T` (or `/`) that opened the chat
                // doesn't also land in the freshly-focused
                // input field.
                ui.input().discard_text_input();
            } else if !already_typing_elsewhere
                && ui.input().key_just_pressed(rift_engine::ui::im::ImKey::KeyR)
            {
                if let Some(name) = self.last_whisper_from.clone() {
                    self.open = true;
                    self.draft = format!("/w {name} ");
                    ui.input().discard_text_input();
                }
            }
            return;
        }

        // Open: handle Esc / Enter / Tab. Body characters are
        // consumed by the focused TextField widget itself.
        // Use the *raw* key accessor here so these bindings
        // keep working while text-capture is on (which it is,
        // because the chat is open and this very frame's
        // gameplay polling is suppressed).
        if ui.input().key_just_pressed_raw(rift_engine::ui::im::ImKey::Escape) {
            self.open = false;
            self.draft.clear();
            return;
        }
        if ui.input().key_just_pressed_raw(rift_engine::ui::im::ImKey::Tab) {
            // Cycle through the channels the player can
            // actively post to (skips SYSTEM and WHISPER).
            self.channel = match self.channel {
                chat_channel::GLOBAL => chat_channel::HUB,
                chat_channel::HUB => chat_channel::FLOOR,
                chat_channel::FLOOR => chat_channel::PARTY,
                _ => chat_channel::GLOBAL,
            };
            // Tab is an explicit channel choice — pin it so
            // floor transitions stop auto-switching.
            self.manual_channel = true;
        }
        if ui.input().enter_just_pressed() {
            self.submit(out);
        }
    }

    /// Render scrollback + (when open) the input field. Must
    /// be called after [`ChatUi::handle_keys`].
    pub fn frame(&mut self, ui: &mut Ui<'_>, time: f32) {
        let theme = *ui.theme();
        let s = ui.screen_size();
        let scale = theme.scale;

        // Reset the cached pointer-consume rect; the input
        // path below resets it when the field is open.
        self.cached_input_rect = None;

        // Bottom-left anchor. Keep clear of HUD bars so the
        // chat doesn't fight with health / mana meters.
        let panel_w = (380.0 * scale).min(s.x * 0.40);
        let panel_h = 200.0 * scale;
        let margin = 16.0 * scale;
        let input_h = if self.open { 36.0 * scale } else { 0.0 };
        let input_gap = if self.open { 6.0 * scale } else { 0.0 };

        let scrollback_rect = Rect::from_xywh(
            margin,
            s.y - margin - input_h - input_gap - panel_h,
            panel_w,
            panel_h,
        );

        // Translucent panel — readable but not opaque enough
        // to obscure the world behind it.
        Frame::panel(&theme)
            .with_fill(Color::rgba(0.05, 0.06, 0.09, 0.55))
            .with_padding(Pad::all(8.0 * scale))
            .show(ui, scrollback_rect, |ui, body| {
                draw_scrollback(ui, body, &self.messages, &theme);
            });

        if self.open {
            let input_rect = Rect::from_xywh(
                margin,
                s.y - margin - input_h,
                panel_w,
                input_h,
            );
            self.cached_input_rect = Some(input_rect);
            // Channel pip drawn to the left of the field so
            // the player always sees which scope a no-prefix
            // send will hit. Clickable — toggles the dropdown
            // picker rendered above the input field.
            let prefix = channel_short_name(self.channel);
            let pip_w = ui.measure_text(prefix, theme.fonts.size_md) + 22.0 * scale;
            let pip_rect = Rect::from_xywh(
                input_rect.x(),
                input_rect.y(),
                pip_w,
                input_rect.height(),
            );
            let pip_id = Id::root("chat").child("pip");
            let pip_hovered = ui.interact_hover(pip_id, pip_rect);
            let pip_clicked = pip_hovered && ui.input().left_clicked();
            let mut pip_fill = channel_pip_color(self.channel);
            if pip_hovered {
                // Lift the pip a touch on hover so the click
                // affordance reads.
                pip_fill = lighten(pip_fill, 0.10);
            }
            Frame::panel(&theme).with_fill(pip_fill).show_only(ui, pip_rect);
            ui.draw_text(
                Pos2::new(
                    pip_rect.x() + 8.0 * scale,
                    pip_rect.y()
                        + (pip_rect.height() - theme.fonts.size_md) * 0.5,
                ),
                prefix,
                theme.fonts.size_md,
                Color::rgb(1.0, 1.0, 1.0),
            );
            // Tiny chevron hint that this is interactable.
            ui.draw_text(
                Pos2::new(
                    pip_rect.x() + pip_rect.width() - 12.0 * scale,
                    pip_rect.y()
                        + (pip_rect.height() - theme.fonts.size_sm) * 0.5,
                ),
                if self.picker_open { "\u{25B2}" } else { "\u{25BC}" },
                theme.fonts.size_sm,
                Color::rgba(1.0, 1.0, 1.0, 0.85),
            );

            let field_rect = Rect::from_xywh(
                pip_rect.x() + pip_rect.width() + 4.0 * scale,
                input_rect.y(),
                input_rect.width() - pip_rect.width() - 4.0 * scale,
                input_rect.height(),
            );
            let field_id = Id::root("chat").child("input");
            let _ = rift_engine::ui::im::widgets::TextField::new(field_id)
                .max_chars(CHAT_MAX_LEN)
                .placeholder("Enter to send  ·  Esc to close")
                .auto_focus(true)
                .show(ui, field_rect, &mut self.draft, time);

            // Toggling the picker is handled *after* the field
            // renders so the field's own blur-on-outside-click
            // logic (which sees the same press) leaves focus
            // alone — we then forcibly restore focus to the
            // field here so the player can keep typing.
            if pip_clicked {
                self.picker_open = !self.picker_open;
                ui.state_mut().focus = Some(field_id);
            }

            if self.picker_open {
                self.draw_picker(ui, &theme, pip_rect, field_id);
            }
        } else {
            // Picker only makes sense while the chat is open.
            self.picker_open = false;
        }
    }

    /// Render the channel-picker dropdown above the input
    /// field. One row per selectable channel; clicking a row
    /// pins that channel as the manual choice and closes the
    /// picker.
    fn draw_picker(
        &mut self,
        ui: &mut Ui<'_>,
        theme: &rift_engine::ui::im::Theme,
        pip_rect: Rect,
        field_id: Id,
    ) {
        let scale = theme.scale;
        // Channels we let the player post to. SYSTEM is server-
        // emit-only; WHISPER needs a target and is reached
        // through `/w` instead.
        let options: [(u8, &str, &str); 4] = [
            (chat_channel::GLOBAL, "G", "Global"),
            (chat_channel::HUB, "H", "Hub"),
            (chat_channel::FLOOR, "F", "Floor"),
            (chat_channel::PARTY, "P", "Party"),
        ];

        let row_h = 26.0 * scale;
        let pad = 4.0 * scale;
        let picker_w = 160.0 * scale;
        let picker_h = row_h * options.len() as f32 + pad * 2.0;
        let picker_rect = Rect::from_xywh(
            pip_rect.x(),
            pip_rect.y() - picker_h - 4.0 * scale,
            picker_w,
            picker_h,
        );
        Frame::panel(theme)
            .with_fill(Color::rgba(0.05, 0.06, 0.09, 0.92))
            .show_only(ui, picker_rect);

        let mut any_row_hovered = false;
        for (i, (channel_id, _short, label)) in options.iter().enumerate() {
            let row_rect = Rect::from_xywh(
                picker_rect.x() + pad,
                picker_rect.y() + pad + row_h * i as f32,
                picker_rect.width() - pad * 2.0,
                row_h,
            );
            let row_id = Id::root("chat").child(("picker_row", *channel_id));
            let hovered = ui.interact_hover(row_id, row_rect);
            let clicked = hovered && ui.input().left_clicked();
            if hovered {
                any_row_hovered = true;
            }
            let active = *channel_id == self.channel;
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
            // Coloured pip on the left of each row so the
            // colour-coding in the scrollback maps to the
            // picker.
            let pip = Rect::from_xywh(
                row_rect.x() + 4.0 * scale,
                row_rect.y() + (row_rect.height() - 12.0 * scale) * 0.5,
                12.0 * scale,
                12.0 * scale,
            );
            ui.draw_rounded_rect(pip, 3.0, channel_pip_color(*channel_id));
            ui.draw_text(
                Pos2::new(
                    pip.x() + pip.width() + 8.0 * scale,
                    row_rect.y()
                        + (row_rect.height() - theme.fonts.size_md) * 0.5,
                ),
                label,
                theme.fonts.size_md,
                Color::rgb(0.95, 0.95, 0.95),
            );
            if clicked {
                self.channel = *channel_id;
                self.manual_channel = true;
                self.picker_open = false;
                ui.state_mut().focus = Some(field_id);
            }
        }
        // Click anywhere outside both the picker and the pip
        // dismisses the dropdown so it doesn't linger after the
        // player loses interest.
        if ui.input().left_clicked() && !any_row_hovered {
            // The pip click toggle in `frame` already runs
            // *before* `draw_picker`, so a click on the pip
            // has already flipped `picker_open` for next
            // frame; closing here would only undo that flip.
            // Detect "click was on the pip" by re-testing
            // hover against the pip rect.
            let (mx, my) = ui.input().mouse_pos();
            if !pip_rect.contains(Pos2::new(mx, my)) {
                self.picker_open = false;
            }
        }
    }

    /// Submit the draft buffer. Parses any leading slash
    /// command, otherwise routes to `self.channel`. Closes the
    /// input field unconditionally — empty / rejected sends
    /// still close so the player can re-open with a fresh
    /// keystroke if they meant to cancel.
    fn submit(&mut self, out: &mut Vec<(u8, Option<String>, String)>) {
        let raw = std::mem::take(&mut self.draft);
        self.open = false;
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return;
        }

        // Slash command parsing. Local-only commands (`/mute`,
        // `/unmute`) never go to the wire; channel routes
        // get pushed onto `out` for the binary to send.
        if let Some(rest) = trimmed.strip_prefix('/') {
            let mut parts = rest.splitn(2, char::is_whitespace);
            let head = parts.next().unwrap_or("").to_ascii_lowercase();
            let body = parts.next().unwrap_or("").trim();
            match head.as_str() {
                "g" | "global" if !body.is_empty() => {
                    out.push((chat_channel::GLOBAL, None, body.to_string()));
                }
                "h" | "hub" if !body.is_empty() => {
                    out.push((chat_channel::HUB, None, body.to_string()));
                }
                "f" | "floor" if !body.is_empty() => {
                    out.push((chat_channel::FLOOR, None, body.to_string()));
                }
                "p" | "party" if !body.is_empty() => {
                    out.push((chat_channel::PARTY, None, body.to_string()));
                }
                "w" | "whisper" | "tell" | "msg" => {
                    if let Some((name, msg)) = split_first_word(body) {
                        if !msg.is_empty() {
                            out.push((
                                chat_channel::WHISPER,
                                Some(name.to_string()),
                                msg.to_string(),
                            ));
                        }
                    }
                }
                "r" | "reply" if !body.is_empty() => {
                    if let Some(name) = self.last_whisper_from.clone() {
                        out.push((
                            chat_channel::WHISPER,
                            Some(name),
                            body.to_string(),
                        ));
                    } else {
                        self.push_local_system(
                            "No-one has whispered you yet.",
                        );
                    }
                }
                "mute" if !body.is_empty() => {
                    self.mutes.insert(body.to_string());
                    self.push_local_system(&format!("Muted '{body}'."));
                }
                "unmute" if !body.is_empty() => {
                    if self.mutes.remove(body) {
                        self.push_local_system(&format!("Unmuted '{body}'."));
                    }
                }
                _ => {
                    // Unknown to chat. Defer to upstream (party
                    // UI) before deciding it's truly unknown.
                    // The UI phase drains `pending_slash` and
                    // either routes it or pushes a system line.
                    self.pending_slash.push((head, body.to_string()));
                }
            }
        } else {
            out.push((self.channel, None, trimmed.to_string()));
        }
    }

    /// Drop a local-only system line into the scrollback. Used
    /// for client-side feedback (`/mute`, unknown command,
    /// etc.) without round-tripping the server.
    pub fn push_local_system(&mut self, text: &str) {
        // System line, no sender → `our_name` doesn't matter
        // for the whisper-attribution path. Pass `None`.
        self.push(
            ChatLine {
                channel: chat_channel::SYSTEM,
                sender: None,
                target: None,
                text: text.to_string(),
            },
            None,
        );
    }
}

fn split_first_word(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start();
    let idx = s.find(char::is_whitespace)?;
    Some((&s[..idx], s[idx..].trim_start()))
}

/// Lighten an RGBA colour by mixing in white. Used to flash the
/// channel pip on hover so the click affordance reads.
fn lighten(c: Color, amount: f32) -> Color {
    let t = amount.clamp(0.0, 1.0);
    Color::rgba(
        c.0[0] + (1.0 - c.0[0]) * t,
        c.0[1] + (1.0 - c.0[1]) * t,
        c.0[2] + (1.0 - c.0[2]) * t,
        c.0[3],
    )
}

fn channel_short_name(channel: u8) -> &'static str {
    match channel {
        chat_channel::GLOBAL => "G",
        chat_channel::HUB => "H",
        chat_channel::FLOOR => "F",
        chat_channel::PARTY => "P",
        chat_channel::WHISPER => "W",
        _ => "?",
    }
}

fn channel_color(channel: u8) -> Color {
    match channel {
        chat_channel::SYSTEM => Color::rgb(0.85, 0.85, 0.55),
        chat_channel::GLOBAL => Color::rgb(0.85, 0.90, 1.00),
        chat_channel::HUB => Color::rgb(0.70, 0.90, 0.85),
        chat_channel::FLOOR => Color::rgb(0.95, 0.85, 0.65),
        chat_channel::PARTY => Color::rgb(0.70, 0.85, 1.00),
        chat_channel::WHISPER => Color::rgb(0.95, 0.70, 0.95),
        _ => Color::rgb(0.80, 0.80, 0.80),
    }
}

fn channel_pip_color(channel: u8) -> Color {
    match channel {
        chat_channel::GLOBAL => Color::rgba(0.20, 0.30, 0.55, 0.85),
        chat_channel::HUB => Color::rgba(0.15, 0.40, 0.35, 0.85),
        chat_channel::FLOOR => Color::rgba(0.45, 0.30, 0.15, 0.85),
        chat_channel::PARTY => Color::rgba(0.15, 0.30, 0.55, 0.85),
        _ => Color::rgba(0.25, 0.25, 0.30, 0.85),
    }
}

fn channel_tag(channel: u8) -> &'static str {
    match channel {
        chat_channel::SYSTEM => "[sys]",
        chat_channel::GLOBAL => "[g]",
        chat_channel::HUB => "[hub]",
        chat_channel::FLOOR => "[floor]",
        chat_channel::PARTY => "[party]",
        chat_channel::WHISPER => "[w]",
        _ => "[?]",
    }
}

/// Render the most recent lines that fit inside `body`,
/// bottom-aligned so the newest line is closest to the input
/// field.
fn draw_scrollback(
    ui: &mut Ui<'_>,
    body: Rect,
    messages: &VecDeque<ChatLine>,
    theme: &rift_engine::ui::im::Theme,
) {
    let line_size = theme.fonts.size_sm;
    let line_pitch = line_size + 4.0 * theme.scale;
    if line_pitch <= 0.0 {
        return;
    }

    // Walk messages newest-first, wrap each one to the panel
    // width, and render bottom-up. Stop as soon as we've
    // drawn enough wrapped rows to fill the panel — older
    // lines simply scroll off the top.
    let mut y = body.y() + body.height() - line_pitch;
    'outer: for line in messages.iter().rev() {
        let color = channel_color(line.channel);
        let prefix = match (line.channel, line.sender.as_deref(), line.target.as_deref()) {
            (chat_channel::SYSTEM, _, _) => format!("{} ", channel_tag(line.channel)),
            (chat_channel::WHISPER, Some(s), Some(t)) => {
                format!("{} [to {t}] {s}: ", channel_tag(line.channel))
            }
            (chat_channel::WHISPER, Some(s), None) => {
                format!("{} [from {s}] ", channel_tag(line.channel))
            }
            (_, Some(s), _) => format!("{} {s}: ", channel_tag(line.channel)),
            (_, None, _) => format!("{} ", channel_tag(line.channel)),
        };
        let formatted = format!("{prefix}{}", line.text);

        // Wrap into rows that each fit inside `body.width()`,
        // breaking on whitespace where possible and falling
        // back to mid-word breaks for un-whitespaceable runs
        // (long URLs, languages without spaces) so nothing
        // ever overflows the panel.
        let rows = wrap_text(ui, &formatted, line_size, body.width());

        // Draw the wrapped block bottom-up (the message's own
        // last row sits at the lowest `y`, the first row is
        // highest). This keeps the *latest* message visually
        // glued to the bottom of the panel.
        for row in rows.iter().rev() {
            if y < body.y() {
                break 'outer;
            }
            ui.draw_text(Pos2::new(body.x(), y), row, line_size, color);
            y -= line_pitch;
        }
    }
}

/// Break `text` into rows that each measure no wider than
/// `max_width`. Prefers whitespace breaks; falls back to
/// per-character splits when a single token is wider than
/// `max_width` (long URLs, unbreakable strings) so the caller
/// never sees a row that overflows the requested width.
fn wrap_text(ui: &mut Ui<'_>, text: &str, size: f32, max_width: f32) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    if max_width <= 0.0 {
        rows.push(text.to_string());
        return rows;
    }
    let mut current = String::new();
    for word in text.split_inclusive(char::is_whitespace) {
        // Trial fit: does the current row + this word still
        // sit within `max_width`?
        let trial = format!("{current}{word}");
        if ui.measure_text(trial.trim_end(), size) <= max_width {
            current = trial;
            continue;
        }
        // Doesn't fit. Flush the current row (trimming the
        // trailing whitespace that split_inclusive carried
        // over) and start a fresh row with `word`.
        if !current.is_empty() {
            rows.push(current.trim_end().to_string());
        }
        // The word itself may exceed the width — split it
        // char by char until it fits.
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
