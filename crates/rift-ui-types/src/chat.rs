//! Chat HUD view models + action enums.
//!
//! Read by `rift_ui::chat::frame_chat`, written by the host
//! (`rift_client::game::chat::ChatUi::frame`) each frame from
//! its scrollback + open/draft state. The widget never touches
//! `rift_net` — channel constants stay as opaque `u8` ids,
//! and every channel-derived label / colour / pip is
//! pre-computed by the host into the view.

/// One pre-formatted chat line ready for the scrollback panel.
///
/// The host owns the "[g] Alice: hi" → tag + sender + body
/// split so the widget can wrap each row uniformly without
/// needing to know about the wire-channel id semantics.
#[derive(Copy, Clone, Debug)]
pub struct ChatLineView<'a> {
    /// Pre-formatted full row text, e.g.
    /// `"[g] Alice: hello"` — including tag prefix and any
    /// `[to X]` / `[from X]` decoration. The widget treats
    /// this as opaque text for word-wrapping.
    pub text: &'a str,
    /// RGBA tint applied to the whole row.
    pub color: [f32; 4],
}

/// One option in the channel-picker dropdown.
#[derive(Copy, Clone, Debug)]
pub struct ChatChannelOption<'a> {
    /// Opaque wire id. Reported back via
    /// [`ChatAction::SelectChannel`] when the player clicks
    /// the row.
    pub id: u8,
    /// Display label, e.g. `"Global"`, `"Floor"`.
    pub label: &'a str,
    /// Coloured pip drawn at the left of the row + on the
    /// channel button next to the input field.
    pub pip_color: [f32; 4],
}

/// View model for the bottom-left chat panel.
#[derive(Copy, Clone, Debug)]
pub struct ChatView<'a> {
    /// Scrollback buffer, oldest → newest. The widget walks
    /// this newest-first and bottom-aligns.
    pub messages: &'a [ChatLineView<'a>],
    /// `true` while the input field is showing.
    pub open: bool,
    /// Whether the channel-picker dropdown is currently open.
    /// Ignored when `open` is `false`.
    pub picker_open: bool,
    /// Active outbound channel — drives the pip label / colour
    /// and which picker row reads as the current selection.
    pub channel: u8,
    /// Single-letter glyph shown on the pip
    /// (`"G"` / `"H"` / `"F"` / `"P"` / `"W"`).
    pub channel_short: &'a str,
    /// Pip colour for `channel`.
    pub channel_pip_color: [f32; 4],
    /// Channels offered in the picker dropdown.
    pub picker_options: &'a [ChatChannelOption<'a>],
    /// Hard cap on the text-field length (chars). Passed
    /// straight to `TextField::max_chars` so wire-side limits
    /// are enforced client-side too.
    pub max_chars: usize,
}

/// Result of one frame of chat rendering. The widget never
/// mutates host state directly; the host matches on this and
/// applies the change.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ChatAction {
    /// Player clicked the channel pip — toggle the picker.
    TogglePicker,
    /// Player picked a row in the dropdown — switch the
    /// active outbound channel + close the picker.
    SelectChannel(u8),
    /// Player clicked outside both the pip and the picker —
    /// dismiss the dropdown.
    ClosePicker,
}
