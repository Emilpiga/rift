//! Player ability loadout — the six wire ids the player has slotted
//! into their action bar.
//!
//! A [`Loadout`] is purely declarative: six `u8` wire ids matching
//! entries in [`crate::abilities::REGISTRY`]. The runtime
//! [`AbilitySlot`](crate::abilities::AbilitySlot) is built from a
//! loadout by looking up each wire id in the registry. Switching
//! abilities = mutate the loadout and re-materialize the slot.
//!
//! The wire id [`EMPTY_SLOT`] (`u8::MAX`) is the sentinel for an
//! empty action-bar slot. Empty slots also fall out of
//! [`Loadout::materialize`] naturally because they don't match
//! any registry entry.
//!
//! Action-bar slots themselves unlock at level milestones (see
//! [`SLOT_UNLOCK_LEVELS`]) so a level-1 hero only has slot 0;
//! the rest fill in over time as the player levels up. Combined
//! with each ability's own [`Ability::unlock_level`], this gates
//! the loadout editor.

use crate::abilities::{lookup, Ability, AbilityState, AbilitySlot, REGISTRY};

/// Number of action-bar slots. Mirrors `AbilitySlot::slots.len()`.
pub const SLOT_COUNT: usize = 6;

/// Sentinel wire id meaning "this action-bar slot is empty". Set
/// to `u8::MAX` so it can never collide with a real ability wire
/// id (the player range is 0..64, the enemy range starts at 64).
pub const EMPTY_SLOT: u8 = u8::MAX;

/// Character level required to unlock each action-bar slot, by
/// slot index. Slot 0 is always available; the rest scale up so
/// new players aren't overwhelmed and the bar fills in as they
/// level. Mirrors the Diablo 3 progression in spirit, tuned for
/// the current XP curve.
pub const SLOT_UNLOCK_LEVELS: [u32; SLOT_COUNT] = [1, 3, 6, 10, 15, 20];

/// `true` if the bar slot at `index` is unlocked at `player_level`.
pub fn is_slot_unlocked(index: usize, player_level: u32) -> bool {
    if index >= SLOT_COUNT {
        return false;
    }
    player_level >= SLOT_UNLOCK_LEVELS[index]
}

/// The six wire ids the player has slotted on the action bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loadout {
    pub slots: [u8; SLOT_COUNT],
}

impl Loadout {
    /// Build a loadout from raw wire ids. Unknown ids are kept
    /// verbatim — they'll materialize to an empty slot. No
    /// validation is done here; call [`Self::is_valid`] to
    /// check.
    pub const fn from_slots(slots: [u8; SLOT_COUNT]) -> Self {
        Self { slots }
    }

    /// Default starter loadout — only Fireball in slot 0,
    /// every other bar slot empty. Players unlock more slots
    /// (and more abilities) as they level up.
    pub const fn default_hero() -> Self {
        use crate::abilities::id;
        Self {
            slots: [
                id::FIRE_BALL,
                EMPTY_SLOT,
                EMPTY_SLOT,
                EMPTY_SLOT,
                EMPTY_SLOT,
                EMPTY_SLOT,
            ],
        }
    }

    /// `true` if every populated slot resolves to a player-castable
    /// ability in [`REGISTRY`]. Empty slots are always valid.
    pub fn is_valid(&self) -> bool {
        self.slots
            .iter()
            .all(|&id| id == EMPTY_SLOT || is_player_ability(id))
    }

    /// `true` if `wire_id` is one of the six slotted abilities.
    /// Server uses this to gate cast requests. Empty slots never
    /// match.
    pub fn contains(&self, wire_id: u8) -> bool {
        wire_id != EMPTY_SLOT && self.slots.contains(&wire_id)
    }

    /// Build a fresh [`AbilitySlot`] from this loadout. Each wire
    /// id is looked up in the registry; unknown / empty ids leave
    /// the slot empty.
    pub fn materialize(&self) -> AbilitySlot {
        let mut bar = AbilitySlot::new();
        for (i, &id) in self.slots.iter().enumerate() {
            if let Some(ab) = lookup(id) {
                bar.slots[i] = Some(AbilityState::new(ab.clone()));
            }
        }
        bar
    }

    /// Replace one slot with a new wire id (or [`EMPTY_SLOT`] to
    /// clear it). No-op when `index` is out of range. Caller is
    /// responsible for re-materializing the [`AbilitySlot`]
    /// afterwards (cooldowns reset on swap).
    pub fn set_slot(&mut self, index: usize, wire_id: u8) {
        if index < SLOT_COUNT {
            self.slots[index] = wire_id;
        }
    }
}

impl Default for Loadout {
    fn default() -> Self {
        Self::default_hero()
    }
}

/// `true` if `wire_id` belongs to an ability the player is allowed
/// to slot. Excludes enemy-only abilities and the empty sentinel.
pub fn is_player_ability(wire_id: u8) -> bool {
    if wire_id == EMPTY_SLOT || wire_id >= 64 {
        return false;
    }
    lookup(wire_id).is_some()
}

/// `true` if `wire_id` is unlocked for a character at
/// `player_level`. Empty / enemy / unknown ids are never
/// unlocked.
pub fn is_ability_unlocked(wire_id: u8, player_level: u32) -> bool {
    let Some(ab) = lookup(wire_id) else { return false };
    ab.wire_id < 64 && player_level >= ab.unlock_level
}

/// Iterator over every player-castable ability in the registry,
/// in registry order. Used by the spellbook UI to render the
/// pickable pool.
pub fn player_abilities() -> impl Iterator<Item = &'static Ability> {
    REGISTRY.iter().filter(|a| a.wire_id < 64)
}
