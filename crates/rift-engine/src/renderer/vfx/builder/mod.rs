//! Composable VFX authoring — stack reusable **chunks** into effects.
//!
//! Presets in [`crate::renderer::vfx::presets`] read as recipes:
//!
//! ```ignore
//! use rift_engine::renderer::vfx::builder::{EffectBuilder, StylePreset};
//!
//! EffectBuilder::persistent()
//!     .style(StylePreset::VoidFrost)
//!     .channel_hand_swirl()
//!     .ribbon(RibbonOpts::frost_outer())
//!     .finish_bundle()
//! ```
//!
//! - [`chunks`] — one function per visual motif (shockwave, smoke, sparks, …).
//! - [`EffectBuilder`] — fluent layer stack + optional lights / velocity inherit.
//! - [`crate::renderer::vfx::style`] — art-direction presets applied when layers are pushed.
//! - [`particle::ParticleOpts`] — full control when no chunk fits yet.

pub mod archetype;
pub mod chunks;
pub mod emission;
pub mod motion;
pub mod particle;
pub mod tint;

pub use archetype::{
    channel_hand_swirl, projectile_trail_layers, ImpactArchetype, ParticleArchetype,
    ProcExplosionArchetype, VfxRole,
};
pub use emission::EmissionProfile;
pub use motion::MotionProfile;
pub use chunks::{
    beam_tick_impact_layers, continuous, fire_beam_ribbons, flash, frost_beam_ribbons,
    impact_burst_layers, loot_beam_base_layer, loot_beam_layers, particle, plasma_core,
    projectile_trail_arcane, projectile_trail_fire, projectile_trail_frost, radial_burst,
    ribbon, shard_burst, shockwave, sky_portal_ring, smoke_billow, smoke_residue, smoke_wake,
    spark_burst, sphere_burst, streak_burst, heal_ground_ring, ContinuousOpts, FlashOpts,
    ImpactTheme, ParticleOpts, PlasmaCoreOpts, RadialBurstOpts, RibbonOpts, ShardBurstOpts,
    ShockwaveOpts, SmokeBillowOpts, SmokeResidueOpts, SmokeWakeOpts, SparkBurstOpts,
    SphereBurstOpts, StreakBurstOpts,
};
pub use tint::{gradient_from_tinted_stops, tint_gradient};

pub use crate::renderer::vfx::style::{StylePreset, StyleProfile};

use crate::renderer::vfx::spec::{Effect, EffectBundle, EffectLight, Layer};

/// Fluent composer for [`Effect`] / [`EffectBundle`].
#[derive(Clone, Debug)]
pub struct EffectBuilder {
    bundle: EffectBundle,
    /// Effect-wide art direction. Applied to every layer (and lights at finish).
    style: Option<StylePreset>,
}

impl EffectBuilder {
    pub fn new(duration: f32) -> Self {
        Self {
            bundle: EffectBundle {
                effect: Effect {
                    duration,
                    layers: Vec::new(),
                },
                light: None,
                tip_light: None,
                inherit_velocity: 0.0,
                style: None,
            },
            style: None,
        }
    }

    /// One-shot burst preset (`duration = 0.05`).
    pub fn oneshot() -> Self {
        Self::new(0.05)
    }

    /// Persistent emitter (`duration = 0.0`).
    pub fn persistent() -> Self {
        Self::new(0.0)
    }

    pub fn timed(duration: f32) -> Self {
        Self::new(duration)
    }

    /// Attach a visual identity preset (void frost, ember, arc lightning, …).
    pub fn style(mut self, preset: StylePreset) -> Self {
        self.style = Some(preset);
        self.bundle.style = Some(preset);
        self
    }

    /// Ensure [`EffectBundle::style`] is set for GPU + themed CPU stylize.
    /// Set bundle + builder style from an impact theme when `.style()` was not called.
    pub fn ensure_style_from_theme(&mut self, theme: ImpactTheme) {
        if self.style.is_none() {
            let preset = theme.default_style();
            self.style = Some(preset);
            self.bundle.style = Some(preset);
        }
    }

    fn profile_for_theme(&self, theme: Option<ImpactTheme>) -> Option<StyleProfile> {
        if let Some(p) = self.style {
            return Some(p.profile());
        }
        theme.map(|t| t.default_style().profile())
    }

    fn stylize_layer(&self, layer: Layer, theme: Option<ImpactTheme>) -> Layer {
        match self.profile_for_theme(theme) {
            Some(p) if self.style.is_some() => p.stylize_layer_gpu(layer),
            Some(p) => p.stylize_layer(layer),
            None => layer,
        }
    }

    fn stylize_layers(
        &self,
        layers: impl IntoIterator<Item = Layer>,
        theme: Option<ImpactTheme>,
    ) -> Vec<Layer> {
        layers
            .into_iter()
            .map(|l| self.stylize_layer(l, theme))
            .collect()
    }

    pub fn layer(mut self, layer: Layer) -> Self {
        let layer = self.stylize_layer(layer, None);
        self.bundle.effect.layers.push(layer);
        self
    }

    pub fn layers(mut self, layers: impl IntoIterator<Item = Layer>) -> Self {
        let styled = self.stylize_layers(layers, None);
        self.bundle.effect.layers.extend(styled);
        self
    }

    pub fn ribbon(self, opts: RibbonOpts) -> Self {
        self.layer(chunks::ribbon(opts))
    }

    pub fn particle(self, opts: impl Into<ParticleOpts>) -> Self {
        self.layer(particle::particle(opts))
    }

    pub fn shockwave(self, opts: ShockwaveOpts) -> Self {
        self.layer(chunks::shockwave(opts))
    }

    pub fn radial_burst(self, opts: RadialBurstOpts) -> Self {
        self.layer(chunks::radial_burst(opts))
    }

    pub fn smoke_residue(self, opts: SmokeResidueOpts) -> Self {
        self.layer(chunks::smoke_residue(opts))
    }

    pub fn smoke_billow(self, opts: SmokeBillowOpts) -> Self {
        self.layer(chunks::smoke_billow(opts))
    }

    pub fn smoke_wake(self, opts: SmokeWakeOpts) -> Self {
        self.layer(chunks::smoke_wake(opts))
    }

    pub fn spark_burst(self, opts: SparkBurstOpts) -> Self {
        self.layer(chunks::spark_burst(opts))
    }

    pub fn sphere_burst(self, opts: SphereBurstOpts) -> Self {
        self.layer(chunks::sphere_burst(opts))
    }

    pub fn streak_burst(self, opts: StreakBurstOpts) -> Self {
        self.layer(chunks::streak_burst(opts))
    }

    pub fn shard_burst(self, opts: ShardBurstOpts) -> Self {
        self.layer(chunks::shard_burst(opts))
    }

    pub fn flash(self, opts: FlashOpts) -> Self {
        self.layer(chunks::flash(opts))
    }

    pub fn plasma_core(self, opts: PlasmaCoreOpts) -> Self {
        self.layer(chunks::plasma_core(opts))
    }

    pub fn continuous(self, opts: ContinuousOpts) -> Self {
        self.layer(chunks::continuous(opts))
    }

    /// Push a semantic particle layer (emission + motion + role).
    pub fn particle_archetype(self, archetype: ParticleArchetype) -> Self {
        self.layer(archetype.into_layer())
    }

    /// Phase 4 — role picks default sprite; kernel accents via `role_pack`.
    pub fn particle_for_role(
        self,
        role: VfxRole,
        emission: EmissionProfile,
        motion: MotionProfile,
        blend: crate::renderer::vfx::spec::BlendMode,
        size: crate::renderer::vfx::spec::Curve,
        color: crate::renderer::vfx::spec::Gradient,
        opacity: f32,
    ) -> Self {
        self.particle_archetype(ParticleArchetype::for_role(
            role, emission, motion, blend, size, color, opacity,
        ))
    }

    /// Hand-base swirl for the active [`.style`] preset (defaults to [`StylePreset::VoidFrost`]).
    pub fn channel_hand_swirl(self) -> Self {
        let preset = self.style.unwrap_or(StylePreset::VoidFrost);
        self.layer(channel_hand_swirl(preset))
    }

    pub fn impact_burst(mut self, theme: ImpactTheme) -> Self {
        self.ensure_style_from_theme(theme);
        let layers = impact_burst_layers(theme);
        let styled = self.stylize_layers(layers, Some(theme));
        self.bundle.effect.layers.extend(styled);
        self
    }

    pub fn beam_tick_impact(mut self, theme: ImpactTheme) -> Self {
        self.ensure_style_from_theme(theme);
        let layers = beam_tick_impact_layers(theme);
        let styled = self.stylize_layers(layers, Some(theme));
        self.bundle.effect.layers.extend(styled);
        self
    }

    pub fn proc_explosion(self) -> Self {
        let mut b = self;
        if b.style.is_none() {
            b = b.style(StylePreset::EmberVoid);
        }
        b.layers(ProcExplosionArchetype::layers())
    }

    pub fn with_light(mut self, light: EffectLight) -> Self {
        self.bundle.light = Some(self.stylize_light(light));
        self
    }

    pub fn with_tip_light(mut self, light: EffectLight) -> Self {
        self.bundle.tip_light = Some(self.stylize_light(light));
        self
    }

    fn stylize_light(&self, light: EffectLight) -> EffectLight {
        match self.style {
            Some(p) => p.profile().stylize_light(light),
            None => light,
        }
    }

    pub fn with_inherit_velocity(mut self, f: f32) -> Self {
        self.bundle.inherit_velocity = f;
        self
    }

    pub fn finish(self) -> Effect {
        self.bundle.effect
    }

    pub fn finish_bundle(self) -> EffectBundle {
        self.bundle
    }
}

impl From<EffectBuilder> for Effect {
    fn from(b: EffectBuilder) -> Self {
        b.finish()
    }
}

impl From<EffectBuilder> for EffectBundle {
    fn from(b: EffectBuilder) -> Self {
        b.finish_bundle()
    }
}
