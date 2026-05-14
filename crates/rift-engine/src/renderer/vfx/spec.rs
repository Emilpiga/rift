//! Declarative effect description — pure data, no runtime state.
//!
//! Built out of small composable pieces:
//!
//! - An [`Effect`] is a list of [`Layer`]s rendered together.
//! - [`Layer::Particles`] drives a billboard particle layer.
//! - [`Layer::Ribbon`] drives a beam-style ribbon between an
//!   origin and a tip.
//! - [`Gradient`] is a multi-stop HDR colour ramp evaluated over
//!   life. Use HDR (RGB > 1) on additive layers for bloom.
//! - [`Curve`] is a multi-stop scalar ramp (size, alpha, etc.).
//! - [`SpawnShape`] picks the spawn distribution.
//! - [`ForceField`] composes external forces (gravity, drag,
//!   orbit, curl noise) per layer.
//! - [`SpriteShape`] selects the procedural fragment shape
//!   (soft glow, hard spark, smoky puff). No texture assets yet.
//!
//! All of these are `Clone + Debug` and cheap to copy; effects
//! are intended to be `pub const fn` builders the gameplay
//! layer composes once and clones at spawn time.

use glam::Vec3;

/// Top-level effect description: a stack of independent layers
/// rendered together. Layers share the same world transform but
/// are otherwise independent (own particles / ribbon, own
/// blend, own duration).
#[derive(Clone, Debug)]
pub struct Effect {
    /// Lifetime of the *spawning side* of every layer in
    /// seconds. Particles already in flight when the effect
    /// expires are still allowed to age out naturally. `0.0`
    /// means infinite — the caller (channel UI, persistent
    /// emitter) is responsible for despawning.
    pub duration: f32,
    pub layers: Vec<Layer>,
}

/// Optional dynamic point light attached to an effect.
///
/// The runtime evaluates this every frame at the effect's
/// current anchor position and pushes a [`PointLight`][crate::
/// renderer::PointLight] for the renderer to consume. Used by
/// projectile trails (fireball glows on the corridor walls as
/// it flies past), impact bursts (a bright flash that decays
/// over the explosion's life), and any other effect that
/// should illuminate its surroundings.
#[derive(Clone, Debug)]
pub struct EffectLight {
    /// HDR linear RGB. Encode brightness here — values >1.0
    /// produce hot bloom-able highlights, the renderer's
    /// per-pixel lighting clamps via the tonemap.
    pub color: Vec3,
    /// Falloff radius in metres. Past this distance the light
    /// contributes nothing.
    pub radius: f32,
    /// Base intensity multiplier. Combined with `intensity_curve`
    /// (if any) and `flicker_amp` to produce the per-frame value.
    pub intensity: f32,
    /// Optional intensity envelope.
    ///
    /// * If `lifetime` is `Some`, this is sampled over
    ///   `elapsed / lifetime` so the light can flash to peak
    ///   then decay independently of the particle pool's
    ///   lifecycle. Use this for impact bursts.
    /// * Otherwise it's sampled over the *effect's* normalised
    ///   life (`elapsed / effect.duration`); for persistent
    ///   effects (`duration == 0`) the curve sits at `t = 0`.
    pub intensity_curve: Option<Curve>,
    /// Optional independent lifetime for the light, in seconds.
    /// When `Some(t)`, the light persists for `t` seconds
    /// regardless of whether the effect's particle pool has
    /// drained or whether spawning has stopped — this is what
    /// lets an explosion's flash decay smoothly *after* the
    /// last ember has aged out, instead of disappearing the
    /// instant the pool empties.
    /// When `None`, the light tracks the particle pool's
    /// lifetime (the legacy behaviour).
    pub lifetime: Option<f32>,
    /// Sinusoidal flicker amplitude as a fraction of intensity
    /// (`0.0 = steady`, `0.15 = ±15%`). Two octaves at slightly
    /// detuned frequencies are summed for an organic feel.
    pub flicker_amp: f32,
    /// Base flicker frequency in Hz. Most projectile trails want
    /// 8..14 Hz for a nervous flame quality.
    pub flicker_hz: f32,
    /// World-space offset added to the effect's anchor before
    /// the light position is computed. Useful when the effect's
    /// emitter is at the projectile's centre but you want the
    /// light slightly above it (or behind it for a trail glow).
    pub offset: Vec3,
    /// When `true`, the light's intensity tracks the effect's
    /// **live particle population** instead of wall-clock time.
    /// The runtime tracks the peak particle count seen since
    /// the effect started; intensity is then driven by
    /// `live / peak` so the light:
    ///
    ///   * peaks at the impact frame when all particles have
    ///     just spawned;
    ///   * decays in lockstep with the impact animation as
    ///     embers / smoke / shockwave puffs age out;
    ///   * smoothly fades to zero as the last particles die.
    ///
    /// `intensity_curve` (if set) shapes the response: it is
    /// sampled at `1 - live/peak` so `t = 0` is the peak and
    /// `t = 1` is "all particles dead". A curve like
    /// `[(0, 1), (1, 0)]` is the linear default; a steeper
    /// curve makes the light fall faster than the particles.
    ///
    /// Mutually exclusive with `lifetime` — when this is set,
    /// `lifetime` is ignored.
    pub follow_particles: bool,
}

impl EffectLight {
    /// Steady (non-pulsing) light at the effect's anchor.
    pub fn steady(color: Vec3, radius: f32, intensity: f32) -> Self {
        Self {
            color,
            radius,
            intensity,
            intensity_curve: None,
            lifetime: None,
            flicker_amp: 0.0,
            flicker_hz: 0.0,
            offset: Vec3::ZERO,
            follow_particles: false,
        }
    }
}

/// An [`Effect`] plus optional engine-side enhancements that
/// don't fit into the pure-data spec without breaking every
/// preset's struct literal.
///
/// New presets that want any of these enhancements (light,
/// velocity inheritance) should return `EffectBundle`. Old
/// presets continue to return `Effect` and convert via
/// `Into<EffectBundle>` automatically.
#[derive(Clone, Debug)]
pub struct EffectBundle {
    pub effect: Effect,
    /// Dynamic point light that follows the effect's anchor.
    pub light: Option<EffectLight>,
    /// Optional second dynamic point light anchored to the
    /// effect's *tip* endpoint (the second endpoint passed to
    /// `set_endpoints`). Use for channeled beams / laser
    /// abilities so the impact end carries continuous
    /// illumination through the channel without the gameplay
    /// layer having to spawn and despawn a separate "tip glow"
    /// effect. The light tracks `set_endpoints` updates each
    /// frame, so an aim sweep paints light across the wall it's
    /// drawn on.
    ///
    /// Falls back to the anchor position when the effect has
    /// never had endpoints set.
    pub tip_light: Option<EffectLight>,
    /// Fraction of the *anchor's* per-frame velocity inherited
    /// by every particle at spawn time. `0.0` (default) keeps
    /// the legacy behaviour: particles spawn with only the
    /// velocity their `SpawnShape` and `forces` give them.
    /// `1.0` makes new particles fly with the projectile they
    /// trail behind, so the trail visibly streaks along the
    /// flight path instead of clumping at the projectile.
    /// Typical values 0.5..0.9 for projectile trails.
    pub inherit_velocity: f32,
}

impl From<Effect> for EffectBundle {
    fn from(effect: Effect) -> Self {
        Self {
            effect,
            light: None,
            tip_light: None,
            inherit_velocity: 0.0,
        }
    }
}

impl EffectBundle {
    pub fn new(effect: Effect) -> Self {
        Self::from(effect)
    }

    pub fn with_light(mut self, light: EffectLight) -> Self {
        self.light = Some(light);
        self
    }

    /// Attach a second light fixed to the effect's tip
    /// endpoint. Intended for ribbon-based beam effects whose
    /// gameplay layer calls `set_endpoints(origin, tip)` each
    /// frame.
    pub fn with_tip_light(mut self, light: EffectLight) -> Self {
        self.tip_light = Some(light);
        self
    }

    pub fn with_inherit_velocity(mut self, f: f32) -> Self {
        self.inherit_velocity = f.clamp(0.0, 1.0);
        self
    }
}

/// One renderable layer in an [`Effect`].
#[derive(Clone, Debug)]
pub enum Layer {
    /// Billboard particle cloud — the bread-and-butter VFX
    /// primitive. See [`ParticleSpec`].
    Particles(ParticleSpec),
    /// Two-endpoint camera-aligned ribbon, used for beams and
    /// laser-style abilities. Origin / tip are supplied at spawn
    /// time and updated every frame by the gameplay layer; the
    /// spec only holds appearance data. See [`RibbonSpec`].
    Ribbon(RibbonSpec),
}

// ─── Particles ────────────────────────────────────────────────────────────

/// Per-layer billboard particle description.
#[derive(Clone, Debug)]
pub struct ParticleSpec {
    pub spawn: SpawnShape,
    pub emission: EmissionMode,
    /// Initial speed range [min, max]. Direction is determined by
    /// the [`SpawnShape`].
    pub speed: (f32, f32),
    /// Particle lifetime [min, max] in seconds.
    pub lifetime: (f32, f32),
    /// Forces applied every tick, in declaration order.
    pub forces: Vec<ForceField>,
    /// Size over normalised life (`0.0 = born`, `1.0 = dead`).
    /// At least one stop required.
    pub size: Curve,
    /// Colour over normalised life. HDR (any channel > 1.0)
    /// boosts brightness for additive blends.
    pub color: Gradient,
    /// Procedural fragment shape evaluated per pixel.
    pub sprite: SpriteShape,
    pub blend: BlendMode,
    /// Multiplier on the sprite's procedural alpha. Lets a
    /// single gradient/sprite drive several visual densities.
    pub opacity: f32,
}

/// How the spawner emits particles over time.
#[derive(Clone, Copy, Debug)]
pub enum EmissionMode {
    /// Continuous emission at `rate` particles / second. Used
    /// for auras, beams, persistent loot pillars.
    Continuous { rate: f32 },
    /// Single one-shot burst, `count` particles at t = 0. Used
    /// for hit sparks, dodge puffs, death explosions.
    Burst { count: u32 },
    /// Hybrid: an initial burst followed by continuous emission
    /// for the layer's duration. Mirrors the legacy
    /// `EmitterConfig::burst_count + spawn_rate` pair.
    BurstAndContinuous { burst: u32, rate: f32 },
}

/// Where new particles are born.
#[derive(Clone, Copy, Debug)]
pub enum SpawnShape {
    /// Single anchor point. Velocity direction comes from the
    /// `forces` list (e.g. an Inertial force seeded from the
    /// caller's aim direction) or is left zero.
    Point,
    /// Random direction on the unit sphere centred at the anchor.
    /// Speed scales the emitted vector. Position is the anchor.
    Sphere,
    /// Random direction within `half_angle` radians of `axis`.
    Cone { axis: Vec3, half_angle: f32 },
    /// Cylindrical column (XZ disc + Y extent) for upward spew
    /// (loot beams, portals).
    Column {
        radius: f32,
        height: f32,
        axis: Vec3,
    },
    /// Cone-shaped column that tapers from `radius_base` at the
    /// bottom to `radius_top` at the top, distributed over
    /// `height` along `axis`. Spawn density is biased toward
    /// the base (quadratic height curve) so the lower section
    /// reads as the "thick root" and the top melts into nothing.
    /// Used by loot-beam style fog plumes where the silhouette
    /// must taper rather than read as a uniform cylinder.
    TaperedColumn {
        radius_base: f32,
        radius_top: f32,
        height: f32,
        axis: Vec3,
    },
    /// Thin-disc ring of radius `radius` on the XZ plane —
    /// targeting reticles, ground impacts.
    Ring { radius: f32, thickness: f32 },
    /// Thin ring in the plane perpendicular to `axis`. Same
    /// shape as [`Self::Ring`] but oriented arbitrarily — used
    /// by the Doctor-Strange-style portal halo, which needs to
    /// orbit a *vertical* mesh ring (axis = +Z) rather than
    /// lying flat on the floor. Outward emission direction is
    /// the ring's radial vector in that plane.
    RingAxis {
        radius: f32,
        thickness: f32,
        axis: Vec3,
    },
    /// Filled disc on the XZ plane — RoF column top, AoE start.
    Disc { radius: f32 },
    /// Line segment between `a` and `b` relative to the spawn
    /// anchor. Used by trail-style emitters that drop sparks
    /// along a beam each frame.
    Line { a: Vec3, b: Vec3 },
}

impl Default for SpawnShape {
    fn default() -> Self {
        Self::Point
    }
}

/// Composable forces applied to particles every tick. Order
/// matters: each force reads the post-previous-force velocity.
#[derive(Clone, Copy, Debug)]
pub enum ForceField {
    /// Constant downward (or arbitrary-axis) acceleration in m/s².
    /// Positive `strength` = accelerate along `axis`.
    Gravity { axis: Vec3, strength: f32 },
    /// Exponential velocity damping, `coefficient` per second.
    /// `1.0` halves velocity in ~0.7 s; `4.0` kills it fast.
    Drag { coefficient: f32 },
    /// Tangential velocity around `axis` through the particle's
    /// origin. Positive = CCW looking down the axis.
    Orbit { axis: Vec3, speed: f32 },
    /// Curl-noise turbulence: smooth divergence-free force field
    /// driven by hash noise. `frequency` is roughly 1 / wave-
    /// length in metres; `strength` is acceleration scale.
    /// Phase advances with effect time so the field animates.
    Curl { frequency: f32, strength: f32 },
    /// Constant directional velocity bias added each frame —
    /// useful for trail particles that should drift along a
    /// beam without inheriting per-particle randomness.
    Wind { velocity: Vec3 },
}

// ─── Ribbon ───────────────────────────────────────────────────────────────

/// Two-endpoint camera-aligned strip. The renderer expands a
/// quad each frame from the ribbon's current `origin` / `tip`
/// world points and the camera right-vector, so the ribbon is
/// always face-on regardless of beam direction.
#[derive(Clone, Debug)]
pub struct RibbonSpec {
    /// Width in world metres at full HDR core (thin near the
    /// edges via the gradient alpha).
    pub width: f32,
    /// Multi-stop colour gradient sampled across the *width*
    /// (`v` axis: 0 = one edge, 1 = other edge). HDR encoded
    /// the same way as [`ParticleSpec::color`].
    pub cross_gradient: Gradient,
    /// Optional gradient sampled along the *length* (`u` axis).
    /// `None` = constant 1.0 along the beam. Lets us fade in
    /// at the hand and pulse along the shaft without authoring
    /// a texture.
    pub length_gradient: Option<Gradient>,
    /// Procedural noise applied to the ribbon brightness for
    /// flow / shimmer. `None` = a clean static beam.
    pub noise: Option<RibbonNoise>,
    pub blend: BlendMode,
}

/// Procedural-noise parameters layered on top of the ribbon's
/// gradient evaluation. Implemented in the fragment shader as
/// scrolling hash noise — no texture asset required.
#[derive(Clone, Copy, Debug)]
pub struct RibbonNoise {
    /// World-units to one noise tile along the beam length.
    /// Smaller = denser noise.
    pub tile: f32,
    /// Length-direction scroll speed in tiles / second. Beam
    /// `flow` is mostly this.
    pub scroll: f32,
    /// Mix factor: 0 = no noise, 1 = noise fully replaces base
    /// brightness. Typical 0.3 .. 0.6.
    pub strength: f32,
    /// Number of octaves to fold in (1..=4). Higher = more
    /// detail, more cost.
    pub octaves: u8,
}

// ─── Curves & gradients ───────────────────────────────────────────────────

/// One stop in a [`Curve`].
#[derive(Clone, Copy, Debug)]
pub struct CurveStop {
    /// Position along the curve, `0.0..=1.0`.
    pub t: f32,
    pub value: f32,
}

/// Multi-stop scalar curve sampled at `t in [0, 1]`. Values
/// outside the stops clamp to the nearest endpoint.
#[derive(Clone, Debug)]
pub struct Curve {
    pub stops: Vec<CurveStop>,
}

impl Curve {
    /// Constant value over the whole life.
    pub fn constant(v: f32) -> Self {
        Self {
            stops: vec![CurveStop { t: 0.0, value: v }],
        }
    }

    /// Two-stop linear ramp from `a` at t=0 to `b` at t=1.
    pub fn linear(a: f32, b: f32) -> Self {
        Self {
            stops: vec![
                CurveStop { t: 0.0, value: a },
                CurveStop { t: 1.0, value: b },
            ],
        }
    }

    /// Construct from an iterator of `(t, value)` tuples. Stops
    /// should be supplied in increasing `t`.
    pub fn from_stops(it: impl IntoIterator<Item = (f32, f32)>) -> Self {
        Self {
            stops: it
                .into_iter()
                .map(|(t, value)| CurveStop { t, value })
                .collect(),
        }
    }

    /// Sample at `t in [0, 1]`. Linear interpolation between
    /// neighbouring stops; clamps outside the range.
    pub fn sample(&self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        if self.stops.is_empty() {
            return 0.0;
        }
        if t <= self.stops[0].t {
            return self.stops[0].value;
        }
        if t >= self.stops[self.stops.len() - 1].t {
            return self.stops[self.stops.len() - 1].value;
        }
        for w in self.stops.windows(2) {
            let (a, b) = (w[0], w[1]);
            if t >= a.t && t <= b.t {
                let span = (b.t - a.t).max(1e-6);
                let local = (t - a.t) / span;
                return a.value + (b.value - a.value) * local;
            }
        }
        self.stops[self.stops.len() - 1].value
    }
}

/// One stop in a [`Gradient`].
#[derive(Clone, Copy, Debug)]
pub struct GradientStop {
    /// Position along the gradient, `0.0..=1.0`.
    pub t: f32,
    /// HDR RGBA — encode brightness in the RGB channels (e.g.
    /// `[2.4, 0.6, 0.1, 1.0]` for a hot-orange ember).
    pub color: [f32; 4],
}

/// Multi-stop HDR colour gradient. `sample(t)` linearly
/// interpolates between neighbouring stops. RGB channels are
/// HDR-friendly (free to exceed 1.0 — additive blends will
/// bloom; alpha blends will saturate).
#[derive(Clone, Debug)]
pub struct Gradient {
    pub stops: Vec<GradientStop>,
}

impl Gradient {
    /// Constant colour over the whole gradient.
    pub fn constant(rgba: [f32; 4]) -> Self {
        Self {
            stops: vec![GradientStop {
                t: 0.0,
                color: rgba,
            }],
        }
    }

    /// Two-stop linear from `a` at t=0 to `b` at t=1.
    pub fn linear(a: [f32; 4], b: [f32; 4]) -> Self {
        Self {
            stops: vec![
                GradientStop { t: 0.0, color: a },
                GradientStop { t: 1.0, color: b },
            ],
        }
    }

    /// Builder convenience: append `(t, rgba)` to the existing
    /// stop list. Stops should be supplied in increasing `t`.
    pub fn stop(mut self, t: f32, rgba: [f32; 4]) -> Self {
        self.stops.push(GradientStop { t, color: rgba });
        self
    }

    /// Construct from an iterator of `(t, rgba)` tuples.
    pub fn from_stops(it: impl IntoIterator<Item = (f32, [f32; 4])>) -> Self {
        Self {
            stops: it
                .into_iter()
                .map(|(t, color)| GradientStop { t, color })
                .collect(),
        }
    }

    pub fn sample(&self, t: f32) -> [f32; 4] {
        let t = t.clamp(0.0, 1.0);
        if self.stops.is_empty() {
            return [0.0, 0.0, 0.0, 0.0];
        }
        if t <= self.stops[0].t {
            return self.stops[0].color;
        }
        if t >= self.stops[self.stops.len() - 1].t {
            return self.stops[self.stops.len() - 1].color;
        }
        for w in self.stops.windows(2) {
            let (a, b) = (w[0], w[1]);
            if t >= a.t && t <= b.t {
                let span = (b.t - a.t).max(1e-6);
                let local = (t - a.t) / span;
                return [
                    a.color[0] + (b.color[0] - a.color[0]) * local,
                    a.color[1] + (b.color[1] - a.color[1]) * local,
                    a.color[2] + (b.color[2] - a.color[2]) * local,
                    a.color[3] + (b.color[3] - a.color[3]) * local,
                ];
            }
        }
        self.stops[self.stops.len() - 1].color
    }
}

// ─── Sprite & blend ───────────────────────────────────────────────────────

/// Procedural fragment shape. Selected by the shader through an
/// integer instance field; no texture assets involved. Numeric
/// values are part of the wire to the GPU — keep them stable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum SpriteShape {
    /// Soft Gaussian glow — the workhorse. Useful for embers,
    /// magic dust, generic alpha particles.
    SoftGlow = 0,
    /// Hard-edged radial spark — tight bright core, no halo.
    /// Useful for hit sparks at additive blend.
    Spark = 1,
    /// Smoky puff: low-frequency hash noise modulating a soft
    /// disc. Animates with the particle's seed.
    Smoke = 2,
    /// Four-point crystalline burst — frost shards, ice crackle.
    Shard = 3,
    /// Filled ring (hollow centre) — shockwaves, AoE rings.
    Ring = 4,
    /// Anisotropic motion line oriented along the particle's
    /// velocity vector. Reads as a crisp streak even at low
    /// speeds (unlike `Spark` which falls back to a dot when
    /// the screen-space velocity is small). Use for falling
    /// embers, dust trails, anything that should always read
    /// as motion.
    Streak = 5,
    /// Vertical ethereal strand — a tall, soft capsule
    /// modulated by scrolling fBm noise. Always anisotropic
    /// along *world up*, regardless of velocity, with a
    /// brighter inner core that wavers organically. Designed
    /// for Diablo-style loot beams: stack a few of these in
    /// a thin column and they read as a single luminous
    /// strand of rising light rather than a cloud of
    /// individual particles. Also useful for ghostly auras,
    /// god-rays, or any "ethereal vertical light" motif.
    Wisp = 6,
    /// Full-beam silk strand — a single sprite that draws
    /// the *entire* loot-beam pillar: a soft ethereal body
    /// plus N sharp sine-wave silk threads spiralling around
    /// it. Always vertical (world-up oriented), always
    /// anchored at the base, with all widths/amplitudes
    /// tapering to pixel-width at the top so the silhouette
    /// melts into air. Designed for ARPG loot pillars where
    /// the beam must read as a sharp HD highlight at close
    /// range while still feeling ethereal at distance.
    SilkStrand = 7,
    /// Ground-aligned fractured disc — a procedural impact decal
    /// drawn flat on the XZ plane instead of camera-facing. The
    /// fragment mask combines broken radial fissures, chipped ring
    /// fragments, and noisy scorched fill so large slam/meteor
    /// impacts read as damage in the world rather than UI rings.
    GroundCrack = 8,
}

impl Default for SpriteShape {
    fn default() -> Self {
        Self::SoftGlow
    }
}

/// How the layer composites against the framebuffer. The
/// renderer maintains one pipeline per blend mode and submits
/// alpha-blended layers before additive ones each frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum BlendMode {
    /// Standard `(SRC_ALPHA, ONE_MINUS_SRC_ALPHA)`. Use for
    /// solid puffs, smoke, anything that should darken what's
    /// behind it.
    Alpha = 0,
    /// `(SRC_ALPHA, ONE)` additive. Use for glows, embers,
    /// beams — anything that should brighten the scene.
    Additive = 1,
    /// `(ONE, ONE_MINUS_SRC_ALPHA)` premultiplied alpha — used
    /// when the gradient already encodes opacity in the RGB.
    Premultiplied = 2,
}

impl Default for BlendMode {
    fn default() -> Self {
        Self::Alpha
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_clamp() {
        let c = Curve::linear(2.0, 8.0);
        assert!((c.sample(-1.0) - 2.0).abs() < 1e-6);
        assert!((c.sample(2.0) - 8.0).abs() < 1e-6);
        assert!((c.sample(0.5) - 5.0).abs() < 1e-6);
    }

    #[test]
    fn gradient_three_stop() {
        let g = Gradient::from_stops([
            (0.0, [0.0, 0.0, 0.0, 1.0]),
            (0.5, [1.0, 0.0, 0.0, 1.0]),
            (1.0, [1.0, 1.0, 1.0, 0.0]),
        ]);
        assert_eq!(g.sample(0.0), [0.0, 0.0, 0.0, 1.0]);
        let mid = g.sample(0.25);
        assert!((mid[0] - 0.5).abs() < 1e-5);
        assert_eq!(g.sample(1.0), [1.0, 1.0, 1.0, 0.0]);
    }
}
