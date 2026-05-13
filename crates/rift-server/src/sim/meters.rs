//! Per-instance combat meters.
//!
//! Tracks cumulative damage dealt, damage taken, and healing
//! done per client over the lifetime of one rift run. Threat is
//! derived live (summed from each alive enemy's `threat` map at
//! capture time) so it tracks the current aggro picture rather
//! than accumulating.
//!
//! Reset by [`super::Sim::reset_meters`], called by the server
//! main loop when a client enters a fresh rift instance.

use std::collections::HashMap;

use hecs::Entity;
use rift_game::abilities::AbilityWireId;
use rift_net::messages::{
    MeterAbilityBreakdown, MeterEntry, MeterTakenAbility, MeterTakenAttackerBreakdown, ServerMsg,
};
use rift_net::ClientId;

use super::enemies::ServerEnemy;
use super::player::ServerPlayer;

/// Reserved wire id for any contribution we couldn't trace to a
/// concrete ability — DoTs from unknown casters, environmental
/// damage, basic-attack auto-hits. Pairs with the same constant
/// understood by the client UI.
pub const ABILITY_ID_OTHER: AbilityWireId = AbilityWireId::new(255);

/// Reserved attacker-kind id for the TAKEN-tab breakdown when
/// we can't (or don't care to) attribute the source to a
/// `MonsterRole`: thorns reflect, anonymous DoTs, environmental
/// damage. Sits in the same `u8` namespace as
/// [`rift_game::monsters::MonsterRole::to_wire_byte`] but stays
/// out of the way of any real role.
pub const ATTACKER_KIND_OTHER: u8 = 255;

/// Per-ability slice of one player's damage / healing rows
/// (the DMG and HPS tabs in the HUD). The TAKEN tab uses the
/// separate two-level [`MeterAccum::taken_by_attacker`] map
/// instead.
#[derive(Default, Clone, Debug)]
pub struct AbilityAccum {
    pub damage_dealt: f32,
    pub healing_done: f32,
}

/// Outer bucket of the TAKEN-tab accumulator: total damage taken
/// from one attacker kind, plus a per-ability inner map.
#[derive(Default, Clone, Debug)]
pub struct AttackerAccum {
    pub damage_taken: f32,
    pub by_ability: HashMap<AbilityWireId, f32>,
}

/// Per-client cumulative counters. Numbers are HP units.
#[derive(Default, Clone, Debug)]
pub struct MeterAccum {
    pub damage_dealt: f32,
    pub damage_taken: f32,
    pub healing_done: f32,
    /// Per-ability slice for damage dealt + healing done.
    /// Keyed by wire-stable id from
    /// `rift_game::abilities::id::*`, or [`ABILITY_ID_OTHER`].
    pub by_ability: HashMap<AbilityWireId, AbilityAccum>,
    /// Two-level breakdown for damage *taken*: outer key is
    /// the attacker kind (`MonsterRole::to_wire_byte()` or
    /// [`ATTACKER_KIND_OTHER`]), inner is per-ability damage.
    /// Drives the TAKEN-tab nested expansion in the HUD.
    pub taken_by_attacker: HashMap<u8, AttackerAccum>,
}

impl MeterAccum {
    /// Credit `amount` damage to a specific ability. Updates
    /// both the top-line total and the ability slice.
    pub fn add_damage(&mut self, ability_id: AbilityWireId, amount: f32) {
        self.damage_dealt += amount;
        self.by_ability.entry(ability_id).or_default().damage_dealt += amount;
    }

    /// Credit `amount` healing to a specific ability.
    pub fn add_healing(&mut self, ability_id: AbilityWireId, amount: f32) {
        self.healing_done += amount;
        self.by_ability.entry(ability_id).or_default().healing_done += amount;
    }

    /// Credit `amount` damage *taken* from a specific source.
    /// `attacker_kind` is `MonsterRole::to_wire_byte()` for
    /// known enemies or [`ATTACKER_KIND_OTHER`] for sources
    /// without an enemy origin (thorns reflect, environmental).
    /// `ability_id` is the wire-stable id of the source
    /// ability or [`ABILITY_ID_OTHER`]. Updates the top-line
    /// `damage_taken` total *and* both axes of the
    /// `taken_by_attacker` two-level map so the TAKEN tab can
    /// drill from attacker → ability.
    pub fn add_damage_taken(&mut self, attacker_kind: u8, ability_id: AbilityWireId, amount: f32) {
        self.damage_taken += amount;
        let bucket = self.taken_by_attacker.entry(attacker_kind).or_default();
        bucket.damage_taken += amount;
        *bucket.by_ability.entry(ability_id).or_default() += amount;
    }
}

/// Per-instance meters. Owned by [`super::Sim`].
#[derive(Default)]
pub struct Meters {
    pub by_client: HashMap<ClientId, MeterAccum>,
    /// Wall time, in seconds, since the last [`Self::reset`].
    /// Lets the client render per-second rates without
    /// keeping its own clock.
    pub elapsed: f32,
}

impl Meters {
    /// Look up a row, inserting a default if absent.
    pub fn entry(&mut self, cid: ClientId) -> &mut MeterAccum {
        self.by_client.entry(cid).or_default()
    }

    /// Build a [`ServerMsg::MeterSnapshot`] for broadcast.
    /// `world` is read to (a) resolve player entities to net
    /// ids and (b) fold enemy threat maps into a per-player
    /// instantaneous threat value.
    pub fn build_snapshot(&self, world: &hecs::World) -> ServerMsg {
        // Resolve `(ClientId, Entity, NetId)` for every connected
        // player so we can both join with `by_client` and key
        // the threat fold by `Entity`.
        let players: Vec<(ClientId, Entity, rift_net::NetId)> = world
            .query::<&ServerPlayer>()
            .iter()
            .map(|(e, p)| (p.client_id, e, p.net_id))
            .collect();

        // Sum threat per attacker entity across alive enemies.
        let mut threat_by_entity: HashMap<Entity, f32> = HashMap::new();
        for (_, en) in world.query::<&ServerEnemy>().iter() {
            if en.is_dying() {
                continue;
            }
            for (&attacker, &amount) in &en.threat {
                *threat_by_entity.entry(attacker).or_insert(0.0) += amount;
            }
        }

        let mut entries = Vec::with_capacity(players.len());
        for (cid, entity, net_id) in players {
            let accum = self.by_client.get(&cid).cloned().unwrap_or_default();
            let threat = threat_by_entity.get(&entity).copied().unwrap_or(0.0);
            // Build the per-ability breakdown sorted descending
            // by total contribution (damage + healing). The
            // client renders rows top-down without resorting.
            let mut abilities: Vec<MeterAbilityBreakdown> = accum
                .by_ability
                .iter()
                .map(|(id, a)| MeterAbilityBreakdown {
                    ability_id: id.raw(),
                    damage_dealt: a.damage_dealt,
                    healing_done: a.healing_done,
                })
                .collect();
            abilities.sort_by(|a, b| {
                let ka = a.damage_dealt + a.healing_done;
                let kb = b.damage_dealt + b.healing_done;
                kb.partial_cmp(&ka).unwrap_or(std::cmp::Ordering::Equal)
            });
            // Two-level breakdown for the TAKEN tab. Inner
            // ability rows are sorted descending by damage so
            // the client can render them top-down; outer
            // attacker rows are sorted the same way by total.
            let mut taken_attackers: Vec<MeterTakenAttackerBreakdown> = accum
                .taken_by_attacker
                .iter()
                .map(|(kind, ack)| {
                    let mut abilities: Vec<MeterTakenAbility> = ack
                        .by_ability
                        .iter()
                        .map(|(id, amt)| MeterTakenAbility {
                            ability_id: id.raw(),
                            damage_taken: *amt,
                        })
                        .collect();
                    abilities.sort_by(|a, b| {
                        b.damage_taken
                            .partial_cmp(&a.damage_taken)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    MeterTakenAttackerBreakdown {
                        attacker_kind: *kind,
                        damage_taken: ack.damage_taken,
                        abilities,
                    }
                })
                .collect();
            taken_attackers.sort_by(|a, b| {
                b.damage_taken
                    .partial_cmp(&a.damage_taken)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            entries.push(MeterEntry {
                net_id,
                damage_dealt: accum.damage_dealt,
                damage_taken: accum.damage_taken,
                healing_done: accum.healing_done,
                threat,
                abilities,
                taken_attackers,
            });
        }

        ServerMsg::MeterSnapshot {
            elapsed_seconds: self.elapsed,
            entries,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_damage_accumulates_total_and_per_ability() {
        let mut a = MeterAccum::default();
        a.add_damage(AbilityWireId::new(7), 10.0);
        a.add_damage(AbilityWireId::new(7), 5.0);
        a.add_damage(AbilityWireId::new(9), 3.0);
        assert_eq!(a.damage_dealt, 18.0);
        assert_eq!(a.by_ability[&AbilityWireId::new(7)].damage_dealt, 15.0);
        assert_eq!(a.by_ability[&AbilityWireId::new(9)].damage_dealt, 3.0);
        // Healing slice on those same abilities stays at zero
        // — the per-ability struct holds both metrics but we
        // only touched damage.
        assert_eq!(a.by_ability[&AbilityWireId::new(7)].healing_done, 0.0);
    }

    #[test]
    fn add_healing_accumulates_separately_from_damage() {
        let mut a = MeterAccum::default();
        a.add_damage(AbilityWireId::new(1), 4.0);
        a.add_healing(AbilityWireId::new(1), 6.0);
        a.add_healing(AbilityWireId::new(2), 2.0);
        assert_eq!(a.damage_dealt, 4.0);
        assert_eq!(a.healing_done, 8.0);
        // Same ability id collects both axes.
        assert_eq!(a.by_ability[&AbilityWireId::new(1)].damage_dealt, 4.0);
        assert_eq!(a.by_ability[&AbilityWireId::new(1)].healing_done, 6.0);
        assert_eq!(a.by_ability[&AbilityWireId::new(2)].healing_done, 2.0);
    }

    #[test]
    fn ability_id_other_is_just_another_bucket() {
        // Unattributed contributions go to the ABILITY_ID_OTHER
        // sentinel; nothing in MeterAccum special-cases it.
        let mut a = MeterAccum::default();
        a.add_damage(ABILITY_ID_OTHER, 12.5);
        a.add_damage(AbilityWireId::new(5), 7.5);
        assert_eq!(a.damage_dealt, 20.0);
        assert_eq!(a.by_ability[&ABILITY_ID_OTHER].damage_dealt, 12.5);
        assert_eq!(a.by_ability[&AbilityWireId::new(5)].damage_dealt, 7.5);
    }

    #[test]
    fn zero_amount_still_creates_bucket() {
        // We don't filter zero contributions — the entry is
        // created so a later non-zero hit on the same ability
        // doesn't lose its prior \"hit count\" semantics. This
        // test pins that behaviour so we notice if it changes.
        let mut a = MeterAccum::default();
        a.add_damage(AbilityWireId::new(3), 0.0);
        assert_eq!(a.damage_dealt, 0.0);
        assert!(a.by_ability.contains_key(&AbilityWireId::new(3)));
    }

    #[test]
    fn meters_entry_inserts_default_on_miss() {
        let mut m = Meters::default();
        let cid = ClientId(42);
        m.entry(cid).add_damage(AbilityWireId::new(1), 5.0);
        assert_eq!(m.by_client[&cid].damage_dealt, 5.0);
        // Re-entry returns the same accum, not a fresh one.
        m.entry(cid).add_damage(AbilityWireId::new(1), 5.0);
        assert_eq!(m.by_client[&cid].damage_dealt, 10.0);
    }
}
