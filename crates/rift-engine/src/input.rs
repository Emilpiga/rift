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
    mouse_pos: (f32, f32),
    /// Characters typed this frame (consumed by `take_chars_typed`).
    chars_typed: Vec<char>,
    /// Backspace pressed this frame (key auto-repeat respected).
    backspace_pressed: u32,
    /// Enter pressed this frame.
    enter_pressed: bool,
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
            mouse_pos: (0.0, 0.0),
            chars_typed: Vec::new(),
            backspace_pressed: 0,
            enter_pressed: false,
        }
    }
}

impl Input {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_key_held(&self, key: KeyCode) -> bool {
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
        self.keys_held.contains(&key) && !self.prev_keys_held.contains(&key)
    }

    /// Call at end of frame to snapshot key state.
    pub fn end_frame(&mut self) {
        self.prev_keys_held = self.keys_held.clone();
        self.chars_typed.clear();
        self.backspace_pressed = 0;
        self.enter_pressed = false;
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

    /// Notify that Enter / Return was pressed this frame.
    pub fn on_enter(&mut self) {
        self.enter_pressed = true;
    }

    /// Drain characters typed this frame.
    pub fn chars_typed(&self) -> &[char] {
        &self.chars_typed
    }

    /// Number of backspaces pressed this frame.
    pub fn backspace_count(&self) -> u32 {
        self.backspace_pressed
    }

    pub fn enter_just_pressed(&self) -> bool {
        self.enter_pressed
    }

    pub fn on_mouse_button(&mut self, button: winit::event::MouseButton, pressed: bool) {
        if button == winit::event::MouseButton::Right {
            if pressed {
                self.right_clicked.set(true);
            }
            self.right_mouse_down = pressed;
            if !pressed {
                self.last_mouse_pos = None;
            }
        }
        if button == winit::event::MouseButton::Left {
            if pressed {
                self.left_clicked.set(true);
            }
            self.left_mouse_down = pressed;
        }
    }

    /// Whether the left mouse button is currently held. Used by
    /// hold-to-channel ability inputs.
    pub fn left_mouse_held(&self) -> bool {
        self.left_mouse_down
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
        let current = (x, y);
        self.mouse_pos = (x as f32, y as f32);
        if self.right_mouse_down {
            if let Some(last) = self.last_mouse_pos {
                let dx = (current.0 - last.0) as f32;
                let dy = (current.1 - last.1) as f32;
                self.camera_yaw -= dx * 0.005;
                self.camera_pitch = (self.camera_pitch + dy * 0.005).clamp(0.1, 1.4);
            }
        }
        self.last_mouse_pos = Some(current);
    }

    pub fn on_scroll(&mut self, delta: f32) {
        self.camera_distance = (self.camera_distance - delta * 0.5).clamp(2.0, 20.0);
    }
}
