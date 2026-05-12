//! Aggregated ability modifiers from a [`super::Loadout`].
//!
//! The combat layer reads this once after equipment changes (cheap)
//! and consults it whenever an ability fires. Stat-only affixes
//! don't appear here — they go into [`super::StatBlock`] via
//! [`super::Item::stats`]. This struct only sees the four
//! gameplay-changing patterns (Amplify, Modify, Transform, Trigger).
//!
//! All maps key by [`crate::abilities::AbilityId`]; combat code that
//! already has the id can do constant-time lookups.

use std::collections::{HashMap, HashSet};

use crate::abilities::AbilityId;

use super::affixes::{AbilityVariant, AffixEffect, ProcAction, ProcEvent};
use super::item::RolledAffix;
use super::uniques::{BespokeId, LegendaryEffect};

/// One proc registered on the character.
#[derive(Clone, Copy, Debug)]
pub struct Proc {
    pub event: ProcEvent,
    pub action: ProcAction,
    /// Trigger probability in `0..=1`.
    pub chance: f32,
}

/// Final aggregated modifier set the combat layer queries.
///
/// All "amplify" multipliers stack **multiplicatively** (matching
/// most ARPG conventions: two `+25 % damage` mods become 1.25 ×
/// 1.25 = 1.5625, not 1.50). This keeps stacking interesting at
/// high gear levels without exponential explosion at low ones.
#[derive(Clone, Debug, Default)]
pub struct AbilityMods {
    /// `final_damage = base * damage_mult[ability]`. Defaults to 1.0.
    pub damage_mult: HashMap<AbilityId, f32>,
    /// `final_cooldown = base * cooldown_mult[ability]`. Defaults to 1.0.
    pub cooldown_mult: HashMap<AbilityId, f32>,
    /// Extra projectile count to add to a fan. Defaults to 0.
    pub extra_projectiles: HashMap<AbilityId, u32>,
    /// Active behavioural transforms. Last-applied wins per ability.
    pub transforms: HashMap<AbilityId, AbilityVariant>,
    /// Registered procs (no aggregation — multiple identical procs
    /// each get their own roll).
    pub procs: Vec<Proc>,
    /// Active bespoke (one-off) unique flags. Each
    /// [`BespokeId`] is a single boolean — equipped or not.
    /// The combat layer reads via [`Self::has_bespoke`].
    pub bespoke: HashSet<BespokeId>,
}

impl AbilityMods {
    pub fn new() -> Self {
        Self::default()
    }

    /// Damage multiplier for `ability` (or 1.0 if no affix touches it).
    pub fn damage_for(&self, ability: AbilityId) -> f32 {
        self.damage_mult.get(&ability).copied().unwrap_or(1.0)
    }

    /// Cooldown multiplier for `ability` (or 1.0).
    pub fn cooldown_for(&self, ability: AbilityId) -> f32 {
        self.cooldown_mult.get(&ability).copied().unwrap_or(1.0)
    }

    /// Bonus projectile count for `ability`.
    pub fn extra_projectiles_for(&self, ability: AbilityId) -> u32 {
        self.extra_projectiles.get(&ability).copied().unwrap_or(0)
    }

    /// Active transform on `ability`, if any.
    pub fn transform_for(&self, ability: AbilityId) -> Option<AbilityVariant> {
        self.transforms.get(&ability).copied()
    }

    /// Procs that fire on `event`.
    pub fn procs_for(&self, event: ProcEvent) -> impl Iterator<Item = &Proc> {
        self.procs.iter().filter(move |p| p.event == event)
    }

    /// `true` when at least one equipped unique stamped
    /// `flag` onto this aggregate (via
    /// [`Self::apply_legendary_effect`]). Combat sites that
    /// gate on a bespoke effect read this single boolean.
    pub fn has_bespoke(&self, flag: BespokeId) -> bool {
        self.bespoke.contains(&flag)
    }

    /// Fold one rolled affix into the running aggregate.
    pub(super) fn apply(&mut self, affix: &RolledAffix) {
        match affix.def.effect {
            AffixEffect::Stat(_) => { /* handled by StatBlock path */ }
            AffixEffect::AmplifyAbilityDamage(id) => {
                let entry = self.damage_mult.entry(id).or_insert(1.0);
                *entry *= 1.0 + affix.value;
            }
            AffixEffect::ReduceAbilityCooldown(id) => {
                let entry = self.cooldown_mult.entry(id).or_insert(1.0);
                *entry *= (1.0 - affix.value).max(0.20); // floor at 20 % of base
            }
            AffixEffect::ExtraProjectiles(id) => {
                let n = affix.value.round().max(0.0) as u32;
                *self.extra_projectiles.entry(id).or_insert(0) += n;
            }
            AffixEffect::TransformAbility(id, variant) => {
                self.transforms.insert(id, variant);
            }
            AffixEffect::Proc(event, action) => {
                self.procs.push(Proc {
                    event,
                    action,
                    chance: affix.value.clamp(0.0, 1.0),
                });
            }
        }
    }

    /// Fold one authored [`LegendaryEffect`] (Phase 4 named
    /// uniques) into the running aggregate. Mirrors [`Self::apply`]
    /// arm-for-arm for the three pattern variants, plus a
    /// [`BespokeId`] arm that records the flag on
    /// [`Self::bespoke`]. The combat layer reads the aggregated
    /// result through the same `damage_for` / `transform_for` /
    /// `extra_projectiles_for` / `procs_for` / `has_bespoke` API.
    pub(super) fn apply_legendary_effect(&mut self, eff: &LegendaryEffect) {
        match *eff {
            LegendaryEffect::Transform { ability, variant } => {
                self.transforms.insert(ability, variant);
            }
            LegendaryEffect::Proc {
                event,
                action,
                chance,
            } => {
                self.procs.push(Proc {
                    event,
                    action,
                    chance: chance.clamp(0.0, 1.0),
                });
            }
            LegendaryEffect::ExtraProjectiles { ability, count } => {
                *self.extra_projectiles.entry(ability).or_insert(0) += count;
            }
            LegendaryEffect::Bespoke(flag) => {
                self.bespoke.insert(flag);
            }
        }
    }
}
