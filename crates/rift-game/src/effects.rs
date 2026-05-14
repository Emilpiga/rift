//! Effects — declarative status-effect data (buffs and debuffs).
//!
//! Pure data, like [`crate::abilities`]. The server consumes it
//! to drive an `EffectStack` component on entities; the client
//! uses [`EffectDef::color`] to paint indicator pips above the
//! target.
//!
//! Wire identity is the [`id::*`] table — stable u8 ids that
//! never reorder. Effects are surfaced in snapshots as a list of
//! `(id, remaining, duration)` tuples (`rift_net::messages::ActiveEffect`).
//!
//! Adding a new effect:
//!  1. Append a new `pub const FOO: u8 = N;` in [`id`].
//!  2. Add a `Some(...)` arm in [`lookup`] returning a static
//!     [`EffectDef`].
//!  3. (Optional) reference the new id from an ability's
//!     `apply_debuff` / `apply_buff` field.
//!
//! No engine / server code needs to know about a new id — the
//! data-driven systems pick it up automatically.

/// Wire-stable effect ids. Append-only.
pub mod id {
    pub const SLOW: u8 = 0;
    pub const BURN: u8 = 1;
    pub const MARK: u8 = 2;
    pub const CHILL: u8 = 3;
    /// Friendly heal-over-time buff. Lives in the same
    /// [`crate::effects`] table as debuffs because the
    /// per-tick clock + duration / refresh / snapshot
    /// machinery is the same; the effect type ([`super::EffectKind::HealOverTime`])
    /// flips the sign and the routing.
    pub const REJUVENATION: u8 = 4;
    /// Reduces healing the target receives. Applied by the
    /// enemy caster's arcane bolt — pressures support play
    /// by punishing top-up casts on a chip-damaged ally.
    pub const NECROTIC: u8 = 5;
    /// HUD-only timer for an active Void Familiar summon. It has
    /// no mechanical [`EffectKind`]; the client synthesizes it
    /// from owned minion snapshot rows so summons share the same
    /// duration-pip language as buffs.
    pub const VOID_FAMILIAR: u8 = 6;
}

/// One mechanical effect a debuff applies while it is active. A
/// [`EffectDef`] may carry several of these.
#[derive(Clone, Copy, Debug)]
pub enum EffectKind {
    /// Multiply incoming damage to the target. `>1.0` amplifies
    /// (e.g. Mark for Death = 1.25), `<1.0` mitigates.
    IncomingDamageMult(f32),
    /// Multiply the target's movement speed. `<1.0` slows.
    MoveSpeedMult(f32),
    /// Apply `dps * interval` damage every `interval` seconds.
    DamageOverTime { dps: f32, interval: f32 },
    /// Restore `hps * interval` HP every `interval` seconds.
    /// Routed through the same buff-stack tick as DoT, but
    /// queued into the heal pipeline instead of the damage
    /// one. Only applies to entities with an HP pool that
    /// supports healing (currently: players).
    HealOverTime { hps: f32, interval: f32 },
    /// Multiply healing the target receives (both direct
    /// heals and HoT ticks). `<1.0` reduces healing
    /// (Necrotic), `>1.0` amplifies it. Stacks
    /// multiplicatively across active effects, same shape
    /// as [`Self::IncomingDamageMult`].
    HealingReceivedMult(f32),
}

/// Static description of one status effect. All entries are
/// `&'static`, resolved through [`lookup`].
#[derive(Clone, Copy, Debug)]
pub struct EffectDef {
    pub id: u8,
    pub name: &'static str,
    /// Default duration in seconds. The applier may override per cast.
    pub default_duration: f32,
    pub effects: &'static [EffectKind],
    /// Indicator color (linear RGB) shown on the target. Used
    /// as the pip fill / frame tint when no `icon` is set.
    pub color: [f32; 3],
    /// Optional icon-atlas name (matches the same registry as
    /// `Ability.icon`). When present, the HUD renders the
    /// icon inside the pip; otherwise it falls back to a flat
    /// colored square.
    pub icon: Option<&'static str>,
}

/// Look up the static description of an effect id.
///
/// Returns `None` for unknown ids so future variants from a
/// newer peer don't crash older clients.
pub fn lookup(effect_id: u8) -> Option<&'static EffectDef> {
    use id::*;
    static SLOW_DEF: EffectDef = EffectDef {
        id: SLOW,
        name: "Slow",
        default_duration: 4.0,
        effects: &[EffectKind::MoveSpeedMult(0.55)],
        color: [0.55, 0.75, 1.00],
        icon: Some("Hunter_18"),
    };
    static BURN_DEF: EffectDef = EffectDef {
        id: BURN,
        name: "Burn",
        default_duration: 4.0,
        effects: &[EffectKind::DamageOverTime {
            dps: 6.0,
            interval: 0.5,
        }],
        color: [1.00, 0.45, 0.15],
        icon: Some("FireMage_12"),
    };
    static MARK_DEF: EffectDef = EffectDef {
        id: MARK,
        name: "Marked",
        default_duration: 6.0,
        effects: &[EffectKind::IncomingDamageMult(1.25)],
        color: [1.00, 0.85, 0.20],
        icon: Some("FireMage_30"),
    };
    static CHILL_DEF: EffectDef = EffectDef {
        id: CHILL,
        name: "Chilled",
        default_duration: 3.0,
        effects: &[
            EffectKind::MoveSpeedMult(0.70),
            EffectKind::IncomingDamageMult(1.10),
        ],
        color: [0.65, 0.90, 1.00],
        icon: Some("FrostMage_7"),
    };
    static REJUVENATION_DEF: EffectDef = EffectDef {
        id: REJUVENATION,
        name: "Rejuvenation",
        default_duration: 10.0,
        // 6 hps × 10 s = 60 HP total, ticked every 1 s. Matches
        // the `Rejuvenation` ability tooltip.
        effects: &[EffectKind::HealOverTime {
            hps: 6.0,
            interval: 1.0,
        }],
        color: [0.55, 1.00, 0.65],
        icon: Some("Druid_16"),
    };
    static NECROTIC_DEF: EffectDef = EffectDef {
        id: NECROTIC,
        name: "Necrotic",
        // Long enough to bridge a heal cooldown for the support
        // ally (HEAL_TARGET = 8s, HoT = 14s) so the debuff
        // pressures rather than just inconveniences.
        default_duration: 6.0,
        // 50% reduced healing received. Tuned to "punish but
        // not negate" — a ~40 HP heal still recovers 20 HP.
        effects: &[EffectKind::HealingReceivedMult(0.5)],
        // Sickly purple-green so it reads "decay" against the
        // green Rejuvenation buff.
        color: [0.55, 0.20, 0.55],
        icon: Some("Necromancer_18"),
    };
    static VOID_FAMILIAR_DEF: EffectDef = EffectDef {
        id: VOID_FAMILIAR,
        name: "Void Familiar",
        default_duration: 28.0,
        effects: &[],
        color: [0.55, 0.75, 1.00],
        icon: Some("Necromancer_14"),
    };
    Some(match effect_id {
        SLOW => &SLOW_DEF,
        BURN => &BURN_DEF,
        MARK => &MARK_DEF,
        CHILL => &CHILL_DEF,
        REJUVENATION => &REJUVENATION_DEF,
        NECROTIC => &NECROTIC_DEF,
        VOID_FAMILIAR => &VOID_FAMILIAR_DEF,
        _ => return None,
    })
}
