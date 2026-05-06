use bytemuck::{Pod, Zeroable};
use glam::Vec3;

/// GPU instance data for a single particle (matches shader layout).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct ParticleInstance {
    pub position: [f32; 3],   // world position
    pub color: [f32; 4],      // RGBA
    pub size_life: [f32; 2],  // size, life (0..1)
}

/// CPU-side particle with physics state.
#[derive(Clone, Debug)]
pub struct Particle {
    pub position: Vec3,
    pub velocity: Vec3,
    pub color: [f32; 4],
    pub size: f32,
    pub life: f32,       // remaining life in seconds
    pub max_life: f32,   // total life (for computing 0..1 ratio)
    pub gravity: f32,    // downward acceleration
    pub drag: f32,       // velocity damping per second (0 = none, 1 = full stop)
    pub size_end: f32,   // size at end of life (interpolated)
    pub color_end: [f32; 4], // color at end of life
    pub origin: Vec3,    // orbit center (XZ plane)
    pub orbital_speed: f32, // radians per second around origin Y axis
}

impl Particle {
    pub fn alive(&self) -> bool {
        self.life > 0.0
    }

    pub fn life_ratio(&self) -> f32 {
        if self.max_life > 0.0 {
            (self.life / self.max_life).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

/// Configuration for a particle emitter.
#[derive(Clone, Debug)]
pub struct EmitterConfig {
    pub spawn_rate: f32,           // particles per second (0 = burst only)
    pub burst_count: u32,          // particles to spawn immediately
    pub lifetime: (f32, f32),      // min, max particle lifetime
    pub speed: (f32, f32),         // min, max initial speed
    pub size: (f32, f32),          // start size min, max
    pub size_end: (f32, f32),      // end size min, max
    pub color_start: [f32; 4],     // start color
    pub color_end: [f32; 4],       // end color (fades to this)
    pub gravity: f32,              // downward force
    pub drag: f32,                 // velocity damping
    pub spread: EmitterSpread,     // emission shape
    pub direction: Vec3,           // primary emission direction
    pub one_shot: bool,            // if true, emitter dies after burst
    pub orbital_speed: (f32, f32), // min, max radians/sec for spiral motion
    pub duration: f32,             // max emitter lifetime in seconds (0 = infinite)
}

/// How particles are spread from the emitter.
#[derive(Clone, Debug)]
pub enum EmitterSpread {
    /// Particles go in all directions equally.
    Sphere,
    /// Particles spread within a cone (half-angle in radians).
    Cone(f32),
    /// Particles distributed in a cylindrical column (radius XZ, height Y).
    Column { radius: f32, height: f32 },
}

impl Default for EmitterConfig {
    fn default() -> Self {
        Self {
            spawn_rate: 10.0,
            burst_count: 0,
            orbital_speed: (0.0, 0.0),
            lifetime: (0.5, 1.5),
            speed: (1.0, 3.0),
            size: (0.1, 0.2),
            size_end: (0.0, 0.05),
            color_start: [1.0, 1.0, 1.0, 1.0],
            color_end: [1.0, 1.0, 1.0, 0.0],
            gravity: 0.0,
            drag: 0.0,
            spread: EmitterSpread::Sphere,
            direction: Vec3::Y,
            one_shot: false,
            duration: 0.0,
        }
    }
}

/// A particle emitter in world space.
pub struct Emitter {
    pub position: Vec3,
    pub config: EmitterConfig,
    pub active: bool,
    spawn_accumulator: f32,
    elapsed: f32,
    seed: u32,
}

impl Emitter {
    pub fn new(position: Vec3, config: EmitterConfig) -> Self {
        Self {
            position,
            config,
            active: true,
            spawn_accumulator: 0.0,
            elapsed: 0.0,
            seed: 0,
        }
    }

    /// Spawn initial burst particles. Call once when emitter is created.
    pub fn burst(&mut self, particles: &mut Vec<Particle>) {
        for _ in 0..self.config.burst_count {
            particles.push(self.spawn_one());
        }
        if self.config.one_shot {
            self.active = false;
        }
    }

    /// Tick emitter, spawning particles based on rate.
    pub fn tick(&mut self, dt: f32, particles: &mut Vec<Particle>) {
        if !self.active {
            return;
        }
        // Auto-deactivate after duration
        if self.config.duration > 0.0 {
            self.elapsed += dt;
            if self.elapsed >= self.config.duration {
                self.active = false;
                return;
            }
        }
        if self.config.spawn_rate <= 0.0 {
            return;
        }

        self.spawn_accumulator += self.config.spawn_rate * dt;
        while self.spawn_accumulator >= 1.0 {
            self.spawn_accumulator -= 1.0;
            particles.push(self.spawn_one());
        }
    }

    fn spawn_one(&mut self) -> Particle {
        self.seed = self.seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let r1 = hash_f32(self.seed, 0);
        let r2 = hash_f32(self.seed, 1);
        let r3 = hash_f32(self.seed, 2);
        let r4 = hash_f32(self.seed, 3);
        let r5 = hash_f32(self.seed, 4);

        let lifetime = lerp(self.config.lifetime.0, self.config.lifetime.1, r1);
        let speed = lerp(self.config.speed.0, self.config.speed.1, r2);
        let size = lerp(self.config.size.0, self.config.size.1, r3);
        let size_end = lerp(self.config.size_end.0, self.config.size_end.1, r3);

        let direction = match &self.config.spread {
            EmitterSpread::Sphere => {
                // Random direction on unit sphere
                let theta = r4 * std::f32::consts::TAU;
                let phi = (r5 * 2.0 - 1.0).acos();
                Vec3::new(
                    phi.sin() * theta.cos(),
                    phi.sin() * theta.sin(),
                    phi.cos(),
                )
            }
            EmitterSpread::Cone(half_angle) => {
                // Random within cone around config.direction
                let angle = r4 * half_angle;
                let rot = r5 * std::f32::consts::TAU;
                let perp = if self.config.direction.y.abs() < 0.99 {
                    self.config.direction.cross(Vec3::Y).normalize()
                } else {
                    self.config.direction.cross(Vec3::X).normalize()
                };
                let perp2 = self.config.direction.cross(perp);
                let offset = (perp * rot.cos() + perp2 * rot.sin()) * angle.sin();
                (self.config.direction * angle.cos() + offset).normalize()
            }
            EmitterSpread::Column { radius, height: _ } => {
                // Upward with slight random horizontal offset
                let theta = r4 * std::f32::consts::TAU;
                let r = r5 * radius;
                let offset = Vec3::new(theta.cos() * r, 0.0, theta.sin() * r);
                (self.config.direction + offset * 0.1).normalize()
            }
        };

        let r6 = hash_f32(self.seed, 5);
        let orbital = lerp(self.config.orbital_speed.0, self.config.orbital_speed.1, r6);
        // Randomize orbital direction
        let orbital = if r4 > 0.5 { orbital } else { -orbital };

        Particle {
            position: self.position + match &self.config.spread {
                EmitterSpread::Column { radius, height } => {
                    let theta = r4 * std::f32::consts::TAU;
                    let r = r5 * radius;
                    let r7 = hash_f32(self.seed, 6);
                    let y_offset = r7 * height;
                    Vec3::new(theta.cos() * r, y_offset, theta.sin() * r)
                }
                _ => Vec3::ZERO,
            },
            velocity: direction * speed,
            color: self.config.color_start,
            size,
            life: lifetime,
            max_life: lifetime,
            gravity: self.config.gravity,
            drag: self.config.drag,
            size_end,
            color_end: self.config.color_end,
            origin: self.position,
            orbital_speed: orbital,
        }
    }
}

/// The particle system: manages all emitters and the particle pool.
pub struct ParticleSystem {
    pub particles: Vec<Particle>,
    pub emitters: Vec<Emitter>,
    max_particles: usize,
}

impl ParticleSystem {
    pub fn new(max_particles: usize) -> Self {
        Self {
            particles: Vec::with_capacity(max_particles),
            emitters: Vec::new(),
            max_particles,
        }
    }

    /// Add an emitter and trigger its initial burst.
    pub fn add_emitter(&mut self, mut emitter: Emitter) -> usize {
        emitter.burst(&mut self.particles);
        self.emitters.push(emitter);
        self.emitters.len() - 1
    }

    /// Remove all emitters (particles in flight will naturally die).
    pub fn clear_emitters(&mut self) {
        self.emitters.clear();
    }

    /// Remove a specific emitter by index.
    pub fn remove_emitter(&mut self, index: usize) {
        if index < self.emitters.len() {
            self.emitters.swap_remove(index);
        }
    }

    /// Deactivate an emitter (stops spawning, existing particles fade naturally).
    pub fn deactivate_emitter(&mut self, index: usize) {
        if index < self.emitters.len() {
            self.emitters[index].active = false;
            self.emitters[index].config.spawn_rate = 0.0;
        }
    }

    /// Tick: advance particles, spawn new ones, remove dead ones.
    pub fn tick(&mut self, dt: f32) {
        // Spawn from emitters
        for emitter in &mut self.emitters {
            emitter.tick(dt, &mut self.particles);
        }

        // Remove dead emitters — only those that have no outstanding index references.
        // We never shrink the emitter list to keep stored indices stable.
        // Instead, trim only from the end if they are inactive.
        while self.emitters.last().map_or(false, |e| !e.active && e.config.spawn_rate == 0.0) {
            self.emitters.pop();
        }

        // Update particles
        for p in &mut self.particles {
            if !p.alive() {
                continue;
            }

            p.life -= dt;
            if p.life <= 0.0 {
                continue;
            }

            // Physics
            p.velocity.y -= p.gravity * dt;
            if p.drag > 0.0 {
                let factor = (1.0 - p.drag * dt).max(0.0);
                p.velocity *= factor;
            }
            p.position += p.velocity * dt;

            // Orbital motion (spiral around origin Y axis)
            if p.orbital_speed.abs() > 0.01 {
                let dx = p.position.x - p.origin.x;
                let dz = p.position.z - p.origin.z;
                let angle = p.orbital_speed * dt;
                let cos_a = angle.cos();
                let sin_a = angle.sin();
                p.position.x = p.origin.x + dx * cos_a - dz * sin_a;
                p.position.z = p.origin.z + dx * sin_a + dz * cos_a;
            }

            // Interpolate color and size based on life ratio
            let t = 1.0 - p.life_ratio(); // 0 at start, 1 at end
            p.color = lerp_color(p.color, p.color_end, t);
            p.size = lerp(p.size, p.size_end, t);
        }

        // Remove dead particles
        self.particles.retain(|p| p.alive());

        // Cap particles
        if self.particles.len() > self.max_particles {
            self.particles.truncate(self.max_particles);
        }
    }

    /// Get instance data for GPU upload.
    pub fn instance_data(&self) -> Vec<ParticleInstance> {
        self.particles
            .iter()
            .filter(|p| p.alive())
            .map(|p| ParticleInstance {
                position: [p.position.x, p.position.y, p.position.z],
                color: p.color,
                size_life: [p.size, p.life_ratio()],
            })
            .collect()
    }

    pub fn particle_count(&self) -> usize {
        self.particles.len()
    }
}

// --- Preset emitter configs ---

impl EmitterConfig {
    /// Loot beam: D3-style pillar of light — dense column of fine rising sparkles.
    pub fn loot_beam(color: [f32; 3]) -> Self {
        Self {
            spawn_rate: 150.0,
            burst_count: 40,
            lifetime: (0.8, 2.0),
            speed: (2.5, 5.0),
            size: (0.03, 0.07),
            size_end: (0.01, 0.03),
            color_start: [color[0] * 1.5, color[1] * 1.5, color[2] * 1.5, 1.0],
            color_end: [color[0] * 0.3, color[1] * 0.3, color[2] * 0.3, 0.0],
            gravity: -3.5, // strong upward pull
            drag: 0.8,
            spread: EmitterSpread::Column { radius: 0.08, height: 6.0 },
            direction: Vec3::Y,
            one_shot: false,
            orbital_speed: (4.0, 8.0), // tight fast spiral
            duration: 0.0,
        }
    }

    /// Loot beam base pulse: bright burst at the item's feet.
    pub fn loot_beam_base(color: [f32; 3]) -> Self {
        Self {
            spawn_rate: 25.0,
            burst_count: 8,
            lifetime: (0.3, 0.8),
            speed: (0.5, 1.5),
            size: (0.04, 0.1),
            size_end: (0.08, 0.2),
            color_start: [color[0] * 2.0, color[1] * 2.0, color[2] * 2.0, 1.0],
            color_end: [color[0] * 0.5, color[1] * 0.5, color[2] * 0.5, 0.0],
            gravity: -0.5,
            drag: 2.0,
            spread: EmitterSpread::Sphere,
            direction: Vec3::Y,
            one_shot: false,
            orbital_speed: (0.0, 0.0),
            duration: 0.0,
        }
    }

    /// Hit spark: burst of fast particles spreading outward.
    pub fn hit_spark(color: [f32; 3]) -> Self {
        Self {
            spawn_rate: 0.0,
            burst_count: 12,
            lifetime: (0.15, 0.4),
            speed: (3.0, 6.0),
            size: (0.04, 0.08),
            size_end: (0.0, 0.02),
            color_start: [color[0], color[1], color[2], 1.0],
            color_end: [color[0] * 0.5, color[1] * 0.5, color[2] * 0.5, 0.0],
            gravity: 8.0,
            drag: 2.0,
            spread: EmitterSpread::Sphere,
            direction: Vec3::Y,
            one_shot: true,
            orbital_speed: (0.0, 0.0),
            duration: 0.0,
        }
    }

    /// Targeting ring: fast-orbiting particles forming a visible ground circle.
    pub fn targeting_ring() -> Self {
        Self {
            spawn_rate: 60.0,
            burst_count: 20,
            lifetime: (0.4, 0.8),
            speed: (0.1, 0.3),
            size: (0.08, 0.15),
            size_end: (0.02, 0.06),
            color_start: [0.3, 1.0, 0.3, 1.0],
            color_end: [0.1, 0.8, 0.1, 0.0],
            gravity: 0.0,
            drag: 1.0,
            spread: EmitterSpread::Column { radius: 1.0, height: 0.3 },
            direction: Vec3::Y,
            one_shot: false,
            orbital_speed: (8.0, 12.0),
            duration: 0.0,
        }
    }

    /// Death explosion: big burst of particles.
    /// Continuous "aura" emitter for status debuffs (poison, mark for
    /// death, burn, slow).  Emits a steady stream of small floating
    /// motes in the given color so it's instantly readable that the
    /// enemy is afflicted.  `rate` is particles/sec, `size` is the
    /// peak particle radius.
    pub fn aura(rgb: [f32; 3], rate: f32, size: f32) -> Self {
        Self {
            spawn_rate: rate,
            burst_count: 0,
            lifetime: (0.45, 0.85),
            speed: (0.4, 1.2),
            size: (size * 0.4, size),
            size_end: (0.0, size * 0.1),
            color_start: [rgb[0], rgb[1], rgb[2], 0.85],
            color_end: [rgb[0] * 0.7, rgb[1] * 0.7, rgb[2] * 0.7, 0.0],
            gravity: -1.5, // motes drift upward
            drag: 1.2,
            spread: EmitterSpread::Column { radius: 0.45, height: 0.6 },
            direction: Vec3::Y,
            one_shot: false,
            orbital_speed: (0.4, 1.0),
            duration: 0.0,
        }
    }

    pub fn death_burst(color: [f32; 3]) -> Self {
        Self {
            spawn_rate: 0.0,
            burst_count: 40,
            lifetime: (0.4, 1.2),
            speed: (3.0, 7.0),
            size: (0.08, 0.2),
            size_end: (0.15, 0.35), // particles GROW as they splatter
            color_start: [color[0], color[1], color[2], 1.0],
            color_end: [color[0] * 0.4, color[1] * 0.2, color[2] * 0.2, 0.0],
            gravity: 10.0, // heavy — splats on ground quickly
            drag: 2.0,
            spread: EmitterSpread::Sphere,
            direction: Vec3::Y,
            one_shot: true,
            orbital_speed: (0.0, 0.0),
            duration: 0.0,
        }
    }

    /// Heal: rising green sparkles.
    pub fn heal() -> Self {
        Self {
            spawn_rate: 15.0,
            burst_count: 5,
            lifetime: (0.5, 1.0),
            speed: (0.5, 1.5),
            size: (0.06, 0.12),
            size_end: (0.0, 0.03),
            color_start: [0.2, 1.0, 0.3, 0.8],
            color_end: [0.1, 0.8, 0.2, 0.0],
            gravity: -1.0,
            drag: 0.5,
            spread: EmitterSpread::Cone(0.3),
            direction: Vec3::Y,
            one_shot: true,
            orbital_speed: (1.0, 2.0),
            duration: 0.0,
        }
    }

    /// Exit portal: swirling blue-white vortex.
    pub fn portal_vortex() -> Self {
        Self {
            spawn_rate: 40.0,
            burst_count: 20,
            lifetime: (0.8, 1.8),
            speed: (0.5, 1.5),
            size: (0.06, 0.14),
            size_end: (0.0, 0.03),
            color_start: [0.3, 0.6, 1.0, 0.9],
            color_end: [0.8, 0.95, 1.0, 0.0],
            gravity: -0.3,
            drag: 0.4,
            spread: EmitterSpread::Column { radius: 0.8, height: 2.0 },
            direction: Vec3::Y,
            one_shot: false,
            orbital_speed: (4.0, 8.0),
            duration: 0.0,
        }
    }

    /// Arrow trail: short-lived particles left behind projectiles.
    pub fn arrow_trail() -> Self {
        Self {
            spawn_rate: 0.0,
            burst_count: 3,
            lifetime: (0.15, 0.35),
            speed: (0.3, 0.8),
            size: (0.05, 0.1),
            size_end: (0.0, 0.02),
            color_start: [1.0, 0.85, 0.3, 0.9],
            color_end: [1.0, 0.4, 0.0, 0.0],
            gravity: 1.5,
            drag: 2.5,
            spread: EmitterSpread::Sphere,
            direction: Vec3::ZERO,
            one_shot: true,
            orbital_speed: (0.0, 0.0),
            duration: 0.0,
        }
    }

    /// Rain of Fire: continuous downpour of flaming embers over a
    /// circular area, used by the "Rain of Fire" AoE ability.
    /// Particles spawn at the top of a 5 m column and fall under
    /// gravity, fading from bright yellow to dark red as they
    /// burn out. The emitter `duration` matches the ability's
    /// damage zone duration so the visual ends with the zone.
    pub fn rain_of_fire() -> Self {
        Self {
            spawn_rate: 90.0,
            burst_count: 14,
            // Long lifetime so embers launched from the top of the
            // column have time to fall the full ~5 m under gravity
            // before they expire. Drag is kept low for the same
            // reason — too much drag and the particles plateau
            // mid-air and fade out before reaching the ground.
            lifetime: (1.4, 1.9),
            // Strong initial downward velocity so the column looks
            // like it's actively raining instead of drifting.
            speed: (6.0, 10.0),
            size: (0.18, 0.32),
            size_end: (0.04, 0.10),
            // Bright yellow-orange flame core fading to a dim,
            // smouldering red. Alpha eases out so embers don't
            // pop off-screen at end-of-life.
            color_start: [1.6, 0.8, 0.15, 1.0],
            color_end:   [0.9, 0.10, 0.0, 0.0],
            gravity: 22.0,
            drag: 0.05,
            spread: EmitterSpread::Column { radius: 3.0, height: 0.5 },
            direction: -Vec3::Y,
            one_shot: false,
            orbital_speed: (0.0, 0.0),
            // Match the ability's zone duration so the visual
            // stops the same moment the damage ticks stop. The
            // particle simulator keeps already-spawned embers
            // alive until their own lifetime expires.
            duration: 2.0,
        }
    }

    /// Elite enemy aura: persistent golden sparkles orbiting the enemy.
    pub fn elite_aura() -> Self {
        Self {
            spawn_rate: 35.0,
            burst_count: 20,
            lifetime: (0.8, 1.6),
            speed: (0.3, 0.8),
            size: (0.18, 0.28),
            size_end: (0.04, 0.08),
            color_start: [1.5, 1.2, 0.3, 1.0],
            color_end: [1.0, 0.3, 0.0, 0.0],
            gravity: -0.5,
            drag: 1.0,
            spread: EmitterSpread::Column { radius: 0.7, height: 1.4 },
            direction: Vec3::Y,
            one_shot: false,
            orbital_speed: (3.0, 5.0),
            duration: 0.0, // permanent until entity dies
        }
    }

    /// Evasive roll: brief afterimage puff.
    pub fn dodge_puff() -> Self {
        Self {
            spawn_rate: 0.0,
            burst_count: 8,
            lifetime: (0.2, 0.4),
            speed: (1.0, 2.5),
            size: (0.1, 0.2),
            size_end: (0.2, 0.4),
            color_start: [0.6, 0.8, 1.0, 0.6],
            color_end: [0.4, 0.6, 0.9, 0.0],
            gravity: -0.5,
            drag: 3.0,
            spread: EmitterSpread::Sphere,
            direction: Vec3::ZERO,
            one_shot: true,
            orbital_speed: (0.0, 0.0),
            duration: 0.0,
        }
    }

    /// Frost Ray beam trail: tiny one-shot puff of icy particles that
    /// drift forward along the beam, used to add motion / pulse to
    /// the otherwise-static beam mesh.
    ///
    /// Spawn at random points along the beam every frame; `direction`
    /// is the beam's forward unit vector.
    pub fn frost_beam_spark(direction: Vec3) -> Self {
        Self {
            spawn_rate: 0.0,
            burst_count: 3,
            lifetime: (0.18, 0.35),
            speed: (2.0, 4.5),
            size: (0.06, 0.12),
            size_end: (0.0, 0.03),
            color_start: [0.75, 0.95, 1.0, 0.95],
            color_end: [0.40, 0.70, 1.0, 0.0],
            gravity: -1.5,
            drag: 4.0,
            spread: EmitterSpread::Cone(0.55), // ~31° half-angle
            direction,
            one_shot: true,
            orbital_speed: (0.0, 0.0),
            duration: 0.0,
        }
    }

    /// Frost Ray impact: cold burst on each pierced target and at
    /// the beam's terminal point (wall / end of range).
    pub fn frost_impact() -> Self {
        Self {
            spawn_rate: 0.0,
            burst_count: 14,
            lifetime: (0.22, 0.55),
            speed: (2.5, 5.0),
            size: (0.08, 0.15),
            size_end: (0.0, 0.04),
            color_start: [0.80, 0.95, 1.0, 1.0],
            color_end: [0.30, 0.55, 0.95, 0.0],
            gravity: 4.0,
            drag: 3.5,
            spread: EmitterSpread::Sphere,
            direction: Vec3::Y,
            one_shot: true,
            orbital_speed: (0.0, 0.0),
            duration: 0.0,
        }
    }
}

// --- Utility ---

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_color(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [
        lerp(a[0], b[0], t),
        lerp(a[1], b[1], t),
        lerp(a[2], b[2], t),
        lerp(a[3], b[3], t),
    ]
}

fn hash_f32(seed: u32, offset: u32) -> f32 {
    let h = seed.wrapping_add(offset.wrapping_mul(2654435761));
    let h = h ^ (h >> 16);
    let h = h.wrapping_mul(0x45d9f3b);
    let h = h ^ (h >> 16);
    (h & 0x00FF_FFFF) as f32 / 16777215.0
}
