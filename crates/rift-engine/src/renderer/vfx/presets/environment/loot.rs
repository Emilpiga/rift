//! Loot-pillar presets — the rising rarity beam and its base
//! pulse anchor.

use glam::Vec3;
use rift_game::loot::Rarity;

use crate::renderer::vfx::builder::{loot_beam_base_layer, loot_beam_layers, EffectBuilder};
use crate::renderer::vfx::spec::*;

/// Rarity-tinted column rising from the drop point. Persistent
/// (`duration = 0.0`); the gameplay layer despawns the effect
/// when the loot is picked up.
pub fn loot_beam(rarity: Rarity) -> Effect {
    EffectBuilder::persistent()
        .layers(loot_beam_layers(rarity))
        .finish()
}

/// Loot pillar base pulse — a soft glow at the drop's feet.
pub fn loot_beam_base(rarity: Rarity) -> Effect {
    EffectBuilder::persistent()
        .layer(loot_beam_base_layer(rarity))
        .finish()
}

/// Anchored-loot halo: a slow gold-cyan ring orbiting the drop's
/// base. Spawned on top of [`loot_beam`] / [`loot_beam_base`].
pub fn loot_anchored_halo() -> Effect {
    EffectBuilder::persistent()
        .particle(ParticleSpec {
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
            color: Gradient::from_stops([
                (0.0, [3.5, 2.6, 0.6, 1.0]),
                (0.5, [1.2, 2.4, 3.0, 1.0]),
                (1.0, [0.2, 0.6, 1.0, 0.0]),
            ]),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Additive,
            opacity: 1.0,
            hybrid: None,
        vfx_role: 0,
    })
        .finish()
}
