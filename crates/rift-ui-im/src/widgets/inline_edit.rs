//! Click-away-to-commit text editing state machine.
//!
//! Wraps the recurring "open an inline `text_field`, commit on
//! Enter, cancel on Esc, **also** commit when the user clicks
//! outside" pattern. The naive version (commit when not
//! focused) breaks because the field isn't focused on its very
//! first frame either, so the rename instantly self-cancels.
//! [`InlineEditState`] tracks a `seen_focus` latch so commit-
//! on-blur fires only AFTER the field has actually been
//! focused at least once.
//!
//! Driver loop:
//!
//! ```ignore
//! if let Some(buf) = state.buffer_mut() {
//!     let resp = text_field(ui, id, rect, buf, ...);
//!     match state.process(
//!         resp.focused,
//!         ui.input().enter_just_pressed(),
//!         ui.input().key_just_pressed_raw(KeyCode::Escape),
//!     ) {
//!         InlineEditOutcome::Editing => {}
//!         InlineEditOutcome::Commit(s) => { /* server.rename(s) */ }
//!         InlineEditOutcome::Cancel => {}
//!     }
//! }
//! ```

/// Persistent state for a single inline-edit session. One
/// instance per editable target (e.g. one per stash-tab
/// rename slot).
#[derive(Default)]
pub struct InlineEditState {
    /// `Some(buffer)` while an edit is in progress.
    buf: Option<String>,
    /// `true` once the field has reported `focused == true`
    /// at least once. Without this latch, the very first
    /// frame of a freshly-opened edit would look like a
    /// blur and commit / cancel before the player could
    /// type anything.
    seen_focus: bool,
}

/// Outcome of one frame's drive of [`InlineEditState`].
pub enum InlineEditOutcome {
    /// The edit is still in progress. Continue rendering the
    /// text field next frame.
    Editing,
    /// Commit the buffer (Enter pressed, or click-away after
    /// at least one focused frame). The contained string is
    /// the trimmed buffer; if the trimmed buffer is empty
    /// the outcome flips to [`InlineEditOutcome::Cancel`]
    /// instead so the caller never has to special-case
    /// "saved an empty name".
    Commit(String),
    /// Discard the buffer (Escape pressed, blurred onto an
    /// empty buffer, or caller-initiated cancel).
    Cancel,
}

impl InlineEditState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Begin an edit with the supplied initial value.
    pub fn begin(&mut self, initial: String) {
        self.buf = Some(initial);
        self.seen_focus = false;
    }

    /// Force-cancel any in-progress edit (e.g. parent panel
    /// closed, target row vanished).
    pub fn cancel(&mut self) {
        self.buf = None;
        self.seen_focus = false;
    }

    /// `true` while an edit is open. Drives e.g.
    /// `Input::set_text_capture` so typed letters don't leak
    /// into world bindings.
    pub fn is_active(&self) -> bool {
        self.buf.is_some()
    }

    /// Mutable access to the live buffer for passing into
    /// `text_field`. Returns `None` when no edit is open.
    pub fn buffer_mut(&mut self) -> Option<&mut String> {
        self.buf.as_mut()
    }

    /// One-frame state machine step. Pass the text field's
    /// `focused` response and the (text-input-aware) Enter /
    /// Escape edge events. Mutates internal state and returns
    /// the resulting outcome.
    pub fn process(
        &mut self,
        text_field_focused: bool,
        enter_pressed: bool,
        escape_pressed: bool,
    ) -> InlineEditOutcome {
        if self.buf.is_none() {
            return InlineEditOutcome::Editing;
        }
        if text_field_focused {
            self.seen_focus = true;
        }
        let blurred = self.seen_focus && !text_field_focused;
        if escape_pressed {
            self.cancel();
            return InlineEditOutcome::Cancel;
        }
        if enter_pressed || blurred {
            // Take the buffer, trim it, decide.
            let raw = self.buf.take().unwrap_or_default();
            self.seen_focus = false;
            let trimmed = raw.trim().to_string();
            if trimmed.is_empty() {
                InlineEditOutcome::Cancel
            } else {
                InlineEditOutcome::Commit(trimmed)
            }
        } else {
            InlineEditOutcome::Editing
        }
    }
}
