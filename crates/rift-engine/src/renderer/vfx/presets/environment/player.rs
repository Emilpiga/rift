//! Player-presence transition presets.
//!
//! Effects fired when a remote player's avatar enters or leaves
//! the world for non-gameplay reasons (logout, disconnect,
//! party split). The intent is purely cosmetic — gameplay
//! despawn paths (death, ghost form) have their own cues
//! (`enemy_soul_return`, `ghost_rise`).

use glam::Vec3;

use crate::renderer::vfx::spec::*;

/// "Rapture" — played at a remote player's last known position
/// the moment their avatar despawns due to a disconnect or
/// logout. Reads as the body bleaching to white, condensing
/// into a brilliant point, and being shot skyward by a holy
/// pillar of light, so other players see them leaving
/// deliberately rather than blink out of existence.
///
/// Built around the same `SilkStrand` sprite the loot beam
/// uses, so the silhouette is unmistakably a *beam* rather
/// than a particle puff. Layers, in render / read order:
///
/// 1. **Body flash** — a single bright spherical pop centred
///    on the avatar's torso. Sells the "body bleaches white"
///    moment and visually swallows the silhouette on the
///    despawn frame.
/// 2. **Ascending pillar** — overlapping `SilkStrand` particles
///    spawned at the body's centre with a strong upward
///    inertial bias. The sprite stretches several times its
///    spawn size along its motion axis, giving a tall vertical
///    bolt that rises and fades over ~0.7 s. Pure-white HDR.
/// 3. **Rising sparks** — fast additive sparks streaking up
///    along the column for high-frequency twinkle and to
///    sell the "shoots up" motion at distance.
/// 4. **Ground halo** — short SoftGlow flare at the feet so
///    the beam reads as anchored to the world rather than
///    floating in mid-air.
pub fn player_rapture() -> Effect {
    Effect {
        // Spawn-side duration. Particle lifetimes outlive this
        // so the beam continues rising after emission stops.
        duration: 0.05,
        layers: vec![
            // 1. Body flash — short HDR burst that engulfs the
            //    avatar silhouette on the despawn frame.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 14 },
                speed: (0.4, 1.6),
                lifetime: (0.30, 0.55),
                forces: vec![ForceField::Drag { coefficient: 4.0 }],
                size: Curve::from_stops([(0.00, 0.85), (0.30, 1.55), (1.00, 0.50)]),
                color: Gradient::from_stops([
                    (0.00, [4.5, 4.8, 5.2, 1.00]),
                    (0.50, [2.4, 2.7, 3.1, 0.75]),
                    (1.00, [0.40, 0.50, 0.70, 0.00]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 2. Ascending pillar — the beam itself.
            //    `SilkStrand` is the same sprite the loot
            //    beam uses; here we want a one-shot bolt
            //    rather than a persistent column, so we burst
            //    a few overlapping strands at the avatar's
            //    centre with strong upward velocity. The
            //    billboard orients along the spawn velocity,
            //    so the strand's long axis points at the sky.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Point,
                emission: EmissionMode::Burst { count: 3 },
                // Fast upward shot. Speed is the magnitude of
                // the spawn velocity vector; the gravity force
                // below adds further upward acceleration.
                speed: (12.0, 16.0),
                lifetime: (0.60, 0.80),
                forces: vec![
                    // Extra acceleration upward — the bolt
                    // visibly *accelerates* toward the sky
                    // rather than coasting at spawn speed.
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 22.0,
                    },
                ],
                // Constant size — the sprite's internal taper
                // handles the visual narrowing at the top.
                size: Curve::from_stops([(0.0, 0.70), (1.0, 0.70)]),
                // Cross-fade so the strand pops in, holds
                // bright through its rise, then dissolves.
                color: Gradient::from_stops([
                    (0.00, [4.5, 4.8, 5.5, 0.00]),
                    (0.10, [5.5, 5.8, 6.5, 0.95]),
                    (0.70, [3.5, 4.0, 5.0, 0.80]),
                    (1.00, [0.6, 0.9, 1.4, 0.00]),
                ]),
                sprite: SpriteShape::SilkStrand,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 3. Rising sparks — fast additive Spark sprites
            //    streaking up alongside the pillar so the
            //    upward motion reads at distance and through
            //    fog, even when the SilkStrand strands are
            //    behind walls.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Column {
                    radius: 0.18,
                    height: 0.40,
                    axis: Vec3::Y,
                },
                emission: EmissionMode::Burst { count: 60 },
                speed: (8.0, 14.0),
                lifetime: (0.45, 0.75),
                forces: vec![
                    ForceField::Gravity {
                        axis: Vec3::Y,
                        strength: 16.0,
                    },
                    ForceField::Drag { coefficient: 0.4 },
                ],
                size: Curve::from_stops([(0.00, 0.10), (0.70, 0.06), (1.00, 0.0)]),
                color: Gradient::from_stops([
                    (0.00, [5.5, 5.8, 6.0, 1.00]),
                    (0.50, [3.0, 3.3, 3.8, 0.85]),
                    (1.00, [0.6, 0.8, 1.2, 0.00]),
                ]),
                sprite: SpriteShape::Spark,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // 4. Ground halo — a low, soft glow at the feet so
            //    the beam reads as rooted to the floor at the
            //    moment it fires, not floating mid-air. Brief
            //    so it doesn't outlive the bolt.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Disc { radius: 0.55 },
                emission: EmissionMode::Burst { count: 18 },
                speed: (0.0, 0.4),
                lifetime: (0.25, 0.45),
                forces: vec![ForceField::Drag { coefficient: 3.0 }],
                size: Curve::from_stops([(0.0, 0.55), (1.0, 0.85)]),
                color: Gradient::from_stops([
                    (0.00, [4.0, 4.4, 5.0, 0.90]),
                    (1.00, [0.40, 0.55, 0.80, 0.00]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
        ],
    }
}
