/// Unique identifier for an ability.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AbilityId {
    // ─── Hunter abilities ─────────────────────
    /// Basic attack: shoot a single arrow.
    SteadyShot,
    /// Multi-shot: fires 3 arrows in a spread.
    MultiShot,
    /// Rapid Fire: channel a burst of fast arrows in one direction.
    RapidFire,
    /// Rain of Arrows: AoE arrow barrage at target location.
    RainOfArrows,
    /// Evasive Roll: dodge roll, brief invulnerability.
    EvasiveRoll,
    /// Mark for Death: debuff target, increased damage taken.
    MarkForDeath,
}

/// How the ability is targeted before firing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TargetingMode {
    /// Fires immediately in the aim direction (default).
    Instant,
    /// Shows a ground circle preview; player clicks to confirm placement.
    Placed { radius: f32 },
}

/// Static definition of an ability.
#[derive(Clone, Debug)]
pub struct Ability {
    pub id: AbilityId,
    pub name: &'static str,
    pub description: &'static str,
    /// Cooldown in seconds.
    pub cooldown: f32,
    /// Resource cost (if any — could be mana/focus/etc.).
    pub resource_cost: f32,
    /// Base damage multiplier (as % of weapon damage).
    pub damage_mult: f32,
    /// Number of projectiles.
    pub projectile_count: u32,
    /// Spread angle in radians (for multi-projectile abilities).
    pub spread_angle: f32,
    /// Range.
    pub range: f32,
    /// Level at which this ability unlocks.
    pub unlock_level: u32,
    /// Duration of effect (for buffs/debuffs), 0 = instant.
    pub duration: f32,
    /// How this ability is targeted.
    pub targeting: TargetingMode,
}

/// Runtime state for an ability on the action bar.
#[derive(Clone, Debug)]
pub struct AbilityState {
    pub ability: Ability,
    pub cooldown_remaining: f32,
    pub charges: u32,
    pub max_charges: u32,
}

impl AbilityState {
    pub fn new(ability: Ability) -> Self {
        let charges = if ability.cooldown > 0.0 { 1 } else { 0 };
        Self {
            ability,
            cooldown_remaining: 0.0,
            charges,
            max_charges: charges,
        }
    }

    pub fn ready(&self) -> bool {
        self.cooldown_remaining <= 0.0
    }

    pub fn try_use(&mut self) -> bool {
        if self.ready() {
            self.cooldown_remaining = self.ability.cooldown;
            true
        } else {
            false
        }
    }

    pub fn tick(&mut self, dt: f32) {
        if self.cooldown_remaining > 0.0 {
            self.cooldown_remaining = (self.cooldown_remaining - dt).max(0.0);
        }
    }

    pub fn cooldown_progress(&self) -> f32 {
        if self.ability.cooldown <= 0.0 { return 1.0; }
        1.0 - (self.cooldown_remaining / self.ability.cooldown)
    }
}

/// The player's action bar — 6 ability slots (like Diablo 3).
#[derive(Clone, Debug)]
pub struct AbilitySlot {
    pub slots: [Option<AbilityState>; 6],
}

impl AbilitySlot {
    pub fn new() -> Self {
        Self { slots: Default::default() }
    }

    pub fn set(&mut self, index: usize, ability: Ability) {
        if index < 6 {
            self.slots[index] = Some(AbilityState::new(ability));
        }
    }

    pub fn tick_all(&mut self, dt: f32) {
        for slot in &mut self.slots {
            if let Some(state) = slot {
                state.tick(dt);
            }
        }
    }

    pub fn try_use(&mut self, index: usize) -> Option<&Ability> {
        if index >= 6 { return None; }
        if let Some(state) = &mut self.slots[index] {
            if state.try_use() {
                return Some(&state.ability);
            }
        }
        None
    }
}

// ─── Hunter ability definitions ──────────────────────────────────────────────

impl Ability {
    pub fn steady_shot() -> Self {
        Self {
            id: AbilityId::SteadyShot,
            name: "Steady Shot",
            description: "Fire a precise arrow at the target.",
            cooldown: 0.5, // ~2 attacks per second base rate
            resource_cost: 0.0,
            damage_mult: 1.0,
            projectile_count: 1,
            spread_angle: 0.0,
            range: 12.0,
            unlock_level: 1,
            duration: 0.0,
            targeting: TargetingMode::Instant,
        }
    }

    pub fn multi_shot() -> Self {
        Self {
            id: AbilityId::MultiShot,
            name: "Multi-Shot",
            description: "Fire 3 arrows in a wide spread.",
            cooldown: 4.0,
            resource_cost: 15.0,
            damage_mult: 0.7,
            projectile_count: 3,
            spread_angle: 0.5, // ~30 degrees total spread
            range: 10.0,
            unlock_level: 3,
            duration: 0.0,
            targeting: TargetingMode::Instant,
        }
    }

    pub fn rapid_fire() -> Self {
        Self {
            id: AbilityId::RapidFire,
            name: "Rapid Fire",
            description: "Channel a burst of 6 rapid arrows.",
            cooldown: 8.0,
            resource_cost: 25.0,
            damage_mult: 0.5,
            projectile_count: 6,
            spread_angle: 0.08, // Tight grouping
            range: 12.0,
            unlock_level: 7,
            duration: 1.0, // 1 second channel
            targeting: TargetingMode::Instant,
        }
    }

    pub fn rain_of_arrows() -> Self {
        Self {
            id: AbilityId::RainOfArrows,
            name: "Rain of Arrows",
            description: "Call down a rain of arrows in an area.",
            cooldown: 12.0,
            resource_cost: 35.0,
            damage_mult: 0.4,
            projectile_count: 12,
            spread_angle: 0.0, // AoE, not spread
            range: 15.0,
            unlock_level: 12,
            duration: 2.0, // Arrows rain over 2 seconds
            targeting: TargetingMode::Placed { radius: 3.0 },
        }
    }

    pub fn evasive_roll() -> Self {
        Self {
            id: AbilityId::EvasiveRoll,
            name: "Evasive Roll",
            description: "Dodge roll in movement direction. Brief invulnerability.",
            cooldown: 6.0,
            resource_cost: 0.0,
            damage_mult: 0.0,
            projectile_count: 0,
            spread_angle: 0.0,
            range: 0.0,
            unlock_level: 5,
            duration: 0.3, // 0.3s invuln frames
            targeting: TargetingMode::Instant,
        }
    }

    pub fn mark_for_death() -> Self {
        Self {
            id: AbilityId::MarkForDeath,
            name: "Mark for Death",
            description: "Mark target. They take 25% increased damage for 6s.",
            cooldown: 15.0,
            resource_cost: 20.0,
            damage_mult: 0.0,
            projectile_count: 0,
            spread_angle: 0.0,
            range: 20.0,
            unlock_level: 10,
            duration: 6.0,
            targeting: TargetingMode::Instant,
        }
    }

    /// Get all hunter abilities (ordered by unlock level).
    pub fn hunter_abilities() -> Vec<Self> {
        vec![
            Self::steady_shot(),
            Self::multi_shot(),
            Self::evasive_roll(),
            Self::rapid_fire(),
            Self::mark_for_death(),
            Self::rain_of_arrows(),
        ]
    }
}
