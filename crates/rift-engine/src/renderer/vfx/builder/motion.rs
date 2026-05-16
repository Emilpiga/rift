//! Force / lifetime motion signatures for particle archetypes.

use glam::Vec3;

use crate::renderer::vfx::spec::ForceField;

/// Composable motion signature — becomes [`ForceField`] list at bake time.
#[derive(Clone, Debug)]
pub struct MotionProfile {
    pub drag: f32,
    /// Positive = upward gravity bias along +Y.
    pub lift: f32,
    /// Downward gravity strength (shard falls).
    pub gravity_down: f32,
    pub curl: Option<(f32, f32)>,
    pub turbulence: Option<(f32, f32)>,
    pub orbit: Option<(Vec3, f32)>,
    pub extra: Vec<ForceField>,
}

impl MotionProfile {
    pub fn still() -> Self {
        Self {
            drag: 0.0,
            lift: 0.0,
            gravity_down: 0.0,
            curl: None,
            turbulence: None,
            orbit: None,
            extra: vec![],
        }
    }

    pub fn void_frost_cloud() -> Self {
        Self {
            drag: 4.0,
            lift: 1.0,
            gravity_down: 0.0,
            curl: None,
            turbulence: None,
            orbit: None,
            extra: vec![],
        }
    }

    pub fn void_frost_shards() -> Self {
        Self {
            drag: 1.4,
            lift: 0.0,
            gravity_down: 4.0,
            curl: None,
            turbulence: None,
            orbit: None,
            extra: vec![],
        }
    }

    pub fn void_frost_beam_tick() -> Self {
        Self {
            drag: 3.0,
            lift: 0.0,
            gravity_down: 0.0,
            curl: None,
            turbulence: None,
            orbit: None,
            extra: vec![],
        }
    }

    pub fn void_frost_hand_swirl() -> Self {
        Self {
            drag: 3.0,
            lift: 1.5,
            gravity_down: 0.0,
            curl: None,
            turbulence: None,
            orbit: Some((Vec3::Y, 6.0)),
            extra: vec![],
        }
    }

    pub fn void_frost_trail_vapor() -> Self {
        Self {
            drag: 2.5,
            lift: 0.0,
            gravity_down: 0.0,
            curl: None,
            turbulence: None,
            orbit: None,
            extra: vec![],
        }
    }

    pub fn ember_cloud() -> Self {
        Self {
            drag: 4.0,
            lift: 0.0,
            gravity_down: 0.0,
            curl: Some((0.85, 6.5)),
            turbulence: Some((1.4, 2.2)),
            orbit: None,
            extra: vec![],
        }
    }

    pub fn ember_beam_tick() -> Self {
        Self {
            drag: 5.0,
            lift: 0.0,
            gravity_down: 0.0,
            curl: None,
            turbulence: None,
            orbit: None,
            extra: vec![],
        }
    }

    pub fn ember_filaments() -> Self {
        Self {
            drag: 1.15,
            lift: 0.0,
            gravity_down: 12.0,
            curl: None,
            turbulence: None,
            orbit: None,
            extra: vec![],
        }
    }

    pub fn ember_hand_swirl() -> Self {
        Self {
            drag: 3.0,
            lift: 1.5,
            gravity_down: 0.0,
            curl: None,
            turbulence: None,
            orbit: Some((Vec3::Y, 6.0)),
            extra: vec![],
        }
    }

    pub fn ember_trail_vapor() -> Self {
        Self {
            drag: 9.0,
            lift: 0.0,
            gravity_down: 0.0,
            curl: None,
            turbulence: None,
            orbit: None,
            extra: vec![],
        }
    }

    pub fn arcane_cloud() -> Self {
        Self {
            drag: 4.0,
            lift: 1.2,
            gravity_down: 0.0,
            curl: None,
            turbulence: None,
            orbit: None,
            extra: vec![],
        }
    }

    pub fn arcane_beam_tick() -> Self {
        Self {
            drag: 5.0,
            lift: 0.0,
            gravity_down: 0.0,
            curl: None,
            turbulence: None,
            orbit: None,
            extra: vec![],
        }
    }

    pub fn arcane_filaments() -> Self {
        Self {
            drag: 1.2,
            lift: 0.0,
            gravity_down: 0.0,
            curl: None,
            turbulence: None,
            orbit: None,
            extra: vec![],
        }
    }

    pub fn arcane_trail_vapor() -> Self {
        Self {
            drag: 2.5,
            lift: 0.0,
            gravity_down: 0.0,
            curl: None,
            turbulence: None,
            orbit: None,
            extra: vec![],
        }
    }

    pub fn into_forces(self) -> Vec<ForceField> {
        let mut forces = Vec::new();
        if self.drag > 0.0 {
            forces.push(ForceField::Drag {
                coefficient: self.drag,
            });
        }
        if self.lift.abs() > 1e-4 {
            forces.push(ForceField::Gravity {
                axis: Vec3::Y,
                strength: self.lift,
            });
        }
        if self.gravity_down > 0.0 {
            forces.push(ForceField::Gravity {
                axis: -Vec3::Y,
                strength: self.gravity_down,
            });
        }
        if let Some((axis, speed)) = self.orbit {
            forces.push(ForceField::Orbit { axis, speed });
        }
        if let Some((frequency, strength)) = self.curl {
            forces.push(ForceField::Curl {
                frequency,
                strength,
            });
        }
        if let Some((frequency, strength)) = self.turbulence {
            forces.push(ForceField::Turbulence {
                frequency,
                strength,
            });
        }
        forces.extend(self.extra);
        forces
    }
}
