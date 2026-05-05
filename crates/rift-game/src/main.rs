use rift_engine::{App, Input, LoadStatus, Renderer, Window};
use rift_game::GameState;

struct RiftApp {
    state: GameState,
}

impl App for RiftApp {
    fn load_step(&mut self, renderer: &mut Renderer) -> anyhow::Result<LoadStatus> {
        self.state.load_step(renderer)
    }

    fn update(&mut self, renderer: &mut Renderer, input: &Input, dt: f32) {
        self.state.update(renderer, input, dt);
    }

    fn shutdown(&mut self, renderer: &mut Renderer) {
        self.state.shutdown(renderer);
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let window = Window::new("Rift Crawler", 1280, 720);
    window.run(RiftApp {
        state: GameState::new(),
    })
}
