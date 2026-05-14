//! Mindbinder zone-control behaviour.
//!
//! Mindbinders kite like a slower caster, but their threat is a
//! placed void sigil: a ground telegraph under the target that
//! resolves after a short wind-up through the shared delayed-AoE
//! ability path.

use glam::Vec3;
use hecs::Entity;
use rift_dungeon::Floor;
use rift_game::abilities::{lookup, AbilityKind, AbilityWireId};
use rift_game::kinematic::{loco, Kinematic};
use rift_net::NetId;

use super::{brute, AiOutcome, AiPhase, EnemyCast, ServerEnemy};

#[derive(Clone, Copy, Debug)]
pub struct Spec {
    pub ability_id: AbilityWireId,
    pub min_range: f32,
    pub max_range: f32,
    pub kite_distance: f32,
    pub strafe_frac: f32,
    pub focus_turn_rate: f32,
    pub velocity_response: f32,
}

pub static SPEC: Spec = Spec {
    ability_id: rift_game::abilities::id::VOID_SIGIL,
    min_range: 5.0,
    max_range: 11.0,
    kite_distance: 8.5,
    strafe_frac: 0.42,
    focus_turn_rate: 9.0,
    velocity_response: 7.5,
};

fn turn_towards(current: f32, target: f32, rate: f32, dt: f32) -> f32 {
    let mut delta = target - current;
    while delta > std::f32::consts::PI {
        delta -= std::f32::consts::TAU;
    }
    while delta < -std::f32::consts::PI {
        delta += std::f32::consts::TAU;
    }
    let alpha = 1.0 - (-rate * dt).exp();
    let mut yaw = current + delta * alpha;
    if yaw > std::f32::consts::PI {
        yaw -= std::f32::consts::TAU;
    }
    if yaw < -std::f32::consts::PI {
        yaw += std::f32::consts::TAU;
    }
    yaw
}

fn focus_target(kinematic: &mut Kinematic, target_pos: Vec3, rate: f32, dt: f32) -> Vec3 {
    let to_target = Vec3::new(
        target_pos.x - kinematic.position.x,
        0.0,
        target_pos.z - kinematic.position.z,
    );
    if to_target.length_squared() > 1.0e-4 {
        let target_yaw = to_target.x.atan2(to_target.z);
        kinematic.aim_yaw = target_yaw;
        kinematic.yaw = turn_towards(kinematic.yaw, target_yaw, rate, dt);
    }
    to_target
}

fn set_drift_velocity(kinematic: &mut Kinematic, desired: Vec3, response: f32, dt: f32) {
    let alpha = 1.0 - (-response * dt).exp();
    kinematic.velocity = kinematic.velocity.lerp(desired, alpha);
    if kinematic.velocity.length_squared() < 0.01 {
        kinematic.velocity = Vec3::ZERO;
    }
    kinematic.locomotion = loco::IDLE;
}

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
    let sigil = lookup(spec.ability_id).expect("REGISTRY missing VOID_SIGIL");
    let (radius, windup) = match sigil.kind {
        AbilityKind::DelayedAoe { radius, windup } => (radius, windup),
        _ => {
            debug_assert!(false, "mindbinder ability must be DelayedAoe");
            (2.5, 0.9)
        }
    };

    let target_pos_for_focus = target.map(|(_, target_pos, _)| target_pos);
    if let AiPhase::MindbinderSigil {
        remaining,
        centre,
        radius,
    } = en.ai_phase
    {
        if let Some(target_pos) = target_pos_for_focus {
            focus_target(kinematic, target_pos, spec.focus_turn_rate, dt);
        }
        set_drift_velocity(kinematic, Vec3::ZERO, spec.velocity_response, dt);
        kinematic.locomotion = loco::IDLE;
        let next = remaining - dt;
        if next <= 0.0 {
            outcome.casts.push(EnemyCast::Resolve {
                owner: net_id,
                attacker_kind: en.role.to_wire_byte(),
                ability_id: spec.ability_id,
                origin: centre,
                aim: Vec3::ZERO,
                damage_mult,
                crit_chance: en.crit_chance,
                crit_damage: en.crit_damage,
                param_a: radius,
            });
            en.attack_cooldown = sigil.cooldown;
            en.ai_phase = AiPhase::Idle;
        } else {
            en.ai_phase = AiPhase::MindbinderSigil {
                remaining: next,
                centre,
                radius,
            };
        }
        return;
    }

    let Some((_target_entity, target_pos, d2)) = target else {
        set_drift_velocity(kinematic, Vec3::ZERO, spec.velocity_response, dt);
        kinematic.locomotion = loco::IDLE;
        en.ai_phase = AiPhase::Idle;
        return;
    };

    let dist = d2.sqrt();
    let to_target = focus_target(kinematic, target_pos, spec.focus_turn_rate, dt);
    let dir_to = to_target.normalize_or_zero();

    let los_blocked = super::cached_los_blocked(en, kinematic, floor, target_pos);
    if los_blocked {
        let target_tile = brute::world_to_tile(target_pos);
        let need_recompute = en.path.is_empty()
            || en.path_target_tile != Some(target_tile)
            || en.path_recompute_in <= 0.0;
        if need_recompute {
            let from = brute::world_to_tile(kinematic.position);
            en.path = floor.path(from, target_tile, 1024).unwrap_or_default();
            en.path_target_tile = Some(target_tile);
            en.path_recompute_in = brute::PATH_RECOMPUTE_INTERVAL;
        }
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
            dir_to
        };
        set_drift_velocity(
            kinematic,
            approach * en.speed * speed_mult,
            spec.velocity_response,
            dt,
        );
        return;
    }

    en.path.clear();
    en.path_target_tile = None;

    let right = Vec3::new(dir_to.z, 0.0, -dir_to.x);
    let strafe_sign = if en.flank_slot % 2 == 0 { 1.0 } else { -1.0 };
    let strafe = right * (strafe_sign * en.speed * speed_mult * spec.strafe_frac);
    if dist > spec.max_range {
        set_drift_velocity(
            kinematic,
            dir_to * en.speed * speed_mult + strafe * 0.5,
            spec.velocity_response,
            dt,
        );
    } else if dist < spec.min_range {
        set_drift_velocity(
            kinematic,
            -dir_to * en.speed * speed_mult * 0.82 + strafe * 0.65,
            spec.velocity_response,
            dt,
        );
    } else {
        let drift = (dist - spec.kite_distance) * 0.18;
        set_drift_velocity(
            kinematic,
            dir_to * drift * speed_mult + strafe,
            spec.velocity_response,
            dt,
        );
    }

    if en.attack_cooldown <= 0.0 && dist <= spec.max_range {
        outcome.casts.push(EnemyCast::Start {
            owner: net_id,
            ability_id: rift_game::abilities::id::VOID_SIGIL_WINDUP,
            origin: kinematic.position,
            target: target_pos,
            dir_x: radius,
            dir_y: windup,
        });
        en.attack_anim_remaining = windup + 0.35;
        en.ai_phase = AiPhase::MindbinderSigil {
            remaining: windup,
            centre: target_pos,
            radius,
        };
    }
}
