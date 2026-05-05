use anyhow::Result;
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::PhysicalKey,
    window::{self, WindowAttributes},
};

use crate::input::Input;
use crate::renderer::Renderer;

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
    last_progress: (f32, String),
    last_frame_time: std::time::Instant,
}

impl<A: App> ApplicationHandler for WinitApp<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let attrs = WindowAttributes::default()
                .with_title(&self.title)
                .with_inner_size(winit::dpi::PhysicalSize::new(self.width, self.height));

            match event_loop.create_window(attrs) {
                Ok(window) => {
                    match Renderer::new(&window) {
                        Ok(renderer) => {
                            // App-side init now happens incrementally via
                            // `load_step` so we can render a loading
                            // screen while it runs. Just stash the
                            // renderer here.
                            self.renderer = Some(renderer);
                            self.initialized = true;
                            self.loading = true;
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
                        Key::Named(NamedKey::Enter) => self.input.on_enter(),
                        Key::Character(s) => {
                            for ch in s.chars() {
                                self.input.on_char(ch);
                            }
                        }
                        _ => {}
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                self.input.on_mouse_button(button, pressed);
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
                        if self.loading {
                            draw_loading_overlay(
                                renderer,
                                self.last_progress.0,
                                &self.last_progress.1,
                            );
                        }
                    } else {
                        self.user_app.update(renderer, &self.input, dt);
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
            WindowEvent::Resized(physical_size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.notify_resized(physical_size.width, physical_size.height);
                }
            }
            _ => {}
        }
    }
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

/// Built-in loading screen: a dark backdrop, centered title, a horizontal
/// progress bar, and the current task label. Drawn into the renderer's
/// overlay batch (which is already wired into the per-frame submit path).
fn draw_loading_overlay(renderer: &mut Renderer, progress: f32, label: &str) {
    let [w, h] = renderer.window_extent();
    let (sw, sh) = (w as f32, h as f32);

    // Switch the clear color to a near-black tone for the loading screen
    // so the swapchain doesn't flash white before our overlay renders.
    renderer.clear_color = [0.02, 0.02, 0.03, 1.0];

    let batch = &mut renderer.overlay_batch;

    // Reset any leftover overlay from the previous frame.
    batch.clear();

    // Full-screen darken (in case clear color isn't honored on some path).
    batch.rect_px(0.0, 0.0, sw, sh, [0.02, 0.02, 0.03, 1.0], sw, sh);

    // Title.
    let title = "Rift Crawler";
    let title_size = 36.0;
    let title_w = batch.measure_text(title, title_size);
    batch.text(
        title,
        (sw - title_w) * 0.5,
        sh * 0.40 - title_size,
        title_size,
        [0.85, 0.80, 0.65, 1.0],
        sw, sh,
    );

    // Progress bar geometry.
    let bar_w = (sw * 0.45).max(240.0);
    let bar_h = 18.0;
    let bar_x = (sw - bar_w) * 0.5;
    let bar_y = sh * 0.50;
    // Bar background.
    batch.rect_px(bar_x, bar_y, bar_w, bar_h, [0.10, 0.10, 0.14, 1.0], sw, sh);
    // Bar fill.
    let fill_w = bar_w * progress.clamp(0.0, 1.0);
    if fill_w > 0.5 {
        batch.rect_px(bar_x, bar_y, fill_w, bar_h, [0.55, 0.45, 0.20, 1.0], sw, sh);
    }
    // Bar border (4 thin rects).
    let border = [0.30, 0.28, 0.22, 1.0];
    let t = 1.5;
    batch.rect_px(bar_x, bar_y, bar_w, t, border, sw, sh);
    batch.rect_px(bar_x, bar_y + bar_h - t, bar_w, t, border, sw, sh);
    batch.rect_px(bar_x, bar_y, t, bar_h, border, sw, sh);
    batch.rect_px(bar_x + bar_w - t, bar_y, t, bar_h, border, sw, sh);

    // Current task label, centered below the bar.
    let label_size = 14.0;
    let label_w = batch.measure_text(label, label_size);
    batch.text(
        label,
        (sw - label_w) * 0.5,
        bar_y + bar_h + 12.0,
        label_size,
        [0.70, 0.70, 0.72, 1.0],
        sw, sh,
    );
}
