use glam::{Mat4, Vec3};
use rift_engine::{App, Input, Mesh, Renderer, Window};
use std::path::Path;

struct RiftDemo;

impl App for RiftDemo {
    fn init(&mut self, renderer: &mut Renderer) -> anyhow::Result<()> {
        // Try to load a glTF model if available
        let model_path = Path::new("assets/models/test.glb");
        if model_path.exists() {
            renderer.load_gltf(model_path, Mat4::from_translation(Vec3::new(0.0, 0.0, 0.0)))?;
        } else {
            // Fallback: a cube hovering above origin
            let cube = Mesh::cube();
            renderer.add_mesh(&cube, Mat4::from_translation(Vec3::new(0.0, 0.75, 0.0)))?;
        }

        // Add a floor grid
        let floor = Mesh::grid(10.0, 10);
        renderer.add_mesh(&floor, Mat4::IDENTITY)?;

        Ok(())
    }

    fn update(&mut self, renderer: &mut Renderer, _input: &Input, _dt: f32) {
        // Slowly rotate the first object
        let t = renderer.elapsed_secs();
        let base_pos = if Path::new("assets/models/test.glb").exists() {
            Vec3::ZERO
        } else {
            Vec3::new(0.0, 0.75, 0.0)
        };
        renderer.objects[0].model_matrix =
            Mat4::from_translation(base_pos)
                * Mat4::from_rotation_y(t * 0.8);
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let window = Window::new("Rift Engine — 3D Scene", 1280, 720);
    window.run(RiftDemo)
}
