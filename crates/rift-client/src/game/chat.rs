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

use rift_engine::ui::im::{Color, Id, Pos2, Rect, TextSelection, Ui};
use rift_net::messages::{chat_channel, CHAT_MAX_LEN};

/// Stable id used both by `frame_chat` (to render the field)
/// and the host (to detect / claim / blur focus). Single
/// source of truth so any change here is reflected on both
/// sides without drifting.
fn chat_field_id() -> Id {
    Id::root("chat").child("input")
}

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
        let len = draft.len();
        self.draft = draft;
        // Force-focus the field so the player can type
        // straight away. Seed the caret at end-of-string so
        // the next keystroke appends rather than overwrites.
        let id = chat_field_id();
        ui.state_mut().focus = Some(id);
        ui.state_mut().text_selection = TextSelection {
            anchor: len,
            caret: len,
        };
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
    ///
    /// The single source of truth for "chat is active" is the
    /// TextField's focus state: `self.open` is kept in sync
    /// with `ui.state().focus == Some(chat_field_id())` at the
    /// start of this method and again at the end of
    /// [`Self::frame`]. This means clicking the field directly
    /// also "opens" the chat (gates gameplay input next frame),
    /// and blurring the field (Esc, Enter-submit, click-out)
    /// closes it without any extra bookkeeping.
    pub fn handle_keys(&mut self, ui: &mut Ui<'_>, out: &mut Vec<(u8, Option<String>, String)>) {
        let field_id = chat_field_id();
        let field_focused = ui.state().focus == Some(field_id);
        let elsewhere_focused = ui.state().focus.is_some() && !field_focused;

        // Sync the "open" mirror to the canonical focus state
        // at the top of the frame so any external focus change
        // (e.g. user clicked the field with the mouse last
        // frame) is reflected before we react to keys.
        self.open = field_focused;

        if !field_focused {
            if elsewhere_focused {
                return;
            }
            // Closed: T / `/` open the chat for new input; R
            // pre-fills a `/w <last_dm_from> ` reply.
            if ui
                .input()
                .key_just_pressed(rift_engine::ui::im::ImKey::KeyT)
                || ui
                    .input()
                    .key_just_pressed(rift_engine::ui::im::ImKey::Slash)
            {
                self.focus_field(ui, String::new());
            } else if ui
                .input()
                .key_just_pressed(rift_engine::ui::im::ImKey::KeyR)
            {
                if let Some(name) = self.last_whisper_from.clone() {
                    self.focus_field(ui, format!("/w {name} "));
                }
            }
            return;
        }

        // Focused: handle Esc / Enter / Tab. Body characters
        // are consumed by the focused TextField widget. Raw
        // accessors stay valid while text-capture is on.
        if ui
            .input()
            .key_just_pressed_raw(rift_engine::ui::im::ImKey::Escape)
        {
            self.blur_field(ui);
            self.draft.clear();
            return;
        }
        if ui
            .input()
            .key_just_pressed_raw(rift_engine::ui::im::ImKey::Tab)
        {
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
            self.blur_field(ui);
        }
    }

    /// Move keyboard focus onto the chat field and seed the
    /// caret at the end of the (possibly pre-filled) draft.
    /// Discards this frame's typed-char buffer so the keystroke
    /// that triggered the open doesn't bleed into the field.
    fn focus_field(&mut self, ui: &mut Ui<'_>, draft: String) {
        let len = draft.len();
        self.draft = draft;
        self.open = true;
        let id = chat_field_id();
        ui.state_mut().focus = Some(id);
        ui.state_mut().text_selection = TextSelection {
            anchor: len,
            caret: len,
        };
        ui.input().discard_text_input();
    }

    /// Release keyboard focus from the chat field. Used after
    /// Enter-submit and Esc-cancel so gameplay regains WASD
    /// without the player having to click elsewhere first.
    fn blur_field(&mut self, ui: &mut Ui<'_>) {
        self.open = false;
        self.picker_open = false;
        let id = chat_field_id();
        if ui.state().focus == Some(id) {
            ui.state_mut().focus = None;
        }
    }

    /// Render scrollback + (when open) the input field, by
    /// flattening into a [`ChatView`] and delegating to the
    /// pure widget in [`rift_ui::chat::frame_chat`]. Picker /
    /// channel-switch events come back as [`ChatAction`]s and
    /// are applied in-place.
    pub fn frame(&mut self, ui: &mut Ui<'_>, time: f32) {
        use rift_ui_types::chat::{ChatAction, ChatChannelOption, ChatLineView, ChatView};

        let theme = *ui.theme();
        let scale = theme.scale;
        let screen = ui.screen_size();

        // Cache the input-field rect for `consumes_mouse`.
        // Mirrors the geometry inside `frame_chat` exactly so
        // a click on the field still gates the basic-attack.
        // The input row is always rendered now, so the cache
        // is always populated.
        let panel_w = (460.0 * scale).min(screen.x * 0.45);
        let margin = 16.0 * scale;
        let input_h = 42.0 * scale;
        self.cached_input_rect = Some(Rect::from_xywh(
            margin,
            screen.y - margin - input_h,
            panel_w,
            input_h,
        ));

        // Build per-line views with the prefix the widget
        // doesn't compose itself (channel-tag + sender +
        // optional `[to X]` / `[from X]` decoration).
        let mut formatted: Vec<String> = Vec::with_capacity(self.messages.len());
        let mut colors: Vec<[f32; 4]> = Vec::with_capacity(self.messages.len());
        for line in self.messages.iter() {
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
            formatted.push(format!("{prefix}{}", line.text));
            colors.push(channel_color(line.channel).0);
        }
        let messages_view: Vec<ChatLineView<'_>> = formatted
            .iter()
            .zip(colors.iter())
            .map(|(text, color)| ChatLineView {
                text: text.as_str(),
                color: *color,
            })
            .collect();

        // Picker options — must match the channels that the
        // Tab cycle visits so the dropdown and Tab agree on
        // selectable scopes.
        let picker_labels: [(u8, &str); 4] = [
            (chat_channel::GLOBAL, "Global"),
            (chat_channel::HUB, "Hub"),
            (chat_channel::FLOOR, "Floor"),
            (chat_channel::PARTY, "Party"),
        ];
        let picker_options: Vec<ChatChannelOption<'_>> = picker_labels
            .iter()
            .map(|(id, label)| ChatChannelOption {
                id: *id,
                label,
                pip_color: channel_pip_color(*id).0,
            })
            .collect();

        let channel_short = channel_short_name(self.channel);
        let channel_pip = channel_pip_color(self.channel).0;
        let view = ChatView {
            messages: &messages_view,
            open: self.open,
            picker_open: self.picker_open,
            channel: self.channel,
            channel_short,
            channel_pip_color: channel_pip,
            picker_options: &picker_options,
            max_chars: CHAT_MAX_LEN,
        };

        let actions = rift_ui::chat::frame_chat(ui, &view, &mut self.draft, time);
        for a in actions {
            match a {
                ChatAction::TogglePicker => self.picker_open = !self.picker_open,
                ChatAction::SelectChannel(c) => {
                    self.channel = c;
                    self.manual_channel = true;
                    self.picker_open = false;
                }
                ChatAction::ClosePicker => self.picker_open = false,
            }
        }

        // Re-sync the "open" mirror at the end of the frame:
        // the field's focus may have flipped during this frame
        // (user clicked the field, clicked outside, etc.).
        // Reading focus here keeps `is_typing()` accurate next
        // tick so gameplay's text-capture flag tracks reality.
        let field_id = chat_field_id();
        self.open = ui.state().focus == Some(field_id);
        if !self.open {
            self.picker_open = false;
        }
    }

    /// Submit the draft buffer. Parses any leading slash
    /// command, otherwise routes to `self.channel`. Closes the
    /// input field unconditionally — empty / rejected sends
    /// still close so the player can re-open with a fresh
    /// keystroke if they meant to cancel.
    fn submit(&mut self, out: &mut Vec<(u8, Option<String>, String)>) {
        let raw = std::mem::take(&mut self.draft);
        // Closing is handled by the caller (`blur_field`) so
        // both Enter-submit and Esc-cancel share the same
        // teardown path.
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
                        out.push((chat_channel::WHISPER, Some(name), body.to_string()));
                    } else {
                        self.push_local_system("No-one has whispered you yet.");
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
