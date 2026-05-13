//! Render-pass modules: directional shadow, point-light cube shadow,
//! sky dome, post-process (HDR + bloom + composite), and overlay (HUD).
//!
//! Each sub-module owns one logical pass's render-pass + pipeline +
//! attachments and exposes a `record(...)` style entry point used by
//! the renderer's `draw_loop`.

pub mod overlay;
pub mod post;
pub mod shadow;
pub mod shadow_point;
pub mod sky;
