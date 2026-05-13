//! Shared ECS components for server-side actors.
//!
//! Players and enemies keep their own behaviour components, but
//! combat-facing identity and health are common data. Pulling those
//! into small shared components lets cross-cutting systems query the
//! data they actually need instead of borrowing the full player or
//! enemy state bundle.

use rift_net::NetId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NetIdentity {
    pub net_id: NetId,
}

impl NetIdentity {
    pub fn new(net_id: NetId) -> Self {
        Self { net_id }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Vitals {
    pub hp: f32,
    pub hp_max: f32,
}

impl Vitals {
    pub fn new(hp_max: f32) -> Self {
        Self { hp: hp_max, hp_max }
    }

    pub fn is_dead(&self) -> bool {
        self.hp <= 0.0
    }

    pub fn health_pct(&self) -> f32 {
        (self.hp / self.hp_max.max(0.001)).clamp(0.0, 1.0)
    }

    pub fn damage(&mut self, amount: f32) -> f32 {
        if amount <= 0.0 || self.is_dead() {
            return 0.0;
        }
        let before = self.hp;
        self.hp = (self.hp - amount).max(0.0);
        before - self.hp
    }

    pub fn heal(&mut self, amount: f32) -> f32 {
        if amount <= 0.0 || self.is_dead() {
            return 0.0;
        }
        let before = self.hp;
        self.hp = (self.hp + amount).min(self.hp_max);
        self.hp - before
    }

    pub fn fill(&mut self) {
        self.hp = self.hp_max;
    }

    pub fn rescale_max(&mut self, hp_max: f32) {
        let hp_pct = if self.hp_max > 0.0 {
            (self.hp / self.hp_max).clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.hp_max = hp_max;
        self.hp = hp_max * hp_pct;
    }
}
