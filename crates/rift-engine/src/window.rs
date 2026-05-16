use anyhow::Result;
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::PhysicalKey,
    window::{self, CursorGrabMode, Fullscreen, WindowAttributes},
};

use crate::input::Input;
use crate::renderer::{DisplayResolution, Renderer};
use std::collections::BTreeSet;

pub struct Window {
    pub title: String,
    pub width: u32,
    pub height: u32,
}

pub trait App {
    /// Perform one step of initial loading. Called every frame until it
    /// returns `LoadStatus::Done`. The engine renders a built-in loading
    /// screen with the reported progress while loading is in progress;
    /// only after `Done` does it begin calling `update`.
    fn load_step(&mut self, renderer: &mut Renderer) -> Result<LoadStatus>;
    fn update(&mut self, renderer: &mut Renderer, input: &Input, dt: f32);
    /// Optional shutdown hook called once before the renderer is
    /// dropped.  Use this to free GPU resources (textures, descriptor
    /// sets, etc.) the app owns outside the renderer — otherwise
    /// validation layers will report "object not destroyed" warnings
    /// at `vkDestroyDevice`.
    fn shutdown(&mut self, _renderer: &mut Renderer) {}
}

/// Result of a single `App::load_step` call.
pub enum LoadStatus {
    /// Still loading; engine should draw the loading screen and call
    /// `load_step` again next frame. `progress` is in 0..=1, `label`
    /// describes the current task.
    Loading { progress: f32, label: String },
    /// Loading complete; engine should begin the normal `update` loop.
    Done,
}

struct WinitApp<A: App> {
    window: Option<winit::window::Window>,
    renderer: Option<Renderer>,
    user_app: A,
    input: Input,
    title: String,
    width: u32,
    height: u32,
    initialized: bool,
    loading: bool,
    loading_screen_presented: bool,
    last_progress: (f32, String),
    last_frame_time: std::time::Instant,
}

impl<A: App> ApplicationHandler for WinitApp<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let attrs = WindowAttributes::default()
                .with_title(&self.title)
                .with_inner_size(winit::dpi::PhysicalSize::new(self.width, self.height))
                .with_visible(false)
                // Launch in borderless fullscreen on the
                // current monitor. `Fullscreen::Borderless(None)`
                // picks whichever monitor the window would
                // otherwise have spawned on, which matches what
                // players expect from a "fullscreen" launch and
                // sidesteps the exclusive-mode pitfalls
                // (alt-tab flicker, refresh-rate negotiation).
                .with_fullscreen(Some(Fullscreen::Borderless(None)));

            match event_loop.create_window(attrs) {
                Ok(window) => {
                    // Confine the OS cursor to the window so a
                    // fast aim-flick can't slide it off-screen
                    // mid-fight. We intentionally do NOT fall
                    // back to `CursorGrabMode::Locked` — Locked
                    // pins the cursor to a fixed point (the
                    // center on Windows), which reads as a
                    // "snap" the first time the player moves
                    // the mouse. Confined keeps the OS cursor
                    // free to roam inside the window rectangle,
                    // which is all the gameplay actually needs.
                    if let Err(e) = window.set_cursor_grab(CursorGrabMode::Confined) {
                        log::warn!(
                            "cursor: Confined grab not supported ({e}); cursor may exit \
                             the window"
                        );
                    }
                    match Renderer::new(&window) {
                        Ok(mut renderer) => {
                            sync_display_resolutions(&window, &mut renderer);
                            // App-side init now happens incrementally via
                            // `load_step` so we can render a loading
                            // screen while it runs. Just stash the
                            // renderer here.
                            draw_loading_overlay(
                                &mut renderer,
                                self.last_progress.0,
                                &self.last_progress.1,
                            );
                            if let Err(e) = renderer.draw_frame() {
                                log::warn!(
                                    "Initial loading frame failed before window reveal: {}",
                                    e
                                );
                            }
                            window.set_visible(true);
                            window.request_redraw();
                            self.renderer = Some(renderer);
                            self.initialized = true;
                            self.loading = true;
                            self.loading_screen_presented = true;
                            log::info!("Window and renderer created");
                        }
                        Err(e) => {
                            log::error!("Failed to create renderer: {}", e);
                            event_loop.exit();
                            return;
                        }
                    }
                    self.window = Some(window);
                }
                Err(e) => {
                    log::error!("Failed to create window: {}", e);
                    event_loop.exit();
                }
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: window::WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                log::info!("Window close requested");
                event_loop.exit();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(key_code) = event.physical_key {
                    match event.state {
                        ElementState::Pressed => self.input.on_key_pressed(key_code),
                        ElementState::Released => self.input.on_key_released(key_code),
                    }
                }
                // Forward typed text (printable chars), backspace and enter
                // to the input system so UI text fields can consume them.
                // Auto-repeat counts as separate presses, which is what
                // text-edit fields want.
                if event.state == ElementState::Pressed {
                    use winit::keyboard::{Key, NamedKey};
                    match &event.logical_key {
                        Key::Named(NamedKey::Backspace) => self.input.on_backspace(),
                        Key::Named(NamedKey::Delete) => self.input.on_delete(),
                        Key::Named(NamedKey::Enter) => self.input.on_enter(),
                        // Space arrives as a *named* key, not as
                        // `Character(" ")`, so route it into the
                        // text stream explicitly. Without this
                        // text fields can never type a space.
                        Key::Named(NamedKey::Space) => self.input.on_char(' '),
                        Key::Character(s) => {
                            for ch in s.chars() {
                                self.input.on_char(ch);
                            }
                        }
                        _ => {}
                    }
                    // Text-input widgets need every press *edge*
                    // (auto-repeat included) for arrow / Home /
                    // End / Delete navigation. The `keys_held`
                    // set above only fires on the very first
                    // physical press, so we forward physical
                    // codes here separately. Modifier keys are
                    // intentionally skipped — selection logic
                    // reads their *held* state via
                    // `is_key_held_raw`.
                    if let PhysicalKey::Code(kc) = event.physical_key {
                        use winit::keyboard::KeyCode as KC;
                        if !matches!(
                            kc,
                            KC::ShiftLeft
                                | KC::ShiftRight
                                | KC::ControlLeft
                                | KC::ControlRight
                                | KC::AltLeft
                                | KC::AltRight
                                | KC::SuperLeft
                                | KC::SuperRight
                        ) {
                            self.input.on_key_event(kc);
                        }
                    }
                }
            }
            WindowEvent::Ime(winit::event::Ime::Commit(text)) => {
                for ch in text.chars() {
                    self.input.on_char(ch);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                self.input.on_mouse_button(button, pressed);
                // RMB starts the camera-rotate drag. We do
                // nothing here: rotation is driven by raw
                // mouse-motion deltas (`DeviceEvent::MouseMotion`)
                // and the cursor is already confined to the
                // window via the `Confined` grab applied at
                // startup, so a fast aim-flick can't slide it
                // off-screen. No hide, no warp, no snap.
                let _ = pressed;
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.input.on_cursor_moved(position.x, position.y);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let scroll = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(pos) => pos.y as f32 * 0.01,
                };
                self.input.on_scroll(scroll);
            }
            WindowEvent::RedrawRequested => {
                let now = std::time::Instant::now();
                let dt = (now - self.last_frame_time).as_secs_f32().min(0.1);
                self.last_frame_time = now;

                if let Some(renderer) = &mut self.renderer {
                    renderer.check_hot_reload();
                    if let Err(e) = renderer.prepare_frame() {
                        log::error!("Prepare frame error: {}", e);
                    }
                    if self.loading {
                        if self.loading_screen_presented {
                            // Drive one step of app initialization, then draw
                            // a loading screen overlay. Keep doing this every
                            // frame until the app reports `Done`.
                            match self.user_app.load_step(renderer) {
                                Ok(LoadStatus::Loading { progress, label }) => {
                                    self.last_progress = (progress.clamp(0.0, 1.0), label);
                                }
                                Ok(LoadStatus::Done) => {
                                    self.loading = false;
                                    log::info!("App loading complete");
                                }
                                Err(e) => {
                                    log::error!("App load_step failed: {}", e);
                                    event_loop.exit();
                                    return;
                                }
                            }
                        }
                        if self.loading {
                            self.loading_screen_presented = true;
                            draw_loading_overlay(
                                renderer,
                                self.last_progress.0,
                                &self.last_progress.1,
                            );
                        }
                    } else {
                        self.user_app.update(renderer, &self.input, dt);
                        if let Some(window) = &self.window {
                            apply_requested_display_resolution(window, renderer);
                        }
                    }
                    self.input.end_frame();
                    if let Err(e) = renderer.draw_frame() {
                        log::error!("Draw error: {}", e);
                    }
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::Focused(focused) => {
                // Re-apply cursor confinement when the window
                // regains focus. Most platforms drop the grab
                // on focus loss; without this Alt-Tab + click-
                // back leaves the cursor free to wander out.
                if focused {
                    if let Some(w) = &self.window {
                        let _ = w.set_cursor_grab(CursorGrabMode::Confined);
                    }
                }
            }
            WindowEvent::Resized(physical_size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.notify_resized(physical_size.width, physical_size.height);
                    if let Some(window) = &self.window {
                        sync_display_resolutions(window, renderer);
                    }
                }
            }
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: winit::event::DeviceEvent,
    ) {
        // Raw mouse motion bypasses cursor-position clamping,
        // so it keeps flowing even while the cursor is locked
        // to the centre of the window during a RMB camera
        // drag. Used to drive yaw without ever hitting the
        // screen edge.
        if let winit::event::DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            self.input.on_mouse_motion(dx, dy);
        }
    }
}

fn sync_display_resolutions(window: &winit::window::Window, renderer: &mut Renderer) {
    let mut seen = BTreeSet::new();
    let mut resolutions = Vec::new();

    if let Some(monitor) = window.current_monitor() {
        for mode in monitor.video_modes() {
            let size = mode.size();
            if size.width < 1024 || size.height < 576 {
                continue;
            }
            if seen.insert((size.width, size.height)) {
                resolutions.push(DisplayResolution {
                    width: size.width,
                    height: size.height,
                });
            }
        }
    }

    resolutions.sort_by(|a, b| {
        (b.width as u64 * b.height as u64)
            .cmp(&(a.width as u64 * a.height as u64))
            .then_with(|| b.width.cmp(&a.width))
    });

    let size = window.inner_size();
    let selected = DisplayResolution {
        width: size.width,
        height: size.height,
    };
    if !resolutions.iter().any(|r| *r == selected) && selected.width > 0 && selected.height > 0 {
        resolutions.push(selected);
    }

    renderer.set_display_resolutions(resolutions, selected);
}

fn apply_requested_display_resolution(window: &winit::window::Window, renderer: &mut Renderer) {
    let Some(requested) = renderer.take_requested_display_resolution() else {
        return;
    };

    let Some(monitor) = window.current_monitor() else {
        log::warn!("Display resolution change skipped: no current monitor");
        return;
    };

    let mut best_mode = None;
    for mode in monitor.video_modes() {
        let size = mode.size();
        if size.width == requested.width && size.height == requested.height {
            let replace = best_mode
                .as_ref()
                .map(|best: &winit::monitor::VideoModeHandle| {
                    mode.refresh_rate_millihertz() > best.refresh_rate_millihertz()
                })
                .unwrap_or(true);
            if replace {
                best_mode = Some(mode);
            }
        }
    }

    if let Some(mode) = best_mode {
        window.set_fullscreen(Some(Fullscreen::Exclusive(mode)));
    } else {
        window.set_fullscreen(None);
        let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(
            requested.width,
            requested.height,
        ));
    }

    sync_display_resolutions(window, renderer);
}

impl Window {
    pub fn new(title: &str, width: u32, height: u32) -> Self {
        Self {
            title: title.to_string(),
            width,
            height,
        }
    }

    pub fn run<A: App>(self, app: A) -> Result<()> {
        let event_loop = EventLoop::new()?;

        let mut winit_app = WinitApp {
            window: None,
            renderer: None,
            user_app: app,
            input: Input::new(),
            title: self.title,
            width: self.width,
            height: self.height,
            initialized: false,
            loading: false,
            loading_screen_presented: false,
            last_progress: (0.0, String::from("Loading…")),
            last_frame_time: std::time::Instant::now(),
        };

        event_loop.run_app(&mut winit_app)?;

        // Give the app a chance to free its own GPU resources before
        // the renderer's allocator is dropped.
        if let Some(renderer) = winit_app.renderer.as_mut() {
            winit_app.user_app.shutdown(renderer);
        }
        Ok(())
    }
}

/// Built-in loading screen drawn into the renderer's overlay batch.
fn draw_loading_overlay(renderer: &mut Renderer, progress: f32, label: &str) {
    // Switch the clear color to a near-black tone for the loading screen
    // so the swapchain doesn't flash white before our overlay renders.
    renderer.clear_color = [0.018, 0.014, 0.012, 1.0];

    let batch = &mut renderer.overlay_batch;

    // Reset any leftover overlay from the previous frame.
    batch.clear();

    draw_forged_loading_backdrop(renderer);

    draw_forged_loading_panel(
        renderer,
        "RIFT CRAWLER",
        "Preparing the rift",
        progress,
        label,
    );
}

pub fn draw_forged_loading_backdrop(renderer: &mut Renderer) {
    let [w, h] = renderer.window_extent();
    let (sw, sh) = (w as f32, h as f32);
    renderer.clear_color = [0.018, 0.015, 0.014, 1.0];
    let batch = &mut renderer.overlay_batch;

    batch.rect_px(0.0, 0.0, sw, sh, [0.018, 0.015, 0.014, 1.0], sw, sh);
    batch.rect_px(0.0, 0.0, sw, sh, [0.030, 0.022, 0.018, 0.55], sw, sh);
}

pub fn draw_forged_loading_panel(
    renderer: &mut Renderer,
    title: &str,
    subtitle: &str,
    progress: f32,
    label: &str,
) {
    let [w, h] = renderer.window_extent();
    let (sw, sh) = (w as f32, h as f32);
    let displayed_progress = renderer.smooth_loading_progress(progress);
    let batch = &mut renderer.overlay_batch;

    let panel_w = (sw * 0.38).clamp(360.0, 560.0);
    let panel_x = (sw - panel_w) * 0.5;
    let panel_y = sh * 0.50 - 76.0;

    let title_size = 30.0;
    let title_w = batch.measure_text(title, title_size, true);
    batch.text(
        title,
        panel_x + (panel_w - title_w) * 0.5,
        panel_y,
        title_size,
        [0.94, 0.84, 0.68, 1.0],
        true,
        sw,
        sh,
    );

    let subtitle_size = 13.0;
    let subtitle_w = batch.measure_text(subtitle, subtitle_size, false);
    batch.text(
        subtitle,
        panel_x + (panel_w - subtitle_w) * 0.5,
        panel_y + 42.0,
        subtitle_size,
        [0.58, 0.53, 0.46, 1.0],
        false,
        sw,
        sh,
    );

    let bar_w = panel_w;
    let bar_h = 8.0;
    let bar_x = panel_x;
    let bar_y = panel_y + 82.0;
    batch.rect_px_grad_v(
        bar_x,
        bar_y,
        bar_w,
        bar_h,
        [0.060, 0.046, 0.038, 1.0],
        [0.018, 0.015, 0.014, 1.0],
        sw,
        sh,
    );
    batch.rect_px(bar_x, bar_y, bar_w, 1.0, [1.0, 0.92, 0.72, 0.08], sw, sh);
    let fill_w = (bar_w * displayed_progress).max(0.0);
    if fill_w > 0.5 {
        batch.rect_px_grad4(
            bar_x,
            bar_y,
            fill_w,
            bar_h,
            [0.96, 0.38, 0.22, 1.0],
            [0.72, 0.16, 0.12, 1.0],
            [0.46, 0.055, 0.045, 1.0],
            [0.58, 0.095, 0.065, 1.0],
            sw,
            sh,
        );
        batch.rect_px_grad_v(
            bar_x,
            bar_y + 1.0,
            fill_w,
            (bar_h * 0.38).max(2.0),
            [1.0, 1.0, 1.0, 0.20],
            [1.0, 1.0, 1.0, 0.015],
            sw,
            sh,
        );
        let streak_w = (bar_h * 1.8).clamp(14.0, 34.0).min(fill_w);
        if streak_w > 2.0 {
            let streak_x = (bar_x + fill_w - streak_w * 1.08).max(bar_x);
            batch.rect_px_grad4(
                streak_x,
                bar_y + 1.0,
                streak_w,
                bar_h - 2.0,
                [1.0, 1.0, 1.0, 0.0],
                [1.0, 1.0, 1.0, 0.22],
                [1.0, 1.0, 1.0, 0.0],
                [1.0, 1.0, 1.0, 0.06],
                sw,
                sh,
            );
        }
        if fill_w < bar_w - 0.5 {
            let cursor_x = bar_x + fill_w;
            let halo_w = (bar_h * 1.25).clamp(9.0, 20.0);
            batch.rect_px_grad4(
                cursor_x - halo_w * 0.55,
                bar_y,
                halo_w,
                bar_h,
                [1.0, 1.0, 1.0, 0.0],
                [1.0, 0.70, 0.38, 0.34],
                [1.0, 1.0, 1.0, 0.0],
                [0.80, 0.20, 0.10, 0.14],
                sw,
                sh,
            );
            batch.rect_px_grad_v(
                cursor_x - 1.0,
                bar_y + 1.0,
                2.0,
                bar_h - 2.0,
                [1.0, 0.94, 0.76, 0.90],
                [0.95, 0.35, 0.18, 0.54],
                sw,
                sh,
            );
        }
    }

    let pct = format!("{:>3}%", (displayed_progress * 100.0).round() as i32);
    let pct_size = 13.0;
    let pct_w = batch.measure_text(&pct, pct_size, false);
    batch.text(
        &pct,
        bar_x + bar_w - pct_w,
        bar_y + bar_h + 18.0,
        pct_size,
        [0.70, 0.62, 0.50, 1.0],
        false,
        sw,
        sh,
    );

    let label_size = 14.0;
    batch.text(
        label,
        bar_x,
        bar_y + bar_h + 18.0,
        label_size,
        [0.70, 0.66, 0.58, 1.0],
        false,
        sw,
        sh,
    );
}
