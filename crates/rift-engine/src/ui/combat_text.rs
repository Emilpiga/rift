use crate::ui::im::{Color, Pos2, Ui, WorldUi};
use glam::{Mat4, Vec3};

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
        Self {
            texts: Vec::new(),
            rng_state: 42,
        }
    }

    fn rand_f32(&mut self) -> f32 {
        self.rng_state = self
            .rng_state
            .wrapping_mul(1664525)
            .wrapping_add(1013904223);
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
            38.0
        } else if damage > 50.0 {
            31.0
        } else {
            24.0
        };

        let drift = self.rand_f32() * 2.0 - 1.0; // -1..1

        self.texts.push(FloatingText {
            world_pos: position + Vec3::new(0.0, 1.5, 0.0),
            text,
            color,
            age: 0.0,
            lifetime: 1.08,
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
            lifetime: 1.0,
            base_size: 27.0,
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
            lifetime: 0.95,
            base_size: 23.0,
            drift_x: drift * 20.0,
            rises: true,
        });
    }

    /// Spawn an arbitrary status notice (e.g. "Inventory full").
    /// Rises like a damage number but with custom text + color so
    /// gameplay systems can surface short, transient warnings to
    /// the local player without needing a dedicated toast widget.
    pub fn spawn_notice(&mut self, position: Vec3, text: impl Into<String>, color: [f32; 4]) {
        self.texts.push(FloatingText {
            world_pos: position + Vec3::new(0.0, 2.2, 0.0),
            text: text.into(),
            color,
            age: 0.0,
            lifetime: 1.6,
            base_size: 22.0,
            drift_x: 0.0,
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

    /// Render all active texts via the immediate-mode UI stack.
    /// Uses [`WorldUi`] for projection so off-screen / behind-camera
    /// anchors are skipped automatically; the per-text fade + pop
    /// animation stays bespoke since no widget covers it.
    pub fn render(&self, ui: &mut Ui<'_>, view_proj: Mat4) {
        let mut wui = WorldUi::new(ui, view_proj);
        for t in &self.texts {
            let progress = t.age / t.lifetime;

            // Vertical movement (world-space; rises for damage,
            // sinks for player-took-damage).
            let vert = if t.rises { t.age * 2.0 } else { -t.age * 1.2 };
            let world_pos = t.world_pos + Vec3::new(0.0, vert, 0.0);

            // Project; bail if off-screen / behind camera.
            let Some(anchor) = wui.world_to_screen(world_pos) else {
                continue;
            };

            // Fade out in last 40%.
            let alpha = if progress > 0.6 {
                1.0 - (progress - 0.6) / 0.4
            } else {
                1.0
            };

            // Scale: quick pop then settle. Slightly larger and
            // slower than the old numbers so hits feel physical
            // without turning into arcade-style splash text.
            let size = if progress < 0.08 {
                t.base_size * (0.62 + progress * 5.75)
            } else if progress < 0.15 {
                t.base_size * (1.0 + (0.15 - progress) * 2.4)
            } else {
                t.base_size * (1.0 - (progress - 0.15) * 0.10)
            };

            let color = Color::rgba(t.color[0], t.color[1], t.color[2], t.color[3] * alpha);
            let inner = wui.ui();
            let tw = inner.measure_text(&t.text, size);
            let px = anchor.x - tw * 0.5 + t.drift_x * progress;
            let py = anchor.y;
            let shadow = Color::rgba(0.0, 0.0, 0.0, 0.72 * alpha);
            inner.draw_text(Pos2::new(px + 2.0, py + 2.0), &t.text, size, shadow);
            inner.draw_text(
                Pos2::new(px - 1.0, py + 2.0),
                &t.text,
                size,
                shadow.fade(0.78),
            );
            inner.draw_text(
                Pos2::new(px, py - 1.0),
                &t.text,
                size,
                Color::rgba(1.0, 1.0, 1.0, 0.16 * alpha),
            );
            inner.draw_text(Pos2::new(px, py), &t.text, size, color);
        }
    }

    pub fn clear(&mut self) {
        self.texts.clear();
    }
}
