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

/// Named handle for a renderer particle effect. The engine
/// (`rift-engine`'s `effect_for_vfx`) maps each variant to a
/// concrete `vfx::presets::*()` `Effect`. Adding a new visual is
/// "add variant + add arm" — no `if ability_id == X` branches.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VfxKind {
    /// Single-shot puff for Evasive Roll.
    DodgePuff,
    /// Falling fire bombardment used by the "Rain of Fire" AoE.
    /// Sustained over the ability's duration so the visual stays
    /// up while the zone ticks damage.
    RainOfFire,
    /// Brief cast-time hand spark, tinted by the caller-supplied
    /// RGB.
    CastSpark { rgb: [f32; 3] },
    /// Warm orange comet trail that follows a player fireball.
    FireballTrail,
    /// Warm orange burst played at fireball detonation.
    FireballImpact,
    /// Violet/indigo trail for enemy caster bolts.
    CasterBoltTrail,
    /// Violet/indigo burst played at caster-bolt detonation.
    CasterBoltImpact,
    /// Sustained ribbon-and-glow effect rendered along the Frost
    /// Ray beam.
    FrostRay,
    /// Self-centred expanding ring of fire used by Fire Wave —
    /// a one-shot blast that grows outward from the caster.
    FireWave,
    /// Placeholder for shapes that don't draw an emitter today
    /// (e.g. Whirlwind aura). Mapped to an empty `Effect`.
    None,
}

/// Named handle for a renderer mesh used by projectile-shaped
/// abilities. The engine's `mesh_for_kind` builds the actual
/// `Mesh`. New projectile look = "add variant + add arm".
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MeshKind {
    /// Warm orange `Mesh::fireball` (player projectiles).
    Fireball,
    /// Smaller violet `Mesh::caster_bolt` (enemy bolts).
    CasterBolt,
}

/// Per-ability-shape visual recipe. Exactly one variant is
/// populated; the variant must match the ability's
/// authoritative [`AbilityKind`]. The client's renderer pattern-
/// matches on this to drive per-frame visuals — no hardcoded
/// `if ability_id == X` branches anywhere.
#[derive(Clone, Copy, Debug)]
pub enum ShapeVisuals {
    /// Spawnable projectile (player fireball, enemy bolt).
    Projectile {
        mesh: MeshKind,
        trail: VfxKind,
        impact: VfxKind,
        /// Uniform world-space scale applied to the mesh.
        scale: f32,
    },
    /// Ground-locked AoE zone (Rain of Fire). The visual sits at
    /// `visual_y` metres above the placed target.
    AoeZone {
        effect: VfxKind,
        visual_y: f32,
    },
    /// Forward beam from the caster's hand (Frost Ray).
    Beam {
        effect: VfxKind,
        /// Vertical offset from the caster's body to the beam
        /// origin when no skinned hand-joint is available.
        hand_offset: f32,
    },
    /// Self-centred aura around the caster (Whirlwind). The
    /// effect is anchored to the caster every frame.
    Aura {
        effect: VfxKind,
    },
    /// No bespoke shape visual — only `cast_spark` (if any) and
    /// the `effects` list run.
    None,
}

/// Top-level visual recipe for an ability: a one-shot cast
/// flash plus a per-shape recipe.
#[derive(Clone, Copy, Debug)]
pub struct AbilityVisuals {
    /// Brief glow on the caster's hand at cast start.
    pub cast_spark: Option<VfxKind>,
    /// Per-shape visual driven by the simulation snapshot.
    pub shape: ShapeVisuals,
}

impl AbilityVisuals {
    /// Empty visual recipe — used for abilities whose only
    /// rendering happens through the `effects` list (e.g.
    /// Mark for Death, Evasive Roll).
    pub const NONE: Self = Self {
        cast_spark: None,
        shape: ShapeVisuals::None,
    };
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
        visual: Option<VfxKind>,
        visual_y: f32,
    },
    SetPlayerAction {
        action: PlayerAction,
        duration: f32,
        clip: &'static [&'static str],
        movement: ActionMovement,
        cancel_cast: bool,
        emitter: Option<VfxKind>,
    },
    /// Spawn a one-shot client-only VFX at the caster's position.
    /// Used for self-centred channels / auras (Fire Wave) where
    /// the visual should sit on the caster regardless of aim or
    /// targeting mode — unlike `SpawnAoeZone` which uses the
    /// `placed_target` (or aim-forward fallback).
    SpawnEmitterAtCaster {
        visual: VfxKind,
        /// Vertical offset from the caster's feet.
        height: f32,
    },
}

/// Damage element of an ability. Drives elemental gear scaling
/// (`Stat::FireDamage` etc.) — when an ability hits, the server
/// looks up the stat matching `element` on the caster and
/// multiplies into the damage pipeline.
///
/// `Physical` is the default for weapon-flavoured abilities;
/// `None` is for utility abilities (movement, debuff-only,
/// pure proc) that don't carry damage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Element {
    Physical,
    Fire,
    Ice,
    Lightning,
    /// No element — utility / debuff / movement abilities. The
    /// damage pipe skips elemental scaling entirely for these.
    None,
}

/// Shape of an ability. Drives archetype gear scaling
/// (`Stat::ProjectileDamage` etc.). Mirrors the structural
/// taxonomy in [`AbilityKind`] but in a flatter form a single
/// `match` can key off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Archetype {
    /// Discrete projectiles (Fireball, Multi-Shot, Rapid Fire).
    Projectile,
    /// Sustained beam channel (Frost Ray).
    Beam,
    /// Persistent or instant area-of-effect (Rain of Fire).
    Aoe,
    /// Self-centred melee aura (Whirlwind).
    Melee,
    /// Pure movement (Evasive Roll). Doesn't scale with any
    /// archetype stat.
    Movement,
    /// Utility / debuff that doesn't fit a damage shape (Mark
    /// for Death). No archetype scaling.
    Utility,
}

/// Which damage-bucket stat the ability scales with. Weapons
/// always carry both `WeaponDamage` and `SpellDamage` lines so a
/// single bow + staff inventory works for any loadout — only the
/// magnitudes differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scaling {
    /// Physical / martial. Taps `Stat::WeaponDamage`.
    Weapon,
    /// Magical. Taps `Stat::SpellDamage`.
    Spell,
    /// No bucket scaling (utility / movement).
    None,
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
    /// Base damage per hit (or per AoE/channel tick), pre-scaling.
    /// The server scales this by the caster's gear/attribute
    /// multiplier on the way to applying damage.
    pub base_damage: f32,
    /// Display-friendly damage scalar shown in HUD tooltips
    /// (`Dmg: 70%`). Authored alongside `base_damage`; the actual
    /// damage number is `base_damage`, not `base_damage *
    /// damage_mult`.
    pub damage_mult: f32,
    /// Display-only projectile count for HUD tooltips. The
    /// authoritative count for projectile abilities lives in
    /// `kind` (`AbilityKind::Projectiles { count, ... }`).
    pub projectile_count: u32,
    /// Display-only spread angle for HUD tooltips. Authoritative
    /// value lives in `kind` for projectile abilities.
    pub spread_angle: f32,
    /// Display-only range for HUD tooltips. The authoritative
    /// range is encoded in `kind` (projectile speed * ttl, beam
    /// `range`, etc.) — this field is purely cosmetic.
    pub range: f32,
    /// Level at which this ability unlocks.
    pub unlock_level: u32,
    /// Damage element. Drives elemental gear scaling on hit
    /// (Fire/Ice/Lightning/Physical % stats); `Element::None`
    /// for utility abilities that don't carry damage.
    pub element: Element,
    /// Damage shape. Drives archetype gear scaling
    /// (Projectile/Beam/AoE/Melee % stats); `Archetype::Movement`
    /// or `Archetype::Utility` opt out of archetype scaling.
    pub archetype: Archetype,
    /// Which damage-bucket stat the ability scales with
    /// (`WeaponDamage` vs `SpellDamage`). `Scaling::None` for
    /// utility abilities.
    pub scaling: Scaling,
    /// Display-only duration for HUD tooltips. Authoritative
    /// duration lives in `kind` for AoE / Channel abilities.
    pub duration: f32,
    /// How this ability is targeted (instant vs placed AoE preview).
    pub targeting: TargetingMode,
    /// Authoritative server-side behaviour: projectiles, AoE
    /// zone, channel, or client-only. The server's cast dispatch
    /// switches on this enum; the client uses it to decide
    /// whether to play a cast pose, whether to gate input on a
    /// channel, etc.
    pub kind: AbilityKind,
    /// Declarative visual recipe — mesh / trail / impact for
    /// projectile shapes, ribbon for beams, persistent emitter
    /// for AoE zones / auras, plus an optional cast-time hand
    /// spark. The renderer consumes this directly; no hardcoded
    /// per-ability branches.
    pub visuals: AbilityVisuals,
    /// Declarative client-side effect list (cast pose, dodge
    /// puff, AoE visual). The engine's `ability_runtime` walks
    /// this list when the ability fires locally.
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
    pub const FIRE_BALL: u8 = 0;
    pub const MULTI_SHOT: u8 = 1;
    pub const EVASIVE_ROLL: u8 = 2;
    pub const RAPID_FIRE: u8 = 3;
    pub const MARK_FOR_DEATH: u8 = 4;
    pub const RAIN_OF_ARROWS: u8 = 5;
    pub const FROST_RAY: u8 = 6;
    pub const WHIRLWIND: u8 = 7;
    pub const FIRE_WAVE: u8 = 8;

    // Enemy ability ids start at 64 to leave room for player
    // abilities to grow without colliding. Wire is u8 so the
    // upper half is plenty. Clients dispatch projectile mesh /
    // VFX off these ids in `world_sync` — never reorder, only
    // append.
    pub const ENEMY_CASTER_BOLT: u8 = 64;
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

// ─── Single ability registry ────────────────────────────────────────────
//
// All player + enemy ability data lives in `REGISTRY`. The entries
// are written inline as `const Ability` values so adding a new
// ability is one append here (plus optionally a new icon / mesh /
// VFX preset). `lookup` is a linear scan — at this scale it's
// faster than any hashing.
//
// `AbilityId` string constants are kept around because talents
// reference them as identity tokens.

pub const FIRE_BALL: AbilityId = AbilityId("fire_ball");
pub const MULTI_SHOT: AbilityId = AbilityId("multi_shot");
pub const RAPID_FIRE: AbilityId = AbilityId("rapid_fire");
pub const RAIN_OF_ARROWS: AbilityId = AbilityId("rain_of_arrows");
pub const EVASIVE_ROLL: AbilityId = AbilityId("evasive_roll");
pub const MARK_FOR_DEATH: AbilityId = AbilityId("mark_for_death");
pub const FROST_RAY: AbilityId = AbilityId("frost_ray");
pub const WHIRLWIND: AbilityId = AbilityId("whirlwind");
pub const FIRE_WAVE: AbilityId = AbilityId("fire_wave");

/// Master ability table. Server cast dispatch and client cooldown
/// UI both read from here. Order is purely cosmetic — `lookup`
/// scans by `wire_id`.
pub static REGISTRY: &[Ability] = &[
    Ability {
        id: FIRE_BALL,
        wire_id: id::FIRE_BALL,
        name: "Fireball",
        description: "Launch a fiery projectile at the target.",
        icon: Some("FireMage_12"),
        cooldown: 0.5,
        resource_cost: 0.0,
        base_damage: 8.0,
        damage_mult: 1.0,
        projectile_count: 1,
        spread_angle: 0.0,
        range: 12.0,
        unlock_level: 1,
        element: Element::Fire,
        archetype: Archetype::Projectile,
        scaling: Scaling::Spell,
        duration: 0.0,
        targeting: TargetingMode::Instant,
        kind: AbilityKind::Projectiles {
            count: 1,
            spread: 0.0,
            speed: 20.0,
            ttl: 2.0,
            pierce: 0,
            apply_debuff: None,
        },
        visuals: AbilityVisuals {
            cast_spark: None,
            shape: ShapeVisuals::Projectile {
                mesh: MeshKind::Fireball,
                trail: VfxKind::FireballTrail,
                impact: VfxKind::FireballImpact,
                scale: 0.6,
            },
        },
        effects: &[AbilityEffect::SpawnProjectiles {
            count: 1,
            spread: 0.0,
            damage_mult: 1.0,
            pierce: 0,
            spawn_offset: SpawnOffset::HAND,
        }],
    },
    Ability {
        id: MULTI_SHOT,
        wire_id: id::MULTI_SHOT,
        name: "Multi-Shot",
        description: "Fire 3 arrows in a wide spread.",
        icon: Some("Hunter_18"),
        cooldown: 4.0,
        resource_cost: 15.0,
        base_damage: 8.0 * 0.7,
        damage_mult: 0.7,
        projectile_count: 3,
        spread_angle: 0.5,
        range: 10.0,
        unlock_level: 3,
        element: Element::Physical,
        archetype: Archetype::Projectile,
        scaling: Scaling::Weapon,
        duration: 0.0,
        targeting: TargetingMode::Instant,
        kind: AbilityKind::Projectiles {
            count: 3,
            spread: 0.5,
            speed: 20.0,
            ttl: 2.0,
            pierce: 0,
            apply_debuff: Some(crate::debuffs::id::SLOW),
        },
        visuals: AbilityVisuals {
            cast_spark: None,
            shape: ShapeVisuals::Projectile {
                mesh: MeshKind::Fireball,
                trail: VfxKind::FireballTrail,
                impact: VfxKind::FireballImpact,
                scale: 0.6,
            },
        },
        effects: &[AbilityEffect::SpawnProjectiles {
            count: 3,
            spread: 0.5,
            damage_mult: 0.7,
            pierce: 0,
            spawn_offset: SpawnOffset::HAND,
        }],
    },
    Ability {
        id: RAPID_FIRE,
        wire_id: id::RAPID_FIRE,
        name: "Rapid Fire",
        description: "Channel a burst of 6 rapid arrows.",
        icon: None,
        cooldown: 8.0,
        resource_cost: 25.0,
        base_damage: 8.0 * 0.5,
        damage_mult: 0.5,
        projectile_count: 6,
        spread_angle: 0.08,
        range: 12.0,
        unlock_level: 7,
        element: Element::Physical,
        archetype: Archetype::Projectile,
        scaling: Scaling::Weapon,
        duration: 1.0,
        targeting: TargetingMode::Instant,
        kind: AbilityKind::Projectiles {
            count: 6,
            spread: 0.08,
            speed: 20.0,
            ttl: 2.0,
            pierce: 0,
            apply_debuff: None,
        },
        visuals: AbilityVisuals {
            cast_spark: None,
            shape: ShapeVisuals::Projectile {
                mesh: MeshKind::Fireball,
                trail: VfxKind::FireballTrail,
                impact: VfxKind::FireballImpact,
                scale: 0.6,
            },
        },
        effects: &[AbilityEffect::SpawnProjectiles {
            count: 6,
            spread: 0.08,
            damage_mult: 0.5,
            pierce: 0,
            spawn_offset: SpawnOffset::HAND,
        }],
    },
    Ability {
        id: RAIN_OF_ARROWS,
        wire_id: id::RAIN_OF_ARROWS,
        name: "Rain of Fire",
        description: "Call down a rain of fire in an area. Burns enemies caught inside.",
        icon: Some("FireMage_35"),
        cooldown: 12.0,
        resource_cost: 35.0,
        base_damage: 8.0 * 0.4,
        damage_mult: 0.4,
        projectile_count: 12,
        spread_angle: 0.0,
        range: 15.0,
        unlock_level: 12,
        element: Element::Fire,
        archetype: Archetype::Aoe,
        scaling: Scaling::Spell,
        duration: 2.0,
        targeting: TargetingMode::Placed { radius: 3.0 },
        kind: AbilityKind::AoeZone {
            radius: 3.0,
            duration: 2.0,
            tick_interval: 0.5,
            apply_debuff: Some(crate::debuffs::id::BURN),
        },
        visuals: AbilityVisuals {
            cast_spark: None,
            shape: ShapeVisuals::AoeZone {
                effect: VfxKind::RainOfFire,
                visual_y: 5.0,
            },
        },
        effects: &[AbilityEffect::SpawnAoeZone {
            radius: 3.0,
            damage_mult: 0.4,
            duration: 2.0,
            tick_interval: 0.5,
            visual: Some(VfxKind::RainOfFire),
            visual_y: 5.0,
        }],
    },
    Ability {
        id: EVASIVE_ROLL,
        wire_id: id::EVASIVE_ROLL,
        name: "Evasive Roll",
        description: "Dodge roll in movement direction. Brief invulnerability.",
        icon: Some("Monk_27"),
        cooldown: 6.0,
        resource_cost: 0.0,
        base_damage: 0.0,
        damage_mult: 0.0,
        projectile_count: 0,
        spread_angle: 0.0,
        range: 0.0,
        unlock_level: 5,
        element: Element::None,
        archetype: Archetype::Movement,
        scaling: Scaling::None,
        duration: 0.3,
        targeting: TargetingMode::Instant,
        kind: AbilityKind::ClientOnly,
        visuals: AbilityVisuals::NONE,
        effects: &[AbilityEffect::SetPlayerAction {
            action: PlayerAction::Roll,
            duration: 0.95,
            clip: &["Roll", "Roll_Forward", "Dodge_Roll", "Dodge"],
            movement: ActionMovement::Forward(11.0),
            cancel_cast: true,
            emitter: Some(VfxKind::DodgePuff),
        }],
    },
    Ability {
        id: MARK_FOR_DEATH,
        wire_id: id::MARK_FOR_DEATH,
        name: "Mark for Death",
        description: "Mark target. They take 25% increased damage for 6s.",
        icon: None,
        cooldown: 15.0,
        resource_cost: 20.0,
        base_damage: 0.0,
        damage_mult: 0.0,
        projectile_count: 0,
        spread_angle: 0.0,
        range: 20.0,
        unlock_level: 10,
        element: Element::None,
        archetype: Archetype::Utility,
        scaling: Scaling::None,
        duration: 6.0,
        targeting: TargetingMode::Instant,
        kind: AbilityKind::ClientOnly,
        visuals: AbilityVisuals::NONE,
        effects: &[],
    },
    Ability {
        id: FROST_RAY,
        wire_id: id::FROST_RAY,
        name: "Frost Ray",
        description: "Channel a freezing beam. Slows and damages.",
        icon: Some("FrostMage_7"),
        cooldown: 0.0,
        resource_cost: 0.0,
        base_damage: 1.6,
        damage_mult: 0.2,
        projectile_count: 0,
        spread_angle: 0.0,
        range: 9.0,
        unlock_level: 6,
        element: Element::Ice,
        archetype: Archetype::Beam,
        scaling: Scaling::Spell,
        // Effectively infinite — channel ends only on key release,
        // movement cancel, or (future) resource depletion.
        duration: f32::INFINITY,
        targeting: TargetingMode::Instant,
        kind: AbilityKind::Channel {
            duration: f32::INFINITY,
            tick_interval: 0.2,
            effect: ChannelEffect::Beam {
                range: 9.0,
                width: 0.6,
                damage_per_tick: 1.6,
                pierce_targets: 2,
            },
            apply_debuff: Some(crate::debuffs::id::CHILL),
            cancel_on_move: true,
        },
        visuals: AbilityVisuals {
            cast_spark: None,
            shape: ShapeVisuals::Beam {
                effect: VfxKind::FrostRay,
                hand_offset: 1.25,
            },
        },
        effects: &[],
    },
    Ability {
        id: WHIRLWIND,
        wire_id: id::WHIRLWIND,
        name: "Whirlwind",
        description: "Spin in place, damaging nearby foes.",
        icon: Some("Barbarian_18"),
        cooldown: 9.0,
        resource_cost: 25.0,
        base_damage: 2.2,
        damage_mult: 0.275,
        projectile_count: 0,
        spread_angle: 0.0,
        range: 2.5,
        unlock_level: 8,
        element: Element::Physical,
        archetype: Archetype::Melee,
        scaling: Scaling::Weapon,
        duration: 2.0,
        targeting: TargetingMode::Instant,
        kind: AbilityKind::Channel {
            duration: 2.0,
            tick_interval: 0.25,
            effect: ChannelEffect::AuraAroundCaster {
                radius: 2.5,
                damage_per_tick: 2.2,
            },
            apply_debuff: None,
            cancel_on_move: false,
        },
        visuals: AbilityVisuals {
            cast_spark: None,
            shape: ShapeVisuals::Aura {
                effect: VfxKind::None,
            },
        },
        effects: &[],
    },
    Ability {
        id: FIRE_WAVE,
        wire_id: id::FIRE_WAVE,
        name: "Fire Wave",
        description: "Release an expanding ring of fire that scorches all nearby enemies and ignites them.",
        icon: Some("FireMage_30"),
        cooldown: 7.0,
        resource_cost: 30.0,
        // HUD-only fields. Authoritative damage lives in
        // `ChannelEffect::AuraAroundCaster::damage_per_tick`.
        base_damage: 7.0,
        damage_mult: 1.0,
        projectile_count: 0,
        spread_angle: 0.0,
        range: 7.5,
        unlock_level: 4,
        element: Element::Fire,
        archetype: Archetype::Aoe,
        scaling: Scaling::Spell,
        // Total channel length is short — the wave reads as a
        // single quick blast, not a sustained aura.
        duration: 0.5,
        targeting: TargetingMode::Instant,
        // Channel + AuraAroundCaster centres damage on the
        // caster (no `placed_target` math), which is exactly
        // what we want for a self-centred wave. Five fast
        // ticks over 0.5 s feels like the wave physically
        // sweeping outward and catching enemies in its path.
        kind: AbilityKind::Channel {
            duration: 0.5,
            tick_interval: 0.10,
            effect: ChannelEffect::AuraAroundCaster {
                radius: 7.5,
                damage_per_tick: 7.0,
            },
            apply_debuff: Some(crate::debuffs::id::BURN),
            // Movement during the half-second pulse feels
            // better than a hard freeze.
            cancel_on_move: false,
        },
        visuals: AbilityVisuals {
            cast_spark: Some(VfxKind::CastSpark { rgb: [3.5, 1.4, 0.4] }),
            shape: ShapeVisuals::Aura {
                effect: VfxKind::FireWave,
            },
        },
        // The Aura `ShapeVisuals` doesn't yet drive client
        // emitters, so we fire the wave VFX as a one-shot at
        // the caster on cast. The 0.7-second preset comfortably
        // covers the channel duration.
        effects: &[AbilityEffect::SpawnEmitterAtCaster {
            visual: VfxKind::FireWave,
            height: 0.15,
        }],
    },
    // ── Enemy abilities (wire ids 64..) ───────────────────────────────
    //
    // Enemy abilities aren't routed through the player cast
    // dispatcher — enemy AI in `rift-server` spawns their effects
    // directly. They live in `REGISTRY` so the *visual* lookup
    // (`ability.visuals.shape`) works uniformly for player and
    // enemy projectiles on the client.
    Ability {
        id: AbilityId("enemy_caster_bolt"),
        wire_id: id::ENEMY_CASTER_BOLT,
        name: "Caster Bolt",
        description: "",
        icon: None,
        cooldown: 0.0,
        resource_cost: 0.0,
        base_damage: 0.0,
        damage_mult: 0.0,
        projectile_count: 0,
        spread_angle: 0.0,
        range: 0.0,
        unlock_level: 0,
        element: Element::None,
        archetype: Archetype::Projectile,
        scaling: Scaling::None,
        duration: 0.0,
        targeting: TargetingMode::Instant,
        // ClientOnly so the player cast pipeline never picks this
        // up; enemy AI bypasses `cast()` entirely.
        kind: AbilityKind::ClientOnly,
        visuals: AbilityVisuals {
            cast_spark: None,
            shape: ShapeVisuals::Projectile {
                mesh: MeshKind::CasterBolt,
                trail: VfxKind::CasterBoltTrail,
                impact: VfxKind::CasterBoltImpact,
                scale: 0.6,
            },
        },
        effects: &[],
    },
];

/// Look up an ability by its wire id. Linear scan over `REGISTRY`
/// — at 8 entries this is faster than hashing.
pub fn lookup(ability_id: u8) -> Option<&'static Ability> {
    REGISTRY.iter().find(|a| a.wire_id == ability_id)
}

/// Owned-clone variant of [`lookup`]. Kept for callers that need
/// to stash an `Ability` in a component / message struct.
pub fn from_wire_id(ability_id: u8) -> Option<Ability> {
    lookup(ability_id).cloned()
}
