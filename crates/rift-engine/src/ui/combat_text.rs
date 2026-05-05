use glam::{Mat4, Vec3};
use crate::renderer::OverlayBatch;

/// A floating damage number that rises and fades.
struct FloatingText {
    world_pos: Vec3,
    text: String,
    color: [f32; 4],
    age: f32,
    /// Total display duration in seconds.
    lifetime: f32,
    /// Base font size (larger for crits/big hits).
    base_size: f32,
    /// Horizontal drift direction (random scatter).
    drift_x: f32,
    /// Whether this text rises (damage) or falls (player hurt).
    rises: bool,
}

/// Manages all active floating damage/heal numbers.
pub struct CombatTextSystem {
    texts: Vec<FloatingText>,
    rng_state: u32,
}

impl CombatTextSystem {
    pub fn new() -> Self {
        Self { texts: Vec::new(), rng_state: 42 }
    }

    fn rand_f32(&mut self) -> f32 {
        self.rng_state = self.rng_state.wrapping_mul(1664525).wrapping_add(1013904223);
        (self.rng_state >> 16) as f32 / 65535.0
    }

    /// Spawn a damage number at a world position.
    pub fn spawn_damage(&mut self, position: Vec3, damage: f32, is_crit: bool) {
        let color = if is_crit {
            [1.0, 0.85, 0.1, 1.0] // bright gold for crits
        } else if damage > 50.0 {
            [1.0, 0.6, 0.2, 1.0] // orange for big hits
        } else {
            [1.0, 1.0, 1.0, 1.0] // white for normal
        };

        let text = if is_crit {
            format!("{}!", damage as u32)
        } else if damage >= 10.0 {
            format!("{}", damage as u32)
        } else {
            format!("{:.1}", damage)
        };

        let base_size = if is_crit {
            32.0
        } else if damage > 50.0 {
            26.0
        } else {
            20.0
        };

        let drift = self.rand_f32() * 2.0 - 1.0; // -1..1

        self.texts.push(FloatingText {
            world_pos: position + Vec3::new(0.0, 1.5, 0.0),
            text,
            color,
            age: 0.0,
            lifetime: 1.0,
            base_size,
            drift_x: drift * 40.0,
            rises: true,
        });
    }

    /// Spawn a player-took-damage number (red, drifts down).
    pub fn spawn_player_damage(&mut self, position: Vec3, damage: f32) {
        let drift = self.rand_f32() * 2.0 - 1.0;
        self.texts.push(FloatingText {
            world_pos: position + Vec3::new(0.0, 2.0, 0.0),
            text: format!("{}", damage as u32),
            color: [1.0, 0.15, 0.15, 1.0],
            age: 0.0,
            lifetime: 0.9,
            base_size: 22.0,
            drift_x: drift * 30.0,
            rises: false,
        });
    }

    /// Spawn a heal number (green).
    pub fn spawn_heal(&mut self, position: Vec3, amount: f32) {
        let drift = self.rand_f32() * 2.0 - 1.0;
        self.texts.push(FloatingText {
            world_pos: position + Vec3::new(0.0, 1.5, 0.0),
            text: format!("+{:.0}", amount),
            color: [0.2, 1.0, 0.3, 1.0],
            age: 0.0,
            lifetime: 0.8,
            base_size: 18.0,
            drift_x: drift * 20.0,
            rises: true,
        });
    }

    /// Advance time and remove expired texts.
    pub fn tick(&mut self, dt: f32) {
        for t in &mut self.texts {
            t.age += dt;
        }
        self.texts.retain(|t| t.age < t.lifetime);
    }

    /// Render all active texts to the overlay batch.
    pub fn render(&self, batch: &mut OverlayBatch, view_proj: Mat4, sw: f32, sh: f32) {
        for t in &self.texts {
            let progress = t.age / t.lifetime;

            // Vertical movement
            let vert = if t.rises {
                t.age * 2.0 // rise upward
            } else {
                -t.age * 1.2 // sink downward for player damage
            };
            let world_pos = t.world_pos + Vec3::new(0.0, vert, 0.0);

            // Project to screen
            let clip = view_proj * world_pos.extend(1.0);
            if clip.w <= 0.0 {
                continue;
            }

            let ndc = clip.truncate() / clip.w;
            if ndc.x < -1.5 || ndc.x > 1.5 || ndc.y < -1.5 || ndc.y > 1.5 {
                continue;
            }

            let px = (ndc.x + 1.0) * 0.5 * sw + t.drift_x * progress;
            let py = (ndc.y + 1.0) * 0.5 * sh;

            // Fade out in last 40%
            let alpha = if progress > 0.6 {
                1.0 - (progress - 0.6) / 0.4
            } else {
                1.0
            };

            // Scale: quick pop then settle
            let size = if progress < 0.08 {
                t.base_size * (0.5 + progress * 6.25) // 50% → 100% in 0.08s
            } else if progress < 0.15 {
                t.base_size * (1.0 + (0.15 - progress) * 3.0) // overshoot then settle
            } else {
                t.base_size * (1.0 - (progress - 0.15) * 0.15) // gentle shrink
            };

            let color = [t.color[0], t.color[1], t.color[2], t.color[3] * alpha];

            let text_w = batch.measure_text(&t.text, size);
            batch.text(&t.text, px - text_w * 0.5, py, size, color, sw, sh);
        }
    }

    pub fn clear(&mut self) {
        self.texts.clear();
    }
}
