//! Cursor → world-space picking utilities.
//!
//! Both helpers are pure: they read mouse coordinates from
//! `Input`, the camera matrices from `Renderer`, and project a
//! ray to a horizontal ground plane. No `GameState` coupling,
//! so they live as free functions.

use glam::Vec3;
use rift_engine::{Input, Renderer};

/// Compute the world position where the cursor ray hits a
/// horizontal ground plane at `ground_y`. Returns `None` if the
/// window has zero extent or the ray is parallel to the plane.
pub fn world_pos(input: &Input, renderer: &Renderer, ground_y: f32) -> Option<Vec3> {
    let (mx, my) = input.mouse_pos();
    let [w, h] = renderer.window_extent();
    if w == 0 || h == 0 {
        return None;
    }

    let ndc_x = (mx / w as f32) * 2.0 - 1.0;
    let ndc_y = (my / h as f32) * 2.0 - 1.0;

    let inv_vp = (renderer.camera.projection_matrix() * renderer.camera.view_matrix()).inverse();
    let near_point = inv_vp.project_point3(glam::Vec3::new(ndc_x, ndc_y, 0.0));
    let far_point = inv_vp.project_point3(glam::Vec3::new(ndc_x, ndc_y, 1.0));
    let ray_dir = (far_point - near_point).normalize();

    if ray_dir.y.abs() < 1e-6 {
        return None;
    }
    let t = (ground_y - near_point.y) / ray_dir.y;
    Some(near_point + ray_dir * t)
}

/// Compute a horizontal aim direction from `player_pos` toward
/// the cursor's projection onto the ground plane at the
/// player's Y. Falls back to `Vec3::NEG_Z` when the cursor is
/// directly over the player or the ray misses.
pub fn aim_dir(input: &Input, renderer: &Renderer, player_pos: Vec3) -> Vec3 {
    if let Some(hit) = world_pos(input, renderer, player_pos.y) {
        let delta = hit - player_pos;
        let flat = Vec3::new(delta.x, 0.0, delta.z);
        if flat.length_squared() > 0.01 {
            return flat.normalize();
        }
    }
    Vec3::NEG_Z
}
