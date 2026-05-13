//! Stalker dash-attack behaviour.
//!
//! Pattern: approach until inside [`Spec::trigger_range`], wind
//! up briefly, dash through the target, then drift backward
//! during a recovery window. The dash applies a one-shot melee
//! hit if the stalker passes inside [`Spec::attack_range_for_hit`]
//! of its target during the dash window.

use glam::Vec3;
use hecs::Entity;
use rift_dungeon::Floor;
use rift_game::kinematic::{loco, Kinematic};
use rift_net::NetId;

use super::{brute, enter_windup, tick_windup, AiOutcome, AiPhase, ServerEnemy, WindupKind};

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
    let Some((target_entity, target_pos, d2)) = target else {
        kinematic.velocity = Vec3::ZERO;
        kinematic.locomotion = loco::IDLE;
        en.ai_phase = AiPhase::StalkerApproach;
        return;
    };
    let dist = d2.sqrt();
    let to_target = Vec3::new(
        target_pos.x - kinematic.position.x,
        0.0,
        target_pos.z - kinematic.position.z,
    );
    // Faces the target unless we're mid-dash with a locked dir.
    if to_target.length_squared() > 1.0e-4 {
        kinematic.yaw = to_target.x.atan2(to_target.z);
        kinematic.aim_yaw = kinematic.yaw;
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
        AiPhase::Windup {
            kind: WindupKind::StalkerDash,
            ..
        }
    ) {
        if let Some(WindupKind::StalkerDash) = tick_windup(en, kinematic, dt) {
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
            // LOS check: if a wall sits between us and the
            // target, follow an A* path around it instead of
            // bee-lining into the geometry. Also gate the
            // dash commit on LOS — dashing into a wall is
            // useless and looks broken.
            let los_blocked = super::cached_los_blocked(en, kinematic, floor, target_pos);
            if !los_blocked && dist <= spec.trigger_range {
                // Lock in the approach by entering the wind-up.
                // Pad attack_anim_remaining post-windup so the
                // attack clip carries through the whole
                // windup+dash window.
                en.path.clear();
                en.path_target_tile = None;
                enter_windup(
                    en,
                    kinematic,
                    net_id,
                    WindupKind::StalkerDash,
                    spec.windup_dur,
                    outcome,
                );
                en.attack_anim_remaining = spec.windup_dur + spec.dash_dur;
                return;
            }
            let dir = if los_blocked {
                // Wall in the way — consume / rebuild a cached
                // A* path toward the target tile and steer
                // toward the next waypoint. Same shape as
                // `brute::tick`'s pathing branch.
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
                if let Some(&(wx, wz)) = en.path.first() {
                    Vec3::new(
                        wx as f32 - kinematic.position.x,
                        0.0,
                        wz as f32 - kinematic.position.z,
                    )
                    .normalize_or_zero()
                } else {
                    to_target.normalize_or_zero()
                }
            } else {
                // Clear LOS but still outside trigger range —
                // bee-line and drop any stale waypoints.
                en.path.clear();
                en.path_target_tile = None;
                to_target.normalize_or_zero()
            };
            kinematic.velocity = dir * en.speed * speed_mult;
            kinematic.locomotion = loco::RUN;
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
            kinematic.velocity = dir * en.speed * spec.dash_speed_mult * speed_mult;
            kinematic.locomotion = loco::RUN;
            // One-shot damage: applied the first frame the
            // dash crosses inside `attack_range_for_hit` of
            // the target.
            let mut landed = hit_landed;
            if !landed && dist <= spec.attack_range_for_hit {
                outcome
                    .melee_damage
                    .push(super::super::combat_ctx::PlayerHit {
                        target: target_entity,
                        attacker_kind: en.role.to_wire_byte(),
                        ability_id: rift_game::abilities::id::MELEE_ATTACK,
                        amount: spec.dash_damage * damage_mult,
                    });
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
            kinematic.velocity = away * en.speed * spec.recover_speed_mult * speed_mult;
            kinematic.locomotion = loco::RUN;
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
