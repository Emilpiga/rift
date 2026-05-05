/// Opaque identifier for an ability. Engine treats this as a hashable
/// key only — concrete IDs are defined by the game (see
/// `rift-game/src/abilities.rs`). New classes / mods can introduce new
/// IDs without touching the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AbilityId(pub &'static str);

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
    /// Declarative effect list executed by `ability_runtime::execute_ability`.
    /// Each constructor populates this; new abilities only add new
    /// effect entries here, no engine-side dispatch code needed.
    pub effects: &'static [crate::combat::ability_runtime::AbilityEffect],
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

