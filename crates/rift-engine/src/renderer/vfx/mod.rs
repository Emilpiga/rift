//! Declarative VFX module.
//!
//! Effects are described as a list of [`Layer`]s — particle clouds,
//! ribbons, future decals — each composed from a small set of
//! reusable building blocks (spawn shape, force fields, curves,
//! gradients, sprite kind, blend mode). The runtime
//! ([`vfx::runtime`](runtime)) takes one of these descriptions and
//! drives the GPU primitives.
//!
//! ## Why this exists
//!
//! The legacy [`crate::renderer::particles`] module is imperative:
//! every distinct visual is its own `EmitterConfig::*` constructor,
//! the colour ramp is two-stop, every spread is a one-of-three
//! enum, and there's no concept of layering or beam ribbons.
//! Beams in particular get hand-emitted from gameplay code on top
//! of a solid cylinder mesh, which looks bad. The new system
//! makes "core glow + scrolling noise + spark trail" a single
//! declarative spec instead of a hundred lines of emitter
//! plumbing in the channel-tick code.
//!
//! ## Module layout
//!
//! - [`spec`]    — the pure data types.
//! - [`style`]   — art-direction presets ([`style::StylePreset`]).
//! - [`builder`] — reusable chunks + [`builder::EffectBuilder`].
//! - `runtime`   — CPU-side simulator + GPU instance build.
//! - `presets`   — named effects (Frost Ray, RoF, dodge puff, ...).
//!
//! The legacy `particles` module is left in place during the
//! migration so existing call sites keep working unchanged.

pub mod builder;
pub mod particle_renderer;
pub mod presets;
pub mod ribbon_renderer;
pub mod runtime;
pub mod spec;
pub mod style;
pub mod textures;

pub use builder::EffectBuilder;
pub use builder::{
    EmissionProfile, ImpactArchetype, MotionProfile, ParticleArchetype, ProcExplosionArchetype,
    VfxRole,
};
pub use style::{gpu_id, StyleGpuAux, StyleGpuPack, StylePreset, StyleProfile};
pub use particle_renderer::ParticleVfxRenderer;
pub use ribbon_renderer::RibbonRenderer;
pub use runtime::{EffectId, VfxParticleInstance, VfxRibbonInstance, VfxSystem};
pub use spec::{
    BlendMode, Curve, CurveStop, Effect, EmissionMode, ForceField, Gradient, GradientStop,
    HybridMaterial, HybridProfile, HybridProfileKind, Layer, ParticleSpec, RibbonNoise,
    RibbonSpec, SpawnShape, SpriteShape,
};
