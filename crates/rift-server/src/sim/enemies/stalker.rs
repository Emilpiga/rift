//! Stalker dash-attack behaviour.
//!
//! Pattern: approach until inside [`Spec::trigger_range`], wind
//! up briefly, dash through the target, then drift backward
//! during a recovery window. The dash applies a one-shot melee
//! hit if the stalker passes inside [`Spec::attack_range_for_hit`]
//! of its target during the dash window.

use glam::Vec3;
use hecs::Entity;
use rift_game::kinematic::loco;

use super::{
    enter_windup, tick_windup, AiOutcome, AiPhase, ServerEnemy, WindupKind,
};

// ---- Spec -----------------------------------------------------

/// Stalker dash-attack tuning.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    /// Distance at which the stalker stops approaching and
    /// starts its dash wind-up. Tuned so the dash reliably
    /// overshoots the player even after they shuffle a bit
    /// during the wind-up — `dash_speed_mult * base_speed *
    /// dash_dur` must be comfortably greater than this.
    pub trigger_range: f32,
    /// Telegraph crouch before the dash (s). Player has this
    /// long to dodge-roll out of the dash line.
    pub windup_dur: f32,
    /// How long the dash itself lasts (s). The product of this
    /// with `dash_speed_mult` and base speed sets the total
    /// dash distance.
    pub dash_dur: f32,
    /// Speed multiplier applied to base `enemy.speed` during
    /// the dash.
    pub dash_speed_mult: f32,
    /// Damage applied if the stalker passes within
    /// `attack_range_for_hit` of its target during the dash
    /// window.
    pub dash_damage: f32,
    /// Distance threshold at which the dash registers a hit on
    /// the target. Independent of `trigger_range` so we can
    /// tune contact tightness separately.
    pub attack_range_for_hit: f32,
    /// Recovery period after the dash ends — stalker drifts
    /// backward at fractional speed and can't re-trigger.
    pub recover_dur: f32,
    /// Multiplier on `enemy.speed` during recovery (sign flips
    /// at apply time so the drift is *away* from the target).
    pub recover_speed_mult: f32,
}

pub static SPEC: Spec = Spec {
    trigger_range: 3.5,
    windup_dur: 0.35,
    dash_dur: 0.55,
    dash_speed_mult: 4.5,
    dash_damage: 12.0,
    attack_range_for_hit: 1.6,
    recover_dur: 1.1,
    recover_speed_mult: 0.8,
};

// ---- Tick -----------------------------------------------------

pub fn tick(
    en: &mut ServerEnemy,
    spec: &Spec,
    target: Option<(Entity, Vec3, f32)>,
    speed_mult: f32,
    damage_mult: f32,
    dt: f32,
    outcome: &mut AiOutcome,
) {
    let Some((target_entity, target_pos, d2)) = target else {
        en.k.velocity = Vec3::ZERO;
        en.k.locomotion = loco::IDLE;
        en.ai_phase = AiPhase::StalkerApproach;
        return;
    };
    let dist = d2.sqrt();
    let to_target = Vec3::new(
        target_pos.x - en.k.position.x,
        0.0,
        target_pos.z - en.k.position.z,
    );
    // Faces the target unless we're mid-dash with a locked dir.
    if to_target.length_squared() > 1.0e-4 {
        en.k.yaw = to_target.x.atan2(to_target.z);
        en.k.aim_yaw = en.k.yaw;
    }

    // Promote `Idle` to `Approach` so brand-new stalkers don't
    // wedge in their initial Idle state.
    if matches!(en.ai_phase, AiPhase::Idle) {
        en.ai_phase = AiPhase::StalkerApproach;
    }

    // Mid-windup: tick the central timer. On expiry snapshot
    // the dash direction *now* so the player can side-step the
    // lunge after the telegraph.
    if matches!(
        en.ai_phase,
        AiPhase::Windup { kind: WindupKind::StalkerDash, .. }
    ) {
        if let Some(WindupKind::StalkerDash) = tick_windup(en, dt) {
            let dir = to_target.normalize_or_zero();
            en.ai_phase = AiPhase::StalkerDash {
                remaining: spec.dash_dur,
                dir,
                hit_landed: false,
            };
        }
        return;
    }

    match en.ai_phase {
        AiPhase::StalkerApproach => {
            if dist <= spec.trigger_range {
                // Lock in the approach by entering the wind-up.
                // Pad attack_anim_remaining post-windup so the
                // attack clip carries through the whole
                // windup+dash window.
                enter_windup(en, WindupKind::StalkerDash, spec.windup_dur, outcome);
                en.attack_anim_remaining = spec.windup_dur + spec.dash_dur;
                return;
            }
            let dir = to_target.normalize_or_zero();
            en.k.velocity = dir * en.speed * speed_mult;
            en.k.locomotion = loco::RUN;
        }
        AiPhase::StalkerDash {
            remaining,
            dir,
            hit_landed,
        } => {
            let next = remaining - dt;
            // Dash drives motion regardless of player range. The
            // dash velocity is uniform — no separation easing,
            // no slowdown near target — so the lunge feels
            // committal.
            en.k.velocity = dir * en.speed * spec.dash_speed_mult * speed_mult;
            en.k.locomotion = loco::RUN;
            // One-shot damage: applied the first frame the
            // dash crosses inside `attack_range_for_hit` of
            // the target.
            let mut landed = hit_landed;
            if !landed && dist <= spec.attack_range_for_hit {
                outcome
                    .melee_damage
                    .push((target_entity, spec.dash_damage * damage_mult));
                landed = true;
            }
            if next <= 0.0 {
                en.ai_phase = AiPhase::StalkerRecover(spec.recover_dur);
            } else {
                en.ai_phase = AiPhase::StalkerDash {
                    remaining: next,
                    dir,
                    hit_landed: landed,
                };
            }
        }
        AiPhase::StalkerRecover(t) => {
            let next = t - dt;
            // Drift backward at a fraction of base speed so the
            // stalker reads as winded after the lunge.
            let away = -to_target.normalize_or_zero();
            en.k.velocity = away * en.speed * spec.recover_speed_mult * speed_mult;
            en.k.locomotion = loco::RUN;
            if next <= 0.0 {
                en.ai_phase = AiPhase::StalkerApproach;
            } else {
                en.ai_phase = AiPhase::StalkerRecover(next);
            }
        }
        // Other-role wind-up phases shouldn't occur on a
        // stalker; if they ever do (component shuffle, save
        // load), reset to a clean Approach.
        AiPhase::Windup { .. } | AiPhase::Idle => {
            en.ai_phase = AiPhase::StalkerApproach;
        }
    }
}
