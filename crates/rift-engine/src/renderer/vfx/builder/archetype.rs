//! Semantic particle archetypes — emission + motion + visual role per style preset.

use super::emission::EmissionProfile;
use super::motion::MotionProfile;
use glam::Vec3;

use super::chunks::shockwave_spec;
use super::{
    continuous, projectile_trail_arcane, projectile_trail_fire, ContinuousOpts,
    FlashOpts, PlasmaCoreOpts, RadialBurstOpts, ShardBurstOpts, ShockwaveOpts, SmokeBillowOpts,
    SmokeResidueOpts, SparkBurstOpts, SphereBurstOpts, StreakBurstOpts,
};
use crate::renderer::vfx::spec::*;
use crate::renderer::vfx::style::StylePreset;

/// Semantic sprite role (authoring); maps to a concrete [`SpriteShape`] at bake time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VfxRole {
    Core,
    Filament,
    Rupture,
    Vapor,
    Impact,
    Residue,
}

impl VfxRole {
    /// Default procedural sprite when a layer does not override [`ParticleArchetype::sprite`].
    pub fn default_sprite(self) -> SpriteShape {
        match self {
            Self::Core | Self::Impact => SpriteShape::SoftGlow,
            Self::Filament => SpriteShape::Spark,
            Self::Rupture => SpriteShape::Ring,
            Self::Vapor | Self::Residue => SpriteShape::Smoke,
        }
    }

    /// Packed into `role_pack.x` — must match `evaluate_role.glsl` role constants.
    pub fn gpu_role_id(self) -> f32 {
        match self {
            Self::Core => 1.0,
            Self::Filament => 2.0,
            Self::Rupture => 3.0,
            Self::Vapor => 4.0,
            Self::Impact => 5.0,
            Self::Residue => 6.0,
        }
    }

    pub fn gpu_role_id_u8(self) -> u8 {
        self.gpu_role_id() as u8
    }
}

/// One particle layer described by intent, not by shader branch index.
#[derive(Clone, Debug)]
pub struct ParticleArchetype {
    pub role: VfxRole,
    pub emission: EmissionProfile,
    pub motion: MotionProfile,
    pub sprite: SpriteShape,
    pub blend: BlendMode,
    pub size: Curve,
    pub color: Gradient,
    pub opacity: f32,
    pub hybrid: Option<HybridMaterial>,
    /// Overrides [`EmissionProfile::resolve`] speed when set.
    pub speed: Option<(f32, f32)>,
    /// Overrides [`EmissionProfile::resolve`] lifetime when set.
    pub lifetime: Option<(f32, f32)>,
    /// Overrides spawn shape from [`EmissionProfile::resolve`] when set.
    pub spawn: Option<SpawnShape>,
    /// When true, [`sprite`] is replaced by [`VfxRole::default_sprite`] at bake time.
    pub use_role_sprite: bool,
}

impl ParticleArchetype {
    /// Phase 4 entry — sprite and role stay aligned unless `sprite` is set explicitly.
    pub fn for_role(
        role: VfxRole,
        emission: EmissionProfile,
        motion: MotionProfile,
        blend: BlendMode,
        size: Curve,
        color: Gradient,
        opacity: f32,
    ) -> Self {
        Self {
            role,
            emission,
            motion,
            sprite: role.default_sprite(),
            blend,
            size,
            color,
            opacity,
            hybrid: None,
            speed: None,
            lifetime: None,
            spawn: None,
            use_role_sprite: true,
        }
    }

    pub fn into_layer(self) -> Layer {
        let (default_spawn, emission, default_speed, default_lifetime) = self.emission.resolve();
        let spawn = self.spawn.unwrap_or(default_spawn);
        let speed = self.speed.unwrap_or(default_speed);
        let lifetime = self.lifetime.unwrap_or(default_lifetime);
        let sprite = if self.use_role_sprite {
            self.role.default_sprite()
        } else {
            self.sprite
        };
        Layer::Particles(
            ParticleSpec {
                spawn,
                emission,
                speed,
                lifetime,
                forces: self.motion.into_forces(),
                size: self.size,
                color: self.color,
                sprite,
                blend: self.blend,
                opacity: self.opacity,
                hybrid: self.hybrid,
                vfx_role: self.role.gpu_role_id_u8(),
            },
        )
    }

    pub fn void_frost_impact_flash() -> Self {
        let o = FlashOpts::frost();
        Self {
            role: VfxRole::Impact,
            emission: EmissionProfile::PointNucleus,
            motion: MotionProfile::still(),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Additive,
            size: o.size,
            color: o.color,
            opacity: o.opacity,
            hybrid: None,
            speed: Some((0.0, 0.0)),
            lifetime: Some(o.lifetime),
            spawn: None,
            use_role_sprite: false,
        }
    }

    pub fn void_frost_vapor_cloud() -> Self {
        from_sphere(SphereBurstOpts::frost_cloud(), VfxRole::Vapor, MotionProfile::void_frost_cloud())
    }

    pub fn void_frost_filaments() -> Self {
        from_shard(ShardBurstOpts::frost(), VfxRole::Filament, MotionProfile::void_frost_shards())
    }

    pub fn void_frost_beam_tick_cloud() -> Self {
        from_sphere(
            SphereBurstOpts {
                count: 14,
                speed: (1.0, 3.0),
                lifetime: (0.25, 0.45),
                size: Curve::from_stops([(0.0, 0.18), (1.0, 0.05)]),
                color: Gradient::from_stops([
                    (0.0, [3.0, 5.5, 7.0, 0.9]),
                    (1.0, [0.2, 0.4, 0.6, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
                lift: 0.0,
                drag: 5.0,
                forces_extra: vec![],
            },
            VfxRole::Core,
            MotionProfile::void_frost_beam_tick(),
        )
    }

    pub fn void_frost_beam_tick_filaments() -> Self {
        from_shard(
            ShardBurstOpts::frost_tick(),
            VfxRole::Filament,
            MotionProfile::void_frost_beam_tick(),
        )
    }

    pub fn void_frost_hand_swirl() -> Self {
        Self {
            role: VfxRole::Core,
            emission: EmissionProfile::HandSwirl { rate: 60.0 },
            motion: MotionProfile::void_frost_hand_swirl(),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Additive,
            size: Curve::from_stops([(0.00, 0.10), (0.40, 0.14), (1.00, 0.0)]),
            color: Gradient::from_stops([
                (0.00, [1.5, 3.0, 4.5, 1.0]),
                (0.50, [0.6, 1.4, 2.2, 0.7]),
                (1.00, [0.2, 0.4, 0.6, 0.0]),
            ]),
            opacity: 1.0,
            hybrid: None,
            speed: None,
            lifetime: None,
            spawn: None,
            use_role_sprite: false,
        }
    }

    pub fn void_frost_trail_core() -> Self {
        Self {
            role: VfxRole::Core,
            emission: EmissionProfile::ProjectileCore { rate: 160.0 },
            motion: MotionProfile {
                drag: 5.0,
                lift: 0.0,
                gravity_down: 0.0,
                curl: None,
                turbulence: None,
                orbit: None,
                extra: vec![],
            },
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Additive,
            size: Curve::from_stops([(0.00, 0.16), (0.30, 0.12), (1.00, 0.02)]),
            color: Gradient::from_stops([
                (0.00, [2.4, 4.6, 6.0, 1.0]),
                (0.40, [0.7, 1.8, 2.8, 0.85]),
                (1.00, [0.10, 0.25, 0.45, 0.0]),
            ]),
            opacity: 1.0,
            hybrid: None,
            speed: None,
            lifetime: None,
            spawn: None,
            use_role_sprite: false,
        }
    }

    pub fn void_frost_trail_vapor() -> Self {
        Self {
            role: VfxRole::Vapor,
            emission: EmissionProfile::TrailVapor { rate: 35.0 },
            motion: MotionProfile::void_frost_trail_vapor(),
            sprite: SpriteShape::Smoke,
            blend: BlendMode::Alpha,
            size: Curve::from_stops([(0.00, 0.12), (1.00, 0.28)]),
            color: Gradient::from_stops([
                (0.00, [0.7, 1.0, 1.4, 0.5]),
                (0.50, [0.20, 0.35, 0.55, 0.28]),
                (1.00, [0.04, 0.06, 0.12, 0.0]),
            ]),
            opacity: 1.0,
            hybrid: None,
            speed: None,
            lifetime: None,
            spawn: None,
            use_role_sprite: false,
        }
    }

    pub fn ember_impact_flash() -> Self {
        let o = FlashOpts::fireball();
        Self {
            role: VfxRole::Impact,
            emission: EmissionProfile::PointNucleus,
            motion: MotionProfile::still(),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Additive,
            size: o.size,
            color: o.color,
            opacity: o.opacity,
            hybrid: None,
            speed: Some((0.0, 0.0)),
            lifetime: Some(o.lifetime),
            spawn: None,
            use_role_sprite: false,
        }
    }

    pub fn ember_detonation_plasma() -> Self {
        let o = PlasmaCoreOpts::fireball();
        Self {
            role: VfxRole::Core,
            emission: EmissionProfile::SphereBurst { count: o.count },
            motion: MotionProfile {
                drag: 4.0,
                lift: 0.0,
                gravity_down: 3.0,
                curl: Some((0.9, 14.0)),
                turbulence: None,
                orbit: None,
                extra: vec![],
            },
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Additive,
            size: o.size,
            color: o.color,
            opacity: o.opacity,
            hybrid: None,
            speed: Some(o.speed),
            lifetime: Some(o.lifetime),
            spawn: None,
            use_role_sprite: false,
        }
    }

    pub fn ember_detonation_smoke() -> Self {
        from_smoke_billow(SmokeBillowOpts::fireball_cloud(), VfxRole::Vapor)
    }

    pub fn ember_detonation_embers() -> Self {
        from_streak(StreakBurstOpts::fireball_embers(), VfxRole::Filament)
    }

    pub fn arcane_impact_flash() -> Self {
        let o = FlashOpts::arcane();
        Self {
            role: VfxRole::Impact,
            emission: EmissionProfile::PointNucleus,
            motion: MotionProfile::still(),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Additive,
            size: o.size,
            color: o.color,
            opacity: o.opacity,
            hybrid: None,
            speed: Some((0.0, 0.0)),
            lifetime: Some(o.lifetime),
            spawn: None,
            use_role_sprite: false,
        }
    }

    pub fn arcane_detonation_cloud() -> Self {
        from_sphere(
            SphereBurstOpts::arcane_cloud(),
            VfxRole::Vapor,
            MotionProfile::arcane_cloud(),
        )
    }

    pub fn arcane_detonation_sparks() -> Self {
        from_spark(
            SparkBurstOpts::arcane(),
            VfxRole::Filament,
            MotionProfile {
                drag: 1.4,
                lift: 0.0,
                gravity_down: 4.0,
                curl: None,
                turbulence: None,
                orbit: None,
                extra: vec![],
            },
        )
    }

    pub fn ember_beam_tick_cloud() -> Self {
        from_sphere(
            SphereBurstOpts {
                count: 14,
                speed: (1.0, 3.0),
                lifetime: (0.25, 0.45),
                size: Curve::from_stops([(0.0, 0.18), (1.0, 0.05)]),
                color: Gradient::from_stops([
                    (0.0, [6.0, 2.8, 0.7, 0.9]),
                    (1.0, [0.6, 0.10, 0.02, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
                lift: 0.0,
                drag: 5.0,
                forces_extra: vec![],
            },
            VfxRole::Core,
            MotionProfile::ember_beam_tick(),
        )
    }

    pub fn ember_beam_tick_shards() -> Self {
        from_shard(
            ShardBurstOpts::fire_tick(),
            VfxRole::Filament,
            MotionProfile::ember_filaments(),
        )
    }

    pub fn arcane_beam_tick_cloud() -> Self {
        from_sphere(
            SphereBurstOpts {
                count: 14,
                speed: (1.0, 3.0),
                lifetime: (0.25, 0.45),
                size: Curve::from_stops([(0.0, 0.18), (1.0, 0.05)]),
                color: Gradient::from_stops([
                    (0.0, [3.6, 1.4, 4.6, 0.9]),
                    (1.0, [0.1, 0.04, 0.30, 0.0]),
                ]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
                lift: 0.0,
                drag: 5.0,
                forces_extra: vec![],
            },
            VfxRole::Core,
            MotionProfile::arcane_beam_tick(),
        )
    }

    pub fn arcane_beam_tick_sparks() -> Self {
        from_spark(
            SparkBurstOpts {
                spawn: SpawnShape::Sphere,
                count: 8,
                speed: (3.0, 6.0),
                lifetime: (0.18, 0.28),
                size: Curve::from_stops([(0.0, 0.08), (1.0, 0.0)]),
                color: Gradient::from_stops([
                    (0.0, [4.5, 1.8, 5.0, 1.0]),
                    (1.0, [0.2, 0.05, 0.4, 0.0]),
                ]),
                opacity: 1.0,
            },
            VfxRole::Filament,
            MotionProfile::arcane_filaments(),
        )
    }

    pub fn proc_shockwave() -> Self {
        from_shockwave(ShockwaveOpts::proc_explosion(), VfxRole::Rupture)
    }

    pub fn proc_flame_wall() -> Self {
        from_radial(RadialBurstOpts::proc_flames(), VfxRole::Core)
    }

    pub fn proc_ember_ring() -> Self {
        from_radial(RadialBurstOpts::proc_embers(), VfxRole::Filament)
    }

    pub fn proc_smoke_puff() -> Self {
        from_smoke_residue(SmokeResidueOpts::proc_explosion_puff(), VfxRole::Residue)
    }

    pub fn ember_hand_swirl() -> Self {
        Self {
            role: VfxRole::Core,
            emission: EmissionProfile::HandSwirl { rate: 60.0 },
            motion: MotionProfile::ember_hand_swirl(),
            sprite: SpriteShape::SoftGlow,
            blend: BlendMode::Additive,
            size: Curve::from_stops([(0.00, 0.10), (0.40, 0.14), (1.00, 0.0)]),
            color: Gradient::from_stops([
                (0.00, [5.0, 2.4, 0.6, 1.0]),
                (0.50, [2.2, 0.9, 0.2, 0.7]),
                (1.00, [0.6, 0.10, 0.02, 0.0]),
            ]),
            opacity: 1.0,
            hybrid: None,
            speed: None,
            lifetime: None,
            spawn: None,
            use_role_sprite: false,
        }
    }
}

fn from_sphere(opts: SphereBurstOpts, role: VfxRole, motion: MotionProfile) -> ParticleArchetype {
    ParticleArchetype {
        role,
        emission: EmissionProfile::SphereBurst {
            count: opts.count,
        },
        motion,
        sprite: opts.sprite,
        blend: opts.blend,
        size: opts.size,
        color: opts.color,
        opacity: opts.opacity,
        hybrid: None,
        speed: Some(opts.speed),
        lifetime: Some(opts.lifetime),
        spawn: None,
        use_role_sprite: false,
    }
}

fn from_smoke_billow(opts: SmokeBillowOpts, role: VfxRole) -> ParticleArchetype {
    ParticleArchetype {
        role,
        emission: EmissionProfile::SphereBurst { count: opts.count },
        motion: MotionProfile {
            drag: 0.0,
            lift: 0.0,
            gravity_down: 0.0,
            curl: None,
            turbulence: None,
            orbit: None,
            extra: opts.forces,
        },
        sprite: SpriteShape::Smoke,
        blend: BlendMode::Alpha,
        size: opts.size,
        color: opts.color,
        opacity: opts.opacity,
        hybrid: Some(HybridMaterial::smoke_billow()),
        speed: Some(opts.speed),
        lifetime: Some(opts.lifetime),
        spawn: Some(opts.spawn),
        use_role_sprite: false,
    }
}

fn from_streak(opts: StreakBurstOpts, role: VfxRole) -> ParticleArchetype {
    ParticleArchetype {
        role,
        emission: EmissionProfile::SphereBurst { count: opts.count },
        motion: MotionProfile {
            drag: opts.drag,
            lift: 0.0,
            gravity_down: opts.gravity,
            curl: None,
            turbulence: None,
            orbit: None,
            extra: vec![],
        },
        sprite: SpriteShape::Streak,
        blend: BlendMode::Additive,
        size: opts.size,
        color: opts.color,
        opacity: opts.opacity,
        hybrid: None,
        speed: Some(opts.speed),
        lifetime: Some(opts.lifetime),
        spawn: Some(opts.spawn),
        use_role_sprite: false,
    }
}

fn from_spark(opts: SparkBurstOpts, role: VfxRole, motion: MotionProfile) -> ParticleArchetype {
    ParticleArchetype {
        role,
        emission: EmissionProfile::SphereBurst { count: opts.count },
        motion,
        sprite: SpriteShape::Spark,
        blend: BlendMode::Additive,
        size: opts.size,
        color: opts.color,
        opacity: opts.opacity,
        hybrid: None,
        speed: Some(opts.speed),
        lifetime: Some(opts.lifetime),
        spawn: Some(opts.spawn),
        use_role_sprite: false,
    }
}

fn from_shard(opts: ShardBurstOpts, role: VfxRole, motion: MotionProfile) -> ParticleArchetype {
    ParticleArchetype {
        role,
        emission: EmissionProfile::ShardBurst {
            count: opts.count,
        },
        motion,
        sprite: SpriteShape::Shard,
        blend: BlendMode::Additive,
        size: opts.size,
        color: opts.color,
        opacity: opts.opacity,
        hybrid: None,
        speed: Some(opts.speed),
        lifetime: Some(opts.lifetime),
        spawn: None,
        use_role_sprite: false,
    }
}

fn from_shockwave(opts: ShockwaveOpts, role: VfxRole) -> ParticleArchetype {
    let spec = shockwave_spec(opts);
    ParticleArchetype {
        role,
        emission: EmissionProfile::PointNucleus,
        motion: MotionProfile::still(),
        sprite: spec.sprite,
        blend: spec.blend,
        size: spec.size,
        color: spec.color,
        opacity: spec.opacity,
        hybrid: None,
        speed: Some(spec.speed),
        lifetime: Some(spec.lifetime),
        spawn: Some(spec.spawn),
        use_role_sprite: false,
    }
}

fn from_radial(opts: RadialBurstOpts, role: VfxRole) -> ParticleArchetype {
    let mut extra = Vec::new();
    if opts.drag > 0.0 {
        extra.push(ForceField::Drag {
            coefficient: opts.drag,
        });
    }
    if opts.lift.abs() > 1e-4 {
        extra.push(ForceField::Gravity {
            axis: if opts.lift > 0.0 {
                Vec3::Y
            } else {
                -Vec3::Y
            },
            strength: opts.lift.abs(),
        });
    }
    ParticleArchetype {
        role,
        emission: EmissionProfile::SphereBurst { count: opts.count },
        motion: MotionProfile {
            drag: 0.0,
            lift: 0.0,
            gravity_down: 0.0,
            curl: None,
            turbulence: None,
            orbit: None,
            extra,
        },
        sprite: opts.sprite,
        blend: opts.blend,
        size: opts.size,
        color: opts.color,
        opacity: opts.opacity,
        hybrid: opts.hybrid,
        speed: Some(opts.speed),
        lifetime: Some(opts.lifetime),
        spawn: Some(SpawnShape::Ring {
            radius: opts.ring_radius,
            thickness: opts.ring_thickness,
        }),
        use_role_sprite: false,
    }
}

fn from_smoke_residue(opts: SmokeResidueOpts, role: VfxRole) -> ParticleArchetype {
    let mut extra = vec![ForceField::Drag { coefficient: 0.5 }];
    if opts.lift.abs() > 1e-4 {
        extra.push(ForceField::Gravity {
            axis: if opts.lift > 0.0 {
                Vec3::Y
            } else {
                -Vec3::Y
            },
            strength: opts.lift.abs(),
        });
    }
    extra.extend(opts.forces_extra);
    ParticleArchetype {
        role,
        emission: EmissionProfile::SphereBurst { count: opts.count },
        motion: MotionProfile {
            drag: 0.0,
            lift: 0.0,
            gravity_down: 0.0,
            curl: None,
            turbulence: None,
            orbit: None,
            extra,
        },
        sprite: SpriteShape::Smoke,
        blend: BlendMode::Alpha,
        size: opts.size,
        color: opts.color,
        opacity: opts.opacity,
        hybrid: opts.hybrid,
        speed: Some(opts.speed),
        lifetime: Some(opts.lifetime),
        spawn: Some(opts.spawn),
        use_role_sprite: false,
    }
}

/// Proc AoE burst (on-dodge, mirrorglass, etc.).
#[derive(Clone, Copy, Debug)]
pub struct ProcExplosionArchetype;

impl ProcExplosionArchetype {
    pub fn layers() -> Vec<Layer> {
        vec![
            ParticleArchetype::proc_shockwave().into_layer(),
            ParticleArchetype::proc_flame_wall().into_layer(),
            ParticleArchetype::proc_ember_ring().into_layer(),
            ParticleArchetype::proc_smoke_puff().into_layer(),
        ]
    }
}

/// High-level combat impact shapes.
#[derive(Clone, Copy, Debug)]
pub enum ImpactArchetype {
    Detonation,
    BeamTick,
}

impl ImpactArchetype {
    pub fn layers(self, preset: StylePreset) -> Vec<Layer> {
        match (self, preset) {
            (Self::Detonation, StylePreset::VoidFrost) => vec![
                ParticleArchetype::void_frost_impact_flash().into_layer(),
                ParticleArchetype::void_frost_vapor_cloud().into_layer(),
                ParticleArchetype::void_frost_filaments().into_layer(),
                from_shockwave(ShockwaveOpts::frost(), VfxRole::Rupture).into_layer(),
            ],
            (Self::Detonation, StylePreset::EmberVoid) => vec![
                ParticleArchetype::ember_impact_flash().into_layer(),
                ParticleArchetype::ember_detonation_plasma().into_layer(),
                ParticleArchetype::ember_detonation_smoke().into_layer(),
                ParticleArchetype::ember_detonation_embers().into_layer(),
                from_shockwave(ShockwaveOpts::fire_impact(), VfxRole::Rupture).into_layer(),
            ],
            (Self::Detonation, StylePreset::ArcLightning) => vec![
                ParticleArchetype::arcane_impact_flash().into_layer(),
                ParticleArchetype::arcane_detonation_cloud().into_layer(),
                ParticleArchetype::arcane_detonation_sparks().into_layer(),
                from_shockwave(ShockwaveOpts::arcane(), VfxRole::Rupture).into_layer(),
            ],
            (Self::BeamTick, StylePreset::VoidFrost) => vec![
                ParticleArchetype::void_frost_beam_tick_cloud().into_layer(),
                ParticleArchetype::void_frost_beam_tick_filaments().into_layer(),
            ],
            (Self::BeamTick, StylePreset::EmberVoid) => vec![
                ParticleArchetype::ember_beam_tick_cloud().into_layer(),
                ParticleArchetype::ember_beam_tick_shards().into_layer(),
            ],
            (Self::BeamTick, StylePreset::ArcLightning) => vec![
                ParticleArchetype::arcane_beam_tick_cloud().into_layer(),
                ParticleArchetype::arcane_beam_tick_sparks().into_layer(),
            ],
        }
    }
}

/// Hand-base swirl for channeled beams.
pub fn channel_hand_swirl(preset: StylePreset) -> Layer {
    match preset {
        StylePreset::VoidFrost => ParticleArchetype::void_frost_hand_swirl().into_layer(),
        StylePreset::EmberVoid => ParticleArchetype::ember_hand_swirl().into_layer(),
        StylePreset::ArcLightning => continuous(ContinuousOpts::arcane_core()),
    }
}

/// Projectile trail stacks keyed by style.
pub fn projectile_trail_layers(preset: StylePreset) -> Vec<Layer> {
    match preset {
        StylePreset::VoidFrost => vec![
            ParticleArchetype::void_frost_trail_core().into_layer(),
            ParticleArchetype::void_frost_trail_vapor().into_layer(),
        ],
        StylePreset::EmberVoid => projectile_trail_fire(),
        StylePreset::ArcLightning => projectile_trail_arcane(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn void_frost_detonation_has_four_layers() {
        let layers = ImpactArchetype::Detonation.layers(StylePreset::VoidFrost);
        assert_eq!(layers.len(), 4);
    }

    #[test]
    fn ember_detonation_has_five_layers() {
        let layers = ImpactArchetype::Detonation.layers(StylePreset::EmberVoid);
        assert_eq!(layers.len(), 5);
    }

    #[test]
    fn arcane_detonation_has_four_layers() {
        let layers = ImpactArchetype::Detonation.layers(StylePreset::ArcLightning);
        assert_eq!(layers.len(), 4);
    }

    #[test]
    fn proc_explosion_has_four_layers() {
        assert_eq!(ProcExplosionArchetype::layers().len(), 4);
    }

    #[test]
    fn vfx_role_default_sprite() {
        assert_eq!(VfxRole::Rupture.default_sprite(), SpriteShape::Ring);
        assert_eq!(VfxRole::Filament.default_sprite(), SpriteShape::Spark);
    }
}
