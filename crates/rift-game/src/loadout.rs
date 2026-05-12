//! Player ability loadout — the six wire ids the player has slotted
//! into their action bar.
//!
//! A [`Loadout`] is purely declarative: six [`AbilityWireId`] entries
//! matching rows in [`crate::abilities::REGISTRY`]. The runtime
//! [`AbilitySlot`](crate::abilities::AbilitySlot) is built from a
//! loadout by looking up each wire id in the registry. Switching
//! abilities = mutate the loadout and re-materialize the slot.
//!
//! The sentinel [`EMPTY_SLOT`] (`AbilityWireId(u8::MAX)`) marks an
//! empty action-bar slot. Empty slots also fall out of
//! [`Loadout::materialize`] naturally because they don't match
//! any registry entry.
//!
//! Action-bar slots themselves unlock at level milestones (see
//! [`SLOT_UNLOCK_LEVELS`]) so a level-1 hero only has slot 0;
//! the rest fill in over time as the player levels up. Combined
//! with each ability's own [`Ability::unlock_level`], this gates
//! the loadout editor.

use crate::abilities::{lookup, Ability, AbilitySlot, AbilityState, AbilityWireId, REGISTRY};
use crate::loot::{EquipSlot, Equipment, ItemSlot, WeaponKind};

/// Number of action-bar slots. Mirrors `AbilitySlot::slots.len()`.
pub const SLOT_COUNT: usize = 6;

/// Sentinel meaning "this action-bar slot is empty". The inner
/// byte is `u8::MAX` so it can never collide with a real ability
/// wire id (the player range is 0..64, enemies start at 64).
pub const EMPTY_SLOT: AbilityWireId = AbilityWireId::new(u8::MAX);

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
    pub slots: [AbilityWireId; SLOT_COUNT],
}

impl Loadout {
    /// Build a loadout from raw wire bytes (the
    /// wire / persistence layout). Unknown ids are kept verbatim
    /// — they'll materialize to an empty slot. Duplicate
    /// non-sentinel ids are normalized so only the last
    /// occurrence wins (mirrors the last-write-wins semantics of
    /// [`Self::set_slot`]) — this is the load-time defense for a
    /// persisted row that somehow ended up with the same ability
    /// in two slots (older builds didn't dedupe before saving).
    /// Call [`Self::is_valid`] to additionally check that every
    /// non-sentinel id resolves to a player-castable ability.
    pub fn from_slots(bytes: [u8; SLOT_COUNT]) -> Self {
        let slots = [
            AbilityWireId::new(bytes[0]),
            AbilityWireId::new(bytes[1]),
            AbilityWireId::new(bytes[2]),
            AbilityWireId::new(bytes[3]),
            AbilityWireId::new(bytes[4]),
            AbilityWireId::new(bytes[5]),
        ];
        let mut s = Self { slots };
        s.normalize();
        s
    }

    /// Inverse of [`Self::from_slots`]: project the typed slot
    /// array back to raw wire bytes for serialisation /
    /// persistence boundaries.
    pub fn to_wire_bytes(&self) -> [u8; SLOT_COUNT] {
        [
            self.slots[0].raw(),
            self.slots[1].raw(),
            self.slots[2].raw(),
            self.slots[3].raw(),
            self.slots[4].raw(),
            self.slots[5].raw(),
        ]
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
    /// ability in [`REGISTRY`] **and** no non-sentinel id appears
    /// twice. Empty slots are always valid.
    pub fn is_valid(&self) -> bool {
        if !self
            .slots
            .iter()
            .all(|&id| id == EMPTY_SLOT || is_player_ability(id))
        {
            return false;
        }
        // O(N^2) is fine — `SLOT_COUNT` is 6.
        for i in 0..SLOT_COUNT {
            if self.slots[i] == EMPTY_SLOT {
                continue;
            }
            for j in (i + 1)..SLOT_COUNT {
                if self.slots[i] == self.slots[j] {
                    return false;
                }
            }
        }
        true
    }

    /// `true` if `wire_id` is one of the six slotted abilities.
    /// Server uses this to gate cast requests. Empty slots never
    /// match.
    pub fn contains(&self, wire_id: AbilityWireId) -> bool {
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

    /// Like [`Self::materialize`] but synthesises slot 0 (the LMB
    /// slot) from the equipped weapon via [`weapon_lmb_ability`].
    /// The persisted slot-0 entry in [`Self::slots`] is ignored —
    /// LMB is locked to the weapon for MVP. Other slots (1..N)
    /// materialise from the persisted loadout exactly as before.
    pub fn materialize_with_weapon(&self, equipment: &Equipment) -> AbilitySlot {
        let mut bar = self.materialize();
        let lmb_id = weapon_lmb_ability(equipment);
        bar.slots[0] = lookup(lmb_id).map(|ab| AbilityState::new(ab.clone()));
        bar
    }

    /// Replace one slot with a new wire id (or [`EMPTY_SLOT`] to
    /// clear it). No-op when `index` is out of range.
    ///
    /// Each ability lives in **at most one** action-bar slot. If
    /// `wire_id` is already in another slot, that other slot is
    /// cleared as part of the assignment — the action bar can't
    /// duplicate-stack the same ability across slots, which
    /// would otherwise let the player parallelize cooldowns by
    /// equipping the same spell N times. Setting a slot to
    /// [`EMPTY_SLOT`] never dedupes (multiple empty slots are
    /// expected). Caller is responsible for re-materializing
    /// the [`AbilitySlot`] afterwards (cooldowns reset on
    /// swap).
    pub fn set_slot(&mut self, index: usize, wire_id: AbilityWireId) {
        if index >= SLOT_COUNT {
            return;
        }
        if wire_id != EMPTY_SLOT {
            for (i, slot) in self.slots.iter_mut().enumerate() {
                if i != index && *slot == wire_id {
                    *slot = EMPTY_SLOT;
                }
            }
        }
        self.slots[index] = wire_id;
    }

    /// Collapse duplicate non-sentinel ids in-place so each
    /// ability appears in at most one slot. Later occurrences
    /// win (matches [`Self::set_slot`] semantics: a fresh
    /// assignment displaces the prior one). Used by
    /// [`Self::from_slots`] / persisted-row loaders to
    /// defensively repair rows that pre-date the dedup
    /// invariant.
    pub fn normalize(&mut self) {
        // Strip any stale Evasive Roll entries — it's now a
        // passive bound to Space, so persisted rows from
        // pre-passive builds need to be repaired on load.
        for s in self.slots.iter_mut() {
            if *s == crate::abilities::id::EVASIVE_ROLL {
                *s = EMPTY_SLOT;
            }
        }
        for i in 0..SLOT_COUNT {
            let id = self.slots[i];
            if id == EMPTY_SLOT {
                continue;
            }
            for j in (i + 1)..SLOT_COUNT {
                if self.slots[j] == id {
                    self.slots[i] = EMPTY_SLOT;
                    break;
                }
            }
        }
    }
}

impl Default for Loadout {
    fn default() -> Self {
        Self::default_hero()
    }
}

/// `true` if `wire_id` belongs to an ability the player is allowed
/// to slot. Excludes enemy-only abilities, the empty sentinel,
/// and passive abilities (currently [`crate::abilities::id::EVASIVE_ROLL`])
/// that live on a fixed key rather than the action bar.
pub fn is_player_ability(wire_id: AbilityWireId) -> bool {
    if wire_id == EMPTY_SLOT || wire_id.raw() >= 64 {
        return false;
    }
    if wire_id == crate::abilities::id::EVASIVE_ROLL {
        return false;
    }
    // Melee attack is a weapon-locked LMB ability — not slottable
    // from the spellbook (it lives in slot 0 only when the
    // player has a Sword / Dagger equipped, via
    // [`weapon_lmb_ability`]).
    if wire_id == crate::abilities::id::MELEE_ATTACK {
        return false;
    }
    lookup(wire_id).is_some()
}

/// `true` if `wire_id` is unlocked for a character at
/// `player_level`. Empty / enemy / unknown ids are never
/// unlocked.
pub fn is_ability_unlocked(wire_id: AbilityWireId, player_level: u32) -> bool {
    let Some(ab) = lookup(wire_id) else {
        return false;
    };
    ab.wire_id.raw() < 64 && player_level >= ab.unlock_level
}

/// Iterator over every player-castable ability in the registry,
/// in registry order. Used by the spellbook UI to render the
/// pickable pool. Skips passives that live on a fixed key
/// (Evasive Roll on Space) — those have a dedicated HUD slot
/// and are not part of the action-bar loadout.
pub fn player_abilities() -> impl Iterator<Item = &'static Ability> {
    REGISTRY.iter().filter(|a| {
        a.wire_id.raw() < 64
            && a.wire_id != crate::abilities::id::EVASIVE_ROLL
            && a.wire_id != crate::abilities::id::MELEE_ATTACK
    })
}

/// Wire id of the ability that should occupy the LMB slot for a
/// player wearing `equipment`. LMB is locked to the weapon for
/// MVP: melee weapons (Sword / Dagger) get
/// [`crate::abilities::id::MELEE_ATTACK`]; caster weapons (Staff
/// / Wand) and an empty weapon slot both fall back to
/// [`crate::abilities::id::FIRE_BALL`].
pub fn weapon_lmb_ability(equipment: &Equipment) -> AbilityWireId {
    use crate::abilities::id;
    let weapon = equipment.get(EquipSlot::Weapon);
    match weapon.map(|it| it.base.slot) {
        Some(ItemSlot::Weapon(WeaponKind::Sword | WeaponKind::Dagger)) => id::MELEE_ATTACK,
        _ => id::FIRE_BALL,
    }
}

/// Authoritative check used by the server to gate a player cast.
/// Accepts the always-available passive (Evasive Roll), every
/// ability the player has slotted in their persisted loadout,
/// and — for the MVP — the weapon-derived LMB ability even when
/// the persisted loadout slot 0 holds a different (or empty)
/// id. Mirrors [`Loadout::materialize_with_weapon`] so the
/// client and server agree on which ids count as castable.
pub fn can_player_cast(
    loadout: &Loadout,
    equipment: &Equipment,
    ability_id: AbilityWireId,
) -> bool {
    if ability_id == crate::abilities::id::EVASIVE_ROLL {
        return true;
    }
    if loadout.contains(ability_id) {
        return true;
    }
    ability_id == weapon_lmb_ability(equipment)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abilities::id;

    #[test]
    fn set_slot_dedupes_when_same_ability_assigned_elsewhere() {
        let mut l = Loadout {
            slots: [
                id::FIRE_BALL,
                id::FROST_RAY,
                EMPTY_SLOT,
                EMPTY_SLOT,
                EMPTY_SLOT,
                EMPTY_SLOT,
            ],
        };
        l.set_slot(2, id::FIRE_BALL);
        // Slot 0 cleared, slot 2 now holds the ability.
        assert_eq!(l.slots[0], EMPTY_SLOT);
        assert_eq!(l.slots[2], id::FIRE_BALL);
        assert_eq!(l.slots[1], id::FROST_RAY);
        assert!(l.is_valid());
    }

    #[test]
    fn set_slot_to_empty_does_not_clear_other_empties() {
        let mut l = Loadout {
            slots: [
                id::FIRE_BALL,
                EMPTY_SLOT,
                EMPTY_SLOT,
                EMPTY_SLOT,
                EMPTY_SLOT,
                EMPTY_SLOT,
            ],
        };
        l.set_slot(0, EMPTY_SLOT);
        assert_eq!(l.slots, [EMPTY_SLOT; SLOT_COUNT]);
    }

    #[test]
    fn from_slots_normalizes_persisted_duplicates() {
        // Pretend a buggy older row stored the same ability twice.
        let l = Loadout::from_slots([
            id::FIRE_BALL.raw(),
            id::FIRE_BALL.raw(),
            EMPTY_SLOT.raw(),
            EMPTY_SLOT.raw(),
            EMPTY_SLOT.raw(),
            EMPTY_SLOT.raw(),
        ]);
        assert!(l.is_valid());
        // Only one slot retained the ability.
        let count = l.slots.iter().filter(|&&s| s == id::FIRE_BALL).count();
        assert_eq!(count, 1);
    }

    #[test]
    fn is_valid_rejects_duplicates() {
        // Construct directly (skipping `from_slots`' normalize)
        // to verify the validator catches the bad state.
        let l = Loadout {
            slots: [
                id::FIRE_BALL,
                id::FIRE_BALL,
                EMPTY_SLOT,
                EMPTY_SLOT,
                EMPTY_SLOT,
                EMPTY_SLOT,
            ],
        };
        assert!(!l.is_valid());
    }
}
