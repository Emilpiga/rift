//! Loot-pillar layers (rarity-scaled).

use glam::Vec3;
use rift_game::loot::Rarity;

use crate::renderer::vfx::spec::*;

struct LootPillarProfile {
    silk_size: f32,
    halo_size: f32,
    halo_radius: f32,
    base_size: f32,
    silk_opacity: f32,
    halo_opacity: f32,
    base_opacity: f32,
    mote_opacity: f32,
    color_gain: f32,
    mote_rate: f32,
    silk_burst: u32,
}

fn profile(rarity: Rarity) -> LootPillarProfile {
    match rarity {
        Rarity::Common => LootPillarProfile {
            silk_size: 0.38,
            halo_size: 0.42,
            halo_radius: 0.32,
            base_size: 0.85,
            silk_opacity: 0.28,
            halo_opacity: 0.18,
            base_opacity: 0.45,
            mote_opacity: 0.22,
            color_gain: 0.75,
            mote_rate: 4.0,
            silk_burst: 1,
        },
        Rarity::Magic => LootPillarProfile {
            silk_size: 0.52,
            halo_size: 0.50,
            halo_radius: 0.38,
            base_size: 0.95,
            silk_opacity: 0.42,
            halo_opacity: 0.24,
            base_opacity: 0.65,
            mote_opacity: 0.35,
            color_gain: 0.95,
            mote_rate: 6.0,
            silk_burst: 2,
        },
        Rarity::Rare => LootPillarProfile {
            silk_size: 0.65,
            halo_size: 0.55,
            halo_radius: 0.45,
            base_size: 1.0,
            silk_opacity: 0.55,
            halo_opacity: 0.30,
            base_opacity: 0.85,
            mote_opacity: 0.50,
            color_gain: 1.15,
            mote_rate: 8.0,
            silk_burst: 2,
        },
        Rarity::Legendary => LootPillarProfile {
            silk_size: 0.82,
            halo_size: 0.68,
            halo_radius: 0.55,
            base_size: 1.25,
            silk_opacity: 0.72,
            halo_opacity: 0.38,
            base_opacity: 1.0,
            mote_opacity: 0.62,
            color_gain: 1.45,
            mote_rate: 11.0,
            silk_burst: 3,
        },
    }
}

fn hdr(color: [f32; 3], gain: f32) -> [f32; 3] {
    [color[0] * gain, color[1] * gain, color[2] * gain]
}

pub fn loot_beam_layers(rarity: Rarity) -> Vec<Layer> {
    let p = profile(rarity);
    let color = hdr(rarity.color(), p.color_gain);
    vec![
        Layer::Particles(ParticleSpec {
            spawn: SpawnShape::Column {
                radius: p.halo_radius,
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
            size: Curve::from_stops([
                (0.0, p.halo_size * 0.85),
                (0.5, p.halo_size),
                (1.0, p.halo_size * 0.85),
            ]),
            color: Gradient::from_stops([
                (0.0, [color[0] * 1.1, color[1] * 1.1, color[2] * 1.1, 0.0]),
                (0.3, [color[0] * 1.2, color[1] * 1.2, color[2] * 1.2, 0.14]),
                (0.7, [color[0] * 1.1, color[1] * 1.1, color[2] * 1.1, 0.14]),
                (1.0, [color[0] * 0.8, color[1] * 0.8, color[2] * 0.8, 0.0]),
            ]),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Additive,
            opacity: p.halo_opacity,
            hybrid: None,
        vfx_role: 0,
    }),
        Layer::Particles(ParticleSpec {
            spawn: SpawnShape::Point,
            emission: EmissionMode::BurstAndContinuous {
                burst: p.silk_burst,
                rate: 1.0,
            },
            speed: (0.0, 0.0),
            lifetime: (2.5, 2.5),
            forces: vec![],
            size: Curve::from_stops([(0.0, p.silk_size), (1.0, p.silk_size)]),
            color: Gradient::from_stops([
                (0.00, [color[0] * 1.1, color[1] * 1.1, color[2] * 1.1, 0.0]),
                (0.20, [color[0] * 1.4, color[1] * 1.4, color[2] * 1.4, 0.55]),
                (0.80, [color[0] * 1.3, color[1] * 1.3, color[2] * 1.3, 0.55]),
                (1.00, [color[0] * 0.9, color[1] * 0.9, color[2] * 0.9, 0.0]),
            ]),
            sprite: SpriteShape::SilkStrand,
            blend: BlendMode::Additive,
            opacity: p.silk_opacity,
            hybrid: None,
        vfx_role: 0,
    }),
        Layer::Particles(ParticleSpec {
            spawn: SpawnShape::Column {
                radius: 0.05,
                height: 0.15,
                axis: Vec3::Y,
            },
            emission: EmissionMode::BurstAndContinuous {
                burst: 2,
                rate: p.mote_rate,
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
            opacity: p.mote_opacity,
            hybrid: None,
        vfx_role: 0,
    }),
    ]
}

pub fn loot_beam_base_layer(rarity: Rarity) -> Layer {
    let p = profile(rarity);
    let color = hdr(rarity.color(), p.color_gain);
    Layer::Particles(ParticleSpec {
        spawn: SpawnShape::Sphere,
        emission: EmissionMode::BurstAndContinuous {
            burst: 6,
            rate: 25.0 * p.base_size,
        },
        speed: (0.5, 1.5),
        lifetime: (0.3, 0.8),
        forces: vec![ForceField::Drag { coefficient: 2.0 }],
        size: Curve::from_stops([
            (0.0, 0.07 * p.base_size),
            (1.0, 0.14 * p.base_size),
        ]),
        color: Gradient::from_stops([
            (0.0, [color[0] * 2.5, color[1] * 2.5, color[2] * 2.5, 1.0]),
            (1.0, [color[0] * 0.5, color[1] * 0.5, color[2] * 0.5, 0.0]),
        ]),
        sprite: SpriteShape::SoftGlow,
        blend: BlendMode::Additive,
        opacity: p.base_opacity,
        hybrid: None,
        vfx_role: 0,
    })
}
