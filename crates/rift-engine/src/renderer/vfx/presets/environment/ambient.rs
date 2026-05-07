//! Ambient world-prop presets — wall torches and other
//! always-on environment effects.

use glam::Vec3;

use crate::renderer::vfx::spec::*;

/// Wall-torch flame — a continuous, looping fire plume that sits
/// on a wall sconce. Three stacked layers compose the look:
///
/// 1. **Core flame**: short-lived bright HDR additive particles
///    rising fast. Keeps the flame's silhouette tight and drives
///    the bloom highlight.
/// 2. **Outer flame**: longer-lived softer particles that drift
///    upward and outward, giving the flame visible volume.
/// 3. **Smoke wisp**: dim translucent puff that lingers above
///    the flame, fading to nothing.
///
/// The effect is `duration: 0.0` (infinite) — gameplay code
/// despawns it when the floor changes. All forces are vertical
/// so the flame stays anchored to its wall position; the
/// `Wind` force adds a tiny upward bias so even slow particles
/// rise reliably.
pub fn wall_torch() -> Effect {
    Effect {
        duration: 0.0,
        layers: vec![
            // Core flame — small, very bright, short life.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Disc { radius: 0.04 },
                emission: EmissionMode::Continuous { rate: 55.0 },
                speed: (1.6, 2.4),
                lifetime: (0.18, 0.30),
                forces: vec![
                    // Upward acceleration so flames lick higher
                    // as they age (negative gravity along Y).
                    ForceField::Gravity { axis: Vec3::Y, strength: 4.5 },
                    ForceField::Drag { coefficient: 1.5 },
                    // Subtle curl gives the flame its dancing
                    // silhouette without expensive simulation.
                    ForceField::Curl { frequency: 4.0, strength: 1.6 },
                ],
                size: Curve::from_stops([
                    (0.00, 0.10),
                    (0.30, 0.16),
                    (1.00, 0.02),
                ]),
                // HDR amber → orange → dim red. Bright enough at
                // birth (~3-4×) to drive bloom; tonemap brings
                // the visible colour back to a clean orange.
                color: Gradient::from_stops([
                    (0.00, [4.5, 2.4, 0.6, 1.00]),
                    (0.40, [3.0, 1.0, 0.2, 1.00]),
                    (1.00, [0.6, 0.1, 0.0, 0.00]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // Outer flame — wider, dimmer, longer-lived. Reads
            // as the flame's volume / aura.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Disc { radius: 0.09 },
                emission: EmissionMode::Continuous { rate: 35.0 },
                speed: (0.7, 1.2),
                lifetime: (0.35, 0.55),
                forces: vec![
                    ForceField::Gravity { axis: Vec3::Y, strength: 2.8 },
                    ForceField::Drag { coefficient: 1.2 },
                    ForceField::Curl { frequency: 2.5, strength: 1.0 },
                ],
                size: Curve::from_stops([
                    (0.00, 0.16),
                    (0.40, 0.22),
                    (1.00, 0.04),
                ]),
                color: Gradient::from_stops([
                    (0.00, [2.5, 1.0, 0.20, 0.85]),
                    (0.50, [1.4, 0.45, 0.10, 0.55]),
                    (1.00, [0.30, 0.08, 0.02, 0.00]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            }),
            // Smoke wisp — slow rising dim grey puff. Alpha-
            // blended so it can sit above the additive flame
            // without blowing out.
            Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Disc { radius: 0.08 },
                emission: EmissionMode::Continuous { rate: 6.0 },
                speed: (0.25, 0.45),
                lifetime: (1.2, 1.8),
                forces: vec![
                    ForceField::Gravity { axis: Vec3::Y, strength: 1.0 },
                    ForceField::Drag { coefficient: 0.6 },
                    ForceField::Curl { frequency: 1.2, strength: 0.6 },
                ],
                size: Curve::from_stops([
                    (0.00, 0.10),
                    (0.50, 0.30),
                    (1.00, 0.55),
                ]),
                color: Gradient::from_stops([
                    (0.00, [0.10, 0.09, 0.08, 0.40]),
                    (0.40, [0.08, 0.07, 0.06, 0.20]),
                    (1.00, [0.04, 0.04, 0.04, 0.00]),
                ]),
                sprite: SpriteShape::Smoke,
                blend: BlendMode::Alpha,
                opacity: 1.0,
            }),
        ],
    }
}
