pub mod asset_decode;
pub mod blood;
pub mod camera;
pub mod depth;
pub mod draw_loop;
pub mod font;
pub mod forward;
pub mod gpu_skin;
pub mod material;
pub mod mesh;
pub mod objects;
pub mod passes;
pub mod pipeline;
pub mod texture;
pub mod uniform;
pub mod uniforms;
pub mod vfx;

// Backwards-compat re-exports: pass modules moved under `passes/`,
// but external code still imports them as `renderer::shadow::*` etc.
pub use passes::{overlay, post, shadow, shadow_point, sky};

pub use forward::{DisplayResolution, Renderer};
pub use overlay::{OverlayBatch, OverlayRenderer};
