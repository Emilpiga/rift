//! Spawn + rate semantics for particle archetypes.

use crate::renderer::vfx::spec::{EmissionMode, SpawnShape};

/// How particles enter the world — separate from motion and colour.
#[derive(Clone, Copy, Debug)]
pub enum EmissionProfile {
    /// Single nucleus at the anchor (impact flash, ground ring core).
    PointNucleus,
    /// Omni burst from a sphere.
    SphereBurst { count: u32 },
    /// Ice / crystal shards.
    ShardBurst { count: u32 },
    /// Steady stream from a point.
    Continuous { rate: f32 },
    /// Hand-gather swirl (channeled beams).
    HandSwirl { rate: f32 },
    /// Trail core emitter.
    ProjectileCore { rate: f32 },
    /// Alpha smoke wake on a projectile.
    TrailVapor { rate: f32 },
}

impl EmissionProfile {
    /// `(spawn, emission, speed range, lifetime range)`.
    pub fn resolve(self) -> (SpawnShape, EmissionMode, (f32, f32), (f32, f32)) {
        match self {
            Self::PointNucleus => (
                SpawnShape::Point,
                EmissionMode::Burst { count: 1 },
                (0.0, 0.0),
                (0.09, 0.12),
            ),
            Self::SphereBurst { count } => (
                SpawnShape::Sphere,
                EmissionMode::Burst { count },
                (2.0, 5.0),
                (0.32, 0.58),
            ),
            Self::ShardBurst { count } => (
                SpawnShape::Sphere,
                EmissionMode::Burst { count },
                (4.0, 9.0),
                (0.26, 0.48),
            ),
            Self::Continuous { rate } => (
                SpawnShape::Sphere,
                EmissionMode::Continuous { rate },
                (0.3, 1.0),
                (0.14, 0.28),
            ),
            Self::HandSwirl { rate } => (
                SpawnShape::Sphere,
                EmissionMode::Continuous { rate },
                (0.3, 1.0),
                (0.25, 0.55),
            ),
            Self::ProjectileCore { rate } => (
                SpawnShape::Sphere,
                EmissionMode::Continuous { rate },
                (0.3, 1.0),
                (0.14, 0.26),
            ),
            Self::TrailVapor { rate } => (
                SpawnShape::Point,
                EmissionMode::Continuous { rate },
                (0.05, 0.4),
                (0.30, 0.55),
            ),
        }
    }
}
