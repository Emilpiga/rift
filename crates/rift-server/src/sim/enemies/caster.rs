//! Caster kite-and-bolt behaviour.
//!
//! Pattern: stay inside a ring around the target — back away
//! when too close, advance when too far — and fire bolts on
//! cooldown. Two LOS-related touches over the basic kite:
//!
//! * **LOS gate on cast** — the caster won't *enter* the
//!   wind-up phase unless [`Floor::line_of_sight`] is clear to
//!   the target. Without this, bolts would fire into walls
//!   and feel like the caster was just stuck shooting
//!   nothing.
//! * **Lateral strafe** — while inside the kite ring the
//!   caster orbits sideways at [`Spec::strafe_frac`] of base
//!   speed, with the direction picked from `flank_slot` so a
//!   pack of casters orbits in alternating arcs.
//!
//! All combat tuning (bolt damage / speed / TTL / cooldown /
//! wind-up) is read from the shared
//! [`rift_game::abilities::REGISTRY`] entry for
//! [`Spec::ability_id`].

use glam::Vec3;
use hecs::Entity;
use rift_dungeon::Floor;
use rift_game::abilities::AbilityWireId;
use rift_game::kinematic::{loco, Kinematic};
use rift_net::NetId;

use super::{enter_windup, tick_windup, AiOutcome, AiPhase, EnemyCast, ServerEnemy, WindupKind};

// ---- Spec -----------------------------------------------------

/// Caster kite-and-bolt tuning.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    /// Wire id of the projectile ability the caster fires (see
    /// [`rift_game::abilities::id`]). The bolt's damage,
    /// speed, lifetime, cooldown, and wind-up are all read
    /// from the shared registry — only the *which ability* is
    /// per-spec.
    pub ability_id: AbilityWireId,
    /// Below this distance the caster backs off.
    pub min_range: f32,
    /// Above this distance the caster advances. Bolts won't
    /// be fired beyond this either.
    pub max_range: f32,
    /// Preferred orbit distance inside the kite ring.
    pub kite_distance: f32,
    /// Lateral strafe speed as a fraction of base. `0.35` is
    /// enough to read as deliberate without overshooting the
    /// kite ring during the orbit.
    pub strafe_frac: f32,
}

pub static SPEC: Spec = Spec {
    ability_id: rift_game::abilities::id::ARCANE_BOLT,
    min_range: 6.0,
    max_range: 14.0,
    kite_distance: 11.0,
    strafe_frac: 0.35,
};

// ---- Tick -----------------------------------------------------

pub fn tick(
    en: &mut ServerEnemy,
    kinematic: &mut Kinematic,
    net_id: NetId,
    spec: &Spec,
    floor: &Floor,
    target: Option<(Entity, Vec3, f32)>,
    speed_mult: f32,
    damage_mult: f32,
    dt: f32,
    outcome: &mut AiOutcome,
) {
    use rift_game::abilities::{lookup, AbilityKind};
    let bolt = lookup(spec.ability_id).expect("REGISTRY missing caster ability");
    let bolt_windup = match bolt.kind {
        AbilityKind::EnemyProjectiles { windup, .. } => windup,
        // Registry mis-authoring: keep the AI alive with a sane
        // default so a bad edit doesn't soft-lock the boss
        // fight. Asserts in debug builds so it's caught before
        // shipping.
        _ => {
            debug_assert!(false, "caster ability must be EnemyProjectiles");
            0.55
        }
    };
    let bolt_cooldown = bolt.cooldown;

    let Some((_target_entity, target_pos, d2)) = target else {
        kinematic.velocity = Vec3::ZERO;
        kinematic.locomotion = loco::IDLE;
        en.ai_phase = AiPhase::Idle;
        return;
    };
    let dist = d2.sqrt();
    let to_target = Vec3::new(
        target_pos.x - kinematic.position.x,
        0.0,
        target_pos.z - kinematic.position.z,
    );
    let dir_to = to_target.normalize_or_zero();
    if to_target.length_squared() > 1.0e-4 {
        kinematic.yaw = to_target.x.atan2(to_target.z);
        kinematic.aim_yaw = kinematic.yaw;
    }

    // Mid-windup: tick the central timer. On expiry fire the
    // bolt. Direction is freshly recomputed at fire time so
    // very-late side-steps still get tracked.
    if matches!(
        en.ai_phase,
        AiPhase::Windup {
            kind: WindupKind::CasterBolt,
            ..
        }
    ) {
        if let Some(WindupKind::CasterBolt) = tick_windup(en, kinematic, dt) {
            outcome.casts.push(EnemyCast::Resolve {
                owner: net_id,
                attacker_kind: en.role.to_wire_byte(),
                ability_id: spec.ability_id,
                origin: kinematic.position,
                aim: dir_to,
                damage_mult,
                crit_chance: en.crit_chance,
                crit_damage: en.crit_damage,
                param_a: 0.0,
            });
            en.attack_cooldown = bolt_cooldown;
        }
        return;
    }

    let los_blocked = super::cached_los_blocked(en, kinematic, floor, target_pos);

    if los_blocked {
        // Wall in the way — the kite ring is meaningless here
        // (a caster strafing sideways behind a pillar never
        // finds an angle), so override with an A* path toward
        // the target. The path naturally routes around the
        // obstacle; the caster keeps walking until LOS pops
        // clear, at which point the next tick falls back to
        // the kite branch and starts casting.
        let target_tile = super::brute::world_to_tile(target_pos);
        let need_recompute = en.path.is_empty()
            || en.path_target_tile != Some(target_tile)
            || en.path_recompute_in <= 0.0;
        if need_recompute {
            let from = super::brute::world_to_tile(kinematic.position);
            en.path = floor.path(from, target_tile, 1024).unwrap_or_default();
            en.path_target_tile = Some(target_tile);
            en.path_recompute_in = super::brute::PATH_RECOMPUTE_INTERVAL;
        }
        // Drop already-reached waypoints (within half a tile of
        // the caster's centre).
        while let Some(&(wx, wz)) = en.path.first() {
            let dx = wx as f32 - kinematic.position.x;
            let dz = wz as f32 - kinematic.position.z;
            if dx * dx + dz * dz < 0.25 {
                en.path.remove(0);
            } else {
                break;
            }
        }
        let approach = if let Some(&(wx, wz)) = en.path.first() {
            Vec3::new(
                wx as f32 - kinematic.position.x,
                0.0,
                wz as f32 - kinematic.position.z,
            )
            .normalize_or_zero()
        } else {
            // Path failed (or hasn't built yet) — just bee-line
            // toward the target as a fallback so we still try
            // to escape the wall corner.
            dir_to
        };
        kinematic.velocity = approach * en.speed * speed_mult;
        kinematic.locomotion = loco::RUN;
        // Don't attempt to cast — LOS is blocked. Re-evaluate
        // next tick once we've moved.
        return;
    }

    // LOS clear — drop any cached waypoints and fall back to
    // the standard kite ring.
    en.path.clear();
    en.path_target_tile = None;

    // Per-caster lateral strafe direction. Slot 0/2/4/6 strafe
    // right, slot 1/3/5/7 strafe left, so a pack of casters
    // orbits the player in alternating arcs instead of all
    // shuffling the same way.
    let right = Vec3::new(dir_to.z, 0.0, -dir_to.x);
    let strafe_sign = if en.flank_slot % 2 == 0 { 1.0 } else { -1.0 };
    let strafe = right * (strafe_sign * en.speed * speed_mult * spec.strafe_frac);

    // Distance-based kiting movement.
    if dist > spec.max_range {
        kinematic.velocity = dir_to * en.speed * speed_mult + strafe * 0.5;
        kinematic.locomotion = loco::RUN;
    } else if dist < spec.min_range {
        kinematic.velocity = -dir_to * en.speed * speed_mult + strafe * 0.5;
        kinematic.locomotion = loco::RUN;
    } else {
        // In the kite ring — strafe sideways while drifting
        // gently toward `kite_distance`. The strafe component
        // is what reads as "actively positioning"; the drift
        // keeps the caster from drifting out of the ring.
        let drift = (dist - spec.kite_distance) * 0.3;
        kinematic.velocity = dir_to * drift * speed_mult + strafe;
        kinematic.locomotion = loco::RUN;
    }

    // LOS-gated cast: only commit to the wind-up if the bolt
    // would actually have a clear flight path.
    if en.attack_cooldown <= 0.0 && dist <= spec.max_range {
        enter_windup(
            en,
            kinematic,
            net_id,
            WindupKind::CasterBolt,
            bolt_windup,
            outcome,
        );
    }
}
