//! Wraith phase-and-scream behaviour.
//!
//! Wraiths deliberately ignore room walls for aggro and movement:
//! they drift straight toward a locked player, then stop for a
//! readable cone-scream wind-up. The scream is a frontal cone, so
//! dodging sideways or behind the wraith cleanly avoids it.

use glam::Vec3;
use hecs::Entity;
use rift_game::kinematic::{loco, Kinematic};
use rift_net::NetId;

use super::{enter_windup, tick_windup, AiOutcome, AiPhase, ServerEnemy, WindupKind};

#[derive(Clone, Copy, Debug)]
pub struct Spec {
    pub scream_range: f32,
    pub preferred_range: f32,
    pub min_range: f32,
    pub scream_half_angle: f32,
    pub scream_damage: f32,
    pub windup_dur: f32,
    pub cooldown: f32,
    pub drift_speed_mult: f32,
}

pub static SPEC: Spec = Spec {
    scream_range: 4.8,
    preferred_range: 3.7,
    min_range: 2.2,
    scream_half_angle: 0.62,
    scream_damage: 12.0,
    windup_dur: 0.48,
    cooldown: 3.2,
    drift_speed_mult: 1.18,
};

pub fn tick(
    en: &mut ServerEnemy,
    kinematic: &mut Kinematic,
    net_id: NetId,
    spec: &Spec,
    target: Option<(Entity, Vec3, f32)>,
    players: &[(Entity, Vec3)],
    speed_mult: f32,
    damage_mult: f32,
    dt: f32,
    outcome: &mut AiOutcome,
) {
    let Some((_target_entity, target_pos, d2)) = target else {
        kinematic.velocity = Vec3::ZERO;
        kinematic.locomotion = loco::IDLE;
        if matches!(
            en.ai_phase,
            AiPhase::Windup {
                kind: WindupKind::WraithScream,
                ..
            }
        ) {
            en.ai_phase = AiPhase::Idle;
        }
        return;
    };

    let to_target = Vec3::new(
        target_pos.x - kinematic.position.x,
        0.0,
        target_pos.z - kinematic.position.z,
    );
    let dir_to = to_target.normalize_or_zero();
    let facing = if dir_to.length_squared() > 1.0e-4 {
        dir_to
    } else {
        Vec3::new(kinematic.yaw.sin(), 0.0, kinematic.yaw.cos()).normalize_or_zero()
    };
    if to_target.length_squared() > 1.0e-4 {
        kinematic.yaw = to_target.x.atan2(to_target.z);
        kinematic.aim_yaw = kinematic.yaw;
    }

    if matches!(
        en.ai_phase,
        AiPhase::Windup {
            kind: WindupKind::WraithScream,
            ..
        }
    ) {
        if let Some(WindupKind::WraithScream) = tick_windup(en, kinematic, dt) {
            resolve_scream(
                en,
                kinematic.position,
                facing,
                spec,
                players,
                damage_mult,
                outcome,
            );
            outcome.casts.push(super::EnemyCast::Start {
                owner: net_id,
                ability_id: rift_game::abilities::id::WRAITH_SCREAM_IMPACT,
                origin: kinematic.position,
                target: target_pos,
                dir_x: facing.x,
                dir_y: facing.z,
            });
            en.attack_cooldown = spec.cooldown;
        }
        return;
    }

    let dist = d2.sqrt();
    if dist <= spec.scream_range && en.attack_cooldown <= 0.0 {
        enter_windup(
            en,
            kinematic,
            net_id,
            WindupKind::WraithScream,
            spec.windup_dur,
            outcome,
        );
        outcome.casts.push(super::EnemyCast::Start {
            owner: net_id,
            ability_id: rift_game::abilities::id::WRAITH_SCREAM_WINDUP,
            origin: kinematic.position,
            target: target_pos,
            dir_x: facing.x,
            dir_y: facing.z,
        });
        en.attack_anim_remaining = spec.windup_dur + 0.35;
        return;
    }

    let right = Vec3::new(facing.z, 0.0, -facing.x);
    let strafe_sign = if en.flank_slot % 2 == 0 { 1.0 } else { -1.0 };
    let strafe = right * (strafe_sign * en.speed * speed_mult * 0.35);
    kinematic.velocity = if dist < spec.min_range {
        -facing * en.speed * speed_mult * spec.drift_speed_mult + strafe * 0.35
    } else if dist < spec.preferred_range {
        strafe
    } else {
        facing * en.speed * speed_mult * spec.drift_speed_mult + strafe * 0.25
    };
    kinematic.locomotion = loco::RUN;
}

fn resolve_scream(
    en: &ServerEnemy,
    origin: Vec3,
    facing: Vec3,
    spec: &Spec,
    players: &[(Entity, Vec3)],
    damage_mult: f32,
    outcome: &mut AiOutcome,
) {
    if facing.length_squared() <= 1.0e-4 {
        return;
    }
    let cos_limit = spec.scream_half_angle.cos();
    let range2 = spec.scream_range * spec.scream_range;
    for (player, pos) in players {
        let rel = Vec3::new(pos.x - origin.x, 0.0, pos.z - origin.z);
        let d2 = rel.length_squared();
        if d2 > range2 {
            continue;
        }
        if d2 > 1.0e-4 && rel.normalize_or_zero().dot(facing) < cos_limit {
            continue;
        }
        outcome
            .melee_damage
            .push(super::super::combat_ctx::PlayerHit {
                target: *player,
                attacker_kind: en.role.to_wire_byte(),
                ability_id: rift_game::abilities::id::WRAITH_SCREAM,
                amount: spec.scream_damage * damage_mult,
            });
    }
}
