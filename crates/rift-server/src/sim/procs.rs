//! Legendary `Proc` consumption (event → action triggers).
//!
//! A proc is `(ProcEvent, ProcAction, chance)`. When `event`
//! happens to a player, roll `chance` deterministically off the
//! current tick + a per-call salt; on success, run `action`.
//! Events: `OnCrit`, `OnHit`, `OnKill`, `OnDodge`,
//! `OnLowHealth`. Actions: `Explosion` (wired), `CastAbility`
//! and `ChainLightning` (declared, not yet routed).
//!
//! Call sites:
//! * `OnHit` / `OnCrit` — `projectile::apply_hits_to_enemies`
//!   per-hit, after damage is applied.
//! * `OnDodge` — `apply_player_damage` when a passive evasion
//!   roll cancels an incoming hit.
//! * `OnLowHealth` — `apply_player_damage` on the tick the
//!   target's HP fraction crosses below 0.30 (one-shot per dip,
//!   re-arms when HP crosses back above the threshold).
//! * `OnKill` — not currently wired (would need killer
//!   attribution threaded through `loot::finalise_kills`).
//!
//! # Adding a new action
//! Add a new `ProcAction` variant in `rift_game::loot::affixes`,
//! then one match arm in [`dispatch`].

use glam::Vec3;
use rift_game::loot::ability_mods::Proc;
use rift_game::loot::{ProcAction, ProcEvent};
use rift_net::{NetId, NetTick};

use super::projectile::{mix64, ServerAoeZone, Team};

/// Sinks the proc dispatcher writes into. The same set of
/// pools the rest of the combat pipeline already touches —
/// keeps wire / world ordering consistent (proc-spawned zones
/// tick the same frame as the kill that produced them).
pub struct ProcSink<'a> {
    pub aoe_zones: &'a mut Vec<ServerAoeZone>,
    pub next_projectile_net_id: &'a mut u32,
    pub tick: NetTick,
}

/// One per-fire context. `team` decides who the spawned AoE
/// damages: `Team::Player` for player→enemy procs (OnHit /
/// OnCrit / OnKill), `Team::Enemy` for player-defensive procs
/// that hurt nearby enemies (OnDodge, OnLowHealth — both
/// emit player-team zones, so callers pass `Player` there too).
pub struct ProcOrigin {
    pub position: Vec3,
    pub attacker_kind: u8,
    /// Wire ability id of the source action (for meter
    /// attribution). Use `super::meters::ABILITY_ID_OTHER` for
    /// non-attributable triggers (OnDodge / OnLowHealth).
    pub ability_id: u8,
    pub team: Team,
    /// Salt for the deterministic chance roll. Mix in something
    /// hit-unique (enemy net-id, frame counter) so multiple
    /// procs the same tick don't all roll identically.
    pub salt: u64,
}

/// Roll `chance` against a deterministic uniform `[0, 1)` derived
/// from `(tick, salt, action_marker)`. Replays produce identical
/// proc fires for the same hit identity.
fn roll(tick: NetTick, salt: u64, marker: u64, chance: f32) -> bool {
    if chance >= 1.0 {
        return true;
    }
    if chance <= 0.0 {
        return false;
    }
    let seed = mix64((tick.0 as u64).wrapping_add(salt).wrapping_add(marker.rotate_left(11)));
    let r = (seed >> 40) as f32 / (1u32 << 24) as f32;
    r < chance
}

/// Dispatch every proc in `procs` whose `event` matches.
/// Today's only fully-modeled action is `Explosion`, which
/// pushes a single-tick AoE zone into `sink.aoe_zones`. Other
/// actions (`CastAbility`, `ChainLightning`) are no-ops; adding
/// them is one match arm here once their concrete spawn paths
/// land.
pub fn dispatch(
    event: ProcEvent,
    procs: &[Proc],
    origin: &ProcOrigin,
    sink: &mut ProcSink<'_>,
) {
    let mut idx: u64 = 0;
    for p in procs {
        idx = idx.wrapping_add(1);
        if p.event != event {
            continue;
        }
        if !roll(sink.tick, origin.salt, idx, p.chance) {
            continue;
        }
        match p.action {
            ProcAction::Explosion { radius, damage } => {
                spawn_explosion(origin, radius, damage, sink);
            }
            // Other actions: not yet wired. Filter is intentional —
            // the call site does't have to gate by action kind, and
            // adding a new payload becomes one arm here.
            ProcAction::CastAbility(_) | ProcAction::ChainLightning { .. } => {}
        }
    }
}

fn spawn_explosion(origin: &ProcOrigin, radius: f32, damage: f32, sink: &mut ProcSink<'_>) {
    let zone_net_id = NetId(*sink.next_projectile_net_id);
    *sink.next_projectile_net_id = sink.next_projectile_net_id.wrapping_add(1).max(1);
    sink.aoe_zones.push(ServerAoeZone {
        owner: zone_net_id,
        ability_id: origin.ability_id,
        attacker_kind: origin.attacker_kind,
        team: origin.team,
        position: origin.position,
        radius,
        damage_per_tick: damage,
        crit_chance: 0.0,
        crit_damage: 0.0,
        // Single-tick "pop" — one application then expires the
        // same frame as the next AoE pass.
        tick_interval: 0.05,
        duration: 0.05,
        elapsed: 0.0,
        tick_timer: 0.05,
        apply_debuff: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rift_game::abilities::id;
    use rift_game::loot::ability_mods::Proc;

    /// Tick / origin / sink scaffolding used by every test —
    /// keeps each test body focused on the behaviour under
    /// inspection rather than the boilerplate around it.
    fn ctx() -> (Vec<ServerAoeZone>, u32, NetTick, ProcOrigin) {
        let zones: Vec<ServerAoeZone> = Vec::new();
        let next_id: u32 = 0x4000_0000;
        let tick = NetTick(42);
        let origin = ProcOrigin {
            position: Vec3::new(1.0, 0.0, 2.0),
            attacker_kind: 7,
            ability_id: id::FIRE_BALL,
            team: Team::Player,
            salt: 0xABCD_1234_5678_9ABC,
        };
        (zones, next_id, tick, origin)
    }

    fn explosion(event: ProcEvent, chance: f32) -> Proc {
        Proc {
            event,
            action: ProcAction::Explosion {
                radius: 3.0,
                damage: 25.0,
            },
            chance,
        }
    }

    /// Chance == 1.0 must always trigger; the spawned zone
    /// inherits the origin's position / team / ability id.
    #[test]
    fn always_fires_at_chance_one_and_spawns_zone() {
        let (mut zones, mut next_id, tick, origin) = ctx();
        let mut sink = ProcSink {
            aoe_zones: &mut zones,
            next_projectile_net_id: &mut next_id,
            tick,
        };
        let procs = [explosion(ProcEvent::OnHit, 1.0)];
        dispatch(ProcEvent::OnHit, &procs, &origin, &mut sink);

        assert_eq!(zones.len(), 1, "chance=1 should always spawn the zone");
        let z = &zones[0];
        assert_eq!(z.position, origin.position);
        assert_eq!(z.team, Team::Player);
        assert_eq!(z.ability_id, id::FIRE_BALL);
        assert_eq!(z.attacker_kind, 7);
        assert_eq!(z.damage_per_tick, 25.0);
        assert_eq!(z.radius, 3.0);
    }

    /// Chance == 0.0 must never trigger — short-circuit branch
    /// in `roll`.
    #[test]
    fn never_fires_at_chance_zero() {
        let (mut zones, mut next_id, tick, origin) = ctx();
        let mut sink = ProcSink {
            aoe_zones: &mut zones,
            next_projectile_net_id: &mut next_id,
            tick,
        };
        let procs = [explosion(ProcEvent::OnHit, 0.0)];
        dispatch(ProcEvent::OnHit, &procs, &origin, &mut sink);
        assert!(zones.is_empty());
    }

    /// Procs registered for one event must not fire on another.
    /// Guards the `p.event != event` filter.
    #[test]
    fn event_filter_isolates_unrelated_procs() {
        let (mut zones, mut next_id, tick, origin) = ctx();
        let mut sink = ProcSink {
            aoe_zones: &mut zones,
            next_projectile_net_id: &mut next_id,
            tick,
        };
        // Mix of OnCrit / OnDodge / OnLowHealth procs — none
        // should fire when we dispatch OnHit.
        let procs = [
            explosion(ProcEvent::OnCrit, 1.0),
            explosion(ProcEvent::OnDodge, 1.0),
            explosion(ProcEvent::OnLowHealth, 1.0),
        ];
        dispatch(ProcEvent::OnHit, &procs, &origin, &mut sink);
        assert!(
            zones.is_empty(),
            "no proc matches the dispatched event, none should fire",
        );
    }

    /// Same `(tick, salt)` must produce the same outcome on
    /// every dispatch — this is the determinism contract that
    /// keeps replays reproducible.
    #[test]
    fn deterministic_for_same_tick_and_salt() {
        let procs = [explosion(ProcEvent::OnHit, 0.5)];
        let mut firsts: Vec<usize> = Vec::new();
        for _ in 0..3 {
            let (mut zones, mut next_id, tick, origin) = ctx();
            let mut sink = ProcSink {
                aoe_zones: &mut zones,
                next_projectile_net_id: &mut next_id,
                tick,
            };
            dispatch(ProcEvent::OnHit, &procs, &origin, &mut sink);
            firsts.push(zones.len());
        }
        assert!(
            firsts.windows(2).all(|w| w[0] == w[1]),
            "identical inputs produced different proc outcomes: {firsts:?}",
        );
    }

    /// Each proc in the same dispatch gets its own `idx` mixed
    /// into the seed, so a list of identical procs at chance
    /// 0.5 doesn't all roll the same direction. Sanity check
    /// that roll-per-proc independence holds (over enough
    /// procs at chance 0.5 we expect *some* fires and *some*
    /// misses, not all-or-nothing).
    #[test]
    fn proc_index_decorrelates_rolls() {
        let (mut zones, mut next_id, tick, origin) = ctx();
        let mut sink = ProcSink {
            aoe_zones: &mut zones,
            next_projectile_net_id: &mut next_id,
            tick,
        };
        let procs = vec![explosion(ProcEvent::OnHit, 0.5); 20];
        dispatch(ProcEvent::OnHit, &procs, &origin, &mut sink);
        let fires = zones.len();
        // With 20 trials at p=0.5 deterministically, the
        // probability of getting a 0-or-20 split for a
        // well-decorrelated sequence is astronomically small.
        // Tolerate a wide band; we only care that the rolls
        // aren't perfectly correlated.
        assert!(
            fires > 0 && fires < 20,
            "expected some hit-some-miss split, got {fires}/20",
        );
    }

    /// `CastAbility` and `ChainLightning` are intentionally
    /// no-op match arms today. Hitting them must not spawn a
    /// zone or panic.
    #[test]
    fn unimplemented_actions_are_silent_noops() {
        let (mut zones, mut next_id, tick, origin) = ctx();
        let mut sink = ProcSink {
            aoe_zones: &mut zones,
            next_projectile_net_id: &mut next_id,
            tick,
        };
        let procs = [
            Proc {
                event: ProcEvent::OnHit,
                action: ProcAction::CastAbility(rift_game::abilities::FIRE_BALL),
                chance: 1.0,
            },
            Proc {
                event: ProcEvent::OnHit,
                action: ProcAction::ChainLightning {
                    max_targets: 3,
                    damage: 10.0,
                },
                chance: 1.0,
            },
        ];
        dispatch(ProcEvent::OnHit, &procs, &origin, &mut sink);
        assert!(zones.is_empty());
    }

    /// Spawned zones must consume from the shared net-id
    /// allocator so wire ids stay unique with the rest of the
    /// projectile / AoE pool.
    #[test]
    fn spawned_zone_consumes_net_id() {
        let (mut zones, mut next_id, tick, origin) = ctx();
        let start = next_id;
        let mut sink = ProcSink {
            aoe_zones: &mut zones,
            next_projectile_net_id: &mut next_id,
            tick,
        };
        let procs = [explosion(ProcEvent::OnHit, 1.0)];
        dispatch(ProcEvent::OnHit, &procs, &origin, &mut sink);
        assert_eq!(zones.len(), 1);
        assert_eq!(zones[0].owner.0, start);
        assert_eq!(next_id, start.wrapping_add(1));
    }
}

