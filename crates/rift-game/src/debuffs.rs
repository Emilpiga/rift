//! Debuffs — declarative status-effect data.
//!
//! Pure data, like [`crate::abilities`]. The server consumes it to
//! drive a [`DebuffStack`] component on entities; the client uses
//! [`DebuffDef::color`] to paint indicator pips above the target.
//!
//! Wire identity is the [`id::*`] table — stable u8 ids that never
//! reorder. Debuffs are surfaced in snapshots as a `u32` bitmask
//! (one bit per id), so adding a 33rd debuff requires widening the
//! mask field; add slot ids contiguously to keep that limit useful.
//!
//! Adding a new debuff:
//!  1. Append a new `pub const FOO: u8 = N;` in [`id`].
//!  2. Add a `Some(...)` arm in [`lookup`] returning a static
//!     [`DebuffDef`].
//!  3. (Optional) reference the new id from an ability's
//!     `apply_debuff` field.
//!
//! No engine / server code needs to know about a new id — the
//! data-driven systems pick it up automatically.

/// Wire-stable debuff ids. Append-only.
pub mod id {
    pub const SLOW: u8 = 0;
    pub const BURN: u8 = 1;
    pub const MARK: u8 = 2;
    pub const CHILL: u8 = 3;
}

/// One mechanical effect a debuff applies while it is active. A
/// [`DebuffDef`] may carry several of these.
#[derive(Clone, Copy, Debug)]
pub enum DebuffEffect {
    /// Multiply incoming damage to the target. `>1.0` amplifies
    /// (e.g. Mark for Death = 1.25), `<1.0` mitigates.
    IncomingDamageMult(f32),
    /// Multiply the target's movement speed. `<1.0` slows.
    MoveSpeedMult(f32),
    /// Apply `dps * interval` damage every `interval` seconds.
    DamageOverTime { dps: f32, interval: f32 },
}

/// Static description of one debuff. All entries are `&'static`,
/// resolved through [`lookup`].
#[derive(Clone, Copy, Debug)]
pub struct DebuffDef {
    pub id: u8,
    pub name: &'static str,
    /// Default duration in seconds. The applier may override per cast.
    pub default_duration: f32,
    pub effects: &'static [DebuffEffect],
    /// Indicator color (linear RGB) shown on the target.
    pub color: [f32; 3],
}

/// Look up the static description of a debuff id.
///
/// Returns `None` for unknown ids so future variants from a newer
/// peer don't crash older clients.
pub fn lookup(debuff_id: u8) -> Option<&'static DebuffDef> {
    use id::*;
    static SLOW_DEF: DebuffDef = DebuffDef {
        id: SLOW,
        name: "Slow",
        default_duration: 4.0,
        effects: &[DebuffEffect::MoveSpeedMult(0.55)],
        color: [0.55, 0.75, 1.00],
    };
    static BURN_DEF: DebuffDef = DebuffDef {
        id: BURN,
        name: "Burn",
        default_duration: 4.0,
        effects: &[DebuffEffect::DamageOverTime { dps: 6.0, interval: 0.5 }],
        color: [1.00, 0.45, 0.15],
    };
    static MARK_DEF: DebuffDef = DebuffDef {
        id: MARK,
        name: "Marked",
        default_duration: 6.0,
        effects: &[DebuffEffect::IncomingDamageMult(1.25)],
        color: [1.00, 0.85, 0.20],
    };
    static CHILL_DEF: DebuffDef = DebuffDef {
        id: CHILL,
        name: "Chilled",
        default_duration: 3.0,
        effects: &[
            DebuffEffect::MoveSpeedMult(0.70),
            DebuffEffect::IncomingDamageMult(1.10),
        ],
        color: [0.65, 0.90, 1.00],
    };
    Some(match debuff_id {
        SLOW => &SLOW_DEF,
        BURN => &BURN_DEF,
        MARK => &MARK_DEF,
        CHILL => &CHILL_DEF,
        _ => return None,
    })
}

/// Convenience: turn a debuff id into a single-bit mask value
/// (`1 << id`). Bits beyond 31 collapse to zero — keep ids small.
#[inline]
pub fn bit_for(debuff_id: u8) -> u32 {
    if debuff_id >= 32 { 0 } else { 1u32 << debuff_id }
}

/// Iterate the debuff ids encoded in a `u32` bitmask.
pub fn iter_mask(mask: u32) -> impl Iterator<Item = u8> {
    (0..32u8).filter(move |i| mask & (1 << i) != 0)
}
