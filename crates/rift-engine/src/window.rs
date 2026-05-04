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
    fn init(&mut self, renderer: &mut Renderer) -> Result<()>;
    fn update(&mut self, renderer: &mut Renderer, input: &Input, dt: f32);
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
                        Ok(mut renderer) => {
                            if let Err(e) = self.user_app.init(&mut renderer) {
                                log::error!("App init failed: {}", e);
                                event_loop.exit();
                                return;
                            }
                            self.renderer = Some(renderer);
                            self.initialized = true;
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
                    // Wait for the GPU to finish using the upcoming frame's
                    // resources BEFORE the game runs systems that may write
                    // into per-frame host-visible buffers (e.g. CPU skinning).
                    if let Err(e) = renderer.prepare_frame() {
                        log::error!("Prepare frame error: {}", e);
                    }
                    self.user_app.update(renderer, &self.input, dt);
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
            last_frame_time: std::time::Instant::now(),
        };

        event_loop.run_app(&mut winit_app)?;
        Ok(())
    }
}
