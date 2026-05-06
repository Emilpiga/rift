//! Abilities — declarative gameplay data.
//!
//! This module owns the `Ability` shape, the wire-id table used by
//! the server for cast dispatch (`id::*`, `AbilityKind`, `lookup`),
//! and the Hunter ability roster. The engine consumes `Ability` and
//! its declarative `AbilityEffect` list as plain data.

use crate::components::PlayerAction;

// ─── Engine-consumed declarative ability shape ───────────────────────────

/// Opaque identifier for an ability. Treated as a hashable key only —
/// concrete IDs are defined below as constants.
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

/// Where on the caster a projectile spawns relative to the body.
#[derive(Clone, Copy, Debug)]
pub struct SpawnOffset {
    pub forward: f32,
    pub up: f32,
    pub right: f32,
}

impl SpawnOffset {
    pub const HAND: Self = Self {
        forward: 0.55,
        up: 1.25,
        right: 0.30,
    };
    pub const ROOT: Self = Self {
        forward: 0.0,
        up: 0.0,
        right: 0.0,
    };
}

/// A small, named subset of the engine particle presets that abilities
/// may declare. The engine maps these to concrete emitter configs.
#[derive(Clone, Copy, Debug)]
pub enum ParticlePreset {
    DodgePuff,
    /// Falling fire bombardment used by the "Rain of Fire" AoE.
    /// Sustained over the ability's duration so the visual stays
    /// up while the zone ticks damage.
    RainOfFire,
    Cast([f32; 3]),
}

/// How the player moves while a `SetPlayerAction` is held.
#[derive(Clone, Copy, Debug)]
pub enum ActionMovement {
    Forward(f32),
    Frozen,
    None,
}

/// One declarative effect of an ability. The data variants below are
/// preserved for the ability roster; `SpawnProjectiles` and
/// `SpawnAoeZone` are server-authoritative — the client runtime
/// ignores them. `SetPlayerAction` and `Custom` run on the client.
#[derive(Clone, Copy, Debug)]
pub enum AbilityEffect {
    SpawnProjectiles {
        count: u32,
        spread: f32,
        damage_mult: f32,
        pierce: u32,
        spawn_offset: SpawnOffset,
    },
    SpawnAoeZone {
        radius: f32,
        damage_mult: f32,
        duration: f32,
        tick_interval: f32,
        visual: Option<ParticlePreset>,
        visual_y: f32,
    },
    SetPlayerAction {
        action: PlayerAction,
        duration: f32,
        clip: &'static [&'static str],
        movement: ActionMovement,
        cancel_cast: bool,
        emitter: Option<ParticlePreset>,
    },
}

/// Static definition of an ability.
#[derive(Clone, Debug)]
pub struct Ability {
    pub id: AbilityId,
    /// Wire-stable u8 id, matching one of `id::*`. Sent to the
    /// server when the player presses the slot key. Decoupled from
    /// the slot index so the action bar can be rearranged without
    /// reshuffling cooldown / dispatch state.
    pub wire_id: u8,
    pub name: &'static str,
    pub description: &'static str,
    /// Filename stem (no extension) of the icon under
    /// `assets/icons/` to render in HUD slots / tooltips.
    /// `None` falls back to a coloured placeholder + 2-letter
    /// abbreviation. Co-located with the ability so adding a
    /// new ability + icon is a single-file change.
    pub icon: Option<&'static str>,
    /// Cooldown in seconds.
    pub cooldown: f32,
    /// Resource cost (if any).
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
    /// Declarative effect list executed by the engine's runtime.
    pub effects: &'static [AbilityEffect],
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

// ─── Wire-side ability table (server cast dispatch) ──────────────────────
//
// Stable u8 ids for abilities on the wire. Never reorder, only append.
// The server's `ability_table` keys off these and the client's slot
// bar maps them onto button presses.

pub mod id {
    pub const STEADY_SHOT: u8 = 0;
    pub const MULTI_SHOT: u8 = 1;
    pub const EVASIVE_ROLL: u8 = 2;
    pub const RAPID_FIRE: u8 = 3;
    pub const MARK_FOR_DEATH: u8 = 4;
    pub const RAIN_OF_ARROWS: u8 = 5;
    pub const FROST_RAY: u8 = 6;
    pub const WHIRLWIND: u8 = 7;
}

/// What an ability does on the authoritative side. Visuals are not
/// modelled here — only damage / spawn behaviour.
#[derive(Clone, Copy, Debug)]
pub enum AbilityKind {
    /// Spawn `count` projectiles in a fan along aim_dir.
    Projectiles {
        count: u32,
        spread: f32,
        speed: f32,
        ttl: f32,
        pierce: u32,
        /// Apply this debuff to every enemy the projectile hits.
        apply_debuff: Option<u8>,
    },
    /// Persistent area-of-effect zone at `placed_target`.
    AoeZone {
        radius: f32,
        duration: f32,
        tick_interval: f32,
        /// Apply this debuff on every enemy each tick hits.
        apply_debuff: Option<u8>,
    },
    /// Channeled ability: while active, fires [`ChannelEffect`] every
    /// `tick_interval` seconds for `duration` seconds. The caster is
    /// locked into a cast pose for the duration; movement may still
    /// be allowed depending on the engine-side `SetPlayerAction`
    /// effect (we don't model that here).
    Channel {
        duration: f32,
        tick_interval: f32,
        effect: ChannelEffect,
        /// Apply this debuff to every enemy struck by a tick.
        apply_debuff: Option<u8>,
        /// If `true`, any horizontal movement input cancels the
        /// channel server-side. Set `false` for self-AoE channels
        /// you want the player to walk during.
        cancel_on_move: bool,
    },
    /// Pure visual / movement on the client side, no server-side damage.
    ClientOnly,
}

/// Per-tick effect of a [`AbilityKind::Channel`]. Designed to grow
/// with new patterns — add a variant + a server arm in
/// `sim::channel::tick`.
#[derive(Clone, Copy, Debug)]
pub enum ChannelEffect {
    /// Damage every enemy within `radius` of the caster's current
    /// position. Use for self-centred AoEs (Whirlwind, Sanctified
    /// Ground, Frost Nova).
    AuraAroundCaster {
        radius: f32,
        damage_per_tick: f32,
    },
    /// Damage every enemy inside a forward beam of length `range`
    /// and half-width `width`. Use for Ray of Frost-style channels.
    /// `pierce_targets` caps how many enemies one tick can hit
    /// (sorted nearest-first); `0` means “stop at the first enemy”.
    Beam {
        range: f32,
        width: f32,
        damage_per_tick: f32,
        pierce_targets: u32,
    },
}

/// Static description of one ability for server dispatch.
#[derive(Clone, Copy, Debug)]
pub struct AbilityDef {
    pub id: u8,
    pub kind: AbilityKind,
    /// Cooldown in seconds.
    pub cooldown: f32,
    /// Base damage per hit (or per AoE tick).
    pub base_damage: f32,
}

/// Look up the static description for an ability id.
pub fn lookup(ability_id: u8) -> Option<AbilityDef> {
    use id::*;
    use crate::debuffs;
    use AbilityKind::*;
    use ChannelEffect::*;
    Some(match ability_id {
        STEADY_SHOT => AbilityDef {
            id: ability_id,
            kind: Projectiles { count: 1, spread: 0.0, speed: 20.0, ttl: 2.0, pierce: 0, apply_debuff: None },
            cooldown: 0.5,
            base_damage: 8.0,
        },
        MULTI_SHOT => AbilityDef {
            id: ability_id,
            kind: Projectiles { count: 3, spread: 0.5, speed: 20.0, ttl: 2.0, pierce: 0, apply_debuff: Some(debuffs::id::SLOW) },
            cooldown: 4.0,
            base_damage: 8.0 * 0.7,
        },
        RAPID_FIRE => AbilityDef {
            id: ability_id,
            kind: Projectiles { count: 6, spread: 0.08, speed: 20.0, ttl: 2.0, pierce: 0, apply_debuff: None },
            cooldown: 8.0,
            base_damage: 8.0 * 0.5,
        },
        RAIN_OF_ARROWS => AbilityDef {
            id: ability_id,
            kind: AoeZone { radius: 3.0, duration: 2.0, tick_interval: 0.5, apply_debuff: Some(debuffs::id::BURN) },
            cooldown: 12.0,
            base_damage: 8.0 * 0.4,
        },
        FROST_RAY => AbilityDef {
            id: ability_id,
            kind: Channel {
                duration: f32::INFINITY,
                tick_interval: 0.2,
                effect: Beam { range: 9.0, width: 0.6, damage_per_tick: 1.6, pierce_targets: 2 },
                apply_debuff: Some(debuffs::id::CHILL),
                cancel_on_move: true,
            },
            cooldown: 0.0,
            base_damage: 1.6,
        },
        WHIRLWIND => AbilityDef {
            id: ability_id,
            kind: Channel {
                duration: 2.0,
                tick_interval: 0.25,
                effect: AuraAroundCaster { radius: 2.5, damage_per_tick: 2.2 },
                apply_debuff: None,
                cancel_on_move: false,
            },
            cooldown: 9.0,
            base_damage: 2.2,
        },
        EVASIVE_ROLL | MARK_FOR_DEATH => AbilityDef {
            id: ability_id,
            kind: ClientOnly,
            cooldown: 0.0,
            base_damage: 0.0,
        },
        _ => return None,
    })
}

/// Build the full `Ability` definition for the given wire id. Used by
/// the client to (a) locally trigger the cast animation when the
/// player presses an ability key, and (b) play the cast animation on
/// remote avatars when a `WorldEvent::AbilityCast` is received.
pub fn from_wire_id(ability_id: u8) -> Option<Ability> {
    use id::*;
    Some(match ability_id {
        STEADY_SHOT => steady_shot(),
        MULTI_SHOT => multi_shot(),
        RAPID_FIRE => rapid_fire(),
        RAIN_OF_ARROWS => rain_of_arrows(),
        EVASIVE_ROLL => evasive_roll(),
        MARK_FOR_DEATH => mark_for_death(),
        FROST_RAY => frost_ray(),
        WHIRLWIND => whirlwind(),
        _ => return None,
    })
}


pub const STEADY_SHOT: AbilityId = AbilityId("steady_shot");
pub const MULTI_SHOT: AbilityId = AbilityId("multi_shot");
pub const RAPID_FIRE: AbilityId = AbilityId("rapid_fire");
pub const RAIN_OF_ARROWS: AbilityId = AbilityId("rain_of_arrows");
pub const EVASIVE_ROLL: AbilityId = AbilityId("evasive_roll");
pub const MARK_FOR_DEATH: AbilityId = AbilityId("mark_for_death");

pub fn steady_shot() -> Ability {
    Ability {
        id: STEADY_SHOT,
        wire_id: id::STEADY_SHOT,
        name: "Steady Shot",
        description: "Fire a precise arrow at the target.",
        icon: Some("Hunter_3"),
        cooldown: 0.5,
        resource_cost: 0.0,
        damage_mult: 1.0,
        projectile_count: 1,
        spread_angle: 0.0,
        range: 12.0,
        unlock_level: 1,
        duration: 0.0,
        targeting: TargetingMode::Instant,
        effects: &[AbilityEffect::SpawnProjectiles {
            count: 1,
            spread: 0.0,
            damage_mult: 1.0,
            pierce: 0,
            spawn_offset: SpawnOffset::HAND,
        }],
    }
}

pub fn multi_shot() -> Ability {
    Ability {
        id: MULTI_SHOT,
        wire_id: id::MULTI_SHOT,
        name: "Multi-Shot",
        description: "Fire 3 arrows in a wide spread.",
        icon: Some("Hunter_18"),
        cooldown: 4.0,
        resource_cost: 15.0,
        damage_mult: 0.7,
        projectile_count: 3,
        spread_angle: 0.5,
        range: 10.0,
        unlock_level: 3,
        duration: 0.0,
        targeting: TargetingMode::Instant,
        effects: &[AbilityEffect::SpawnProjectiles {
            count: 3,
            spread: 0.5,
            damage_mult: 0.7,
            pierce: 0,
            spawn_offset: SpawnOffset::HAND,
        }],
    }
}

pub fn rapid_fire() -> Ability {
    Ability {
        id: RAPID_FIRE,
        wire_id: id::RAPID_FIRE,
        name: "Rapid Fire",
        description: "Channel a burst of 6 rapid arrows.",
        icon: None,
        cooldown: 8.0,
        resource_cost: 25.0,
        damage_mult: 0.5,
        projectile_count: 6,
        spread_angle: 0.08,
        range: 12.0,
        unlock_level: 7,
        duration: 1.0,
        targeting: TargetingMode::Instant,
        effects: &[AbilityEffect::SpawnProjectiles {
            count: 6,
            spread: 0.08,
            damage_mult: 0.5,
            pierce: 0,
            spawn_offset: SpawnOffset::HAND,
        }],
    }
}

pub fn rain_of_arrows() -> Ability {
    Ability {
        id: RAIN_OF_ARROWS,
        wire_id: id::RAIN_OF_ARROWS,
        name: "Rain of Fire",
        description: "Call down a rain of fire in an area. Burns enemies caught inside.",
        icon: Some("FireMage_35"),
        cooldown: 12.0,
        resource_cost: 35.0,
        damage_mult: 0.4,
        projectile_count: 12,
        spread_angle: 0.0,
        range: 15.0,
        unlock_level: 12,
        duration: 2.0,
        targeting: TargetingMode::Placed { radius: 3.0 },
        effects: &[AbilityEffect::SpawnAoeZone {
            radius: 3.0,
            damage_mult: 0.4,
            duration: 2.0,
            tick_interval: 0.5,
            visual: Some(ParticlePreset::RainOfFire),
            visual_y: 5.0,
        }],
    }
}

pub fn evasive_roll() -> Ability {
    Ability {
        id: EVASIVE_ROLL,
        wire_id: id::EVASIVE_ROLL,
        name: "Evasive Roll",
        description: "Dodge roll in movement direction. Brief invulnerability.",
        icon: Some("Monk_27"),
        cooldown: 6.0,
        resource_cost: 0.0,
        damage_mult: 0.0,
        projectile_count: 0,
        spread_angle: 0.0,
        range: 0.0,
        unlock_level: 5,
        duration: 0.3,
        targeting: TargetingMode::Instant,
        effects: &[AbilityEffect::SetPlayerAction {
            action: PlayerAction::Roll,
            duration: 0.95,
            clip: &["Roll", "Roll_Forward", "Dodge_Roll", "Dodge"],
            movement: ActionMovement::Forward(11.0),
            cancel_cast: true,
            emitter: Some(ParticlePreset::DodgePuff),
        }],
    }
}

pub fn mark_for_death() -> Ability {
    Ability {
        id: MARK_FOR_DEATH,
        wire_id: id::MARK_FOR_DEATH,
        name: "Mark for Death",
        description: "Mark target. They take 25% increased damage for 6s.",
        icon: None,
        cooldown: 15.0,
        resource_cost: 20.0,
        damage_mult: 0.0,
        projectile_count: 0,
        spread_angle: 0.0,
        range: 20.0,
        unlock_level: 10,
        duration: 6.0,
        targeting: TargetingMode::Instant,
        effects: &[],
    }
}

pub const FROST_RAY: AbilityId = AbilityId("frost_ray");
pub const WHIRLWIND: AbilityId = AbilityId("whirlwind");

/// Channeled forward beam. Server applies damage + Chill every
/// 0.2s while the player holds the action key. No cooldown and
/// effectively infinite duration — a future resource system will
/// gate how long the channel can run.
pub fn frost_ray() -> Ability {
    Ability {
        id: FROST_RAY,
        wire_id: id::FROST_RAY,
        name: "Frost Ray",
        description: "Channel a freezing beam. Slows and damages.",
        icon: Some("FrostMage_7"),
        cooldown: 0.0,
        resource_cost: 0.0,
        damage_mult: 0.2,
        projectile_count: 0,
        spread_angle: 0.0,
        range: 9.0,
        unlock_level: 6,
        // Effectively infinite — channel ends only on key release,
        // movement cancel, or (future) resource depletion.
        duration: f32::INFINITY,
        targeting: TargetingMode::Instant,
        effects: &[],
    }
}

/// Channeled self-AoE. Server damages everything in a 2.5m radius
/// every 0.25s for 2s.
pub fn whirlwind() -> Ability {
    Ability {
        id: WHIRLWIND,
        wire_id: id::WHIRLWIND,
        name: "Whirlwind",
        description: "Spin in place, damaging nearby foes.",
        icon: Some("Barbarian_18"),
        cooldown: 9.0,
        resource_cost: 25.0,
        damage_mult: 0.275,
        projectile_count: 0,
        spread_angle: 0.0,
        range: 2.5,
        unlock_level: 8,
        duration: 2.0,
        targeting: TargetingMode::Instant,
        effects: &[],
    }
}

/// Full hunter roster, ordered for the action bar. Slot order is
/// independent of wire ids — keys 1..=5 + LMB just look up the
/// ability in this array and dispatch by `Ability::wire_id`.
pub fn hunter_roster() -> [Ability; 6] {
    [
        steady_shot(),
        multi_shot(),
        evasive_roll(),
        whirlwind(),
        frost_ray(),
        rain_of_arrows(),
    ]
}
