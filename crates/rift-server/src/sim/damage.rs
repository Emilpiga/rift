//! Enemy → player damage application. Split out of the
//! main `sim/mod.rs`. Free function (not on `Sim`) so the
//! `step()` borrow split keeps working — caller passes the
//! disjoint mutable references in.

use rift_net::ids::ClientId;
use rift_net::messages::WorldEvent;
use rift_net::{NetId, NetTick};

use super::player::{self, ServerPlayer};
use super::{combat_ctx, meters, procs, projectile, GHOST_RISE_DELAY};

/// Apply queued enemy → player damage and emit one `Damage`
/// event per applied hit. Defensive stats consumed here:
/// * `armor` — flat damage reduction via
///   [`CharacterStats::armor_damage_reduction`].
/// * `physical_resist` / `elemental_resist` — element-typed
///   resist multiplier on top, gated by the source ability's
///   [`Element`].
/// * `evasion` — passive dodge roll; on success the hit is
///   cancelled outright (no damage, no meter row, no Damage
///   event), and any `OnDodge` procs fire.
/// `OnLowHealth` procs fire the tick the target's HP first
/// dips below [`player::LOW_HP_PROC_THRESHOLD`]; the latch
/// re-arms in [`ServerPlayer::tick_health_regen`].
/// Deaths transition the player into the "dead" snapshot flag
/// and queue a `(client_id, net_id)` entry for the caller to
/// broadcast as `WorldEvent::Death`.
pub(super) fn apply_player_damage(
    world: &mut hecs::World,
    events: &mut Vec<WorldEvent>,
    deaths: &mut Vec<(ClientId, NetId)>,
    meters: &mut meters::Meters,
    aoe_zones: &mut Vec<projectile::ServerAoeZone>,
    next_projectile_net_id: &mut u32,
    tick: NetTick,
    pending: Vec<combat_ctx::PlayerHit>,
) {
    use rift_game::abilities::Element;
    use rift_game::loot::ProcEvent;

    let mut idx_salt: u64 = 0;
    for hit in pending {
        idx_salt = idx_salt.wrapping_add(1);
        let combat_ctx::PlayerHit { target: player_entity, attacker_kind, ability_id, amount } = hit;

        // Resolve the source ability's element / archetype for
        // resist routing. Unknown ids (`OTHER`, environmental)
        // bypass elemental resists — they're untyped.
        let element = rift_game::abilities::lookup(ability_id)
            .map(|a| a.element)
            .unwrap_or(Element::None);

        // ── Pre-mutation reads: snapshot stats + roll evasion.
        // We need stats before the mutable borrow so the dodge
        // path can decide to skip damage entirely without
        // re-borrowing.
        let (evasion, armor_dr, resist_mult, dodge_procs, position, net_id, client_id) = {
            let Ok(p) = world.get::<&ServerPlayer>(player_entity) else { continue };
            if p.hp <= 0.0 {
                continue;
            }
            let dodge_procs: Vec<rift_game::loot::ability_mods::Proc> =
                p.ability_mods.procs.iter().copied().collect();
            (
                p.stats.evasion.clamp(0.0, 0.95),
                p.stats.armor_damage_reduction(p.level),
                p.stats.incoming_resist_mult(element),
                dodge_procs,
                p.k.position,
                p.net_id,
                p.client_id,
            )
        };

        // Evasion roll. Deterministic salt mixes tick + target
        // + attacker so the same hit never re-rolls across a
        // replay. On dodge: skip damage entirely and fire any
        // `OnDodge` procs against nearby enemies.
        if evasion > 0.0 {
            let seed = projectile::mix64(
                (tick.0 as u64)
                    ^ ((net_id.0 as u64) << 8)
                    ^ ((ability_id as u64) << 24)
                    ^ idx_salt.rotate_left(13),
            );
            let r = (seed >> 40) as f32 / (1u32 << 24) as f32;
            if r < evasion {
                let mut sink = procs::ProcSink {
                    aoe_zones,
                    next_projectile_net_id,
                    tick,
                };
                let origin = procs::ProcOrigin {
                    position,
                    attacker_kind: meters::ATTACKER_KIND_OTHER,
                    ability_id: meters::ABILITY_ID_OTHER,
                    team: projectile::Team::Player,
                    salt: idx_salt ^ 0xD0D6_E5EE,
                };
                procs::dispatch(ProcEvent::OnDodge, &dodge_procs, &origin, &mut sink);
                continue;
            }
        }

        // Defensive multiplier chain: armor → element resist.
        // Both pre-clamped in `CharacterStats`; the product is
        // safely > 0 because each factor is in [0.25, 1.0] given
        // our 0.75 caps.
        let mitigated = amount * (1.0 - armor_dr) * resist_mult;

        let (was_low_hp_armed, hp_before, hp_after, died) = {
            let Ok(mut p) = world.get::<&mut ServerPlayer>(player_entity) else { continue };
            if p.hp <= 0.0 {
                continue;
            }
            let was_alive = p.hp > 0.0;
            let before = p.hp;
            p.hp = (p.hp - mitigated).max(0.0);
            let after = p.hp;
            let died = was_alive && p.hp <= 0.0;
            if died {
                p.is_ghost = false;
                p.ghost_rise_timer = Some(GHOST_RISE_DELAY);
            }
            // Track + arm the OnLowHealth latch. Latch is only
            // tripped (set false + fire proc) on the tick HP
            // first crosses the threshold from above; re-arms
            // in `tick_health_regen` once HP rises back above
            // `LOW_HP_PROC_REARM`.
            let was_armed = p.low_hp_proc_armed;
            if was_armed
                && p.hp_max > 0.0
                && before / p.hp_max >= player::LOW_HP_PROC_THRESHOLD
                && after / p.hp_max < player::LOW_HP_PROC_THRESHOLD
            {
                p.low_hp_proc_armed = false;
            }
            (was_armed, before, after, died)
        };

        // Credit the meter row before emitting the wire event
        // so the broadcast picks up the same hit. Per-ability
        // `damage_taken` rolls into the top-line total inside
        // `add_damage_taken`.
        meters
            .entry(client_id)
            .add_damage_taken(attacker_kind, ability_id, mitigated);
        events.push(WorldEvent::Damage {
            target: net_id,
            amount: mitigated,
            crit: false,
            position: position.to_array(),
        });
        if died {
            events.push(WorldEvent::Death {
                entity: net_id,
                killer: None,
            });
            deaths.push((client_id, net_id));
        }

        // Fire OnLowHealth procs after the damage event so the
        // visual/aoe arrives on the same tick the bar dropped.
        // Skipped on the killing blow — the player's already in
        // ghost state, no point in firing a panic proc on a
        // corpse.
        if was_low_hp_armed
            && !died
            && hp_before != hp_after
        {
            let low_procs: Vec<rift_game::loot::ability_mods::Proc> = world
                .get::<&ServerPlayer>(player_entity)
                .map(|p| p.ability_mods.procs.iter().copied().collect())
                .unwrap_or_default();
            // Re-check the latch — it was set false above only
            // when the threshold was crossed this hit.
            let crossed = world
                .get::<&ServerPlayer>(player_entity)
                .map(|p| !p.low_hp_proc_armed)
                .unwrap_or(false);
            if crossed {
                let mut sink = procs::ProcSink {
                    aoe_zones,
                    next_projectile_net_id,
                    tick,
                };
                let origin = procs::ProcOrigin {
                    position,
                    attacker_kind: meters::ATTACKER_KIND_OTHER,
                    ability_id: meters::ABILITY_ID_OTHER,
                    team: projectile::Team::Player,
                    salt: idx_salt ^ 0x10AE_4ED1,
                };
                procs::dispatch(ProcEvent::OnLowHealth, &low_procs, &origin, &mut sink);
            }
        }
    }
}
