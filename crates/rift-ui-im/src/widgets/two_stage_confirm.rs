//! Two-stage confirmation button helper.
//!
//! Implements the "first click arms, second click within N
//! seconds commits" pattern shared by destructive actions
//! (Salvage Trash, Delete Character, Abandon Floor, Forfeit
//! Match, ...). Auto-disarms after the window expires so a
//! stale arm can't surprise the player on a later open.
//!
//! Drawing is the caller's responsibility — this module only
//! tracks the timer state. The render side typically wants to
//! flip its label and fill colour while armed, which is too
//! site-specific to standardise.
//!
//! Driver loop:
//!
//! ```ignore
//! state.tick(now);                 // auto-disarm check
//! if confirm_btn.armed() { /* draw red label */ }
//! if user_clicked {
//!     match state.click(now) {
//!         TwoStageOutcome::Armed => { /* show "Confirm?" */ }
//!         TwoStageOutcome::Confirmed => { /* run action */ }
//!     }
//! }
//! ```

/// Persistent state for one two-stage button. One per call
/// site. `Default` uses a 3-second confirmation window — the
/// same value [`TwoStageConfirm::new`] picks — so dropping a
/// `TwoStageConfirm` field into a `#[derive(Default)]` parent
/// struct Just Works without the parent having to write a
/// custom `new()` purely to pick a window length.
#[derive(Clone, Copy, Debug)]
pub struct TwoStageConfirm {
    /// `Some(armed_at_seconds)` while waiting for the
    /// confirming second click. Disarmed when `None`.
    armed_at: Option<f64>,
    /// How long an arm stays valid. Stored on the struct so a
    /// caller doesn't have to thread the constant through
    /// every `click` / `tick` call. Defaults to 3 s.
    window_s: f64,
}

impl Default for TwoStageConfirm {
    fn default() -> Self {
        Self::new()
    }
}

/// Outcome of one [`TwoStageConfirm::click`].
#[derive(Clone, Copy, Debug)]
pub enum TwoStageOutcome {
    /// First click — button is now armed and waiting for the
    /// confirming second click.
    Armed,
    /// Second click landed inside the window — caller should
    /// run the destructive action. State is auto-disarmed.
    Confirmed,
}

impl TwoStageConfirm {
    /// Build with the default 3-second confirmation window.
    pub fn new() -> Self {
        Self {
            armed_at: None,
            window_s: 3.0,
        }
    }

    /// Build with a custom confirmation window. Pass a value
    /// in seconds.
    pub fn with_window(window_s: f64) -> Self {
        Self {
            armed_at: None,
            window_s,
        }
    }

    /// `true` while the button is in its primed state.
    /// Callers use this to flip the label / colour.
    pub fn armed(&self) -> bool {
        self.armed_at.is_some()
    }

    /// Drop a stale arm if the window expired since the last
    /// arm. Call this once per frame BEFORE `armed()` reads or
    /// the click handler — cheap (two `f64` ops) and prevents
    /// the player coming back five minutes later and
    /// accidentally confirming.
    pub fn tick(&mut self, now_s: f64) {
        if let Some(t) = self.armed_at {
            if now_s - t > self.window_s {
                self.armed_at = None;
            }
        }
    }

    /// Process a click. Returns whether the click armed the
    /// button or fired the action. Pass the same monotonic
    /// clock you use for [`Self::tick`].
    pub fn click(&mut self, now_s: f64) -> TwoStageOutcome {
        match self.armed_at {
            Some(t) if now_s - t <= self.window_s => {
                self.armed_at = None;
                TwoStageOutcome::Confirmed
            }
            _ => {
                self.armed_at = Some(now_s);
                TwoStageOutcome::Armed
            }
        }
    }

    /// Force-disarm (e.g. parent panel closed).
    pub fn reset(&mut self) {
        self.armed_at = None;
    }
}
