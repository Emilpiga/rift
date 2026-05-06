//! Single-line text input.
//!
//! Owns no state of its own — the caller passes a `&mut String`
//! that gets mutated in place. Focus is tracked via [`Id`] in
//! [`UiState`](super::super::state::UiState): clicking the field
//! claims focus; clicking outside any text-field releases it.
//! While focused, the field reads `input.chars_typed()` /
//! `input.backspace_count()` and claims the keyboard so WASD
//! movement etc. can be gated.
//!
//! Enter doesn't auto-submit — surface that with a sibling
//! Button or check `input.enter_just_pressed()` while
//! `Response::focused` is true.

use super::super::id::Id;
use super::super::rect::{Pos2, Rect};
use super::super::response::Response;
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

        // Focus management: click anywhere takes/releases focus.
        // Focusing *consumes* the click so a stacked sibling
        // doesn't also fire on the same press. Blurring uses the
        // non-consuming `left_just_pressed` so the same click
        // still reaches whatever the user actually clicked on
        // (e.g. a Continue button next to the field).
        if hovered && ui.input().left_clicked() {
            ui.state_mut().focus = Some(id);
        } else if !hovered
            && ui.input().left_just_pressed()
            && ui.state().focus == Some(id)
        {
            ui.state_mut().focus = None;
        }

        // Auto-focus the very first time we render this id.
        if self.auto_focus && ui.state().focus.is_none() {
            ui.state_mut().focus = Some(id);
        }

        let focused = ui.state().focus == Some(id);

        // Apply keyboard input while focused.
        if focused {
            ui.claim_keyboard();
            for ch in ui.input().chars_typed() {
                if value.chars().count() < self.max_chars && !ch.is_control() {
                    value.push(*ch);
                }
            }
            for _ in 0..ui.input().backspace_count() {
                value.pop();
            }
        }

        // Draw frame.
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

        // Draw text or placeholder.
        let text_size = theme.fonts.size_lg;
        let pad_x = 12.0;
        let pad_y = (rect.height() - text_size) * 0.5;
        let pos = Pos2::new(rect.x() + pad_x, rect.y() + pad_y);
        if value.is_empty() {
            ui.draw_text(pos, self.placeholder, text_size, theme.colors.text_dim);
        } else {
            ui.draw_text(pos, value, text_size, theme.colors.text);
        }
        // Caret: blink at ~2 Hz when focused.
        if focused && self.caret {
            let on = ((time * 2.0) as i32) % 2 == 0;
            if on {
                let glyph_w = ui.measure_text("M", text_size);
                let cx = rect.x() + pad_x + ui.measure_text(value, text_size);
                ui.draw_rect(
                    Rect::from_xywh(cx + 1.0, pos.y, (glyph_w * 0.1).max(2.0), text_size),
                    theme.colors.text,
                );
            }
        }

        Response {
            id,
            rect,
            hovered,
            pressed: false,
            clicked: false,
            drag_started: false,
            drag_released: false,
            focused,
        }
    }
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

/// Helper: title text in `theme.colors.accent`.
pub fn title(ui: &mut Ui<'_>, pos: Pos2, text: &str) -> Rect {
    let theme = *ui.theme();
    let size = theme.fonts.size_xl;
    let w = ui.draw_text(pos, text, size, theme.colors.accent);
    Rect::from_xywh(pos.x, pos.y, w, size)
}
