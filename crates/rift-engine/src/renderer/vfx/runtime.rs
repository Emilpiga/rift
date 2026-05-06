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
use glam::Vec3;

use super::spec::{
    Effect, EmissionMode, ForceField, Layer, ParticleSpec, RibbonSpec, SpawnShape,
    SpriteShape,
};

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
    /// `4` = ring. Cast from [`SpriteShape`] discriminant.
    pub sprite: u32,
    /// `0` = alpha, `1` = additive, `2` = premultiplied. The
    /// renderer groups by this field so all alpha particles
    /// draw before any additive ones.
    pub blend: u32,
    pub _pad: u32,
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
            origin: anchor,
            tip: anchor,
            brightness: 1.0,
            elapsed: 0.0,
            spawning: true,
            layers,
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

    /// Advance the simulation by `dt` seconds. Spawns new
    /// particles, applies forces, ages out dead particles, and
    /// rebuilds the cached instance buffers.
    pub fn tick(&mut self, dt: f32) {
        self.particle_instances.clear();
        self.ribbon_instances.clear();
        let total_cap = self.max_particles;

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

                // 1) Tick particle layers.
                for layer in inst.layers.iter_mut() {
                    if let LayerState::Particles(p) = layer {
                        tick_particles(p, anchor, dt, inst.spawning, total_cap);
                    }
                }

                // 2) Drop the slot once not spawning *and* every
                //    particle pool has drained. Ribbons disappear
                //    immediately when `spawning` flips false (no
                //    pool to drain) — that's the contract `despawn`
                //    relies on for persistent (duration == 0) beams
                //    like Frost Ray.
                let any_pool_alive = inst.layers.iter().any(|l| match l {
                    LayerState::Particles(p) => !p.pool.is_empty(),
                    LayerState::Ribbon(_) => false,
                });
                inst.spawning || any_pool_alive
            };
            if !alive {
                self.free_slot(slot);
            }
        }

        // 3) Rebuild instance buffers. We do this after the tick
        //    so freed slots are excluded.
        for inst in &self.instances {
            if inst.spec.is_none() {
                continue;
            }
            let anchor = inst.follow_anchor.unwrap_or(inst.anchor);
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
                        let (noise_strength, noise_scroll, noise_tile, noise_octaves) =
                            match r.spec.noise {
                                Some(n) => (n.strength, n.scroll, n.tile.max(1e-3), n.octaves as f32),
                                None => (0.0, 0.0, 1.0, 1.0),
                            };
                        self.ribbon_instances.push(VfxRibbonInstance {
                            origin: [
                                inst.origin.x,
                                inst.origin.y,
                                inst.origin.z,
                                r.spec.width,
                            ],
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
fn tick_particles(p: &mut ParticlesState, anchor: Vec3, dt: f32, spawning: bool, total_cap: usize) {
    // 1) Spawn — only when the effect is still allowed to.
    if spawning {
        let to_spawn = match p.spec.emission {
            EmissionMode::Continuous { rate } => {
                p.spawn_acc += rate * dt;
                let n = p.spawn_acc.floor();
                p.spawn_acc -= n;
                n as u32
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
                p.spawn_acc += rate * dt;
                let n = p.spawn_acc.floor();
                p.spawn_acc -= n;
                initial + n as u32
            }
        };

        for _ in 0..to_spawn {
            if p.pool.len() >= total_cap.max(1) {
                break;
            }
            p.pool.push(spawn_one(&p.spec, anchor, &mut p.rng_seed));
        }
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
            apply_force(force, &mut position, &mut velocity, origin, noise_phase, dt2);
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
            let n = axis.normalize_or_zero();
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
        ForceField::Curl { frequency, strength } => {
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

fn spawn_one(spec: &ParticleSpec, anchor: Vec3, rng: &mut u32) -> LiveParticle {
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
            (Vec3::ZERO, (axis * angle.cos() + lateral).normalize_or(axis))
        }
        SpawnShape::Column { radius, height, axis } => {
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
        SpawnShape::Ring { radius, thickness } => {
            let theta = r3 * std::f32::consts::TAU;
            let radial = lerp(radius - thickness * 0.5, radius + thickness * 0.5, r4);
            let off = Vec3::new(theta.cos() * radial, 0.0, theta.sin() * radial);
            // Outward direction in XZ.
            let dir = Vec3::new(theta.cos(), 0.0, theta.sin());
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

    LiveParticle {
        position: anchor + offset,
        velocity: direction * speed,
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
        out.push(VfxParticleInstance {
            position: q.position.to_array(),
            size,
            color: col,
            seed: q.seed,
            sprite,
            blend,
            _pad: 0,
        });
    }
}

// ─── Math helpers ─────────────────────────────────────────────────────────

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

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
        let h = hash3(
            (i.x + dx) as i32,
            (i.y + dy) as i32,
            (i.z + dz) as i32,
        );
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
        sys.tick(0.016);
        assert_eq!(sys.particle_instances().len(), 8);
    }

    #[test]
    fn pool_drains_after_lifetime() {
        let mut sys = VfxSystem::new(1024);
        let _id = sys.spawn(simple_burst(), Vec3::ZERO);
        for _ in 0..40 {
            sys.tick(0.016);
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
        sys.tick(0.016);
        assert!(sys.ribbon_instances().is_empty());
        sys.set_endpoints(id, Vec3::ZERO, Vec3::new(5.0, 0.0, 0.0));
        sys.tick(0.016);
        assert_eq!(sys.ribbon_instances().len(), 1);
    }
}
