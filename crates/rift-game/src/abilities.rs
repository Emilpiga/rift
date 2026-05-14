//! Abilities — declarative gameplay data.
//!
//! This module owns the `Ability` shape, the wire-id table used by
//! the server for cast dispatch (`id::*`, `AbilityKind`, `lookup`),
//! and the Hunter ability roster. The engine consumes `Ability` and
//! its declarative `AbilityEffect` list as plain data.

use crate::components::PlayerAction;
use serde::{Deserialize, Serialize};

// ─── Engine-consumed declarative ability shape ───────────────────────────

/// Opaque identifier for an ability. Treated as a hashable key only —
/// concrete IDs are defined below as constants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AbilityId(pub &'static str);

/// Stable wire / persistence byte for an ability — the numeric
/// counterpart of [`AbilityId`]. Carried on every cast event,
/// projectile, channel tick, damage log and slot-bar entry; the
/// authoritative table is the `id::*` module below. Values are
/// **never reordered, only appended**, so this byte is a stable
/// long-term identity for an ability across save / wire formats.
///
/// `#[serde(transparent)]` makes it byte-identical to a `u8` on
/// the bincode wire, so introducing the newtype doesn't bump any
/// protocol version. Inner byte is `pub` so static initialisers
/// for the const id table can build values in a `const` context.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(transparent)]
#[repr(transparent)]
pub struct AbilityWireId(pub u8);

impl AbilityWireId {
    /// Const constructor for use inside `pub const` initialisers
    /// in the `id::*` table.
    pub const fn new(byte: u8) -> Self {
        Self(byte)
    }

    /// Raw wire byte. Use only at protocol / persistence
    /// boundaries — gameplay code should keep the newtype.
    pub const fn raw(self) -> u8 {
        self.0
    }
}

impl From<u8> for AbilityWireId {
    fn from(b: u8) -> Self {
        Self(b)
    }
}

impl From<AbilityWireId> for u8 {
    fn from(id: AbilityWireId) -> u8 {
        id.0
    }
}

impl std::fmt::Display for AbilityWireId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// How the ability is targeted before firing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TargetingMode {
    /// Fires immediately in the aim direction (default).
    Instant,
    /// Shows a ground circle preview; player clicks to confirm placement.
    Placed { radius: f32 },
    /// Requires an entity target (currently: alive players for
    /// friendly heals). The client picks the target through hover
    /// + click — see `client::game::abilities::targeting`.
    TargetEntity,
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
    /// Violet/indigo trail for enemy arcane bolts.
    ArcaneBoltTrail,
    /// Violet/indigo burst played at arcane-bolt detonation.
    ArcaneBoltImpact,
    /// Cold cyan trail attached to a Frost Shatter shard
    /// projectile (the shards emitted by Frost Ray's pulse
    /// finisher). Sibling of `ArcaneBoltTrail`, recoloured to
    /// match the frost palette so the shards visually descend
    /// from the beam they spawned from.
    FrostShardTrail,
    /// Cold cyan burst played when a Frost Shatter shard hits.
    /// Sibling of `ArcaneBoltImpact`.
    FrostShardImpact,
    /// Sustained ribbon-and-glow effect rendered along the Frost
    /// Ray beam.
    FrostRay,
    /// Warm-orange counterpart to [`Self::FrostRay`] — the
    /// channeled beam Embercrown's `FireballToBeam` transform
    /// fires. Same ribbon-and-light structure, fire palette.
    FireBeam,
    /// Self-centred expanding ring of fire used by Fire Wave —
    /// a one-shot blast that grows outward from the caster.
    FireWave,
    /// Soft golden glow that pulses once on a target on heal
    /// cast. Marks "you just got healed" — paired with a small
    /// hand-spark on the caster.
    HealBurst,
    /// Sustained gentle green sparkle attached to a target
    /// receiving a heal-over-time buff. Stays up for the buff's
    /// duration so other players can see who's currently
    /// regenerating.
    HealOverTimeAura,
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
    /// Smaller violet `Mesh::arcane_bolt` (enemy bolts).
    ArcaneBolt,
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
    AoeZone { effect: VfxKind, visual_y: f32 },
    /// Forward beam from the caster's hand (Frost Ray).
    Beam {
        effect: VfxKind,
        /// Vertical offset from the caster's body to the beam
        /// origin when no skinned hand-joint is available.
        hand_offset: f32,
    },
    /// Self-centred aura around the caster (Whirlwind). The
    /// effect is anchored to the caster every frame.
    Aura { effect: VfxKind },
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
    /// Marker: the ability spawns projectiles. The server
    /// reads count / spread / speed / ttl / pierce from
    /// [`AbilityKind::Projectiles`]; this variant carries
    /// only the hand-relative spawn offset (which has no
    /// equivalent in `AbilityKind`).
    SpawnProjectiles { spawn_offset: SpawnOffset },
    /// Marker: the ability drops a ground AoE zone. The
    /// server reads radius / duration / tick-interval from
    /// [`AbilityKind::AoeZone`]; this variant carries only
    /// the client-side visual recipe (which has no
    /// equivalent in `AbilityKind`).
    SpawnAoeZone {
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

/// Spellbook-side grouping. Derived from `Element` + `Archetype`
/// so adding a new ability never requires touching this list —
/// the category falls out of the data the ability already carries.
///
/// `All` is a sentinel used by the spellbook UI to mean "show
/// everything"; it is never returned by `Ability::category`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    All,
    Fire,
    Cold,
    Lightning,
    Physical,
    Utility,
}

impl Category {
    /// Display label for the category tab. Short so the left rail
    /// stays narrow; the icon ladder lives in the spellbook UI.
    pub fn label(self) -> &'static str {
        match self {
            Category::All => "All",
            Category::Fire => "Fire",
            Category::Cold => "Cold",
            Category::Lightning => "Lightning",
            Category::Physical => "Physical",
            Category::Utility => "Utility",
        }
    }

    /// Iteration order for the spellbook left-rail. `All` first,
    /// then elemental columns, then physical, then catch-all.
    pub fn all() -> &'static [Category] {
        &[
            Category::All,
            Category::Fire,
            Category::Cold,
            Category::Lightning,
            Category::Physical,
            Category::Utility,
        ]
    }
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

/// Per-ability sound recipe. Co-located with the rest of the
/// ability data so authoring one place sets every gameplay +
/// presentation knob. The client's audio module wraps these
/// `&'static str` asset paths in volume / falloff specs at
/// playback time; the paths themselves are the only piece that
/// varies per-ability.
///
/// Default is silent (`None` everywhere) -- abilities without a
/// dedicated SFX simply ship muted instead of fanning out a
/// `match` arm in a separate table.
#[derive(Clone, Copy, Debug, Default)]
pub struct AbilityAudio {
    /// One-shot at cast time, anchored at the caster's hand
    /// height.
    pub cast: Option<&'static str>,
    /// Looping emitter attached to the projectile while it
    /// travels. Only meaningful for projectile-shaped abilities.
    pub travel: Option<&'static str>,
    /// One-shot at projectile impact / detonation.
    pub impact: Option<&'static str>,
}

impl AbilityAudio {
    /// Const-friendly silent default for use in the REGISTRY.
    pub const SILENT: Self = Self {
        cast: None,
        travel: None,
        impact: None,
    };
}

/// Static definition of an ability.
#[derive(Clone, Debug)]
pub struct Ability {
    pub id: AbilityId,
    /// Wire-stable id, matching one of `id::*`. Sent to the
    /// server when the player presses the slot key. Decoupled from
    /// the slot index so the action bar can be rearranged without
    /// reshuffling cooldown / dispatch state.
    pub wire_id: AbilityWireId,
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
    /// Per-second essence drain while a channel ability is held.
    /// `0.0` for instant-cast abilities (which use
    /// [`Self::resource_cost`] instead). The server drains this
    /// every channel tick and ends the channel cleanly when the
    /// caster runs out of essence.
    pub channel_cost_per_sec: f32,
    /// Base damage per hit (or per AoE/channel tick), pre-scaling.
    /// The server scales this by the caster's gear/attribute
    /// multiplier on the way to applying damage.
    pub base_damage: f32,
    /// Display-friendly damage scalar shown in HUD tooltips
    /// (`Dmg: 70%`). Authored alongside `base_damage`; the actual
    /// damage number is `base_damage`, not `base_damage *
    /// damage_mult`.
    pub damage_mult: f32,
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
    /// Per-ability sound recipe. Defaults to silent
    /// ([`AbilityAudio::SILENT`]); set the relevant fields when
    /// authoring an ability that needs cast / travel / impact
    /// SFX. Client looks this up through [`lookup`] -- no
    /// separate side-table.
    pub audio: AbilityAudio,
}

/// Runtime state for an ability on the action bar.
#[derive(Clone, Debug)]
pub struct AbilityState {
    pub ability: Ability,
    pub cooldown_remaining: f32,
    pub charges: u32,
    pub max_charges: u32,
}

impl Ability {
    /// Spellbook category derived from the ability's damage
    /// element + shape. Element wins for the four damaging
    /// columns; movement / utility archetypes (which usually
    /// carry `Element::None`) fall into the `Utility` bucket.
    /// Pure `Element::None` damaging abilities (a rarity in the
    /// current data, but possible in the future) likewise sort
    /// into `Utility` so they always have a home.
    pub fn category(&self) -> Category {
        match self.element {
            Element::Fire => Category::Fire,
            Element::Ice => Category::Cold,
            Element::Lightning => Category::Lightning,
            Element::Physical => match self.archetype {
                Archetype::Movement | Archetype::Utility => Category::Utility,
                _ => Category::Physical,
            },
            Element::None => Category::Utility,
        }
    }

    /// Authoritative projectile count for HUD tooltips. Reads
    /// from [`AbilityKind`] so there's no second source of
    /// truth to keep in sync. Returns `0` for shapes that
    /// don't spawn projectiles.
    pub fn projectile_count(&self) -> u32 {
        match self.kind {
            AbilityKind::Projectiles { count, .. }
            | AbilityKind::EnemyProjectiles { count, .. } => count,
            _ => 0,
        }
    }

    /// Authoritative spread angle (radians) for HUD tooltips.
    /// Mirrors [`Self::projectile_count`] -- derived from
    /// [`AbilityKind`].
    pub fn spread_angle(&self) -> f32 {
        match self.kind {
            AbilityKind::Projectiles { spread, .. }
            | AbilityKind::EnemyProjectiles { spread, .. } => spread,
            _ => 0.0,
        }
    }

    /// Authoritative ability duration (seconds) for HUD
    /// tooltips. Reads from [`AbilityKind`] for AoE zones and
    /// channels; `0.0` for instant shapes.
    pub fn duration(&self) -> f32 {
        match self.kind {
            AbilityKind::AoeZone { duration, .. } => duration,
            AbilityKind::Channel { duration, .. } => duration,
            _ => 0.0,
        }
    }
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
        if self.ability.cooldown <= 0.0 {
            return 1.0;
        }
        1.0 - (self.cooldown_remaining / self.ability.cooldown)
    }
}

/// The player's action bar — 6 ability slots (like Diablo 3),
/// plus a fixed passive **Roll** slot that's always available
/// on the Space key regardless of what the player has slotted.
/// Roll is intentionally not part of the spellbook pool: it's
/// the universal dodge, like Diablo 4's Evade. It still ticks
/// its own cooldown via the normal [`AbilityState`] path so the
/// HUD and the server share the same vocabulary.
#[derive(Clone, Debug)]
pub struct AbilitySlot {
    pub slots: [Option<AbilityState>; 6],
    /// Passive dodge bound to Space. Always populated with
    /// [`id::EVASIVE_ROLL`] when the registry contains it; only
    /// `None` in degenerate test builds where the ability was
    /// stripped.
    pub roll: Option<AbilityState>,
}

impl AbilitySlot {
    pub fn new() -> Self {
        // Auto-populate the passive Roll slot from the registry
        // so every materialised loadout includes the dodge —
        // callers never need to remember to wire it up.
        let roll = lookup(id::EVASIVE_ROLL).map(|ab| AbilityState::new(ab.clone()));
        Self {
            slots: Default::default(),
            roll,
        }
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
        if let Some(state) = &mut self.roll {
            state.tick(dt);
        }
    }

    pub fn try_use(&mut self, index: usize) -> Option<&Ability> {
        if index >= 6 {
            return None;
        }
        if let Some(state) = &mut self.slots[index] {
            if state.try_use() {
                return Some(&state.ability);
            }
        }
        None
    }

    /// Try to consume the passive Roll slot. Returns the
    /// ability definition on success so the caller can spawn
    /// the same local VFX / cast request as a normal slot.
    pub fn try_use_roll(&mut self) -> Option<&Ability> {
        let state = self.roll.as_mut()?;
        if state.try_use() {
            Some(&state.ability)
        } else {
            None
        }
    }
}

// ─── Wire-side ability table (server cast dispatch) ──────────────────────
//
// Stable u8 ids for abilities on the wire. Never reorder, only append.
// The server's `ability_table` keys off these and the client's slot
// bar maps them onto button presses.

pub mod id {
    use super::AbilityWireId;

    pub const FIRE_BALL: AbilityWireId = AbilityWireId::new(0);
    pub const FIREBALL_VOLLEY: AbilityWireId = AbilityWireId::new(1);
    pub const EVASIVE_ROLL: AbilityWireId = AbilityWireId::new(2);
    pub const RAPID_FIRE: AbilityWireId = AbilityWireId::new(3);
    pub const MARK_FOR_DEATH: AbilityWireId = AbilityWireId::new(4);
    pub const RAIN_OF_ARROWS: AbilityWireId = AbilityWireId::new(5);
    pub const FROST_RAY: AbilityWireId = AbilityWireId::new(6);
    pub const WHIRLWIND: AbilityWireId = AbilityWireId::new(7);
    pub const FIRE_WAVE: AbilityWireId = AbilityWireId::new(8);
    pub const HEAL_TARGET: AbilityWireId = AbilityWireId::new(9);
    pub const HEAL_OVER_TIME_TARGET: AbilityWireId = AbilityWireId::new(10);
    /// Synthetic ability fired by the `FrostRayShatter`
    /// legendary transform: shards spawned at the beam
    /// terminus when a player ends a Frost Ray channel.
    /// Not on the loadout bar — the player never casts it
    /// directly. Has its own wire id so the client renders
    /// the shards as visible projectiles (Frost Ray itself
    /// is a `Beam` and has no projectile visuals).
    pub const FROST_SHATTER_SHARD: AbilityWireId = AbilityWireId::new(11);

    /// Generic enemy basic-attack — the bare melee swing /
    /// dash hit that doesn't have its own dedicated registry
    /// entry. Used as the meter-attribution id for brute &
    /// boss basic swings and stalker dash hits so the TAKEN
    /// tab can roll those up under one row instead of
    /// dumping them all into "Other". Not on any loadout bar
    /// — the player never casts it directly.
    pub const MELEE_ATTACK: AbilityWireId = AbilityWireId::new(12);

    /// Synthetic beam ability used by the Embercrown
    /// `FireballToBeam` transform. Not on any loadout bar —
    /// the server re-stamps the cast event under this id so
    /// clients render the beam visual / play the channel
    /// pose, since [`FIRE_BALL`]'s own visuals are
    /// projectile-shaped.
    pub const FIREBALL_BEAM: AbilityWireId = AbilityWireId::new(13);

    /// **Neutral fist attack** — the always-available basic
    /// attack every fresh character has even with zero talents
    /// invested. Unlike [`MELEE_ATTACK`] this is fully mobile:
    /// the player keeps locomotion while the swing plays as an
    /// upper-body overlay (no kinematic action lock, no
    /// `forward_step` lunge, no aim lock). Pre-equipped to
    /// loadout slot 0 by default but can be swapped out like
    /// any other ability. Never gated by the talent tree.
    pub const PUNCH: AbilityWireId = AbilityWireId::new(14);
    pub const VOID_FAMILIAR: AbilityWireId = AbilityWireId::new(15);

    // Enemy ability ids start at 64 to leave room for player
    // abilities to grow without colliding. Wire is u8 so the
    // upper half is plenty. Clients dispatch projectile mesh /
    // VFX off these ids in `world_sync` — never reorder, only
    // append.
    pub const ARCANE_BOLT: AbilityWireId = AbilityWireId::new(64);
    /// Ground-slam wind-up: a one-shot `WorldEvent::AbilityCast`
    /// emitted when a slam-style attack enters its telegraph
    /// phase. The cast event's `target` carries the slam centre
    /// and the `dir.x` field carries the radius (m); clients
    /// drive a sustained ground-ring telegraph off it. Player-
    /// castable in principle — today only the boss uses it.
    pub const GROUND_SLAM_WINDUP: AbilityWireId = AbilityWireId::new(65);
    /// Ground-slam impact: paired with `GROUND_SLAM_WINDUP`,
    /// emitted when the wind-up resolves. Same target/radius
    /// payload convention as the wind-up event.
    pub const GROUND_SLAM_IMPACT: AbilityWireId = AbilityWireId::new(66);
    /// Multi-bolt cone fan. Authored as a single
    /// [`AbilityKind::EnemyProjectiles`] entry with `count > 1`
    /// and a non-zero spread; the boss uses it in phase 2+.
    pub const ARCANE_FAN: AbilityWireId = AbilityWireId::new(67);
    /// Boss ground slam (the gameplay ability — distinct from
    /// the visual-only WINDUP/IMPACT events). Kind is
    /// [`AbilityKind::DelayedAoe`].
    pub const GROUND_SLAM: AbilityWireId = AbilityWireId::new(68);
    /// Boss summons: spawn brute reinforcements in a ring
    /// around the caster after a wind-up. Kind is
    /// [`AbilityKind::Summon`].
    pub const SUMMON_BRUTES: AbilityWireId = AbilityWireId::new(69);
    /// Wraith cone scream. Server-resolved directly by the
    /// Wraith AI so it can hit a cone of players, but it has
    /// a registry row for combat-meter attribution.
    pub const WRAITH_SCREAM: AbilityWireId = AbilityWireId::new(70);
    /// Mindbinder ground-control sigil. Kind is
    /// [`AbilityKind::DelayedAoe`]; the AI places it under the
    /// target and reuses the ground-slam telegraph visuals.
    pub const VOID_SIGIL: AbilityWireId = AbilityWireId::new(71);
    /// Visual-only Wraith scream wind-up event. `dir` carries
    /// the cone facing direction.
    pub const WRAITH_SCREAM_WINDUP: AbilityWireId = AbilityWireId::new(72);
    /// Visual-only Wraith scream release event. `dir` carries
    /// the cone facing direction.
    pub const WRAITH_SCREAM_IMPACT: AbilityWireId = AbilityWireId::new(73);
    /// Visual-only Mindbinder sigil wind-up event. `target`
    /// carries the sigil centre and `dir.x` carries radius.
    pub const VOID_SIGIL_WINDUP: AbilityWireId = AbilityWireId::new(74);
    /// Visual-only Mindbinder sigil impact event. Same payload
    /// convention as [`VOID_SIGIL_WINDUP`].
    pub const VOID_SIGIL_IMPACT: AbilityWireId = AbilityWireId::new(75);
    /// Hidden player-team projectile fired by the Void Familiar's
    /// server-side AI. It lives in the non-loadout wire range so
    /// the spellbook never offers it as a direct player cast, but
    /// clients can still look up projectile visuals and meters can
    /// attribute familiar damage cleanly.
    pub const VOID_FAMILIAR_BOLT: AbilityWireId = AbilityWireId::new(76);
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
    /// Enemy-cast projectiles that damage *players* (mirror of
    /// [`AbilityKind::Projectiles`] with the target side flipped).
    /// `windup` is the telegraph freeze the casting AI plays
    /// before the bolts spawn — the AI ticks it down and only
    /// calls `cast_for_enemy` once it expires, so this field is
    /// authoritative tuning, not runtime state.
    ///
    /// In principle a player could ever take an "enemy-style"
    /// telegraphed ranged attack; the variant is named for who
    /// gets hurt, not who casts. Spawned projectiles share the
    /// `ServerProjectile` component with player bolts but carry
    /// `Team::Enemy`, which routes them to the player target
    /// list in the unified projectile tick.
    EnemyProjectiles {
        count: u32,
        spread: f32,
        speed: f32,
        ttl: f32,
        windup: f32,
        size: f32,
        /// Optional debuff applied to the player on hit. Wire
        /// id from `rift_game::effects::id::*`. None today
        /// (no enemy ability uses it yet) but the field is
        /// plumbed end-to-end so a future caster’s slow / DoT
        /// is one literal instead of a refactor.
        apply_debuff: Option<u8>,
    },
    /// Single-resolve self-centred AoE at the caster's body
    /// position. Damage = `Ability.base_damage`. After the AI's
    /// wind-up resolves, every player inside `radius` takes the
    /// damage exactly once. Used by boss Slam.
    ///
    /// Telegraph visuals are driven separately: the caster emits
    /// a `WorldEvent::AbilityCast` carrying `radius` + `windup`
    /// when the wind-up starts (see `id::GROUND_SLAM_WINDUP`),
    /// and a paired impact event on resolve.
    DelayedAoe { radius: f32, windup: f32 },
    /// Spawn `count` enemies in a ring around the caster after
    /// `windup` seconds. Used by boss summons. `hp_mult` scales
    /// the floor's base enemy HP — summons want to be a real
    /// threat through the enrage phase.
    Summon {
        count: u32,
        role: crate::monsters::MonsterRole,
        hp_mult: f32,
        ring_radius: f32,
        windup: f32,
    },
    /// Spawn or refresh a player-owned minion. The authoritative
    /// follow / target / attack behaviour lives in `rift-server`,
    /// but the tuning is registry data so talents, tooltips, and
    /// server dispatch all agree on the same ability surface.
    MinionSummon {
        role: crate::monsters::MonsterRole,
        duration: f32,
        hp: f32,
        follow_distance: f32,
        attack_range: f32,
        attack_interval: f32,
        attack_damage: f32,
        projectile_speed: f32,
        projectile_ttl: f32,
    },
    /// Player melee swing — a short forward arc resolved server-side
    /// on the cast tick. Every enemy whose XZ hit disc overlaps the
    /// `radius` reach and half-`arc_radians` cone around the aim
    /// direction takes the ability's scaled damage exactly once. There is no
    /// projectile or persistent zone; the caster is locked into the
    /// `Attack` action for the registry-authored duration so the
    /// swing animation can play. Authored on Sword / Dagger LMB.
    MeleeArc { radius: f32, arc_radians: f32 },
    /// Friendly single-target instant heal. Restores `amount`
    /// HP to the targeted player (clamped at `hp_max`). Does
    /// nothing if the target is dead, ghosting, or out of
    /// range / line of sight — those gates live in
    /// `ability::submit`.
    HealTarget { amount: f32 },
    /// Friendly single-target heal over time. Applies the
    /// [`crate::effects::id::REJUVENATION`] effect to the
    /// target; the effect tick produces healing rows in
    /// `effect::tick` the same way DoT debuffs produce damage
    /// rows, so the gameplay clock is fully data-driven off
    /// the buff stack.
    HealOverTimeTarget {
        /// Buff id to apply (always `REJUVENATION` today, but
        /// the field is here for symmetry with the DoT pipe).
        apply_buff: u8,
    },
}

/// Per-tick effect of a [`AbilityKind::Channel`]. Designed to grow
/// with new patterns — add a variant + a server arm in
/// `sim::channel::tick`.
#[derive(Clone, Copy, Debug)]
pub enum ChannelEffect {
    /// Damage every enemy within `radius` of the caster's current
    /// position. Use for self-centred AoEs (Whirlwind, Sanctified
    /// Ground, Frost Nova).
    AuraAroundCaster { radius: f32, damage_per_tick: f32 },
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
pub const FIREBALL_VOLLEY: AbilityId = AbilityId("fireball_volley");
pub const FIREBALL_BEAM: AbilityId = AbilityId("fireball_beam");
pub const RAPID_FIRE: AbilityId = AbilityId("rapid_fire");
pub const RAIN_OF_ARROWS: AbilityId = AbilityId("rain_of_arrows");
pub const EVASIVE_ROLL: AbilityId = AbilityId("evasive_roll");
pub const MARK_FOR_DEATH: AbilityId = AbilityId("mark_for_death");
pub const FROST_RAY: AbilityId = AbilityId("frost_ray");
pub const WHIRLWIND: AbilityId = AbilityId("whirlwind");
pub const FIRE_WAVE: AbilityId = AbilityId("fire_wave");
pub const HEAL_TARGET: AbilityId = AbilityId("heal_target");
pub const HEAL_OVER_TIME_TARGET: AbilityId = AbilityId("heal_over_time_target");
pub const FROST_SHATTER_SHARD: AbilityId = AbilityId("frost_shatter_shard");
pub const MELEE_ATTACK: AbilityId = AbilityId("melee_attack");
pub const PUNCH: AbilityId = AbilityId("punch");
pub const VOID_FAMILIAR: AbilityId = AbilityId("void_familiar");
pub const VOID_FAMILIAR_BOLT: AbilityId = AbilityId("void_familiar_bolt");

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
        resource_cost: 12.0,
        channel_cost_per_sec: 0.0,
        base_damage: 8.0,
        damage_mult: 1.0,
        range: 12.0,
        unlock_level: 1,
        element: Element::Fire,
        archetype: Archetype::Projectile,
        scaling: Scaling::Spell,
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
            spawn_offset: SpawnOffset::HAND,
        }],
        audio: AbilityAudio {
            cast: Some("vfx/abilities/fireball/fireball_cast.mp3"),
            travel: Some("vfx/abilities/fireball/fireball_travel.mp3"),
            impact: Some("vfx/abilities/fireball/fireball_impact.mp3"),
        },
    },
    Ability {
        id: FIREBALL_VOLLEY,
        wire_id: id::FIREBALL_VOLLEY,
        name: "Fireball Volley",
        description: "Hurl 3 fireballs in a wide arc.",
        icon: Some("FireMage_18"),
        cooldown: 4.0,
        resource_cost: 15.0,
        channel_cost_per_sec: 0.0,
        base_damage: 8.0 * 0.7,
        damage_mult: 0.7,
        range: 10.0,
        unlock_level: 3,
        element: Element::Fire,
        archetype: Archetype::Projectile,
        scaling: Scaling::Spell,
        targeting: TargetingMode::Instant,
        kind: AbilityKind::Projectiles {
            count: 3,
            spread: 0.5,
            speed: 20.0,
            ttl: 2.0,
            pierce: 0,
            apply_debuff: Some(crate::effects::id::BURN),
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
            spawn_offset: SpawnOffset::HAND,
        }],
        audio: AbilityAudio::SILENT,
    },
    Ability {
        id: RAPID_FIRE,
        wire_id: id::RAPID_FIRE,
        name: "Rapid Fire",
        description: "Channel a burst of 6 rapid arrows.",
        icon: None,
        cooldown: 8.0,
        resource_cost: 25.0,
        channel_cost_per_sec: 0.0,
        base_damage: 8.0 * 0.5,
        damage_mult: 0.5,
        range: 12.0,
        unlock_level: 7,
        element: Element::Physical,
        archetype: Archetype::Projectile,
        scaling: Scaling::Weapon,
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
            spawn_offset: SpawnOffset::HAND,
        }],
        audio: AbilityAudio::SILENT,
    },
    Ability {
        id: RAIN_OF_ARROWS,
        wire_id: id::RAIN_OF_ARROWS,
        name: "Rain of Fire",
        description: "Call down a rain of fire in an area. Burns enemies caught inside.",
        icon: Some("FireMage_35"),
        cooldown: 12.0,
        resource_cost: 35.0,
        channel_cost_per_sec: 0.0,
        base_damage: 8.0 * 0.4,
        damage_mult: 0.4,
        range: 15.0,
        unlock_level: 12,
        element: Element::Fire,
        archetype: Archetype::Aoe,
        scaling: Scaling::Spell,
        targeting: TargetingMode::Placed { radius: 3.0 },
        kind: AbilityKind::AoeZone {
            radius: 3.0,
            duration: 2.0,
            tick_interval: 0.5,
            apply_debuff: Some(crate::effects::id::BURN),
        },
        visuals: AbilityVisuals {
            cast_spark: None,
            shape: ShapeVisuals::AoeZone {
                effect: VfxKind::RainOfFire,
                visual_y: 5.0,
            },
        },
        effects: &[AbilityEffect::SpawnAoeZone {
            visual: Some(VfxKind::RainOfFire),
            visual_y: 5.0,
        }],
        audio: AbilityAudio::SILENT,
    },
    Ability {
        id: EVASIVE_ROLL,
        wire_id: id::EVASIVE_ROLL,
        name: "Evasive Roll",
        description: "Dodge roll in movement direction. Brief invulnerability.",
        icon: Some("Monk_27"),
        cooldown: 6.0,
        resource_cost: 0.0,
        channel_cost_per_sec: 0.0,
        base_damage: 0.0,
        damage_mult: 0.0,
        range: 0.0,
        unlock_level: 1,
        element: Element::None,
        archetype: Archetype::Movement,
        scaling: Scaling::None,
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
        audio: AbilityAudio::SILENT,
    },
    Ability {
        id: MARK_FOR_DEATH,
        wire_id: id::MARK_FOR_DEATH,
        name: "Mark for Death",
        description: "Mark target. They take 25% increased damage for 6s.",
        icon: None,
        cooldown: 15.0,
        resource_cost: 20.0,
        channel_cost_per_sec: 0.0,
        base_damage: 0.0,
        damage_mult: 0.0,
        range: 20.0,
        unlock_level: 10,
        element: Element::None,
        archetype: Archetype::Utility,
        scaling: Scaling::None,
        targeting: TargetingMode::Instant,
        kind: AbilityKind::ClientOnly,
        visuals: AbilityVisuals::NONE,
        effects: &[],
        audio: AbilityAudio::SILENT,
    },
    Ability {
        id: FROST_RAY,
        wire_id: id::FROST_RAY,
        name: "Frost Ray",
        description: "Channel a freezing beam. Slows and damages.",
        icon: Some("FrostMage_7"),
        cooldown: 0.0,
        resource_cost: 0.0,
        channel_cost_per_sec: 18.0,
        base_damage: 1.6,
        damage_mult: 0.2,
        range: 9.0,
        unlock_level: 1,
        element: Element::Ice,
        archetype: Archetype::Beam,
        scaling: Scaling::Spell,
        // Effectively infinite — channel ends only on key release,
        // movement cancel, or (future) resource depletion.
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
            apply_debuff: Some(crate::effects::id::CHILL),
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
        audio: AbilityAudio::SILENT,
    },
    // Synthetic ability — never on the player's loadout bar.
    // Fired by the `FrostRayShatter` legendary transform when
    // a Frost Ray channel ends; carries its own projectile
    // visuals so the shards are visible (Frost Ray itself
    // declares `ShapeVisuals::Beam`, which the client
    // projectile-spawn path skips). Cold cyan trail + impact
    // presets keep the shards visually keyed to the parent
    // beam they spawned from.
    Ability {
        id: FROST_SHATTER_SHARD,
        wire_id: id::FROST_SHATTER_SHARD,
        name: "Frost Shard",
        description: "Shatter shard from Frost Ray's terminus.",
        icon: None,
        cooldown: 0.0,
        resource_cost: 0.0,
        channel_cost_per_sec: 0.0,
        base_damage: 0.0,
        damage_mult: 1.0,
        range: 0.0,
        unlock_level: 1,
        element: Element::Ice,
        archetype: Archetype::Projectile,
        scaling: Scaling::Spell,
        targeting: TargetingMode::Instant,
        // Server constructs the projectile rows directly in
        // `transforms::on_channel_end`; the registry shape
        // is here only so client projectile rendering and
        // meter attribution have a real ability row to read.
        kind: AbilityKind::Projectiles {
            count: 1,
            spread: 0.0,
            speed: 14.0,
            ttl: 0.45,
            pierce: 1,
            apply_debuff: None,
        },
        visuals: AbilityVisuals {
            cast_spark: None,
            shape: ShapeVisuals::Projectile {
                mesh: MeshKind::ArcaneBolt,
                trail: VfxKind::FrostShardTrail,
                impact: VfxKind::FrostShardImpact,
                scale: 0.5,
            },
        },
        effects: &[],
        audio: AbilityAudio::SILENT,
    },
    // Player Sword / Dagger LMB. Resolved server-side as a
    // forward arc on the cast tick (`AbilityKind::MeleeArc`).
    // Enemy callers (brute / stalker / boss) reuse this id
    // for attribution on their swing-style damage rows — they
    // never go through `submit` / `dispatch` for it, they
    // just stamp `ability_id: MELEE_ATTACK` on the
    // `combat_ctx::EnemyHit` row, so the registry kind /
    // damage numbers below are irrelevant to those code
    // paths. The `effects` list drives the local cast pose
    // for the player only.
    Ability {
        id: MELEE_ATTACK,
        wire_id: id::MELEE_ATTACK,
        name: "Melee",
        description: "Swing your weapon in a short forward arc.",
        icon: None,
        cooldown: 0.4,
        resource_cost: 0.0,
        channel_cost_per_sec: 0.0,
        base_damage: 12.0,
        damage_mult: 1.0,
        range: 2.5,
        unlock_level: 1,
        element: Element::Physical,
        archetype: Archetype::Melee,
        scaling: Scaling::Weapon,
        targeting: TargetingMode::Instant,
        kind: AbilityKind::MeleeArc {
            radius: 2.5,
            // ~120° cone — wide enough to feel forgiving on
            // off-axis enemies, tight enough that you can't
            // tag mobs behind you.
            arc_radians: 2.094,
        },
        visuals: AbilityVisuals::NONE,
        // Swing pose is driven through the generic
        // declarative-effects pipeline: a `SetPlayerAction`
        // entry cross-fades `Sword_Attack` onto the full-body
        // animator and stamps `PlayerAction::Attack` for the
        // swing's duration. Movement is owned by the
        // kinematic (`Kinematic::apply_input` reads
        // `action::ATTACK` and applies the
        // [`rift_game::kinematic::MELEE_ATTACK::forward_step`]
        // ease-out along the locked `attack_dir`), so we
        // set `ActionMovement::None` here — a `Forward`
        // entry would write the engine's `Velocity` component
        // and fight the kinematic-driven lunge.
        effects: &[AbilityEffect::SetPlayerAction {
            action: PlayerAction::Attack,
            duration: crate::kinematic::MELEE_ATTACK.duration,
            clip: &["Sword_Attack"],
            movement: ActionMovement::None,
            cancel_cast: true,
            emitter: None,
        }],
        audio: AbilityAudio::SILENT,
    },
    // Neutral fist attack. Always available — every fresh
    // character starts with this in loadout slot 0 regardless
    // of talent investment (see `TALENT_TREE.md` §2.1). The
    // fantasy: bare-handed jab/cross combo you can throw
    // while moving, before you've invested in any combat
    // route. Damage shape is `MeleeArc` for the contact
    // resolve, but the effects list is intentionally empty:
    // no `SetPlayerAction`, so the kinematic does NOT engage
    // its action lock (no `forward_step` lunge, no aim lock,
    // no locomotion override). The upper-body swing pose is
    // driven separately by the cast-pose FSM (`SpellCast`)
    // wiring in `trigger_local_cast` — a follow-up change.
    // Until that wiring lands the cast resolves correctly
    // server-side but plays no client animation.
    Ability {
        id: PUNCH,
        wire_id: id::PUNCH,
        name: "Punch",
        description: "A quick bare-handed jab. Always available.",
        icon: Some("Barbarian_3"),
        cooldown: 0.75,
        resource_cost: 0.0,
        channel_cost_per_sec: 0.0,
        base_damage: 4.0,
        damage_mult: 1.0,
        range: 2.05,
        // `unlock_level: 0` flags this as never-gated. Loadout
        // slot 0 unlocks at level 1 so the player can use
        // Punch from frame one.
        unlock_level: 0,
        element: Element::Physical,
        archetype: Archetype::Melee,
        scaling: Scaling::Weapon,
        targeting: TargetingMode::Instant,
        kind: AbilityKind::MeleeArc {
            radius: 2.05,
            // Still tighter than Sword Slash, but forgiving
            // enough that a bare-handed starter hit can clip
            // visible enemy bodies instead of demanding a
            // centre-point aim. ~108°.
            arc_radians: 1.885,
        },
        visuals: AbilityVisuals::NONE,
        effects: &[],
        audio: AbilityAudio::SILENT,
    },
    Ability {
        id: VOID_FAMILIAR,
        wire_id: id::VOID_FAMILIAR,
        name: "Void Familiar",
        description: "Summon a short-lived void familiar that follows you and harasses nearby enemies.",
        icon: Some("Necromancer_10"),
        cooldown: 18.0,
        resource_cost: 20.0,
        channel_cost_per_sec: 0.0,
        base_damage: 0.0,
        damage_mult: 0.0,
        range: 0.0,
        unlock_level: 1,
        element: Element::None,
        archetype: Archetype::Utility,
        scaling: Scaling::None,
        targeting: TargetingMode::Instant,
        kind: AbilityKind::MinionSummon {
            role: crate::monsters::MonsterRole::Wraith,
            duration: 28.0,
            hp: 45.0,
            follow_distance: 2.4,
            attack_range: 8.5,
            attack_interval: 1.25,
            attack_damage: 6.0,
            projectile_speed: 15.0,
            projectile_ttl: 1.2,
        },
        visuals: AbilityVisuals {
            cast_spark: Some(VfxKind::CastSpark {
                rgb: [0.48, 0.28, 1.0],
            }),
            shape: ShapeVisuals::None,
        },
        effects: &[AbilityEffect::SpawnEmitterAtCaster {
            visual: VfxKind::CastSpark {
                rgb: [0.48, 0.28, 1.0],
            },
            height: 0.8,
        }],
        audio: AbilityAudio::SILENT,
    },
    Ability {
        id: VOID_FAMILIAR_BOLT,
        wire_id: id::VOID_FAMILIAR_BOLT,
        name: "Void Familiar Bolt",
        description: "A familiar's void bolt.",
        icon: None,
        cooldown: 0.0,
        resource_cost: 0.0,
        channel_cost_per_sec: 0.0,
        base_damage: 6.0,
        damage_mult: 1.0,
        range: 8.5,
        unlock_level: 0,
        element: Element::Lightning,
        archetype: Archetype::Projectile,
        scaling: Scaling::None,
        targeting: TargetingMode::Instant,
        kind: AbilityKind::Projectiles {
            count: 1,
            spread: 0.0,
            speed: 15.0,
            ttl: 1.2,
            pierce: 0,
            apply_debuff: None,
        },
        visuals: AbilityVisuals {
            cast_spark: None,
            shape: ShapeVisuals::Projectile {
                mesh: MeshKind::ArcaneBolt,
                trail: VfxKind::ArcaneBoltTrail,
                impact: VfxKind::ArcaneBoltImpact,
                scale: 0.5,
            },
        },
        effects: &[],
        audio: AbilityAudio::SILENT,
    },
    // Synthetic ability — never on a player's loadout bar.
    // Stamped by the Embercrown `FireballToBeam` transform
    // so clients render the beam visual + channel pose when
    // a Fireball cast morphs into a beam. The server builds
    // the actual `ChannelEffect::Beam` inline in
    // `sim::ability` (the parameters are derived from the
    // caster's gear there); the registry-level `kind` here
    // mirrors those numbers so HUD tooltip / meter
    // attribution have plausible values to read.
    Ability {
        id: FIREBALL_BEAM,
        wire_id: id::FIREBALL_BEAM,
        name: "Fireball Beam",
        description: "Fireball channels into a short piercing fire beam.",
        icon: Some("FireMage_12"),
        cooldown: 0.0,
        resource_cost: 0.0,
        channel_cost_per_sec: 0.0,
        base_damage: 8.0,
        damage_mult: 1.0,
        range: 12.0,
        unlock_level: 1,
        element: Element::Fire,
        archetype: Archetype::Beam,
        scaling: Scaling::Spell,
        targeting: TargetingMode::Instant,
        kind: AbilityKind::Channel {
            duration: 0.55,
            tick_interval: 0.11,
            effect: ChannelEffect::Beam {
                range: 12.0,
                width: 1.0,
                damage_per_tick: 8.0 / 5.0,
                pierce_targets: 32,
            },
            apply_debuff: Some(crate::effects::id::BURN),
            cancel_on_move: false,
        },
        visuals: AbilityVisuals {
            cast_spark: None,
            shape: ShapeVisuals::Beam {
                effect: VfxKind::FireBeam,
                hand_offset: 1.25,
            },
        },
        effects: &[],
        audio: AbilityAudio::SILENT,
    },
    Ability {
        id: WHIRLWIND,
        wire_id: id::WHIRLWIND,
        name: "Whirlwind",
        description: "Spin in place, damaging nearby foes.",
        icon: Some("Barbarian_18"),
        cooldown: 9.0,
        resource_cost: 25.0,
        channel_cost_per_sec: 0.0,
        base_damage: 2.2,
        damage_mult: 0.275,
        range: 2.5,
        unlock_level: 8,
        element: Element::Physical,
        archetype: Archetype::Melee,
        scaling: Scaling::Weapon,
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
        audio: AbilityAudio::SILENT,
    },
    Ability {
        id: FIRE_WAVE,
        wire_id: id::FIRE_WAVE,
        name: "Fire Wave",
        description: "Release an expanding ring of fire that scorches all nearby enemies and ignites them.",
        icon: Some("FireMage_30"),
        cooldown: 7.0,
        resource_cost: 30.0,
        channel_cost_per_sec: 0.0,
        // HUD-only fields. Authoritative damage lives in
        // `ChannelEffect::AuraAroundCaster::damage_per_tick`.
        base_damage: 7.0,
        damage_mult: 1.0,
        range: 7.5,
        unlock_level: 4,
        element: Element::Fire,
        archetype: Archetype::Aoe,
        scaling: Scaling::Spell,
        // Total channel length is short — the wave reads as a
        // single quick blast, not a sustained aura.
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
            apply_debuff: Some(crate::effects::id::BURN),
            // Movement during the half-second pulse feels
            // better than a hard freeze.
            cancel_on_move: false,
        },
        visuals: AbilityVisuals {
            cast_spark: Some(VfxKind::CastSpark {
                rgb: [3.5, 1.4, 0.4],
            }),
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
        audio: AbilityAudio::SILENT,
    },
    // ── Friendly support abilities ─────────────────────────────────────
    //
    // Single-target friendly casts. `TargetingMode::TargetEntity`
    // tells the client to pick an alive player (self or other);
    // the server gates by `IsTargetEntity` (alive, in range, has
    // line of sight).
    Ability {
        id: HEAL_TARGET,
        wire_id: id::HEAL_TARGET,
        name: "Heal",
        description: "Restore 40 HP to a friendly target. Cast on yourself with Shift, or on a teammate by hovering / clicking.",
        icon: Some("Druid_17"),
        cooldown: 8.0,
        resource_cost: 0.0,
        channel_cost_per_sec: 0.0,
        // HUD-only: heal amount mirrored from the kind for
        // tooltips. Authoritative value lives on the kind.
        base_damage: 40.0,
        damage_mult: 1.0,
        range: 15.0,
        unlock_level: 1,
        element: Element::None,
        archetype: Archetype::Aoe,
        scaling: Scaling::Spell,
        targeting: TargetingMode::TargetEntity,
        kind: AbilityKind::HealTarget { amount: 40.0 },
        visuals: AbilityVisuals {
            cast_spark: Some(VfxKind::CastSpark {
                rgb: [1.6, 2.4, 1.2],
            }),
            shape: ShapeVisuals::None,
        },
        // Heal cast emits a one-shot HealBurst on the target;
        // the engine consumes the matching wire event.
        effects: &[],
        audio: AbilityAudio::SILENT,
    },
    Ability {
        id: HEAL_OVER_TIME_TARGET,
        wire_id: id::HEAL_OVER_TIME_TARGET,
        name: "Rejuvenation",
        description: "Imbue a friendly target with 60 HP regenerated over 10 seconds.",
        icon: Some("Druid_16"),
        cooldown: 14.0,
        resource_cost: 0.0,
        channel_cost_per_sec: 0.0,
        base_damage: 60.0,
        damage_mult: 1.0,
        range: 15.0,
        unlock_level: 1,
        element: Element::None,
        archetype: Archetype::Aoe,
        scaling: Scaling::Spell,
        targeting: TargetingMode::TargetEntity,
        kind: AbilityKind::HealOverTimeTarget {
            apply_buff: crate::effects::id::REJUVENATION,
        },
        visuals: AbilityVisuals {
            cast_spark: Some(VfxKind::CastSpark {
                rgb: [1.4, 2.6, 1.6],
            }),
            shape: ShapeVisuals::None,
        },
        effects: &[],
        audio: AbilityAudio::SILENT,
    },
    // ── Enemy abilities (wire ids 64..) ───────────────────────────────
    //
    // Enemy abilities aren't routed through the player cast
    // dispatcher — enemy AI in `rift-server` ticks its own
    // wind-ups and calls `ability::cast_for_enemy` at resolve.
    // The data lives here so all tuning (damage / speed / radius
    // / wind-up) is in a single table both the AI and the client
    // visual-lookup pipeline read from.
    Ability {
        id: AbilityId("arcane_bolt"),
        wire_id: id::ARCANE_BOLT,
        name: "Arcane Bolt",
        description: "",
        icon: None,
        // Cooldown is enforced by the casting AI's per-attack
        // timer, not by the player cooldown table — but the
        // value lives here so the AI can read it.
        cooldown: 2.4,
        resource_cost: 0.0,
        channel_cost_per_sec: 0.0,
        base_damage: 9.0,
        damage_mult: 1.0,
        range: 14.0 * 1.5,
        unlock_level: 0,
        element: Element::None,
        archetype: Archetype::Projectile,
        scaling: Scaling::None,
        targeting: TargetingMode::Instant,
        kind: AbilityKind::EnemyProjectiles {
            count: 1,
            spread: 0.0,
            speed: 14.0,
            ttl: 1.5,
            windup: 0.55,
            size: 0.45,
            // Necrotic on hit: 50% reduced healing received for
            // 6 s. Pressures the support player (`HealTarget` /
            // Rejuvenation) without hard-countering them.
            apply_debuff: Some(crate::effects::id::NECROTIC),
        },
        visuals: AbilityVisuals {
            cast_spark: None,
            shape: ShapeVisuals::Projectile {
                mesh: MeshKind::ArcaneBolt,
                trail: VfxKind::ArcaneBoltTrail,
                impact: VfxKind::ArcaneBoltImpact,
                scale: 0.6,
            },
        },
        effects: &[],
        audio: AbilityAudio::SILENT,
    },
    // Boss multi-bolt fan — same projectile path as the
    // single-bolt caster attack but with `count > 1` and a
    // wider spread.
    Ability {
        id: AbilityId("arcane_fan"),
        wire_id: id::ARCANE_FAN,
        name: "Arcane Fan",
        description: "",
        icon: None,
        cooldown: 4.0,
        resource_cost: 0.0,
        channel_cost_per_sec: 0.0,
        base_damage: 10.0,
        damage_mult: 1.0,
        range: 14.0 * 1.5,
        unlock_level: 0,
        element: Element::None,
        archetype: Archetype::Projectile,
        scaling: Scaling::None,
        targeting: TargetingMode::Instant,
        kind: AbilityKind::EnemyProjectiles {
            count: 7,
            spread: 0.95,
            speed: 16.0,
            ttl: 1.8,
            windup: 0.7,
            size: 0.45,
            apply_debuff: None,
        },
        visuals: AbilityVisuals {
            cast_spark: None,
            shape: ShapeVisuals::Projectile {
                mesh: MeshKind::ArcaneBolt,
                trail: VfxKind::ArcaneBoltTrail,
                impact: VfxKind::ArcaneBoltImpact,
                scale: 0.6,
            },
        },
        effects: &[],
        audio: AbilityAudio::SILENT,
    },
    // Boss ground slam — single-resolve self-centred AoE.
    // The wire id 68 is the gameplay ability; the per-cast
    // visual telegraph + impact use the existing
    // GROUND_SLAM_WINDUP / GROUND_SLAM_IMPACT visual ids
    // emitted as side-channel `AbilityCast` events.
    Ability {
        id: AbilityId("ground_slam"),
        wire_id: id::GROUND_SLAM,
        name: "Ground Slam",
        description: "",
        icon: None,
        cooldown: 3.2,
        resource_cost: 0.0,
        channel_cost_per_sec: 0.0,
        base_damage: 32.0,
        damage_mult: 1.0,
        range: 4.6,
        unlock_level: 0,
        element: Element::Physical,
        archetype: Archetype::Aoe,
        scaling: Scaling::None,
        targeting: TargetingMode::Instant,
        kind: AbilityKind::DelayedAoe {
            radius: 4.6,
            windup: 0.85,
        },
        visuals: AbilityVisuals {
            cast_spark: None,
            shape: ShapeVisuals::None,
        },
        effects: &[],
        audio: AbilityAudio::SILENT,
    },
    // Boss summons — ring of brutes around the caster after
    // a wind-up.
    Ability {
        id: AbilityId("summon_brutes"),
        wire_id: id::SUMMON_BRUTES,
        name: "Summon Brutes",
        description: "",
        icon: None,
        cooldown: 10.0,
        resource_cost: 0.0,
        channel_cost_per_sec: 0.0,
        base_damage: 0.0,
        damage_mult: 0.0,
        range: 3.0,
        unlock_level: 0,
        element: Element::None,
        archetype: Archetype::Utility,
        scaling: Scaling::None,
        targeting: TargetingMode::Instant,
        kind: AbilityKind::Summon {
            count: 4,
            role: crate::monsters::MonsterRole::Brute,
            hp_mult: 1.2,
            ring_radius: 4.2,
            windup: 1.0,
        },
        visuals: AbilityVisuals {
            cast_spark: None,
            shape: ShapeVisuals::None,
        },
        effects: &[],
        audio: AbilityAudio::SILENT,
    },
    Ability {
        id: AbilityId("wraith_scream"),
        wire_id: id::WRAITH_SCREAM,
        name: "Wraith Scream",
        description: "",
        icon: None,
        cooldown: 3.2,
        resource_cost: 0.0,
        channel_cost_per_sec: 0.0,
        base_damage: 12.0,
        damage_mult: 1.0,
        range: 4.8,
        unlock_level: 0,
        element: Element::None,
        archetype: Archetype::Aoe,
        scaling: Scaling::None,
        targeting: TargetingMode::Instant,
        kind: AbilityKind::ClientOnly,
        visuals: AbilityVisuals::NONE,
        effects: &[],
        audio: AbilityAudio::SILENT,
    },
    Ability {
        id: AbilityId("void_sigil"),
        wire_id: id::VOID_SIGIL,
        name: "Void Sigil",
        description: "",
        icon: None,
        cooldown: 4.8,
        resource_cost: 0.0,
        channel_cost_per_sec: 0.0,
        base_damage: 14.0,
        damage_mult: 1.0,
        range: 11.0,
        unlock_level: 0,
        element: Element::None,
        archetype: Archetype::Aoe,
        scaling: Scaling::None,
        targeting: TargetingMode::Instant,
        kind: AbilityKind::DelayedAoe {
            radius: 2.6,
            windup: 0.95,
        },
        visuals: AbilityVisuals::NONE,
        effects: &[],
        audio: AbilityAudio::SILENT,
    },
    // Visual-only ability events — no kind/data, just the wire
    // id so the client renderer can dispatch off it.
    // Ground-slam wind-up + impact pair. Both are
    // visual-only ability events (no projectile / AoE-zone
    // entity) — the actual damage is applied by the caster's
    // own logic when the wind-up resolves. The client reads
    // `target` for the slam centre and `dir.x` for the radius
    // (m), packed into the AbilityCast event by the server.
    Ability {
        id: AbilityId("ground_slam_windup"),
        wire_id: id::GROUND_SLAM_WINDUP,
        name: "Slam (wind-up)",
        description: "",
        icon: None,
        cooldown: 0.0,
        resource_cost: 0.0,
        channel_cost_per_sec: 0.0,
        base_damage: 0.0,
        damage_mult: 0.0,
        range: 0.0,
        unlock_level: 0,
        element: Element::Physical,
        archetype: Archetype::Aoe,
        scaling: Scaling::None,
        targeting: TargetingMode::Instant,
        kind: AbilityKind::ClientOnly,
        visuals: AbilityVisuals {
            cast_spark: None,
            shape: ShapeVisuals::None,
        },
        effects: &[],
        audio: AbilityAudio::SILENT,
    },
    Ability {
        id: AbilityId("ground_slam_impact"),
        wire_id: id::GROUND_SLAM_IMPACT,
        name: "Slam (impact)",
        description: "",
        icon: None,
        cooldown: 0.0,
        resource_cost: 0.0,
        channel_cost_per_sec: 0.0,
        base_damage: 0.0,
        damage_mult: 0.0,
        range: 0.0,
        unlock_level: 0,
        element: Element::Physical,
        archetype: Archetype::Aoe,
        scaling: Scaling::None,
        targeting: TargetingMode::Instant,
        kind: AbilityKind::ClientOnly,
        visuals: AbilityVisuals {
            cast_spark: None,
            shape: ShapeVisuals::None,
        },
        effects: &[],
        audio: AbilityAudio::SILENT,
    },
];

/// Look up an ability by its wire id. Linear scan over `REGISTRY`
/// — at 8 entries this is faster than hashing.
pub fn lookup(ability_id: AbilityWireId) -> Option<&'static Ability> {
    REGISTRY.iter().find(|a| a.wire_id == ability_id)
}

/// Look up an ability by its [`AbilityId`] (string newtype).
/// Used by proc / talent dispatch where the source carries the
/// identity token rather than the wire id.
pub fn lookup_by_id(id: AbilityId) -> Option<&'static Ability> {
    REGISTRY.iter().find(|a| a.id == id)
}

/// Owned-clone variant of [`lookup`]. Kept for callers that need
/// to stash an `Ability` in a component / message struct.
pub fn from_wire_id(ability_id: AbilityWireId) -> Option<Ability> {
    lookup(ability_id).cloned()
}
