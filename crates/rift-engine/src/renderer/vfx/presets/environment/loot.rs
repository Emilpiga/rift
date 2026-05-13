//! Loot-pillar presets — the rising rarity beam and its base
//! pulse anchor.

use glam::Vec3;

use crate::renderer::vfx::spec::*;

/// Rarity-tinted column rising from the drop point. Persistent
/// (`duration = 0.0`); the gameplay layer despawns the effect
/// when the loot is picked up.
///
/// Visual recipe — sharp HD silk pillar:
///
///   * Layer 1 (ground halo): wide low-opacity SoftGlow at
///     ankle height — bleeds rarity colour into surrounding
///     air so the beam doesn't sit on the world like a decal.
///   * Layer 2 (silk pillar): two or three overlapping
///     `SilkStrand` particles. The sprite is the entire
///     beam: a soft ethereal central body plus 5 sharp
///     sine-wave silk threads spiralling around it, all
///     tapering to pixel-width at the top and melting into
///     air. Per-particle seed offsets the thread phases so
///     overlapping particles produce a richer, less
///     repeating swirl.
///   * Layer 3 (tinting motes): sparse rising sparks for
///     life and rarity-colour readability at distance.
pub fn loot_beam(color: [f32; 3]) -> Effect {
    Effect {
        duration: 0.0,
        layers: vec![
            // ---- Ground halo ----
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Column {
                    radius: 0.45,
                    height: 0.35,
                    axis: Vec3::Y,
                },
                emission: EmissionMode::BurstAndContinuous {
                    burst: 5,
                    rate: 3.0,
                },
                speed: (0.0, 0.10),
                lifetime: (1.8, 3.0),
                forces: vec![ForceField::Drag { coefficient: 2.0 }],
                size: Curve::from_stops([(0.0, 0.55), (0.5, 0.70), (1.0, 0.55)]),
                color: Gradient::from_stops([
                    (0.0, [color[0] * 1.1, color[1] * 1.1, color[2] * 1.1, 0.0]),
                    (0.3, [color[0] * 1.2, color[1] * 1.2, color[2] * 1.2, 0.14]),
                    (0.7, [color[0] * 1.1, color[1] * 1.1, color[2] * 1.1, 0.14]),
                    (1.0, [color[0] * 0.8, color[1] * 0.8, color[2] * 0.8, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 0.30,
            }),
            // ---- Silk pillar ----
            // The sprite IS the beam. We spawn 2-3 overlapping
            // SilkStrand particles so different seed offsets
            // produce different thread phases — the visible
            // swirl combines all of them and reads as a
            // dense, evolving silk braid rather than a single
            // repeating sine pattern.
            //
            // Sized to give a tall visible pillar:
            // size 0.65 × (1 + stretch 8) = 5.85 m billboard,
            // anchored at base — visible beam rises ~5.5 m.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::BurstAndContinuous {
                    // Two concurrent overlapping pillars,
                    // refreshed slowly with cross-fade.
                    burst: 2,
                    rate: 1.0,
                },
                speed: (0.0, 0.0),
                lifetime: (2.5, 2.5),
                forces: vec![],
                // Constant size — the sprite handles all the
                // visual taper internally.
                size: Curve::from_stops([(0.0, 0.65), (1.0, 0.65)]),
                // Cross-fade in/out so the periodic respawn
                // is invisible. Subtle pre-mul — the beam
                // should feel ethereal, not laser-bright.
                color: Gradient::from_stops([
                    (0.00, [color[0] * 1.1, color[1] * 1.1, color[2] * 1.1, 0.0]),
                    (0.20, [color[0] * 1.4, color[1] * 1.4, color[2] * 1.4, 0.55]),
                    (0.80, [color[0] * 1.3, color[1] * 1.3, color[2] * 1.3, 0.55]),
                    (1.00, [color[0] * 0.9, color[1] * 0.9, color[2] * 0.9, 0.0]),
                ]),
                sprite: SpriteShape::SilkStrand,
                blend: BlendMode::Additive,
                opacity: 0.55,
            }),
            // ---- Tinting motes ----
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Column {
                    radius: 0.05,
                    height: 0.15,
                    axis: Vec3::Y,
                },
                emission: EmissionMode::BurstAndContinuous {
                    burst: 2,
                    rate: 8.0,
                },
                speed: (1.0, 1.8),
                lifetime: (0.9, 1.6),
                forces: vec![
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 1.6,
                    },
                    ForceField::Drag { coefficient: 1.2 },
                    ForceField::Orbit {
                        axis: Vec3::Y,
                        speed: 1.6,
                    },
                ],
                size: Curve::from_stops([(0.0, 0.04), (1.0, 0.015)]),
                color: Gradient::from_stops([
                    (0.0, [color[0] * 1.8, color[1] * 1.8, color[2] * 1.8, 1.0]),
                    (1.0, [color[0] * 0.3, color[1] * 0.3, color[2] * 0.3, 0.0]),
                ]),
                sprite: SpriteShape::Spark,
                blend: BlendMode::Additive,
                opacity: 0.5,
            }),
        ],
    }
}

/// Loot pillar base pulse — a soft glow at the drop's feet.
/// Used in tandem with [`loot_beam`] for a clearer ground anchor.
pub fn loot_beam_base(color: [f32; 3]) -> Effect {
    Effect {
        duration: 0.0,
        layers: vec![Layer::Particles(ParticleSpec {
            spawn: SpawnShape::Sphere,
            emission: EmissionMode::BurstAndContinuous {
                burst: 6,
                rate: 25.0,
            },
            speed: (0.5, 1.5),
            lifetime: (0.3, 0.8),
            forces: vec![ForceField::Drag { coefficient: 2.0 }],
            size: Curve::from_stops([(0.0, 0.07), (1.0, 0.14)]),
            color: Gradient::from_stops([
                (0.0, [color[0] * 2.5, color[1] * 2.5, color[2] * 2.5, 1.0]),
                (1.0, [color[0] * 0.5, color[1] * 0.5, color[2] * 0.5, 0.0]),
            ]),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Additive,
            opacity: 1.0,
        })],
    }
}

/// Anchored-loot halo: a slow gold-cyan ring orbiting the drop's
/// base. Spawned on top of [`loot_beam`] / [`loot_beam_base`] so
/// rarity stays readable while the unique trait is unmistakable
/// at gameplay distance. Persistent — caller despawns on pickup.
pub fn loot_anchored_halo() -> Effect {
    Effect {
        duration: 0.0,
        layers: vec![Layer::Particles(ParticleSpec {
            // Wide low ring so the halo reads as "anchor" and
            // doesn't compete with the vertical beam above.
            spawn: SpawnShape::Ring {
                radius: 0.6,
                thickness: 0.12,
            },
            emission: EmissionMode::BurstAndContinuous {
                burst: 12,
                rate: 60.0,
            },
            speed: (0.2, 0.5),
            lifetime: (1.2, 2.0),
            forces: vec![
                ForceField::Drag { coefficient: 1.5 },
                ForceField::Orbit {
                    axis: Vec3::Y,
                    speed: 1.4,
                },
            ],
            size: Curve::from_stops([(0.0, 0.10), (1.0, 0.04)]),
            // Gold → cyan gradient picks the trait out from any
            // rarity beam. Bright pre-multiply so the bloom
            // pass actually catches it.
            color: Gradient::from_stops([
                (0.0, [3.5, 2.6, 0.6, 1.0]),
                (0.5, [1.2, 2.4, 3.0, 1.0]),
                (1.0, [0.2, 0.6, 1.0, 0.0]),
            ]),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Additive,
            opacity: 1.0,
        })],
    }
}
