//! Named **Unique** items — hand-authored Legendaries (Phase 4).
//!
//! See `ITEMS.md` §2.4 and §3 Phase 4. Items that roll
//! [`super::Rarity::Legendary`] consult this table during
//! [`super::Item::roll`]. If a [`UniqueDef`] matches the rolled
//! `(equip_slot, base_id)` the item adopts the unique's
//! [`LegendaryEffect`] instead of the procedural legendary affix
//! pool, picks up the authored name + flavor for its tooltip, and
//! stamps its own `id` into [`super::Item::unique_id`] so the
//! identity survives save / wipe round-trips.
//!
//! # Layout
//!
//! - [`LegendaryEffect`] — the four authored patterns Phase 4
//!   exposes (Transform, Proc, ExtraProjectiles, Bespoke). The
//!   first three mirror the [`super::affixes::AffixEffect`]
//!   variants the existing affix pool already produces, so the
//!   combat layer reuses the same dispatch paths via
//!   [`super::ability_mods::AbilityMods::apply_legendary_effect`].
//!   `Bespoke` is the escape hatch for one-offs that don't fit the
//!   three generic patterns; each `BespokeId` is one match arm in
//!   the combat layer.
//! - [`UniqueRoll`] — `Fixed` vs `Pool`. Pool uniques sample one
//!   ability from a curated `&'static [AbilityId]` at roll time;
//!   the picked index persists on the [`super::Item`] (see
//!   `unique_pick`) so the resolved effect is stable across save
//!   / load. Used today by `Mirrorglass Amulet` for its
//!   "random damage ability" cast pool.
//! - [`UniqueDef`] — one row in the [`UNIQUES`] table. The match
//!   predicate is a `fn(&BaseItem) -> bool` so each unique
//!   declares its own family / kind rule inline.
//! - [`UNIQUES`] — the seed catalogue. Five entries for the Phase
//!   4 launch (Embercrown, Splinterstep, Cleavebreaker, Mirrorglass
//!   Amulet, Shardspire). Stormcaller's Reach is deliberately
//!   deferred until Chain Lightning ships as a real ability.

use crate::abilities::{self, AbilityId};

use super::affixes::{AbilityVariant, ProcAction, ProcEvent};
use super::items::{BaseItem, EquipSlot, ItemSlot, WeaponKind};

/// Concrete legendary effect attached to an item. One per unique;
/// the active set on a player is therefore exactly the number of
/// equipped unique items.
///
/// Three of the four variants mirror the existing affix patterns
/// (`Transform`, `Proc`, `ExtraProjectiles`) so the combat layer
/// can reuse its dispatch code unchanged. `Bespoke` is the
/// catch-all for effects that don't reduce to one of the generic
/// patterns; adding a new bespoke effect is one variant on
/// [`BespokeId`] plus one match arm wherever it's consumed.
#[derive(Clone, Copy, Debug)]
pub enum LegendaryEffect {
    /// Reskin `ability` with the named [`AbilityVariant`]. Same
    /// semantics as [`super::affixes::AffixEffect::TransformAbility`]
    /// but authored directly on the unique so no `AffixDef` row
    /// is needed.
    Transform {
        ability: AbilityId,
        variant: AbilityVariant,
    },
    /// Register a proc. Mirrors
    /// [`super::affixes::AffixEffect::Proc`] but the `chance` is
    /// authored on the unique (no roll range) so every drop of
    /// the same unique reads identically.
    Proc {
        event: ProcEvent,
        action: ProcAction,
        chance: f32,
    },
    /// Add `count` projectiles to the ability's fan. Mirrors
    /// [`super::affixes::AffixEffect::ExtraProjectiles`] but with
    /// a fixed integer count rather than a rolled magnitude.
    ExtraProjectiles { ability: AbilityId, count: u32 },
    /// Bespoke effect — the combat layer matches the
    /// [`BespokeId`] directly. Use sparingly: every variant is a
    /// match arm at every consumer.
    Bespoke(BespokeId),
}

/// One-off legendary effects that don't reduce to one of the
/// generic [`LegendaryEffect`] patterns. Each variant maps to a
/// single match arm in the combat layer; aggregated onto the
/// player via [`super::ability_mods::AbilityMods::bespoke`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BespokeId {
    /// Mirrorglass Amulet — the wearer's evasive roll bypasses
    /// its cooldown gate. Triggered at the cast-dispatch site
    /// for [`abilities::EVASIVE_ROLL`].
    MirrorglassFreeRoll,
}

/// How the unique's effect is selected at roll time.
///
/// Most uniques are deterministic — one fixed [`LegendaryEffect`]
/// authored on the row, identical on every drop. A small number
/// of uniques (today: Mirrorglass) sample one ability from a
/// curated pool; the rolled index lives on the [`super::Item`]
/// so the same drop reads the same effect after a save / load
/// round-trip.
#[derive(Clone, Copy, Debug)]
pub enum UniqueRoll {
    /// Authored effect — no per-instance variation.
    Fixed(LegendaryEffect),
    /// Pool roll. Pick one ability from `pool` uniformly at roll
    /// time; the [`Item::unique_pick`] persists the index, and
    /// [`build`] turns the picked ability back into a concrete
    /// [`LegendaryEffect`] via `make`.
    ///
    /// Today's only consumer is Mirrorglass, whose `make` wraps
    /// the picked ability as
    /// `Proc(OnHit, CastAbility(...), chance)`.
    Pool {
        pool: &'static [AbilityId],
        make: fn(AbilityId) -> LegendaryEffect,
    },
}

/// One authored unique. Items roll one of these instead of a
/// procedural legendary affix when their `(slot, base)` matches.
#[derive(Clone, Copy, Debug)]
pub struct UniqueDef {
    /// Stable string id — persisted on the item, never reused.
    pub id: &'static str,
    /// Authored display name (tooltip headline).
    pub name: &'static str,
    /// Slot the unique targets. Matched against
    /// `BaseItem::equip_slot` before [`matches_base`].
    pub equip_slot: EquipSlot,
    /// Predicate over the candidate base. Lets each unique
    /// declare its own family / weapon-kind rule inline (e.g.
    /// "any staff that can roll Ice", "wands only").
    pub matches_base: fn(&BaseItem) -> bool,
    /// Effect generator — `Fixed` or `Pool`.
    pub roll: UniqueRoll,
    /// One-line italic flavour rendered under the headline in
    /// the tooltip.
    pub flavor: &'static str,
}

impl UniqueDef {
    /// Build the concrete [`LegendaryEffect`] for this unique
    /// instance, given the persisted pool-pick index. `pick` is
    /// ignored for `Fixed` rolls. Returns `None` if `pick` is
    /// out-of-range for the unique's pool (corrupt save / build
    /// drift after a unique's pool was shrunk).
    pub fn build(&self, pick: Option<u8>) -> Option<LegendaryEffect> {
        match self.roll {
            UniqueRoll::Fixed(eff) => Some(eff),
            UniqueRoll::Pool { pool, make } => {
                let p = pick.unwrap_or(0) as usize;
                let ability = *pool.get(p)?;
                Some(make(ability))
            }
        }
    }

    /// `true` when this unique's roll requires a `unique_pick`
    /// index to be sampled at roll time. Pure helper so the roll
    /// site doesn't reach into [`UniqueRoll`] directly.
    pub fn needs_pick(&self) -> bool {
        matches!(self.roll, UniqueRoll::Pool { .. })
    }

    /// Length of the pool this unique samples from (or 0 for
    /// `Fixed`). Used by the roll site to pick a uniform index.
    pub fn pool_len(&self) -> u8 {
        match self.roll {
            UniqueRoll::Fixed(_) => 0,
            UniqueRoll::Pool { pool, .. } => pool.len() as u8,
        }
    }
}

// ---------------------------------------------------------------
// Base-matcher helpers — declared as `fn` items so they can be
// stored in `UniqueDef::matches_base` (closures don't coerce
// to `fn` pointers).
// ---------------------------------------------------------------

fn any_helm(_b: &BaseItem) -> bool {
    true
}
fn any_boots(_b: &BaseItem) -> bool {
    true
}
fn any_amulet(_b: &BaseItem) -> bool {
    true
}
fn any_wand(b: &BaseItem) -> bool {
    matches!(b.slot, ItemSlot::Weapon(WeaponKind::Wand))
}
fn ice_staff(b: &BaseItem) -> bool {
    use crate::loot::families::Element;
    matches!(b.slot, ItemSlot::Weapon(WeaponKind::Staff)) && b.family.allows_element(Element::Ice)
}

// ---------------------------------------------------------------
// Pool builders — `fn` items so they can sit in `UniqueRoll::Pool`.
// ---------------------------------------------------------------

/// Mirrorglass on-hit proc: wrap the picked ability as a free
/// cast at 10 %. The cast goes through
/// [`ProcAction::CastAbility`], which the dispatcher already
/// recognises (today it queues the request; the cast pipeline
/// drain is Phase 4 follow-up work).
fn mirrorglass_proc(ability: AbilityId) -> LegendaryEffect {
    LegendaryEffect::Proc {
        event: ProcEvent::OnHit,
        action: ProcAction::CastAbility(ability),
        chance: 0.10,
    }
}

/// Curated damage-ability pool Mirrorglass samples its OnHit
/// proc from. Authored conservatively — every entry is a real,
/// player-castable damage spell so the proc always produces
/// something useful regardless of the wearer's loadout.
pub const MIRRORGLASS_POOL: &[AbilityId] = &[
    abilities::FIRE_BALL,
    abilities::FIREBALL_VOLLEY,
    abilities::FROST_RAY,
];

// ---------------------------------------------------------------
// The seed catalogue.
// ---------------------------------------------------------------

/// All authored uniques. Lookup by `id` via [`find`]; iteration
/// during [`super::Item::roll`] picks the first match for the
/// rolled `(equip_slot, base)`.
pub static UNIQUES: &[UniqueDef] = &[
    UniqueDef {
        id: "embercrown",
        name: "Embercrown",
        equip_slot: EquipSlot::Helm,
        matches_base: any_helm,
        roll: UniqueRoll::Fixed(LegendaryEffect::Transform {
            ability: abilities::FIRE_BALL,
            variant: AbilityVariant::FireballToBeam,
        }),
        flavor: "The fire forgets it was ever a stone.",
    },
    UniqueDef {
        id: "splinterstep",
        name: "Splinterstep",
        equip_slot: EquipSlot::Boots,
        matches_base: any_boots,
        roll: UniqueRoll::Fixed(LegendaryEffect::Proc {
            event: ProcEvent::OnDodge,
            action: ProcAction::Explosion {
                radius: 3.5,
                damage: 40.0,
            },
            chance: 1.0,
        }),
        flavor: "Every step a shrapnel of glass.",
    },
    UniqueDef {
        id: "cleavebreaker",
        name: "Cleavebreaker",
        equip_slot: EquipSlot::Weapon,
        matches_base: any_wand,
        roll: UniqueRoll::Fixed(LegendaryEffect::ExtraProjectiles {
            ability: abilities::FIREBALL_VOLLEY,
            count: 2,
        }),
        flavor: "The wand splits, and the world with it.",
    },
    UniqueDef {
        id: "mirrorglass_amulet",
        name: "Mirrorglass Amulet",
        equip_slot: EquipSlot::Amulet,
        matches_base: any_amulet,
        roll: UniqueRoll::Pool {
            pool: MIRRORGLASS_POOL,
            make: mirrorglass_proc,
        },
        flavor: "What the mirror sees, the mirror returns.",
    },
    // Mirrorglass also grants the free evasive-roll bypass. The
    // single-`LegendaryEffect` slot above already owns the
    // signature OnHit proc, so the free-roll lives on a second
    // entry that matches the same base. Both rows land on a
    // Mirrorglass drop because `Item::roll` collects every
    // matching unique into the legendary slot. Splitting the
    // effects keeps each row pure (one effect, one tooltip line)
    // and avoids inventing a multi-effect variant for this single
    // case.
    //
    // NB: today `Item::roll` selects exactly one matching unique
    // per drop, so the free-roll companion is **not** yet wired.
    // When the multi-effect roll site lands (Phase 4 follow-up)
    // re-include this row by uncommenting it.
    //
    // UniqueDef {
    //     id: "mirrorglass_amulet_freeroll",
    //     name: "Mirrorglass Amulet",
    //     equip_slot: EquipSlot::Amulet,
    //     matches_base: any_amulet,
    //     roll: UniqueRoll::Fixed(LegendaryEffect::Bespoke(
    //         BespokeId::MirrorglassFreeRoll,
    //     )),
    //     flavor: "",
    // },
    UniqueDef {
        id: "shardspire",
        name: "Shardspire",
        equip_slot: EquipSlot::Weapon,
        matches_base: ice_staff,
        roll: UniqueRoll::Fixed(LegendaryEffect::Transform {
            ability: abilities::FROST_RAY,
            variant: AbilityVariant::FrostRayShatter,
        }),
        flavor: "A spire of frozen breath, still listening.",
    },
];

/// Lookup a unique by stable string id. `None` for unknown ids —
/// the caller (persistence rehydration, tooltip render) treats
/// `None` as "unknown unique; render the item as a procedural
/// legendary" which is fully consistent with the rest of Phase
/// 4's roll site.
pub fn find(id: &str) -> Option<&'static UniqueDef> {
    UNIQUES.iter().find(|u| u.id == id)
}

/// Render a one-line tooltip string for `eff`. Mirrors the
/// rolled-affix tooltip style so unique lines read identically
/// to procedural legendary lines in the UI.
pub fn tooltip_line(eff: &LegendaryEffect) -> String {
    let name_of = |id: AbilityId| {
        crate::abilities::REGISTRY
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.name)
            .unwrap_or("???")
    };
    match *eff {
        LegendaryEffect::Transform { ability, variant } => match variant {
            AbilityVariant::FireballToBeam => {
                format!("{} becomes a piercing beam", name_of(ability))
            }
            AbilityVariant::FrostRayShatter => {
                // Hard-break on the em-dash so the cause /
                // effect halves of the description land on
                // their own rows in the tooltip. The
                // classifier in `rift-client` flat-maps each
                // line on `\n` before rendering.
                format!(
                    "{} sends a frost pulse along the beam.\nWhen it reaches the end, the beam shatters into shards.",
                    name_of(ability)
                )
            }
            AbilityVariant::WhirlwindVortex => format!("{} pulls enemies inward", name_of(ability)),
        },
        LegendaryEffect::Proc {
            event,
            action,
            chance,
        } => {
            let when = match event {
                ProcEvent::OnHit => "On hit",
                ProcEvent::OnCrit => "On crit",
                ProcEvent::OnKill => "On kill",
                ProcEvent::OnDodge => "On dodge",
                ProcEvent::OnLowHealth => "Below 30% HP",
            };
            let what = match action {
                ProcAction::CastAbility(id) => format!("cast {}", name_of(id)),
                ProcAction::Explosion { radius, damage } => {
                    format!("explode for {:.0} ({:.1} m)", damage, radius)
                }
            };
            format!("{}: {} ({:.0}%)", when, what, chance * 100.0)
        }
        LegendaryEffect::ExtraProjectiles { ability, count } => {
            format!("{} fires +{} projectiles", name_of(ability), count)
        }
        LegendaryEffect::Bespoke(id) => match id {
            BespokeId::MirrorglassFreeRoll => "Evasive Roll ignores its cooldown".to_string(),
        },
    }
}

/// Every unique that targets `base.equip_slot` and accepts
/// `base` under its predicate. The roll site picks one entry
/// from this list uniformly; an empty result means the legendary
/// drop falls back to the procedural legendary-effect roll.
pub fn candidates_for(base: &BaseItem) -> Vec<&'static UniqueDef> {
    UNIQUES
        .iter()
        .filter(|u| base.equip_slot == Some(u.equip_slot) && (u.matches_base)(base))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_unique_id_is_unique() {
        let mut ids: Vec<&'static str> = UNIQUES.iter().map(|u| u.id).collect();
        ids.sort();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate unique id in UNIQUES table");
    }

    #[test]
    fn pool_uniques_have_non_empty_pool() {
        for u in UNIQUES {
            if let UniqueRoll::Pool { pool, .. } = u.roll {
                assert!(!pool.is_empty(), "unique `{}` has empty pool", u.id);
                assert!(
                    pool.len() <= u8::MAX as usize,
                    "unique `{}` pool exceeds u8 index",
                    u.id,
                );
            }
        }
    }

    #[test]
    fn find_resolves_every_authored_id() {
        for u in UNIQUES {
            assert!(find(u.id).is_some(), "find() missed unique `{}`", u.id);
        }
    }

    #[test]
    fn find_returns_none_for_unknown() {
        assert!(find("not_a_real_unique").is_none());
    }

    /// Every authored unique must match **at least one** base in
    /// the live `BASE_ITEMS` catalogue. A unique whose
    /// `matches_base` predicate excludes every shipped base is
    /// dead content — it can never drop — so it's almost
    /// certainly a typo against a renamed base.
    #[test]
    fn every_unique_matches_at_least_one_base() {
        use super::super::items::BASE_ITEMS;
        for u in UNIQUES {
            let any = BASE_ITEMS
                .iter()
                .any(|b| b.equip_slot == Some(u.equip_slot) && (u.matches_base)(b));
            assert!(
                any,
                "unique `{}` ({:?}) matches no base in BASE_ITEMS — \
                 either the predicate is too strict or a referenced base was renamed",
                u.id, u.equip_slot,
            );
        }
    }

    #[test]
    fn build_returns_fixed_effect_ignoring_pick() {
        let u = find("embercrown").expect("embercrown");
        match u.build(None) {
            Some(LegendaryEffect::Transform { ability, .. }) => {
                assert_eq!(ability, abilities::FIRE_BALL);
            }
            other => panic!("expected Transform effect, got {:?}", other),
        }
        // Pick is ignored for Fixed.
        assert!(matches!(
            u.build(Some(99)),
            Some(LegendaryEffect::Transform { .. })
        ));
    }

    #[test]
    fn build_pool_resolves_to_picked_ability() {
        let u = find("mirrorglass_amulet").expect("mirrorglass");
        for (i, expected) in MIRRORGLASS_POOL.iter().enumerate() {
            match u.build(Some(i as u8)) {
                Some(LegendaryEffect::Proc {
                    action: ProcAction::CastAbility(a),
                    ..
                }) => assert_eq!(a, *expected),
                other => panic!("pool pick {i} unexpected: {:?}", other),
            }
        }
        // Out-of-range index yields None.
        assert!(u.build(Some(99)).is_none());
    }
}
