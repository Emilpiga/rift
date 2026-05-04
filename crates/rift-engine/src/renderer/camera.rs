use glam::{Mat4, Vec3, Vec4};

pub struct Camera {
    pub position: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    pub fov_y: f32,
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
}

impl Camera {
    pub fn new(aspect: f32) -> Self {
        Self {
            position: Vec3::new(0.0, 2.0, 5.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov_y: 45.0_f32.to_radians(),
            aspect,
            near: 0.1,
            far: 100.0,
        }
    }

    pub fn view_matrix(&self) -> Mat4 {
        Mat4::look_at_rh(self.position, self.target, self.up)
    }

    pub fn projection_matrix(&self) -> Mat4 {
        let mut proj = Mat4::perspective_rh(self.fov_y, self.aspect, self.near, self.far);
        // Vulkan clip space has Y pointing down; flip it.
        proj.y_axis.y *= -1.0;
        proj
    }

    pub fn view_projection(&self) -> Mat4 {
        self.projection_matrix() * self.view_matrix()
    }

    /// Orbit around the target by yaw (horizontal) and pitch (vertical) deltas.
    pub fn orbit(&mut self, yaw: f32, pitch: f32) {
        let offset = self.position - self.target;
        let radius = offset.length();

        // Convert to spherical coordinates
        let theta = offset.z.atan2(offset.x) - yaw;
        let phi = (offset.y / radius).acos() + pitch;

        // Clamp phi to avoid flipping
        let phi = phi.clamp(0.05, std::f32::consts::PI - 0.05);

        self.position = self.target + Vec3::new(
            radius * phi.sin() * theta.cos(),
            radius * phi.cos(),
            radius * phi.sin() * theta.sin(),
        );
    }

    /// Pan the camera (move target and position together).
    pub fn pan(&mut self, dx: f32, dy: f32) {
        let forward = (self.target - self.position).normalize();
        let right = forward.cross(self.up).normalize();
        let up = right.cross(forward).normalize();

        let offset = right * (-dx) + up * dy;
        self.position += offset;
        self.target += offset;
    }

    /// Zoom by moving closer to or further from the target.
    pub fn zoom(&mut self, amount: f32) {
        let offset = self.position - self.target;
        let distance = (offset.length() - amount).max(0.5);
        self.position = self.target + offset.normalize() * distance;
    }

    /// Extract 6 frustum planes from the view-projection matrix.
    /// Each plane is [A, B, C, D] where Ax+By+Cz+D >= 0 is inside.
    pub fn frustum_planes(&self) -> [Vec4; 6] {
        let vp = self.view_projection();
        let row0 = Vec4::new(vp.x_axis.x, vp.y_axis.x, vp.z_axis.x, vp.w_axis.x);
        let row1 = Vec4::new(vp.x_axis.y, vp.y_axis.y, vp.z_axis.y, vp.w_axis.y);
        let row2 = Vec4::new(vp.x_axis.z, vp.y_axis.z, vp.z_axis.z, vp.w_axis.z);
        let row3 = Vec4::new(vp.x_axis.w, vp.y_axis.w, vp.z_axis.w, vp.w_axis.w);

        let mut planes = [
            row3 + row0, // left
            row3 - row0, // right
            row3 + row1, // bottom
            row3 - row1, // top
            row3 + row2, // near
            row3 - row2, // far
        ];

        // Normalize planes
        for p in &mut planes {
            let len = Vec3::new(p.x, p.y, p.z).length();
            if len > 0.0 {
                *p /= len;
            }
        }
        planes
    }

    /// Test if a sphere (center + radius) is inside or intersects the frustum.
    pub fn sphere_in_frustum(&self, planes: &[Vec4; 6], center: Vec3, radius: f32) -> bool {
        for plane in planes {
            let dist = plane.x * center.x + plane.y * center.y + plane.z * center.z + plane.w;
            if dist < -radius {
                return false;
            }
        }
        true
    }
}
