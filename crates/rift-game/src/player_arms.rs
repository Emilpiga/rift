//! Player arms that follow the cursor, giving the impression of holding a weapon.
//!
//! Arms are not gameplay entities — they're purely visual decals attached to the
//! player. Each frame, [`PlayerArms::sync`] recomputes their world transforms from
//! the player's position and the current aim direction.

use glam::{Mat4, Quat, Vec3};
use rift_engine::renderer::mesh::Mesh;
use rift_engine::Renderer;

/// Local-space shoulder offsets relative to the player's feet (in player local frame).
/// The player mesh has its torso top around y=0.7 with shoulders ~0.3 wide.
const RIGHT_SHOULDER: Vec3 = Vec3::new(0.30, 0.65, 0.0);
const LEFT_SHOULDER: Vec3 = Vec3::new(-0.30, 0.65, 0.0);
/// Arm dimensions when the unit-length arm mesh is scaled. The arm mesh is built
/// extending along +Z, so X/Y are the cross-section and Z is the length.
const ARM_SCALE: Vec3 = Vec3::new(0.10, 0.10, 0.55);

/// Render-object indices for the two visible arms.
pub struct PlayerArms {
    right_obj: Option<usize>,
    left_obj: Option<usize>,
}

impl PlayerArms {
    pub fn new() -> Self {
        Self { right_obj: None, left_obj: None }
    }

    /// Allocate the two render objects (initially hidden). Call after player spawn.
    pub fn init(&mut self, renderer: &mut Renderer) -> anyhow::Result<()> {
        let mesh = Mesh::player_arm();
        renderer.add_mesh(&mesh, Mat4::ZERO)?;
        self.right_obj = Some(renderer.objects.len() - 1);
        renderer.add_mesh(&mesh, Mat4::ZERO)?;
        self.left_obj = Some(renderer.objects.len() - 1);
        Ok(())
    }

    /// Drop cached object indices (call before regenerating the world).
    pub fn clear(&mut self) {
        self.right_obj = None;
        self.left_obj = None;
    }

    /// Update the arm transforms to follow the cursor aim direction.
    pub fn sync(&self, player_pos: Vec3, aim_dir: Vec3, renderer: &mut Renderer) {
        // Yaw rotation that sends local +Z to `aim_dir` (horizontal plane).
        let yaw = aim_dir.x.atan2(aim_dir.z);
        let rot = Quat::from_rotation_y(yaw);

        for (slot_obj, local_shoulder) in [
            (self.right_obj, RIGHT_SHOULDER),
            (self.left_obj, LEFT_SHOULDER),
        ] {
            let Some(obj_idx) = slot_obj else { continue };
            if obj_idx >= renderer.objects.len() {
                continue;
            }
            // World shoulder = player_pos + R * local_shoulder.
            let world_shoulder = player_pos + rot * local_shoulder;
            renderer.objects[obj_idx].model_matrix =
                Mat4::from_translation(world_shoulder)
                    * Mat4::from_quat(rot)
                    * Mat4::from_scale(ARM_SCALE);
        }
    }

    /// World-space position of the right hand / weapon tip — used as the spawn
    /// origin for projectiles so they appear to come out of the held weapon.
    pub fn right_hand_tip(player_pos: Vec3, aim_dir: Vec3) -> Vec3 {
        let yaw = aim_dir.x.atan2(aim_dir.z);
        let rot = Quat::from_rotation_y(yaw);
        // Shoulder + full arm length along local +Z (which is `aim_dir` after rot).
        player_pos + rot * RIGHT_SHOULDER + aim_dir * ARM_SCALE.z
    }
}
