use std::cell::Cell;
use std::collections::HashSet;
use winit::keyboard::KeyCode;

/// Tracks input state (keys held, mouse state, camera orbit).
pub struct Input {
    keys_held: HashSet<KeyCode>,
    prev_keys_held: HashSet<KeyCode>,
    // Camera orbit
    camera_yaw: f32,
    camera_pitch: f32,
    camera_distance: f32,
    // Mouse
    right_mouse_down: bool,
    left_clicked: Cell<bool>,
    right_clicked: Cell<bool>,
    last_mouse_pos: Option<(f64, f64)>,
    /// Whether the left mouse button is currently held down. Tracks
    /// hold-state for channeled ability inputs (the action button).
    left_mouse_down: bool,
    /// `left_mouse_down` from the previous frame. Compared at
    /// frame end to surface `left_just_released`. Not used by
    /// the click-vs-hold path — that one consumes
    /// `left_clicked` directly.
    prev_left_mouse_down: bool,
    /// True for one frame after the left mouse button is
    /// released. Used by drag-and-drop to detect drop events
    /// without consuming the press, the way `left_clicked`
    /// would.
    left_just_released: bool,
    mouse_pos: (f32, f32),
    /// Characters typed this frame (consumed by `take_chars_typed`).
    chars_typed: Vec<char>,
    /// Backspace pressed this frame (key auto-repeat respected).
    backspace_pressed: u32,
    /// Delete pressed this frame (key auto-repeat respected).
    /// Mirrors `backspace_pressed` so text-edit widgets get a
    /// matching forward-delete count without inventing their
    /// own auto-repeat tracking.
    delete_pressed: u32,
    /// One entry per non-modifier key event this frame
    /// (auto-repeat included). Lets text-input widgets respond
    /// to held arrows / Home / End / Delete naturally rather
    /// than once per physical press. Cleared in `end_frame`.
    key_events: Vec<KeyCode>,
    /// Mirror of `key_events`, pre-translated to `rift_ui_im::ImKey`
    /// (variants without a mapping are skipped). The `UiInput`
    /// trait returns `&[ImKey]`; populating this field at event
    /// time lets the accessor be a zero-allocation borrow.
    key_events_im: Vec<rift_ui_im::ImKey>,
    /// Enter pressed this frame.
    enter_pressed: bool,
    /// When set, gameplay-style key polling
    /// (`is_key_held` / `key_just_pressed`) reports nothing.
    /// Widget-style accessors (`chars_typed`,
    /// `backspace_count`, `enter_just_pressed`) are unaffected
    /// so a focused text field can still receive its input.
    /// Toggled per-frame by callers that own a text-capture
    /// surface (chat, character-select, etc.) — defaults to
    /// off so gameplay isn't accidentally muted on startup.
    text_capture: Cell<bool>,
    /// When set for the current frame, widget-style text
    /// accessors (`chars_typed`, `backspace_count`,
    /// `enter_just_pressed`) report nothing. Used when a UI
    /// surface opens *because of* a keystroke that would
    /// otherwise leak into the freshly-focused text field
    /// (the chat HUD's `T`-to-open being the canonical
    /// example). Auto-cleared at the start of every frame's
    /// `end_frame`.
    text_swallow: Cell<bool>,
    /// When set for the current frame, mouse-driven camera
    /// control (right-drag yaw, scroll-wheel zoom) is
    /// suppressed. The raw scroll delta still surfaces via
    /// [`Self::scroll_delta`] so UI widgets (the talent
    /// panel's pan-zoom canvas) can read it; only the
    /// camera-side application is muted. Toggled per-frame
    /// by callers that own a fullscreen modal surface.
    mouse_camera_capture: Cell<bool>,
    /// Vertical mouse-wheel delta accumulated this frame. Reset
    /// in [`Self::end_frame`]. Positive values mean scroll
    /// *up* / *toward* the user (matches winit's `LineDelta.y`).
    /// UI widgets read this via [`Self::scroll_delta`] to drive
    /// scrollable panels.
    scroll_delta: f32,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            keys_held: HashSet::new(),
            prev_keys_held: HashSet::new(),
            camera_yaw: 0.0,
            camera_pitch: 0.5, // ~30 degrees above
            camera_distance: 8.0,
            right_mouse_down: false,
            left_clicked: Cell::new(false),
            right_clicked: Cell::new(false),
            last_mouse_pos: None,
            left_mouse_down: false,
            prev_left_mouse_down: false,
            left_just_released: false,
            mouse_pos: (0.0, 0.0),
            chars_typed: Vec::new(),
            backspace_pressed: 0,
            delete_pressed: 0,
            key_events: Vec::new(),
            key_events_im: Vec::new(),
            enter_pressed: false,
            text_capture: Cell::new(false),
            text_swallow: Cell::new(false),
            mouse_camera_capture: Cell::new(false),
            scroll_delta: 0.0,
        }
    }
}

impl Input {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_key_held(&self, key: KeyCode) -> bool {
        if self.text_capture.get() {
            return false;
        }
        self.keys_held.contains(&key)
    }

    pub fn camera_yaw(&self) -> f32 {
        self.camera_yaw
    }

    pub fn camera_pitch(&self) -> f32 {
        self.camera_pitch
    }

    pub fn camera_distance(&self) -> f32 {
        self.camera_distance
    }

    pub fn on_key_pressed(&mut self, key: KeyCode) {
        self.keys_held.insert(key);
    }

    pub fn on_key_released(&mut self, key: KeyCode) {
        self.keys_held.remove(&key);
    }

    /// Returns true if the key was just pressed this frame (wasn't held last frame).
    pub fn key_just_pressed(&self, key: KeyCode) -> bool {
        if self.text_capture.get() {
            return false;
        }
        self.keys_held.contains(&key) && !self.prev_keys_held.contains(&key)
    }

    /// Same as [`Self::key_just_pressed`] but ignores the
    /// text-capture flag. For widget-internal use only — the
    /// chat HUD reads its own Esc / Tab / R bindings through
    /// here so they keep working while typing (which is
    /// exactly when text-capture is on).
    pub fn key_just_pressed_raw(&self, key: KeyCode) -> bool {
        self.keys_held.contains(&key) && !self.prev_keys_held.contains(&key)
    }

    /// Same as [`Self::is_key_held`] but ignores text-capture.
    /// Widget-internal: text-input editing needs to read raw
    /// modifier state (Ctrl, Shift) precisely while
    /// text-capture is on.
    pub fn is_key_held_raw(&self, key: KeyCode) -> bool {
        self.keys_held.contains(&key)
    }

    /// Set the text-capture flag. While `on`, gameplay-style
    /// key polling (`is_key_held` / `key_just_pressed`) is
    /// suppressed so typing into a chat / form field doesn't
    /// also fire WASD / hotbar bindings. Widget-facing
    /// accessors are unaffected. Reset every frame by the
    /// caller (no auto-clear) — the chat HUD calls this at
    /// the top of each frame from its own `is_typing()` so
    /// the flag tracks the open/closed state without staleness.
    pub fn set_text_capture(&self, on: bool) {
        self.text_capture.set(on);
    }

    /// Set the mouse-camera-capture flag. While `on`,
    /// right-drag yaw and scroll-wheel zoom no longer modify
    /// the camera; the raw scroll delta still flows through
    /// to UI widgets. Mirrors [`Self::set_text_capture`]'s
    /// per-frame contract — callers re-set this at the top of
    /// every frame so the flag tracks the modal's open state.
    pub fn set_mouse_camera_capture(&self, on: bool) {
        self.mouse_camera_capture.set(on);
    }

    /// Call at end of frame to snapshot key state.
    pub fn end_frame(&mut self) {
        self.prev_keys_held.clone_from(&self.keys_held);
        self.chars_typed.clear();
        self.backspace_pressed = 0;
        self.delete_pressed = 0;
        self.key_events.clear();
        self.key_events_im.clear();
        self.enter_pressed = false;
        self.left_just_released = false;
        self.prev_left_mouse_down = self.left_mouse_down;
        // Discard any unconsumed click events. `left_clicked` /
        // `right_clicked` are "this frame only" rising-edge
        // signals — without an explicit clear, a click that
        // happens with no UI widget hovered (e.g. a basic-attack
        // left click in gameplay) stays latched in the cell and
        // will fire the next time *anything* hovered tries to
        // consume it. That used to cause the rift-portal modal
        // to auto-confirm: open it after attacking and the
        // centred "Enter" button instantly absorbed the stale
        // click on its first draw.
        self.left_clicked.set(false);
        self.right_clicked.set(false);
        self.text_swallow.set(false);
        self.scroll_delta = 0.0;
    }

    /// Push a typed character (printable). Called by the window event loop.
    pub fn on_char(&mut self, ch: char) {
        // Drop control characters; backspace/enter are tracked separately.
        if !ch.is_control() {
            self.chars_typed.push(ch);
        }
    }

    /// Notify the input system that backspace was pressed (auto-repeat counts).
    pub fn on_backspace(&mut self) {
        self.backspace_pressed = self.backspace_pressed.saturating_add(1);
    }

    /// Notify the input system that the forward-delete key was
    /// pressed (auto-repeat counts).
    pub fn on_delete(&mut self) {
        self.delete_pressed = self.delete_pressed.saturating_add(1);
    }

    /// Record a non-character key edge (auto-repeat included).
    /// Text-input widgets read these via [`Self::key_events`]
    /// to drive caret movement / selection without needing
    /// per-key fields.
    pub fn on_key_event(&mut self, key: KeyCode) {
        self.key_events.push(key);
        if let Some(im) = winit_to_im_key(key) {
            self.key_events_im.push(im);
        }
    }

    /// Notify that Enter / Return was pressed this frame.
    pub fn on_enter(&mut self) {
        self.enter_pressed = true;
    }

    /// Drain characters typed this frame.
    pub fn chars_typed(&self) -> &[char] {
        if self.text_swallow.get() {
            return &[];
        }
        &self.chars_typed
    }

    /// Number of backspaces pressed this frame.
    pub fn backspace_count(&self) -> u32 {
        if self.text_swallow.get() {
            return 0;
        }
        self.backspace_pressed
    }

    /// Number of forward-delete key presses this frame.
    pub fn delete_count(&self) -> u32 {
        if self.text_swallow.get() {
            return 0;
        }
        self.delete_pressed
    }

    /// Non-character key edges fired this frame, in arrival
    /// order. Auto-repeat presses are included so a held arrow
    /// key produces multiple entries per frame at the OS
    /// repeat rate. Text-input widgets walk this slice in
    /// order to drive caret motion / selection.
    pub fn key_events(&self) -> &[KeyCode] {
        if self.text_swallow.get() {
            return &[];
        }
        &self.key_events
    }

    pub fn enter_just_pressed(&self) -> bool {
        if self.text_swallow.get() {
            return false;
        }
        self.enter_pressed
    }

    /// Drop any text-input events buffered for this frame
    /// (typed chars + backspace + enter). Used when a UI
    /// surface opens *because of* a keystroke that would
    /// otherwise leak into the freshly-focused text field —
    /// e.g. the chat HUD's `T` to open: without this, the
    /// `T` press also lands in the field as the first
    /// character. Interior-mutable so widget code with
    /// `&Input` can call it; auto-clears at the next
    /// `end_frame`.
    pub fn discard_text_input(&self) {
        self.text_swallow.set(true);
    }

    pub fn on_mouse_button(&mut self, button: winit::event::MouseButton, pressed: bool) {
        if button == winit::event::MouseButton::Right {
            if pressed {
                self.right_clicked.set(true);
            }
            self.right_mouse_down = pressed;
            // Drop the last-known cursor position on *both*
            // edges. On release this prevents the next drag
            // from inheriting a stale anchor; on press it
            // prevents the very first CursorMoved after the
            // grab-toggle (window grab is released while RMB
            // is held so the player can rotate past the window
            // edge) from producing a huge synthetic `dx` that
            // snaps the camera. The first move after the press
            // just sets the anchor; the second move is what
            // actually starts rotating.
            self.last_mouse_pos = None;
        }
        if button == winit::event::MouseButton::Left {
            if pressed {
                self.left_clicked.set(true);
            } else if self.left_mouse_down {
                self.left_just_released = true;
            }
            self.left_mouse_down = pressed;
        }
    }

    /// Whether the left mouse button is currently held. Used by
    /// hold-to-channel ability inputs.
    pub fn left_mouse_held(&self) -> bool {
        self.left_mouse_down
    }

    /// Whether the left mouse button became pressed this frame
    /// (rising edge). Non-consuming; safe to call from multiple
    /// places. Used by drag-and-drop to start a drag without
    /// stealing the click from a single-click action elsewhere.
    pub fn left_just_pressed(&self) -> bool {
        self.left_mouse_down && !self.prev_left_mouse_down
    }

    /// Whether the left mouse button was released this frame
    /// (falling edge). Non-consuming. Used by drag-and-drop to
    /// detect the drop event.
    pub fn left_just_released(&self) -> bool {
        self.left_just_released
    }

    /// Returns true if left mouse was clicked this frame (consumes the click).
    pub fn left_clicked(&self) -> bool {
        let clicked = self.left_clicked.get();
        self.left_clicked.set(false);
        clicked
    }

    /// Returns true if right mouse was clicked this frame (consumes the click).
    pub fn right_clicked(&self) -> bool {
        let clicked = self.right_clicked.get();
        self.right_clicked.set(false);
        clicked
    }

    /// Current mouse position in pixels (screen-space).
    pub fn mouse_pos(&self) -> (f32, f32) {
        self.mouse_pos
    }

    pub fn on_cursor_moved(&mut self, x: f64, y: f64) {
        // Just record the absolute position — UI hit-testing
        // and click handling needs it. Camera yaw is driven
        // off raw mouse-motion deltas (see `on_mouse_motion`)
        // so that locking the cursor during RMB drag doesn't
        // freeze rotation.
        self.mouse_pos = (x as f32, y as f32);
        self.last_mouse_pos = Some((x, y));
    }

    /// Raw mouse motion delta (DeviceEvent::MouseMotion). Used
    /// to drive camera yaw while RMB is held. Independent of
    /// the OS cursor's screen position, which is what lets the
    /// player keep rotating past the window edge — the cursor
    /// is locked to the centre of the window during the drag
    /// and the raw deltas keep flowing.
    pub fn on_mouse_motion(&mut self, dx: f64, _dy: f64) {
        if self.right_mouse_down && !self.mouse_camera_capture.get() {
            // Pitch is locked: the camera holds the standard
            // top-down ARPG angle (~30°). Yaw remains free so
            // the player can rotate around their character
            // with right-mouse drag.
            self.camera_yaw -= (dx as f32) * 0.005;
        }
    }

    pub fn on_scroll(&mut self, delta: f32) {
        if !self.mouse_camera_capture.get() {
            self.camera_distance = (self.camera_distance - delta * 0.5).clamp(2.0, 20.0);
        }
        // Also expose the raw frame-scoped delta so UI
        // widgets (scrollable panels) can react. Camera zoom
        // and UI scroll co-exist for now — widgets that want
        // to claim the wheel can do so by checking hover
        // before reading [`Self::scroll_delta`].
        self.scroll_delta += delta;
    }

    /// Vertical mouse-wheel delta accumulated this frame.
    /// Positive = scroll up / toward the user. Widgets that
    /// consume this should hit-test the cursor against their
    /// rect first so background panels don't steal scroll
    /// from foreground ones.
    pub fn scroll_delta(&self) -> f32 {
        self.scroll_delta
    }
}

// ─── UiInput bridge ────────────────────────────────────────────
//
// `rift_ui_im::UiInput` is the trait widgets actually call. Keeping
// the impl here (rather than in the UI crate) lets the UI crate
// stay winit-free: the engine maps `ImKey` to `winit::KeyCode` at
// the boundary and forwards everything else verbatim.

impl rift_ui_im::UiInput for Input {
    fn is_key_held(&self, key: rift_ui_im::ImKey) -> bool {
        Input::is_key_held(self, im_key_to_winit(key))
    }
    fn key_just_pressed(&self, key: rift_ui_im::ImKey) -> bool {
        Input::key_just_pressed(self, im_key_to_winit(key))
    }
    fn is_key_held_raw(&self, key: rift_ui_im::ImKey) -> bool {
        Input::is_key_held_raw(self, im_key_to_winit(key))
    }
    fn key_just_pressed_raw(&self, key: rift_ui_im::ImKey) -> bool {
        Input::key_just_pressed_raw(self, im_key_to_winit(key))
    }
    fn chars_typed(&self) -> &[char] {
        Input::chars_typed(self)
    }
    fn backspace_count(&self) -> u32 {
        Input::backspace_count(self)
    }
    fn delete_count(&self) -> u32 {
        Input::delete_count(self)
    }
    fn key_events(&self) -> &[rift_ui_im::ImKey] {
        // The engine pushes ImKey-shaped events directly into
        // `key_events_im` so this accessor doesn't need to allocate
        // (and the widget side, which only sees a `&dyn UiInput`,
        // can rely on the borrow living as long as `self`).
        &self.key_events_im
    }
    fn enter_just_pressed(&self) -> bool {
        Input::enter_just_pressed(self)
    }
    fn discard_text_input(&self) {
        Input::discard_text_input(self)
    }
    fn mouse_pos(&self) -> (f32, f32) {
        Input::mouse_pos(self)
    }
    fn left_mouse_held(&self) -> bool {
        Input::left_mouse_held(self)
    }
    fn left_just_pressed(&self) -> bool {
        Input::left_just_pressed(self)
    }
    fn left_just_released(&self) -> bool {
        Input::left_just_released(self)
    }
    fn left_clicked(&self) -> bool {
        Input::left_clicked(self)
    }
    fn right_clicked(&self) -> bool {
        Input::right_clicked(self)
    }
    fn scroll_delta(&self) -> f32 {
        Input::scroll_delta(self)
    }
    fn set_text_capture(&self, on: bool) {
        Input::set_text_capture(self, on)
    }
}

/// Map a widget-facing `ImKey` to its `winit::keyboard::KeyCode`
/// equivalent. New variants get added in lock-step with
/// `rift_ui_im::ImKey`.
fn im_key_to_winit(k: rift_ui_im::ImKey) -> winit::keyboard::KeyCode {
    use rift_ui_im::ImKey as I;
    use winit::keyboard::KeyCode as K;
    match k {
        I::ShiftLeft => K::ShiftLeft,
        I::ShiftRight => K::ShiftRight,
        I::ControlLeft => K::ControlLeft,
        I::ControlRight => K::ControlRight,
        I::AltLeft => K::AltLeft,
        I::AltRight => K::AltRight,
        I::ArrowLeft => K::ArrowLeft,
        I::ArrowRight => K::ArrowRight,
        I::ArrowUp => K::ArrowUp,
        I::ArrowDown => K::ArrowDown,
        I::Home => K::Home,
        I::End => K::End,
        I::Escape => K::Escape,
        I::Enter => K::Enter,
        I::Tab => K::Tab,
        I::Backspace => K::Backspace,
        I::Delete => K::Delete,
        I::KeyA => K::KeyA,
        I::KeyB => K::KeyB,
        I::KeyC => K::KeyC,
        I::KeyI => K::KeyI,
        I::KeyN => K::KeyN,
        I::KeyR => K::KeyR,
        I::KeyT => K::KeyT,
        I::KeyV => K::KeyV,
        I::KeyX => K::KeyX,
        I::KeyZ => K::KeyZ,
        I::Slash => K::Slash,
    }
}

/// Inverse of `im_key_to_winit`; returns `None` for keys not
/// modelled in `ImKey` so the engine can simply skip them when
/// populating `key_events_im`.
pub(crate) fn winit_to_im_key(k: winit::keyboard::KeyCode) -> Option<rift_ui_im::ImKey> {
    use rift_ui_im::ImKey as I;
    use winit::keyboard::KeyCode as K;
    Some(match k {
        K::ShiftLeft => I::ShiftLeft,
        K::ShiftRight => I::ShiftRight,
        K::ControlLeft => I::ControlLeft,
        K::ControlRight => I::ControlRight,
        K::AltLeft => I::AltLeft,
        K::AltRight => I::AltRight,
        K::ArrowLeft => I::ArrowLeft,
        K::ArrowRight => I::ArrowRight,
        K::ArrowUp => I::ArrowUp,
        K::ArrowDown => I::ArrowDown,
        K::Home => I::Home,
        K::End => I::End,
        K::Escape => I::Escape,
        K::Enter => I::Enter,
        K::Tab => I::Tab,
        K::Backspace => I::Backspace,
        K::Delete => I::Delete,
        K::KeyA => I::KeyA,
        K::KeyB => I::KeyB,
        K::KeyC => I::KeyC,
        K::KeyI => I::KeyI,
        K::KeyN => I::KeyN,
        K::KeyR => I::KeyR,
        K::KeyT => I::KeyT,
        K::KeyV => I::KeyV,
        K::KeyX => I::KeyX,
        K::KeyZ => I::KeyZ,
        K::Slash => I::Slash,
        _ => return None,
    })
}
