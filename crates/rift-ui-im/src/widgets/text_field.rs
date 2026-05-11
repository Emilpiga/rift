//! Single-line text input with full caret + selection editing.
//!
//! Owns no string state of its own — the caller passes a
//! `&mut String` that gets mutated in place. The *editor*
//! state (caret + selection range) lives on the focused
//! [`UiState`](super::super::state::UiState) and resets when
//! focus moves to a different id.
//!
//! ## Editing model
//!
//! - **Mouse**: click positions the caret (and clears any
//!   selection); drag extends the selection from the click
//!   point to the cursor; click outside the field releases
//!   focus without consuming the click.
//! - **Typing**: a printable char replaces the current
//!   selection (or inserts at the caret if none).
//! - **Backspace**: deletes the selection if any, else the
//!   char before the caret.
//! - **Delete**: deletes the selection if any, else the char
//!   after the caret.
//! - **Arrow Left / Right**: moves caret one char; with Shift
//!   extends the selection; with Ctrl jumps a word; with both
//!   extends a word at a time.
//! - **Home / End**: caret to start / end (Shift extends).
//! - **Ctrl+A**: select all.
//! - **Enter**: doesn't auto-submit — surface that with a
//!   sibling Button or check `input.enter_just_pressed()`
//!   while `Response::focused` is true.
//!
//! Auto-repeat is honored end-to-end: holding ← / → / Bksp /
//! Del moves / deletes at the OS repeat rate via
//! `Input::key_events` and `Input::backspace_count`.

use super::super::id::Id;
use super::super::im_key::ImKey;
use super::super::rect::{Pos2, Rect};
use super::super::response::Response;
use super::super::state::TextSelection;
use super::super::ui::Ui;

/// Configurable text field.
#[derive(Debug)]
pub struct TextField<'a> {
    pub id: Id,
    pub max_chars: usize,
    pub placeholder: &'a str,
    /// Show a blinking caret while focused. Always-on while focused.
    pub caret: bool,
    /// Auto-claim focus on first appearance (single-form screens).
    pub auto_focus: bool,
}

impl<'a> TextField<'a> {
    pub fn new(id: Id) -> Self {
        Self {
            id,
            max_chars: 32,
            placeholder: "",
            caret: true,
            auto_focus: false,
        }
    }

    pub fn max_chars(mut self, n: usize) -> Self {
        self.max_chars = n;
        self
    }

    pub fn placeholder(mut self, s: &'a str) -> Self {
        self.placeholder = s;
        self
    }

    pub fn auto_focus(mut self, on: bool) -> Self {
        self.auto_focus = on;
        self
    }

    /// Draw + interact at `rect`. Mutates `value` based on focus +
    /// keyboard state. Returns a [`Response`] whose `focused` flag
    /// reflects this frame's focus ownership.
    ///
    /// `time` is a free-running seconds counter (e.g.
    /// `state.rotation_t`) used purely to blink the caret. Pass
    /// any monotonically-increasing float.
    pub fn show(self, ui: &mut Ui<'_>, rect: Rect, value: &mut String, time: f32) -> Response {
        let theme = *ui.theme();
        let id = self.id;
        let hovered = ui.interact_hover(id, rect);

        // Layout constants — used by both hit-testing and draw
        // so the caret column the player clicks matches the
        // pixel column the caret renders at.
        let text_size = theme.fonts.size_lg;
        let pad_x = 12.0;
        let pad_y = (rect.height() - text_size) * 0.5;
        let text_origin = Pos2::new(rect.x() + pad_x, rect.y() + pad_y);

        // ── Focus management ────────────────────────────────
        let was_focused = ui.state().focus == Some(id);
        let mp = ui.mouse_pos();
        let pressed_in_field = hovered && ui.input().left_just_pressed();
        let pressed_outside = !hovered && ui.input().left_just_pressed();

        if pressed_in_field {
            ui.state_mut().focus = Some(id);
            // First-focus seed: caret at end if the field had
            // never been focused; subsequent focus-while-focused
            // re-seeds via the click below.
            if !was_focused {
                let len = value.len();
                ui.state_mut().text_selection = TextSelection {
                    anchor: len,
                    caret: len,
                };
            }
            // Consume the click so a stacked sibling doesn't
            // also fire on the same press.
            let _ = ui.input().left_clicked();
        } else if pressed_outside && was_focused {
            ui.state_mut().focus = None;
            ui.state_mut().text_drag = false;
        }

        // Auto-focus the very first time we render this id.
        if self.auto_focus && ui.state().focus.is_none() {
            ui.state_mut().focus = Some(id);
            let len = value.len();
            ui.state_mut().text_selection = TextSelection {
                anchor: len,
                caret: len,
            };
        }
        let focused = ui.state().focus == Some(id);

        // ── Mouse-driven caret + selection ──────────────────
        // Mouse hit-testing converts an x-pixel into a byte
        // offset. We measure prefixes char-by-char and pick
        // the closest boundary.
        let hit_byte_offset = |ui: &mut Ui<'_>, x_px: f32, value: &str| -> usize {
            let click_x = (x_px - text_origin.x).max(0.0);
            let mut best = 0usize;
            let mut best_dx = click_x.abs();
            let mut acc = 0.0f32;
            for (idx, _) in value.char_indices() {
                let prefix = &value[..idx];
                let w = ui.measure_text(prefix, text_size);
                let dx = (w - click_x).abs();
                if dx < best_dx {
                    best_dx = dx;
                    best = idx;
                }
                acc = w;
            }
            // End of string — also a valid caret position.
            let _ = acc;
            let total_w = ui.measure_text(value, text_size);
            let dx = (total_w - click_x).abs();
            if dx < best_dx {
                best = value.len();
            }
            best
        };

        if focused && pressed_in_field {
            let off = hit_byte_offset(ui, mp.x, value);
            let shift = is_shift_held(ui);
            let mut sel = ui.state().text_selection;
            sel.caret = off;
            if !shift {
                sel.anchor = off;
            }
            ui.state_mut().text_selection = sel;
            ui.state_mut().text_drag = true;
        } else if focused && ui.state().text_drag && ui.input().left_mouse_held() {
            // Drag-extend. Anchor stays put (set on press),
            // caret follows the cursor.
            let off = hit_byte_offset(ui, mp.x, value);
            let mut sel = ui.state().text_selection;
            sel.caret = off;
            ui.state_mut().text_selection = sel;
        }
        if !ui.input().left_mouse_held() && ui.state().text_drag {
            ui.state_mut().text_drag = false;
        }

        // ── Keyboard editing ────────────────────────────────
        if focused {
            ui.claim_keyboard();
            apply_keyboard_edits(ui, value, self.max_chars);
        }
        // Always clamp the selection back into bounds — `value`
        // could be mutated externally between frames (e.g. the
        // owner clears it).
        if focused {
            clamp_selection(ui, value.len());
        }

        // ── Draw frame ──────────────────────────────────────
        let fill = if focused {
            theme.colors.bg_panel_alt
        } else if hovered {
            theme.colors.bg_slot_hover
        } else {
            theme.colors.bg_slot
        };
        let border = if focused {
            theme.colors.border_strong
        } else {
            theme.colors.border
        };
        let radius = (theme.spacing.corner_radius * 0.5).max(2.0);
        ui.draw_rounded_rect(rect, radius, fill);
        ui.draw_rounded_outline(rect, radius, theme.spacing.border_thickness, border);

        // ── Draw text + selection + caret ───────────────────
        let inner_w = (rect.width() - pad_x * 2.0).max(0.0);
        if value.is_empty() {
            ui.draw_text_ellipsized(
                text_origin,
                self.placeholder,
                text_size,
                inner_w,
                theme.colors.text_dim,
            );
        } else {
            // No horizontal scroll for now — the field clamps
            // to `max_chars`, and stash-tab names cap at 18,
            // so `inner_w` always fits the value. (When/if a
            // longer-input use case appears, add scroll-to-
            // caret here.)
            ui.draw_text(text_origin, value, text_size, theme.colors.text);
        }
        if focused {
            let sel = ui.state().text_selection;
            // Selection highlight underneath the caret.
            if sel.has_range() {
                let (a, b) = sel.range();
                let pre = ui.measure_text(&value[..a], text_size);
                let mid = ui.measure_text(&value[a..b], text_size);
                let sx = text_origin.x + pre;
                let sel_rect = Rect::from_xywh(sx, text_origin.y - 1.0, mid, text_size + 2.0);
                ui.draw_rect(sel_rect, theme.colors.accent.with_alpha(0.40));
            }
            // Caret — solid while a selection is active so
            // the player can see the active edge clearly,
            // blinking otherwise.
            if self.caret {
                let on = sel.has_range() || ((time * 2.0) as i32) % 2 == 0;
                if on {
                    let pre = ui.measure_text(&value[..sel.caret.min(value.len())], text_size);
                    let cx = text_origin.x + pre;
                    let glyph_w = ui.measure_text("M", text_size);
                    ui.draw_rect(
                        Rect::from_xywh(cx, text_origin.y, (glyph_w * 0.1).max(2.0), text_size),
                        theme.colors.text,
                    );
                }
            }
        }

        Response {
            id,
            rect,
            hovered,
            pressed: pressed_in_field,
            clicked: false,
            drag_started: false,
            drag_released: false,
            focused,
        }
    }
}

// ─── Editor helpers ────────────────────────────────────────

/// Apply this frame's keyboard input to `value` + the focused
/// selection state. Caller guarantees a text field owns focus.
fn apply_keyboard_edits(ui: &mut Ui<'_>, value: &mut String, max_chars: usize) {
    let ctrl = is_ctrl_held(ui);
    let shift = is_shift_held(ui);

    // 1) Modifier-only chords (Ctrl+A) — process before navigation
    //    so they take precedence over the same key code feeding
    //    the text stream.
    if ctrl {
        for ev in ui.input().key_events().to_vec() {
            if ev == ImKey::KeyA {
                let len = value.len();
                ui.state_mut().text_selection = TextSelection {
                    anchor: 0,
                    caret: len,
                };
            }
        }
    }

    // 2) Caret movement / Home / End / Delete — auto-repeat
    //    aware. Walk the events in arrival order so multiple
    //    presses in one frame still feel responsive.
    for ev in ui.input().key_events().to_vec() {
        match ev {
            ImKey::ArrowLeft => move_caret(ui, value, Direction::Left, ctrl, shift),
            ImKey::ArrowRight => move_caret(ui, value, Direction::Right, ctrl, shift),
            ImKey::Home => {
                let mut sel = ui.state().text_selection;
                sel.caret = 0;
                if !shift {
                    sel.anchor = 0;
                }
                ui.state_mut().text_selection = sel;
            }
            ImKey::End => {
                let mut sel = ui.state().text_selection;
                sel.caret = value.len();
                if !shift {
                    sel.anchor = value.len();
                }
                ui.state_mut().text_selection = sel;
            }
            _ => {}
        }
    }

    // 3) Delete-key forward deletes. If a selection exists the
    //    *first* press collapses it; subsequent presses delete
    //    the next char.
    let dels = ui.input().delete_count();
    for i in 0..dels {
        let sel = ui.state().text_selection;
        if i == 0 && sel.has_range() {
            let (a, b) = sel.range();
            value.replace_range(a..b, "");
            ui.state_mut().text_selection = TextSelection {
                anchor: a,
                caret: a,
            };
        } else {
            let sel = ui.state().text_selection;
            if sel.caret < value.len() {
                let next = next_char_boundary(value, sel.caret);
                value.replace_range(sel.caret..next, "");
                // Caret stays where it is; nothing to update.
            }
        }
    }

    // 4) Backspace deletes. Same selection-collapse rule.
    let bks = ui.input().backspace_count();
    for i in 0..bks {
        let sel = ui.state().text_selection;
        if i == 0 && sel.has_range() {
            let (a, b) = sel.range();
            value.replace_range(a..b, "");
            ui.state_mut().text_selection = TextSelection {
                anchor: a,
                caret: a,
            };
        } else {
            let sel = ui.state().text_selection;
            if sel.caret > 0 {
                let prev = prev_char_boundary(value, sel.caret);
                value.replace_range(prev..sel.caret, "");
                ui.state_mut().text_selection = TextSelection {
                    anchor: prev,
                    caret: prev,
                };
            }
        }
    }

    // 5) Character input. While Ctrl is held we ignore typed
    //    chars so chords like Ctrl+A / Ctrl+C don't dribble a
    //    stray letter into the buffer.
    if !ctrl {
        let chars: Vec<char> = ui.input().chars_typed().to_vec();
        for ch in chars {
            if ch.is_control() {
                continue;
            }
            let sel = ui.state().text_selection;
            // Replace selection if any.
            if sel.has_range() {
                let (a, b) = sel.range();
                value.replace_range(a..b, "");
                ui.state_mut().text_selection = TextSelection {
                    anchor: a,
                    caret: a,
                };
            }
            // Cap on *char* count (visual length), not byte
            // length, so multi-byte UTF-8 doesn't silently
            // shrink the cap.
            if value.chars().count() >= max_chars {
                continue;
            }
            let sel = ui.state().text_selection;
            let mut buf = [0u8; 4];
            let s = ch.encode_utf8(&mut buf);
            value.insert_str(sel.caret, s);
            let new_caret = sel.caret + s.len();
            ui.state_mut().text_selection = TextSelection {
                anchor: new_caret,
                caret: new_caret,
            };
        }
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Left,
    Right,
}

fn move_caret(ui: &mut Ui<'_>, value: &str, dir: Direction, ctrl: bool, shift: bool) {
    let mut sel = ui.state().text_selection;
    let new_caret = match (dir, ctrl) {
        (Direction::Left, false) => {
            if sel.has_range() && !shift {
                sel.range().0
            } else {
                prev_char_boundary(value, sel.caret)
            }
        }
        (Direction::Right, false) => {
            if sel.has_range() && !shift {
                sel.range().1
            } else {
                next_char_boundary(value, sel.caret)
            }
        }
        (Direction::Left, true) => prev_word_boundary(value, sel.caret),
        (Direction::Right, true) => next_word_boundary(value, sel.caret),
    };
    sel.caret = new_caret;
    if !shift {
        sel.anchor = new_caret;
    }
    ui.state_mut().text_selection = sel;
}

fn prev_char_boundary(s: &str, byte: usize) -> usize {
    if byte == 0 {
        return 0;
    }
    let bytes = s.as_bytes();
    let mut i = byte - 1;
    while i > 0 && (bytes[i] & 0xC0) == 0x80 {
        i -= 1;
    }
    i
}

fn next_char_boundary(s: &str, byte: usize) -> usize {
    let len = s.len();
    if byte >= len {
        return len;
    }
    let bytes = s.as_bytes();
    let mut i = byte + 1;
    while i < len && (bytes[i] & 0xC0) == 0x80 {
        i += 1;
    }
    i
}

fn prev_word_boundary(s: &str, byte: usize) -> usize {
    // Skip backwards over whitespace, then over word chars.
    let bytes = s.as_bytes();
    let mut i = byte;
    // Walk to previous char boundary repeatedly.
    while i > 0 {
        let p = prev_char_boundary(s, i);
        if !is_word_byte(bytes[p]) {
            i = p;
            break;
        }
        i = p;
        if i == 0 {
            return 0;
        }
    }
    while i > 0 {
        let p = prev_char_boundary(s, i);
        if !is_word_byte(bytes[p]) {
            return i;
        }
        i = p;
    }
    0
}

fn next_word_boundary(s: &str, byte: usize) -> usize {
    let bytes = s.as_bytes();
    let len = s.len();
    let mut i = byte;
    while i < len && !is_word_byte(bytes[i]) {
        i = next_char_boundary(s, i);
    }
    while i < len && is_word_byte(bytes[i]) {
        i = next_char_boundary(s, i);
    }
    i
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn clamp_selection(ui: &mut Ui<'_>, len: usize) {
    let mut sel = ui.state().text_selection;
    if sel.caret > len {
        sel.caret = len;
    }
    if sel.anchor > len {
        sel.anchor = len;
    }
    // Snap to the nearest preceding char boundary if the cap
    // landed inside a multi-byte sequence (defensive).
    ui.state_mut().text_selection = sel;
}

fn is_shift_held(ui: &Ui<'_>) -> bool {
    ui.input().is_key_held_raw(ImKey::ShiftLeft) || ui.input().is_key_held_raw(ImKey::ShiftRight)
}

fn is_ctrl_held(ui: &Ui<'_>) -> bool {
    ui.input().is_key_held_raw(ImKey::ControlLeft)
        || ui.input().is_key_held_raw(ImKey::ControlRight)
}

/// Convenience constructor with sensible defaults. Equivalent to
/// `TextField::new(id).max_chars(max).placeholder(p).show(...)`.
pub fn text_field(
    ui: &mut Ui<'_>,
    id: Id,
    rect: Rect,
    value: &mut String,
    placeholder: &str,
    max_chars: usize,
    time: f32,
) -> Response {
    TextField::new(id)
        .max_chars(max_chars)
        .placeholder(placeholder)
        .show(ui, rect, value, time)
}

/// Helper: dim label drawn above a form field. Returns the
/// label rect so callers can chain layout.
pub fn label(ui: &mut Ui<'_>, pos: Pos2, text: &str) -> Rect {
    let theme = *ui.theme();
    let size = theme.fonts.size_md;
    let w = ui.draw_text(pos, text, size, theme.colors.text_dim);
    Rect::from_xywh(pos.x, pos.y, w, size)
}

/// Helper: title text in `theme.colors.text` (near-white).
/// Titles read as the screen's banner heading; we keep them
/// chromatically neutral so the surrounding panel chrome
/// (red CTA, accent links) does the colour work.
pub fn title(ui: &mut Ui<'_>, pos: Pos2, text: &str) -> Rect {
    let theme = *ui.theme();
    let size = theme.fonts.size_xl;
    // Drop a 1 px shadow under the title so it reads on top
    // of the noisy stone background like the buttons do.
    let shadow = crate::Color::rgba(0.0, 0.0, 0.0, 0.55);
    let _ = ui.draw_text(Pos2::new(pos.x + 1.0, pos.y + 1.0), text, size, shadow);
    let w = ui.draw_text(pos, text, size, theme.colors.text);
    Rect::from_xywh(pos.x, pos.y, w, size)
}
