//! CPU-side simulator for [`Effect`] instances.
//!
//! Every active effect lives as one [`EffectInstance`] in
//! [`VfxSystem`]. Particle layers tick deterministically off the
//! same hash-based RNG used in the legacy emitter, so spawn
//! distributions stay stable for a given seed. Ribbon layers
//! carry only their endpoints + spec — the GPU expands the quad
//! each frame.
//!
//! This module owns *no* GPU state. It produces flat instance
//! buffers (`particle_instances` / `ribbon_instances`) that the
//! renderer uploads. Keeps the simulator usable from tests and
//! tools.
//!
//! ## Lifecycle
//!
//! 1. `system.spawn(effect, transform)` — returns an [`EffectId`].
//! 2. `system.tick(dt)` — advances every live effect.
//! 3. `system.set_endpoints(id, origin, tip)` — for ribbon /
//!    beam effects whose two world points come from gameplay
//!    (caster's hand → first wall hit).
//! 4. `system.particle_instances()` / `system.ribbon_instances()`
//!    — read-only flat slices for GPU upload.
//! 5. `system.despawn(id)` — stop spawning; particles already in
//!    flight age out naturally.

use bytemuck::{Pod, Zeroable};
use glam::{Quat, Vec3};

use super::spec::{
    Effect, EffectBundle, EffectLight, EmissionMode, ForceField, Layer, ParticleSpec, RibbonSpec,
    SpawnShape, SpriteShape,
};
use crate::renderer::forward::PointLight;

/// Stable handle for one active effect. Wraps a generational
/// index so freed slots can be reused without aliasing old IDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EffectId {
    index: u32,
    generation: u32,
}

/// GPU-friendly per-particle instance produced by a particle
/// layer. Mirrors the legacy [`crate::renderer::particles::
/// ParticleInstance`] but carries blend / sprite / brightness
/// fields the new shader consumes. Old presets continue to use
/// the legacy struct via the legacy renderer; the new pipeline
/// reads this struct.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct VfxParticleInstance {
    pub position: [f32; 3],
    pub size: f32,
    /// HDR colour pre-multiplied by `opacity` (alpha kept
    /// separately so additive blends still respect emission
    /// strength).
    pub color: [f32; 4],
    /// Per-particle deterministic seed in `[0, 1)` — fed into
    /// the noise / shape evaluators in the fragment shader so
    /// no two particles look identical even at the same age.
    pub seed: f32,
    /// `0` = soft glow, `1` = spark, `2` = smoke, `3` = shard,
    /// `4` = ring, `5` = streak, `6` = wisp, `7` = silk strand,
    /// `8` = ground crack. Cast from [`SpriteShape`]
    /// discriminant.
    pub sprite: u32,
    /// `0` = alpha, `1` = additive, `2` = premultiplied. The
    /// renderer groups by this field so all alpha particles
    /// draw before any additive ones.
    pub blend: u32,
    pub _pad: u32,
    /// World-space velocity in m/s. Drives screen-space motion
    /// stretch in the vertex shader: fast-moving particles
    /// elongate along the projection of this vector onto the
    /// near plane, so embers and sparks read as crisp streaks
    /// instead of dots. Slow particles fall back to an
    /// axis-aligned billboard.
    pub velocity: [f32; 3],
    /// Per-particle rotation phase in radians. The vertex
    /// shader rotates the billboard quad by this amount around
    /// the camera-facing axis, so smoke / shard / ring sprites
    /// no longer all share the same orientation. Encoded as
    /// `seed * TAU + age * spin_rate` so each particle
    /// continuously spins; the rate is inferred from `seed`.
    pub spin: f32,
}

/// GPU-friendly per-ribbon instance. The renderer expands a quad
/// in the vertex shader from `(origin, tip, width, ...)` plus
/// the camera right-vector, so the CPU only needs to push the
/// endpoints. One instance = one beam.
///
/// Gradients are pre-baked at spawn time into fixed-size arrays
/// (`cross[8]` across the width, `length[4]` along the beam) so
/// the fragment shader can sample them without external LUT
/// textures or storage buffers. Stops are evenly spaced — the
/// `Gradient` is sampled at `t = i / (N-1)` for each slot.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct VfxRibbonInstance {
    /// World-space origin (xyz) + ribbon width (w) in metres.
    pub origin: [f32; 4],
    /// World-space tip (xyz) + effect-local time (w) in seconds.
    pub tip: [f32; 4],
    /// `[brightness, noise_strength, noise_scroll, noise_tile]`.
    /// `noise_strength == 0` disables noise entirely.
    pub params: [f32; 4],
    /// `[noise_octaves, _, _, _]` packed as floats — the vertex
    /// stream layout doesn't allow `uint` here without a
    /// dedicated attribute, so we stash it here.
    pub flags: [f32; 4],
    /// 8 RGBA stops sampled across the *width* of the ribbon.
    pub cross: [[f32; 4]; 8],
    /// 4 RGBA stops sampled along the *length* of the ribbon.
    /// All-ones if the spec didn't supply a length gradient.
    pub length: [[f32; 4]; 4],
}

/// One active effect in the system.
struct EffectInstance {
    generation: u32,
    /// `None` once the slot is free.
    spec: Option<Effect>,
    /// World-space position the effect was spawned at. Used as
    /// the spawn anchor for every particle layer.
    anchor: Vec3,
    /// Optional per-frame override of `anchor`. Lets the gameplay
    /// layer follow a moving caster without respawning the effect.
    follow_anchor: Option<Vec3>,
    /// Per-effect orientation applied to every particle layer's
    /// spawn shape and orbit-force axes. Defaults to identity
    /// (no rotation). Used by world-anchored effects that want
    /// to billboard toward the camera/player without spawning a
    /// fresh emitter every frame — e.g. the rift portal disc.
    orientation: Quat,
    /// Beam endpoints for ribbon layers. Updated each frame from
    /// gameplay code via [`VfxSystem::set_endpoints`]. Defaults
    /// to `(anchor, anchor)` so a not-yet-aimed beam draws as a
    /// degenerate quad (invisible).
    origin: Vec3,
    tip: Vec3,
    /// Brightness multiplier currently fed to ribbon instances.
    brightness: f32,
    /// Seconds elapsed since spawn. Drives ribbon noise scroll
    /// and effect-duration cutoff.
    elapsed: f32,
    /// Whether the effect is still allowed to spawn new
    /// particles. Set false on explicit despawn or duration
    /// expiry; existing particles continue to age out.
    spawning: bool,
    /// Per-layer state — emission accumulators, RNG seeds,
    /// per-layer particle pools.
    layers: Vec<LayerState>,
    /// Optional point light pushed each frame at the effect's
    /// current anchor. Cloned out of the [`EffectBundle`] passed
    /// to [`VfxSystem::spawn_bundle`].
    light: Option<EffectLight>,
    /// Optional second light pinned to `tip` rather than the
    /// anchor. Used by ribbon-based beam effects so the
    /// impact end of a channeled spell stays continuously
    /// illuminated even while the per-burst impact effects
    /// flicker on and off.
    tip_light: Option<EffectLight>,
    /// Fraction of the anchor's per-frame velocity inherited by
    /// freshly-spawned particles. See
    /// [`EffectBundle::inherit_velocity`].
    inherit_velocity: f32,
    /// Anchor position at the start of this frame's tick — used
    /// to compute the anchor's velocity (delta / dt) for
    /// velocity inheritance. `None` until the second tick (we
    /// can't know the velocity from a single sample).
    prev_anchor: Option<Vec3>,
    /// Anchor velocity computed at the most recent tick.
    /// Re-used by the per-layer spawner so every spawn within
    /// a frame inherits the same velocity.
    anchor_velocity: Vec3,
    /// Maximum live particle count seen across all this
    /// effect's layers since spawn. Used to drive
    /// `EffectLight::follow_particles`: light intensity tracks
    /// `live / peak`, so the light peaks when the impact has
    /// just spawned all its particles and decays in lockstep
    /// with the pool draining. Stays at the post-impact peak
    /// after spawning stops, giving the curve a stable
    /// reference point.
    peak_particle_count: u32,
    /// Smoothed `[0, 1]` envelope used by
    /// `EffectLight::follow_particles`. Tracks
    /// `live / peak` upward while the effect is still
    /// spawning, then decays monotonically toward zero with
    /// a fixed time constant once spawning has stopped. This
    /// hides the staircase-shaped collapse of multi-wave
    /// emitters (e.g. fireball impact, where flash + smoke +
    /// embers are separate layers with non-overlapping
    /// lifetimes) — without it the light flashes brighter
    /// every time a later wave repopulates the pool, then
    /// snaps off when the final particle dies.
    light_envelope: f32,
}

enum LayerState {
    Particles(ParticlesState),
    Ribbon(RibbonState),
}

struct ParticlesState {
    spec: ParticleSpec,
    rng_seed: u32,
    /// Carry-over fractional spawn count between frames so the
    /// emission rate stays accurate at any framerate.
    spawn_acc: f32,
    /// Whether the initial burst has already been emitted.
    burst_done: bool,
    pool: Vec<LiveParticle>,
}

struct RibbonState {
    spec: RibbonSpec,
    /// Gradients pre-sampled at spawn time so the fragment
    /// shader can index a fixed-size array rather than walk a
    /// `Vec<GradientStop>` (impossible on GPU). Re-baked only
    /// when the spec itself is replaced — currently never.
    cross_baked: [[f32; 4]; 8],
    length_baked: [[f32; 4]; 4],
}

/// CPU-side simulated particle. Instance data is rebuilt every
/// frame from the live pool so the GPU buffer is dense.
#[derive(Clone, Debug)]
struct LiveParticle {
    position: Vec3,
    velocity: Vec3,
    /// Origin point used by `Orbit` forces — usually the spawn
    /// anchor at birth time.
    origin: Vec3,
    age: f32,
    max_life: f32,
    seed: f32,
    /// Random per-particle phase fed into curl-noise so two
    /// particles at the same world point evolve differently.
    noise_phase: f32,
}

impl LiveParticle {
    fn alive(&self) -> bool {
        self.age < self.max_life
    }

    fn life_t(&self) -> f32 {
        if self.max_life > 0.0 {
            (self.age / self.max_life).clamp(0.0, 1.0)
        } else {
            1.0
        }
    }
}

/// Public VFX system — owns active effects + their particle
/// pools. Kept renderer-agnostic so tests can drive it headlessly.
pub struct VfxSystem {
    instances: Vec<EffectInstance>,
    /// Free-list (LIFO) of indices reusable for new effects.
    free: Vec<u32>,
    /// Generation bumped on every despawn to invalidate stale IDs.
    next_generation: u32,
    /// Soft cap on simultaneous live particles across all layers.
    /// New spawns past this point are silently dropped — the
    /// runtime never grows unboundedly.
    max_particles: usize,
    /// Cached instance buffers, rebuilt each `tick`.
    particle_instances: Vec<VfxParticleInstance>,
    ribbon_instances: Vec<VfxRibbonInstance>,
}

impl VfxSystem {
    pub fn new(max_particles: usize) -> Self {
        Self {
            instances: Vec::new(),
            free: Vec::new(),
            next_generation: 1,
            max_particles,
            particle_instances: Vec::new(),
            ribbon_instances: Vec::new(),
        }
    }

    /// Spawn a fresh effect at `anchor`. Returns the handle the
    /// caller uses to drive the effect (set endpoints, despawn).
    pub fn spawn(&mut self, effect: Effect, anchor: Vec3) -> EffectId {
        self.spawn_bundle(EffectBundle::from(effect), anchor)
    }

    /// Spawn a fresh effect described by an [`EffectBundle`] —
    /// effect + optional point light + optional velocity
    /// inheritance. Use this entry-point for projectile trails
    /// and impacts so the engine can drive the attached light
    /// and inherit velocity from the moving anchor.
    pub fn spawn_bundle(&mut self, bundle: EffectBundle, anchor: Vec3) -> EffectId {
        let EffectBundle {
            effect,
            light,
            tip_light,
            inherit_velocity,
        } = bundle;
        let mut layers: Vec<LayerState> = Vec::with_capacity(effect.layers.len());
        for (i, layer) in effect.layers.iter().enumerate() {
            match layer {
                Layer::Particles(spec) => layers.push(LayerState::Particles(ParticlesState {
                    spec: spec.clone(),
                    rng_seed: hash_u32(self.next_generation, i as u32),
                    spawn_acc: 0.0,
                    burst_done: false,
                    pool: Vec::new(),
                })),
                Layer::Ribbon(spec) => {
                    let cross_baked = bake_gradient_8(&spec.cross_gradient);
                    let length_baked = match &spec.length_gradient {
                        Some(g) => bake_gradient_4(g),
                        None => [[1.0; 4]; 4],
                    };
                    layers.push(LayerState::Ribbon(RibbonState {
                        spec: spec.clone(),
                        cross_baked,
                        length_baked,
                    }));
                }
            }
        }

        let id = EffectId {
            index: 0,
            generation: self.next_generation,
        };
        self.next_generation = self.next_generation.wrapping_add(1).max(1);

        let instance = EffectInstance {
            generation: id.generation,
            spec: Some(effect),
            anchor,
            follow_anchor: None,
            orientation: Quat::IDENTITY,
            origin: anchor,
            tip: anchor,
            brightness: 1.0,
            elapsed: 0.0,
            spawning: true,
            layers,
            light,
            tip_light,
            inherit_velocity: inherit_velocity.clamp(0.0, 1.0),
            prev_anchor: None,
            anchor_velocity: Vec3::ZERO,
            peak_particle_count: 0,
            light_envelope: 0.0,
        };

        let index = if let Some(slot) = self.free.pop() {
            self.instances[slot as usize] = instance;
            slot
        } else {
            self.instances.push(instance);
            (self.instances.len() - 1) as u32
        };

        EffectId {
            index,
            generation: id.generation,
        }
    }

    /// Update a beam-style effect's two world endpoints. Called
    /// every frame by the gameplay code that owns the caster's
    /// hand position + first-wall raycast.
    pub fn set_endpoints(&mut self, id: EffectId, origin: Vec3, tip: Vec3) {
        if let Some(inst) = self.get_mut(id) {
            inst.origin = origin;
            inst.tip = tip;
        }
    }

    /// Update the spawn anchor — used when a persistent effect
    /// (loot pillar, aura) needs to follow a moving entity. Does
    /// not retroactively reposition particles already in flight.
    pub fn set_anchor(&mut self, id: EffectId, anchor: Vec3) {
        if let Some(inst) = self.get_mut(id) {
            inst.follow_anchor = Some(anchor);
        }
    }

    /// Replace the per-effect orientation. Future spawns rotate
    /// their spawn-shape offset / launch direction by this
    /// quaternion, and `Orbit` force axes are rotated likewise
    /// at integration time. Already-airborne particles keep
    /// flying along their existing trajectories — change this
    /// every frame for a smooth re-aim.
    pub fn set_orientation(&mut self, id: EffectId, orientation: Quat) {
        if let Some(inst) = self.get_mut(id) {
            inst.orientation = orientation;
        }
    }

    /// Adjust the ribbon brightness multiplier. Cheap — no
    /// gradient / noise rebuild required.
    pub fn set_brightness(&mut self, id: EffectId, brightness: f32) {
        if let Some(inst) = self.get_mut(id) {
            inst.brightness = brightness.max(0.0);
        }
    }

    /// Stop the effect from spawning. Particles already in
    /// flight finish their lifetimes naturally; the slot is
    /// reused once the pool drains.
    pub fn despawn(&mut self, id: EffectId) {
        if let Some(inst) = self.get_mut(id) {
            inst.spawning = false;
        }
    }

    /// Wipe every active effect immediately, including any live
    /// particles. Used on floor transitions so loot beams,
    /// frost trails, and other long-lived emitters from the
    /// previous floor don't bleed into the new one.
    pub fn clear_all(&mut self) {
        for inst in &mut self.instances {
            inst.spec = None;
            inst.spawning = false;
            inst.layers.clear();
        }
        self.particle_instances.clear();
        self.ribbon_instances.clear();
    }

    /// Whether the effect is still alive (spawning *or* still
    /// has live particles).
    pub fn is_alive(&self, id: EffectId) -> bool {
        self.get(id).is_some()
    }

    /// Snapshot of every live particle this frame. Stable order
    /// per-call; suitable for direct GPU upload.
    pub fn particle_instances(&self) -> &[VfxParticleInstance] {
        &self.particle_instances
    }

    /// Snapshot of every live ribbon this frame. One element per
    /// `Layer::Ribbon` of every active effect.
    pub fn ribbon_instances(&self) -> &[VfxRibbonInstance] {
        &self.ribbon_instances
    }

    /// Append a [`PointLight`] to `out` for every live effect
    /// whose [`EffectBundle::light`] was set at spawn time.
    /// `time_secs` drives the optional flicker; pass the
    /// renderer's `elapsed_secs()` so it stays in phase across
    /// frames.
    ///
    /// Lights are attached at the effect's *current* anchor
    /// (`follow_anchor` if set, else `anchor`) plus the light's
    /// `offset`, and are intensity-modulated by the optional
    /// `intensity_curve` evaluated over normalised effect life.
    /// Persistent effects (`duration == 0`) sample the curve at
    /// `t = 0` so the curve is effectively a constant — use
    /// `flicker_amp` to add liveliness instead.
    pub fn collect_lights(&self, time_secs: f32, out: &mut Vec<PointLight>) {
        for inst in &self.instances {
            let Some(spec) = inst.spec.as_ref() else {
                continue;
            };
            if inst.light.is_none() && inst.tip_light.is_none() {
                continue;
            }
            // Compute the per-instance envelope once. Both the
            // anchor light and the optional tip light share the
            // same effect-life "alive-ness" — they're driven by
            // the same particle pool / lifetime / duration —
            // and gating each one on the same `(alive, curve_t)`
            // keeps them perfectly in lockstep through fades.
            //
            // We pick the gating mode from whichever light
            // exists; if both are present we use the anchor
            // light (the "primary" light by convention; tip
            // lights are auxiliary).
            let gating_light = inst
                .light
                .as_ref()
                .or(inst.tip_light.as_ref())
                .expect("checked above");
            let envelope = match compute_envelope(inst, spec, gating_light) {
                Some(e) => e,
                None => continue,
            };

            if let Some(light) = inst.light.as_ref() {
                let pos = anchor_for(inst) + light.offset;
                push_effect_light(inst, light, pos, envelope, time_secs, out);
            }
            if let Some(light) = inst.tip_light.as_ref() {
                // Tip lights live at `inst.tip` (the second
                // endpoint passed to `set_endpoints`). Effects
                // that never set endpoints have `tip == anchor`,
                // so the tip light degenerates to a duplicate
                // of the anchor light — harmless, but a bit
                // wasteful. Presets attach `tip_light` only on
                // ribbon-based beams, so this case shouldn't
                // arise in practice.
                let pos = inst.tip + light.offset;
                push_effect_light(inst, light, pos, envelope, time_secs, out);
            }
        }
    }

    /// Advance the simulation by `dt` seconds. Spawns new
    /// particles, applies forces, ages out dead particles, and
    /// rebuilds the cached instance buffers.
    ///
    /// `cull` (when `Some`) skips the per-particle integration
    /// for effects whose anchor is more than `cull.1` metres
    /// from `cull.0`. The effect's `elapsed` and
    /// `light_envelope` are still advanced so attached lights
    /// fade and the slot eventually expires — only the heavy
    /// per-particle simulation and the GPU instance push are
    /// skipped. Visual risk is zero as long as `cull.1` is
    /// safely past anything the player can see (we recommend
    /// `fog_end + 5 m`); particles inside that radius take
    /// the unmodified path.
    pub fn tick(&mut self, dt: f32, cull: Option<(Vec3, f32)>) {
        self.particle_instances.clear();
        self.ribbon_instances.clear();
        let total_cap = self.max_particles;
        let cull_dist_sq = cull.map(|(_, d)| d * d);

        // Sweep every slot. We iterate by index so we can free
        // slots in place without an extra collection.
        for slot in 0..self.instances.len() {
            let alive = {
                let inst = &mut self.instances[slot];
                if inst.spec.is_none() {
                    continue;
                }
                inst.elapsed += dt;
                let duration = inst.spec.as_ref().map(|e| e.duration).unwrap_or(0.0);
                if duration > 0.0 && inst.elapsed >= duration {
                    inst.spawning = false;
                }

                let anchor = inst.follow_anchor.unwrap_or(inst.anchor);
                let orientation = inst.orientation;

                // ---- Distance cull ----
                // Effects far past the camera's fog/draw range
                // skip the heavy per-particle integration
                // entirely. We still advance the lifetime
                // counter (already done above via `elapsed`)
                // and decay the light envelope so any
                // attached light fades correctly while
                // off-screen. Liveness for a culled effect
                // ignores the (now stale) particle pool —
                // a culled burst with no persistent light
                // therefore drops immediately, freeing the
                // slot. Visually identical to the player
                // because by definition the effect is past
                // the fog wall.
                let culled = match (cull, cull_dist_sq) {
                    (Some((origin, _)), Some(dsq)) => (anchor - origin).length_squared() > dsq,
                    _ => false,
                };
                if culled {
                    if !inst.spawning {
                        let tau = 0.55_f32;
                        let k = (-dt / tau).exp();
                        inst.light_envelope = (inst.light_envelope * k).max(0.0);
                    }
                    let light_alive = match inst.light.as_ref() {
                        Some(EffectLight {
                            lifetime: Some(t), ..
                        }) => inst.elapsed < *t,
                        Some(EffectLight {
                            follow_particles: true,
                            ..
                        }) => inst.light_envelope > 1e-3,
                        _ => false,
                    };
                    // Yield from the `let alive = { ... }`
                    // block. Falling through to the slot-free
                    // arm below if not alive.
                    inst.spawning || light_alive
                } else {
                    // Anchor velocity for inheritance. We only get a
                    // meaningful number on the second-and-later tick
                    // (we can't infer velocity from a single sample).
                    // For the very first frame after spawn this is
                    // zero, so trail particles spawned that frame
                    // sit at the projectile centre — fine, the next
                    // frame onwards they fly with it.
                    let anchor_vel = if let Some(prev) = inst.prev_anchor {
                        if dt > 1e-5 {
                            (anchor - prev) / dt
                        } else {
                            Vec3::ZERO
                        }
                    } else {
                        Vec3::ZERO
                    };
                    inst.prev_anchor = Some(anchor);
                    inst.anchor_velocity = anchor_vel;
                    let inherit = inst.inherit_velocity;

                    // 1) Tick particle layers.
                    for layer in inst.layers.iter_mut() {
                        if let LayerState::Particles(p) = layer {
                            tick_particles(
                                p,
                                anchor,
                                orientation,
                                anchor_vel,
                                inherit,
                                dt,
                                inst.spawning,
                                total_cap,
                            );
                        }
                    }

                    // 2) Track peak live-particle count for any
                    //    light using `follow_particles`. Updated
                    //    *after* the tick so freshly-spawned
                    //    particles count toward the peak. Stops
                    //    rising once spawning has stopped, which
                    //    means after impact the peak stays fixed
                    //    and the live/peak ratio strictly decreases
                    //    as embers age out.
                    let live_now: u32 = inst
                        .layers
                        .iter()
                        .map(|l| match l {
                            LayerState::Particles(p) => p.pool.len() as u32,
                            LayerState::Ribbon(_) => 0,
                        })
                        .sum();
                    if live_now > inst.peak_particle_count {
                        inst.peak_particle_count = live_now;
                    }

                    // 2b) Update the smoothed light envelope. While
                    //     particles are still spawning, the
                    //     envelope rises toward the live ratio so
                    //     the light hits full intensity at the
                    //     spawn peak. Once spawning has stopped,
                    //     it ignores `live_now` entirely and
                    //     decays exponentially with a fixed time
                    //     constant — this is what the user sees
                    //     as the impact light "fading out". The
                    //     constant (~0.85 s half-life) is tuned
                    //     to match the longest particle lifetime
                    //     in the fireball explosion preset, so
                    //     the light goes dark as the last embers
                    //     would naturally die.
                    let peak = inst.peak_particle_count.max(1) as f32;
                    let target = (live_now as f32 / peak).clamp(0.0, 1.0);
                    if inst.spawning {
                        if target > inst.light_envelope {
                            inst.light_envelope = target;
                        }
                    } else {
                        // Exponential decay: env *= exp(-dt / tau).
                        // tau ≈ 0.55 s gives ~0.85 s half-life.
                        let tau = 0.55_f32;
                        let k = (-dt / tau).exp();
                        inst.light_envelope = (inst.light_envelope * k).max(0.0);
                    }

                    // 3) Drop the slot once not spawning *and* every
                    //    particle pool has drained *and* any attached
                    //    light with an independent lifetime has
                    //    expired. Ribbons disappear immediately when
                    //    `spawning` flips false (no pool to drain) —
                    //    that's the contract `despawn` relies on for
                    //    persistent (duration == 0) beams like Frost
                    //    Ray.
                    let any_pool_alive = inst.layers.iter().any(|l| match l {
                        LayerState::Particles(p) => !p.pool.is_empty(),
                        LayerState::Ribbon(_) => false,
                    });
                    let light_alive = match inst.light.as_ref() {
                        Some(EffectLight {
                            lifetime: Some(t), ..
                        }) => inst.elapsed < *t,
                        // `follow_particles` lights drive their
                        // own decay envelope after the last
                        // particle dies — the impact flash needs
                        // ~0.85 s of ramp-down to read as a
                        // fading flame, not a hard cut. Keep the
                        // slot alive until that envelope settles
                        // close to zero. Without this retention
                        // the slot is dropped the instant
                        // `any_pool_alive` flips false and the
                        // light disappears mid-fade.
                        Some(EffectLight {
                            follow_particles: true,
                            ..
                        }) => inst.light_envelope > 1e-3,
                        _ => false,
                    };
                    inst.spawning || any_pool_alive || light_alive
                } // end of `if !culled` arm of the cull check
            };
            if !alive {
                self.free_slot(slot);
            }
        }

        // 3) Rebuild instance buffers. We do this after the tick
        //    so freed slots are excluded. Culled effects also
        //    skip this step because we left their pools at
        //    last frame's data — pushing those would render
        //    stale particles. The `culled` predicate is
        //    cheap to recompute here.
        for inst in &self.instances {
            if inst.spec.is_none() {
                continue;
            }
            let anchor = inst.follow_anchor.unwrap_or(inst.anchor);
            if let (Some((origin, _)), Some(dsq)) = (cull, cull_dist_sq) {
                let _ = origin;
                if (anchor - origin).length_squared() > dsq {
                    continue;
                }
            }
            for layer in &inst.layers {
                match layer {
                    LayerState::Particles(p) => {
                        encode_particle_instances(p, &mut self.particle_instances);
                    }
                    LayerState::Ribbon(r) => {
                        // Ribbons stop emitting the instant
                        // `spawning` flips false (despawn) — they
                        // have no pool to drain, and lingering
                        // particles on other layers shouldn't
                        // keep a stale beam visible.
                        if !inst.spawning {
                            continue;
                        }
                        if (inst.tip - inst.origin).length_squared() < 1e-8 {
                            // Degenerate — still allowed; gameplay
                            // hasn't pushed real endpoints yet.
                            continue;
                        }
                        let _ = anchor;
                        let (noise_strength, noise_scroll, noise_tile, noise_octaves) = match r
                            .spec
                            .noise
                        {
                            Some(n) => (n.strength, n.scroll, n.tile.max(1e-3), n.octaves as f32),
                            None => (0.0, 0.0, 1.0, 1.0),
                        };
                        self.ribbon_instances.push(VfxRibbonInstance {
                            origin: [inst.origin.x, inst.origin.y, inst.origin.z, r.spec.width],
                            tip: [inst.tip.x, inst.tip.y, inst.tip.z, inst.elapsed],
                            params: [inst.brightness, noise_strength, noise_scroll, noise_tile],
                            flags: [noise_octaves, 0.0, 0.0, 0.0],
                            cross: r.cross_baked,
                            length: r.length_baked,
                        });
                    }
                }
            }
        }
    }

    fn get(&self, id: EffectId) -> Option<&EffectInstance> {
        let inst = self.instances.get(id.index as usize)?;
        if inst.generation == id.generation && inst.spec.is_some() {
            Some(inst)
        } else {
            None
        }
    }

    fn get_mut(&mut self, id: EffectId) -> Option<&mut EffectInstance> {
        let inst = self.instances.get_mut(id.index as usize)?;
        if inst.generation == id.generation && inst.spec.is_some() {
            Some(inst)
        } else {
            None
        }
    }

    fn free_slot(&mut self, slot: usize) {
        if let Some(inst) = self.instances.get_mut(slot) {
            if inst.spec.is_some() {
                inst.spec = None;
                inst.layers.clear();
                self.free.push(slot as u32);
            }
        }
    }
}

/// Per-layer particle simulation. Spawns first (so a freshly-
/// created layer with `Burst` produces this-frame particles),
/// then integrates physics on the resulting pool.
fn tick_particles(
    p: &mut ParticlesState,
    anchor: Vec3,
    orientation: Quat,
    anchor_velocity: Vec3,
    inherit_velocity: f32,
    dt: f32,
    spawning: bool,
    total_cap: usize,
) {
    // 1) Spawn. `Burst` emitters are one-shot and must fire
    //    regardless of `spawning` — short-duration effects
    //    (blood_splatter @ 0.05s) would otherwise be eaten on
    //    the first tick whenever `dt >= duration` (a single
    //    hitchy frame). The continuous half of an emission
    //    still respects `spawning` so a finished effect doesn't
    //    keep dribbling new particles forever.
    let to_spawn = match p.spec.emission {
        EmissionMode::Continuous { rate } => {
            if spawning {
                p.spawn_acc += rate * dt;
                let n = p.spawn_acc.floor();
                p.spawn_acc -= n;
                n as u32
            } else {
                0
            }
        }
        EmissionMode::Burst { count } => {
            if !p.burst_done {
                p.burst_done = true;
                count
            } else {
                0
            }
        }
        EmissionMode::BurstAndContinuous { burst, rate } => {
            let initial = if !p.burst_done {
                p.burst_done = true;
                burst
            } else {
                0
            };
            let cont = if spawning {
                p.spawn_acc += rate * dt;
                let n = p.spawn_acc.floor();
                p.spawn_acc -= n;
                n as u32
            } else {
                0
            };
            initial + cont
        }
    };

    for _ in 0..to_spawn {
        if p.pool.len() >= total_cap.max(1) {
            break;
        }
        p.pool.push(spawn_one(
            &p.spec,
            anchor,
            orientation,
            anchor_velocity * inherit_velocity,
            &mut p.rng_seed,
        ));
    }

    // 2) Integrate.
    let dt2 = dt;
    for part in p.pool.iter_mut() {
        part.age += dt2;
        if !part.alive() {
            continue;
        }
        let origin = part.origin;
        let noise_phase = part.noise_phase;
        let mut velocity = part.velocity;
        let mut position = part.position;
        for force in &p.spec.forces {
            apply_force(
                force,
                &mut position,
                &mut velocity,
                origin,
                orientation,
                noise_phase,
                dt2,
            );
        }
        part.velocity = velocity;
        part.position = position + part.velocity * dt2;
    }

    // 3) Reap.
    p.pool.retain(|q| q.alive());
}

fn apply_force(
    force: &ForceField,
    pos: &mut Vec3,
    velocity: &mut Vec3,
    origin: Vec3,
    orientation: Quat,
    noise_phase: f32,
    dt: f32,
) {
    match force {
        ForceField::Gravity { axis, strength } => {
            *velocity += *axis * (*strength * dt);
        }
        ForceField::Drag { coefficient } => {
            let factor = (1.0 - coefficient * dt).max(0.0);
            *velocity *= factor;
        }
        ForceField::Orbit { axis, speed } => {
            // Rotate `pos - origin` around `axis` by `speed * dt`.
            // The spec axis lives in effect-local space; the
            // per-effect orientation re-aims orbiting layers
            // along with the rest of the emitter.
            let n = (orientation * *axis).normalize_or_zero();
            if n.length_squared() < 0.5 {
                return;
            }
            let theta = speed * dt;
            let (s, c) = (theta.sin(), theta.cos());
            let r = *pos - origin;
            // Rodrigues' rotation
            let rot = r * c + n.cross(r) * s + n * n.dot(r) * (1.0 - c);
            *pos = origin + rot;
        }
        ForceField::Curl {
            frequency,
            strength,
        } => {
            // Cheap pseudo-curl: take the gradient of a hash
            // potential field at three offsets and use the
            // perpendicular components. Not divergence-free in
            // theory but visually indistinguishable for our
            // purposes and ~5 hashes per particle per tick.
            let p = *pos * (*frequency);
            let phase = noise_phase;
            let nx = noise3(p + Vec3::new(phase, 0.0, 0.0));
            let ny = noise3(p + Vec3::new(0.0, phase + 17.0, 0.0));
            let nz = noise3(p + Vec3::new(0.0, 0.0, phase + 31.0));
            let acc = Vec3::new(ny - nz, nz - nx, nx - ny) * (*strength);
            *velocity += acc * dt;
        }
        ForceField::Wind { velocity: w } => {
            *velocity += *w * dt;
        }
    }
}

fn spawn_one(
    spec: &ParticleSpec,
    anchor: Vec3,
    orientation: Quat,
    inherited_velocity: Vec3,
    rng: &mut u32,
) -> LiveParticle {
    let r1 = next_rand(rng);
    let r2 = next_rand(rng);
    let r3 = next_rand(rng);
    let r4 = next_rand(rng);
    let r5 = next_rand(rng);
    let r6 = next_rand(rng);

    let lifetime = lerp(spec.lifetime.0, spec.lifetime.1, r1);
    let speed = lerp(spec.speed.0, spec.speed.1, r2);

    let (offset, direction) = match spec.spawn {
        SpawnShape::Point => (Vec3::ZERO, Vec3::ZERO),
        SpawnShape::Sphere => {
            let theta = r3 * std::f32::consts::TAU;
            let phi = (r4 * 2.0 - 1.0).acos();
            let dir = Vec3::new(phi.sin() * theta.cos(), phi.cos(), phi.sin() * theta.sin());
            (Vec3::ZERO, dir)
        }
        SpawnShape::Cone { axis, half_angle } => {
            let axis = axis.normalize_or(Vec3::Y);
            let angle = r3 * half_angle;
            let rot = r4 * std::f32::consts::TAU;
            let perp = if axis.y.abs() < 0.99 {
                axis.cross(Vec3::Y).normalize()
            } else {
                axis.cross(Vec3::X).normalize()
            };
            let perp2 = axis.cross(perp);
            let lateral = (perp * rot.cos() + perp2 * rot.sin()) * angle.sin();
            (
                Vec3::ZERO,
                (axis * angle.cos() + lateral).normalize_or(axis),
            )
        }
        SpawnShape::Column {
            radius,
            height,
            axis,
        } => {
            let axis = axis.normalize_or(Vec3::Y);
            // Pick an arbitrary orthonormal basis around `axis`.
            let perp = if axis.y.abs() < 0.99 {
                axis.cross(Vec3::Y).normalize()
            } else {
                axis.cross(Vec3::X).normalize()
            };
            let perp2 = axis.cross(perp);
            let theta = r3 * std::f32::consts::TAU;
            let r = (r4).sqrt() * radius;
            let h = r5 * height;
            let off = perp * (theta.cos() * r) + perp2 * (theta.sin() * r) + axis * h;
            (off, axis)
        }
        SpawnShape::TaperedColumn {
            radius_base,
            radius_top,
            height,
            axis,
        } => {
            let axis = axis.normalize_or(Vec3::Y);
            let perp = if axis.y.abs() < 0.99 {
                axis.cross(Vec3::Y).normalize()
            } else {
                axis.cross(Vec3::X).normalize()
            };
            let perp2 = axis.cross(perp);
            // Bias height toward the base: squaring the uniform
            // sample produces ~3× density at the bottom vs the
            // top, so the column visibly thickens downward even
            // before the radius lerp kicks in.
            let h_t = r5 * r5;
            // Radius shrinks linearly with height. h_t is the
            // base-to-top fraction so 0 = base radius, 1 = top
            // radius. With density already biased low + radius
            // lerping toward `radius_top` (typically near zero)
            // the silhouette rounds out into nothing at the top.
            let radius_here = lerp(radius_base, radius_top, h_t);
            let theta = r3 * std::f32::consts::TAU;
            let r = (r4).sqrt() * radius_here;
            let h = h_t * height;
            let off = perp * (theta.cos() * r) + perp2 * (theta.sin() * r) + axis * h;
            (off, axis)
        }
        SpawnShape::Ring { radius, thickness } => {
            let theta = r3 * std::f32::consts::TAU;
            let radial = lerp(radius - thickness * 0.5, radius + thickness * 0.5, r4);
            let off = Vec3::new(theta.cos() * radial, 0.0, theta.sin() * radial);
            // Outward direction in XZ.
            let dir = Vec3::new(theta.cos(), 0.0, theta.sin());
            (off, dir)
        }
        SpawnShape::RingAxis {
            radius,
            thickness,
            axis,
        } => {
            // Build an orthonormal basis in the plane perpendicular
            // to `axis`. Same trick as `Cone` / `Column`.
            let axis = axis.normalize_or(Vec3::Y);
            let perp = if axis.y.abs() < 0.99 {
                axis.cross(Vec3::Y).normalize()
            } else {
                axis.cross(Vec3::X).normalize()
            };
            let perp2 = axis.cross(perp);
            let theta = r3 * std::f32::consts::TAU;
            let radial = lerp(radius - thickness * 0.5, radius + thickness * 0.5, r4);
            let off = perp * (theta.cos() * radial) + perp2 * (theta.sin() * radial);
            // Outward radial vector in the ring's plane.
            let dir = (perp * theta.cos() + perp2 * theta.sin()).normalize_or(perp);
            (off, dir)
        }
        SpawnShape::Disc { radius } => {
            let theta = r3 * std::f32::consts::TAU;
            let r = (r4).sqrt() * radius;
            let off = Vec3::new(theta.cos() * r, 0.0, theta.sin() * r);
            (off, Vec3::Y)
        }
        SpawnShape::Line { a, b } => {
            let off = a + (b - a) * r3;
            let dir = (b - a).normalize_or_zero();
            (off, dir)
        }
    };

    // Re-aim the spawn shape: a per-effect quaternion (set by
    // gameplay code via `set_orientation`) rotates the offset
    // and launch direction so the whole emitter visually faces
    // the desired direction without rebuilding the spec.
    let offset = orientation * offset;
    let direction = orientation * direction;

    LiveParticle {
        position: anchor + offset,
        velocity: direction * speed + inherited_velocity,
        origin: anchor + offset,
        age: 0.0,
        max_life: lifetime,
        seed: r5,
        noise_phase: r6 * 100.0,
    }
}

fn encode_particle_instances(p: &ParticlesState, out: &mut Vec<VfxParticleInstance>) {
    let blend = p.spec.blend as u32;
    let sprite = match p.spec.sprite {
        SpriteShape::SoftGlow => 0u32,
        SpriteShape::Spark => 1,
        SpriteShape::Smoke => 2,
        SpriteShape::Shard => 3,
        SpriteShape::Ring => 4,
        SpriteShape::Streak => 5,
        SpriteShape::Wisp => 6,
        SpriteShape::SilkStrand => 7,
        SpriteShape::GroundCrack => 8,
    };
    for q in &p.pool {
        if !q.alive() {
            continue;
        }
        let t = q.life_t();
        let size = p.spec.size.sample(t);
        let mut col = p.spec.color.sample(t);
        let alpha = (col[3] * p.spec.opacity).clamp(0.0, 1.0);
        col[3] = alpha;
        // Per-particle rotation: a constant offset from the
        // seed (so adjacent particles start out of phase) plus
        // a slow continuous spin proportional to age. The spin
        // rate itself is keyed off `seed` so identical sprites
        // tumble at slightly different speeds — important for
        // the `Smoke` / `Shard` / `Ring` shapes where uniform
        // rotation would be obvious.
        //
        // `Wisp`, `SilkStrand`, and `GroundCrack` are the
        // exception: they rely on the vertex shader projecting
        // world-up into screen space / aligning to world XZ.
        // Any animated spin would tilt or rotate them visibly
        // after spawn, so keep their orientation stable.
        let spin = if matches!(p.spec.sprite, SpriteShape::Wisp | SpriteShape::SilkStrand) {
            0.0
        } else if matches!(p.spec.sprite, SpriteShape::GroundCrack) {
            q.seed * std::f32::consts::TAU
        } else {
            q.seed * std::f32::consts::TAU + q.age * (0.4 + q.seed * 1.6)
        };
        out.push(VfxParticleInstance {
            position: q.position.to_array(),
            size,
            color: col,
            seed: q.seed,
            sprite,
            blend,
            _pad: 0,
            velocity: q.velocity.to_array(),
            spin,
        });
    }
}

// ─── Math helpers ─────────────────────────────────────────────────────────

use rift_math::lerp;

fn bake_gradient_8(g: &super::spec::Gradient) -> [[f32; 4]; 8] {
    let mut out = [[0.0; 4]; 8];
    for i in 0..8 {
        let t = i as f32 / 7.0;
        out[i] = g.sample(t);
    }
    out
}

fn bake_gradient_4(g: &super::spec::Gradient) -> [[f32; 4]; 4] {
    let mut out = [[0.0; 4]; 4];
    for i in 0..4 {
        let t = i as f32 / 3.0;
        out[i] = g.sample(t);
    }
    out
}

fn next_rand(state: &mut u32) -> f32 {
    *state = state.wrapping_mul(1664525).wrapping_add(1013904223);
    let v = (*state >> 8) & 0x00FF_FFFF;
    v as f32 / 16_777_215.0
}

fn anchor_for(inst: &EffectInstance) -> Vec3 {
    inst.follow_anchor.unwrap_or(inst.anchor)
}

/// Determine an effect instance's normalised "alive-ness"
/// `curve_t` in `[0, 1]` for light gating, or `None` if the
/// light should not be emitted this frame.
///
///   * `follow_particles` — light tracks the live particle
///     population via the smoothed envelope. `t = 0` is peak,
///     `t = 1` is "fully extinguished".
///   * `lifetime: Some(t)` — light has its own wall-clock
///     window independent of particles.
///   * Otherwise — light is implicitly tied to the particle
///     pool: visible while spawning or any pool has live
///     particles, sampled over `elapsed / duration` for
///     finite-duration effects.
fn compute_envelope(inst: &EffectInstance, spec: &Effect, light: &EffectLight) -> Option<f32> {
    if light.follow_particles {
        let live_now: u32 = inst
            .layers
            .iter()
            .map(|l| match l {
                LayerState::Particles(p) => p.pool.len() as u32,
                LayerState::Ribbon(_) => 0,
            })
            .sum();
        if inst.light_envelope < 1e-3 && live_now == 0 && !inst.spawning {
            return None;
        }
        Some((1.0 - inst.light_envelope).clamp(0.0, 1.0))
    } else if let Some(lt) = light.lifetime {
        if inst.elapsed >= lt {
            return None;
        }
        Some((inst.elapsed / lt).clamp(0.0, 1.0))
    } else {
        let any_pool_alive = inst.layers.iter().any(|l| match l {
            LayerState::Particles(p) => !p.pool.is_empty(),
            LayerState::Ribbon(_) => false,
        });
        if !inst.spawning && !any_pool_alive {
            return None;
        }
        let t = if spec.duration > 0.0 {
            (inst.elapsed / spec.duration).clamp(0.0, 1.0)
        } else {
            0.0
        };
        Some(t)
    }
}

/// Push a single [`PointLight`] for an effect's anchor or tip
/// light. Shared between the primary `light` and the optional
/// `tip_light` so the two stay perfectly in lockstep through
/// fades, flicker, and the `follow_particles` envelope.
fn push_effect_light(
    inst: &EffectInstance,
    light: &EffectLight,
    pos: Vec3,
    curve_t: f32,
    time_secs: f32,
    out: &mut Vec<PointLight>,
) {
    let curve_mul = if let Some(curve) = light.intensity_curve.as_ref() {
        curve.sample(curve_t)
    } else if light.follow_particles {
        inst.light_envelope
    } else {
        1.0
    };

    let flicker = if light.flicker_amp > 0.0 && light.flicker_hz > 0.0 {
        let phase_off = ((inst.generation as f32) * 0.6180339).fract() * std::f32::consts::TAU;
        let f = light.flicker_hz;
        let t = time_secs;
        let a = (t * f * std::f32::consts::TAU + phase_off).sin();
        let b = (t * f * 1.43 * std::f32::consts::TAU + phase_off + 1.7).sin();
        1.0 + (a * 0.6 + b * 0.4) * light.flicker_amp
    } else {
        1.0
    };

    let intensity = (light.intensity * curve_mul * flicker).max(0.0);
    if intensity <= 1e-4 || light.radius <= 1e-3 {
        return;
    }
    out.push(PointLight {
        position: pos,
        color: light.color,
        radius: light.radius,
        intensity,
    });
}

fn hash_u32(a: u32, b: u32) -> u32 {
    let mut h = a ^ b.wrapping_mul(2654435761);
    h ^= h >> 16;
    h = h.wrapping_mul(0x45d9f3b);
    h ^= h >> 16;
    h
}

/// Cheap hash-noise sampled at a 3D point. Returns ~[-1, 1].
/// Used by [`ForceField::Curl`].
fn noise3(p: Vec3) -> f32 {
    let i = p.floor();
    let f = p - i;
    let u = f * f * (Vec3::splat(3.0) - 2.0 * f); // smoothstep
    let n = |dx: f32, dy: f32, dz: f32| -> f32 {
        let h = hash3((i.x + dx) as i32, (i.y + dy) as i32, (i.z + dz) as i32);
        // Map to [-1, 1].
        (h as f32 / u32::MAX as f32) * 2.0 - 1.0
    };
    let c000 = n(0.0, 0.0, 0.0);
    let c100 = n(1.0, 0.0, 0.0);
    let c010 = n(0.0, 1.0, 0.0);
    let c110 = n(1.0, 1.0, 0.0);
    let c001 = n(0.0, 0.0, 1.0);
    let c101 = n(1.0, 0.0, 1.0);
    let c011 = n(0.0, 1.0, 1.0);
    let c111 = n(1.0, 1.0, 1.0);
    let x00 = c000 + (c100 - c000) * u.x;
    let x10 = c010 + (c110 - c010) * u.x;
    let x01 = c001 + (c101 - c001) * u.x;
    let x11 = c011 + (c111 - c011) * u.x;
    let y0 = x00 + (x10 - x00) * u.y;
    let y1 = x01 + (x11 - x01) * u.y;
    y0 + (y1 - y0) * u.z
}

fn hash3(x: i32, y: i32, z: i32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B9);
    h ^= (y as u32).wrapping_mul(0x85EB_CA6B);
    h ^= (z as u32).wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^= h >> 16;
    h
}

/// Convenience: `Vec3::normalize` returns `NaN` for zero
/// vectors. glam already exposes `normalize_or` for this; the
/// runtime relies on it directly.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::vfx::spec::*;

    fn simple_burst() -> Effect {
        Effect {
            duration: 0.0,
            layers: vec![Layer::Particles(ParticleSpec {
                spawn: SpawnShape::Sphere,
                emission: EmissionMode::Burst { count: 8 },
                speed: (1.0, 1.0),
                lifetime: (0.5, 0.5),
                forces: vec![],
                size: Curve::constant(0.1),
                color: Gradient::constant([1.0, 1.0, 1.0, 1.0]),
                sprite: SpriteShape::SoftGlow,
                blend: BlendMode::Additive,
                opacity: 1.0,
            })],
        }
    }

    #[test]
    fn burst_appears_after_first_tick() {
        let mut sys = VfxSystem::new(1024);
        let _id = sys.spawn(simple_burst(), Vec3::ZERO);
        sys.tick(0.016, Some((Vec3::ZERO, 100.0)));
        assert_eq!(sys.particle_instances().len(), 8);
    }

    #[test]
    fn pool_drains_after_lifetime() {
        let mut sys = VfxSystem::new(1024);
        let _id = sys.spawn(simple_burst(), Vec3::ZERO);
        for _ in 0..40 {
            sys.tick(0.016, Some((Vec3::ZERO, 100.0)));
        }
        assert_eq!(sys.particle_instances().len(), 0);
    }

    #[test]
    fn ribbon_only_emits_when_endpoints_set() {
        let mut sys = VfxSystem::new(1024);
        let id = sys.spawn(
            Effect {
                duration: 1.0,
                layers: vec![Layer::Ribbon(RibbonSpec {
                    width: 0.5,
                    cross_gradient: Gradient::constant([1.0, 1.0, 1.0, 1.0]),
                    length_gradient: None,
                    noise: None,
                    blend: BlendMode::Additive,
                })],
            },
            Vec3::ZERO,
        );
        sys.tick(0.016, Some((Vec3::ZERO, 100.0)));
        assert!(sys.ribbon_instances().is_empty());
        sys.set_endpoints(id, Vec3::ZERO, Vec3::new(5.0, 0.0, 0.0));
        sys.tick(0.016, Some((Vec3::ZERO, 100.0)));
        assert_eq!(sys.ribbon_instances().len(), 1);
    }
}
