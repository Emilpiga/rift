use glam::Vec3;
use hecs::World;

use super::behavior::{BehaviorNode, Status};
use super::blackboard::{Blackboard, PendingAction};
use super::pathfinding::NavGrid;
use crate::ecs::components::{Enemy, Health, Player, Transform, Velocity};
use crate::physics::{self, Aabb, Ray};

/// AI component that pairs a behavior tree with per-entity state.
#[derive(Clone, Debug)]
pub struct AiAgent {
    pub tree: BehaviorNode,
    pub blackboard: Blackboard,
}

impl AiAgent {
    pub fn new(tree: BehaviorNode, home_pos: Vec3) -> Self {
        Self {
            tree,
            blackboard: Blackboard::new(home_pos),
        }
    }
}

/// Simple RNG for wander positions (no external dependency).
struct SimpleRng(u64);

impl SimpleRng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        // Map to -1.0..1.0
        ((self.0 >> 33) as f32 / (u32::MAX as f32 / 2.0)) - 1.0
    }
}

/// How often to recompute A* paths (seconds).
const PATH_RECOMPUTE_INTERVAL: f32 = 0.6;
/// Maximum A* search steps per entity per frame.
const MAX_PATH_STEPS: usize = 400;
/// Distance threshold to advance to next waypoint.
const WAYPOINT_REACH_DIST: f32 = 0.8;
/// Max enemies that can recompute paths per frame.
const MAX_PATHFINDS_PER_FRAME: usize = 5;
/// Enemies beyond this distance skip heavy AI (pathfinding, LOS).
const AI_ACTIVE_RANGE: f32 = 28.0;

/// Run the AI behavior system for all entities with an AiAgent component.
pub fn ai_system(world: &mut World, dt: f32, nav_grid: &NavGrid, wall_aabbs: &[Aabb]) {
    // Find player position
    let player_pos: Option<Vec3> = world
        .query::<(&Transform, &Player)>()
        .iter()
        .map(|(_, (t, _))| t.position)
        .next();

    let Some(player_pos) = player_pos else { return };

    // Use pre-cached wall AABBs for line-of-sight (no per-frame query)
    let statics = wall_aabbs;

    // Gather entity data: (entity, position, speed, health_pct)
    let entities: Vec<(hecs::Entity, Vec3, f32, f32)> = world
        .query::<(&Transform, &Enemy, &Health)>()
        .without::<&crate::ecs::components::Dying>()
        .iter()
        .map(|(e, (t, en, h))| (e, t.position, en.speed, h.current / h.max))
        .collect();

    let mut pathfinds_this_frame = 0usize;

    // Tick each AI agent
    for (entity, pos, speed, health_pct) in &entities {
        // Get agent mutably
        let Ok(mut agent) = world.get::<&mut AiAgent>(*entity) else {
            continue;
        };

        // Distance culling: enemies far from player only do minimal updates
        let dist_to_player = (*pos - player_pos).length();
        if dist_to_player > AI_ACTIVE_RANGE {
            // Just idle — no pathfinding, no LOS checks
            agent.blackboard.distance_to_target = dist_to_player;
            agent.blackboard.can_see_target = false;
            drop(agent);
            if let Ok(mut vel) = world.get::<&mut Velocity>(*entity) {
                vel.linear = Vec3::ZERO;
            }
            continue;
        }

        // Update blackboard perception
        let to_player = player_pos - *pos;
        let dist = to_player.length();
        agent.blackboard.distance_to_target = dist;
        agent.blackboard.target_pos = Some(player_pos);

        // Line-of-sight check (skip if close — must be visible)
        if dist < 3.0 {
            agent.blackboard.can_see_target = true;
        } else if dist > 0.1 {
            let (ray, ray_len) = Ray::between(*pos + Vec3::Y * 0.5, player_pos + Vec3::Y * 0.5);
            agent.blackboard.can_see_target = !physics::raycast_any(&ray, ray_len, statics);
        } else {
            agent.blackboard.can_see_target = true;
        }

        // Tick timers
        agent.blackboard.action_timer = (agent.blackboard.action_timer - dt).max(0.0);
        agent.blackboard.leap_timer = (agent.blackboard.leap_timer - dt).max(0.0);
        agent.blackboard.path_age += dt;
        if !agent.blackboard.can_see_target {
            agent.blackboard.lost_sight_timer += dt;
        } else {
            agent.blackboard.lost_sight_timer = 0.0;
        }

        // Tick the behavior tree
        let tree = agent.tree.clone();
        let mut rng = SimpleRng(entity.id() as u64 ^ (agent.blackboard.action_timer.to_bits() as u64));
        let can_pathfind = pathfinds_this_frame < MAX_PATHFINDS_PER_FRAME;

        // If currently mid-leap, override the BT and just commit to the leap.
        let result = if agent.blackboard.leap_timer > 0.0 {
            let leap_to = agent.blackboard.leap_target;
            let to_leap = leap_to - *pos;
            let dir = Vec3::new(to_leap.x, 0.0, to_leap.z).normalize_or_zero();
            // Leap is 2.5x speed; ends naturally as timer expires.
            TickResult::running(dir * speed * 2.5)
        } else {
            tick_node(
                &tree,
                &mut agent.blackboard,
                *pos,
                *speed,
                *health_pct,
                nav_grid,
                &mut rng,
                can_pathfind,
            )
        };
        if agent.blackboard.did_pathfind {
            pathfinds_this_frame += 1;
            agent.blackboard.did_pathfind = false;
        }

        drop(agent);

        // Compute separation force from nearby enemies
        let separation = compute_separation(*pos, *entity, &entities);

        // Apply the velocity result from the tree, plus local wall avoidance + separation
        let desired_vel = result.velocity;
        if let Ok(mut vel) = world.get::<&mut Velocity>(*entity) {
            if desired_vel.length_squared() > 0.01 {
                // Add wall repulsion steering to prevent getting stuck on wall edges
                let repulsion = nav_grid.wall_repulsion(*pos, 1.5);
                let steered = desired_vel + repulsion * speed * 0.8 + separation * speed;
                vel.linear = steered.normalize_or_zero() * desired_vel.length();
            } else {
                // Even when idle, apply separation to prevent stacking
                vel.linear = separation * speed * 0.5;
            }
        }
    }
}

/// Compute separation steering force to keep enemies from overlapping.
fn compute_separation(pos: Vec3, self_entity: hecs::Entity, others: &[(hecs::Entity, Vec3, f32, f32)]) -> Vec3 {
    const SEPARATION_RADIUS: f32 = 1.8;
    const SEPARATION_STRENGTH: f32 = 1.2;

    let mut force = Vec3::ZERO;
    let mut count = 0u32;

    for (entity, other_pos, _, _) in others {
        if *entity == self_entity {
            continue;
        }
        let diff = pos - *other_pos;
        let dist_sq = diff.x * diff.x + diff.z * diff.z; // ignore Y
        if dist_sq < SEPARATION_RADIUS * SEPARATION_RADIUS && dist_sq > 0.001 {
            let dist = dist_sq.sqrt();
            // Stronger push when closer (inverse proportion)
            let strength = (1.0 - dist / SEPARATION_RADIUS) * SEPARATION_STRENGTH;
            force += Vec3::new(diff.x, 0.0, diff.z).normalize_or_zero() * strength;
            count += 1;
        }
    }

    if count > 0 {
        force / count as f32
    } else {
        Vec3::ZERO
    }
}

/// Output from ticking a node (what the entity should do).
struct TickResult {
    status: Status,
    velocity: Vec3,
}

impl TickResult {
    fn success(velocity: Vec3) -> Self {
        Self { status: Status::Success, velocity }
    }
    fn failure() -> Self {
        Self { status: Status::Failure, velocity: Vec3::ZERO }
    }
    fn running(velocity: Vec3) -> Self {
        Self { status: Status::Running, velocity }
    }
}

fn tick_node(
    node: &BehaviorNode,
    bb: &mut Blackboard,
    pos: Vec3,
    speed: f32,
    health_pct: f32,
    nav: &NavGrid,
    rng: &mut SimpleRng,
    can_pathfind: bool,
) -> TickResult {
    match node {
        // ─── Composites ──────────────────────────────────────────
        BehaviorNode::Selector(children) => {
            for child in children {
                let result = tick_node(child, bb, pos, speed, health_pct, nav, rng, can_pathfind);
                match result.status {
                    Status::Success | Status::Running => return result,
                    Status::Failure => continue,
                }
            }
            TickResult::failure()
        }

        BehaviorNode::Sequence(children) => {
            let mut last_vel = Vec3::ZERO;
            for child in children {
                let result = tick_node(child, bb, pos, speed, health_pct, nav, rng, can_pathfind);
                last_vel = result.velocity;
                match result.status {
                    Status::Failure => return TickResult::failure(),
                    Status::Running => return TickResult::running(result.velocity),
                    Status::Success => continue,
                }
            }
            TickResult::success(last_vel)
        }

        // ─── Decorators ──────────────────────────────────────────
        BehaviorNode::Inverter(child) => {
            let result = tick_node(child, bb, pos, speed, health_pct, nav, rng, can_pathfind);
            match result.status {
                Status::Success => TickResult { status: Status::Failure, velocity: result.velocity },
                Status::Failure => TickResult { status: Status::Success, velocity: result.velocity },
                Status::Running => result,
            }
        }

        BehaviorNode::RepeatUntilFail(child) => {
            let result = tick_node(child, bb, pos, speed, health_pct, nav, rng, can_pathfind);
            match result.status {
                Status::Failure => TickResult::success(Vec3::ZERO),
                _ => TickResult::running(result.velocity),
            }
        }

        // ─── Conditions ──────────────────────────────────────────
        BehaviorNode::InRange(range) => {
            if bb.distance_to_target <= *range {
                TickResult::success(Vec3::ZERO)
            } else {
                TickResult::failure()
            }
        }

        BehaviorNode::CanSeeTarget => {
            if bb.can_see_target {
                TickResult::success(Vec3::ZERO)
            } else {
                TickResult::failure()
            }
        }

        BehaviorNode::CooldownReady => {
            if bb.action_timer <= 0.0 {
                TickResult::success(Vec3::ZERO)
            } else {
                TickResult::failure()
            }
        }

        BehaviorNode::HealthBelow(threshold) => {
            if health_pct < *threshold {
                TickResult::success(Vec3::ZERO)
            } else {
                TickResult::failure()
            }
        }

        // ─── Actions ─────────────────────────────────────────────
        BehaviorNode::ChaseTarget => {
            let Some(target) = bb.target_pos else {
                return TickResult::failure();
            };

            let to_target = target - pos;
            if to_target.length() < 0.5 {
                bb.path.clear();
                return TickResult::success(Vec3::ZERO);
            }

            // If we have line-of-sight, move directly (no pathfinding needed)
            if bb.can_see_target {
                bb.path.clear();
                let dir = Vec3::new(to_target.x, 0.0, to_target.z).normalize_or_zero();
                return TickResult::running(dir * speed);
            }

            // Recompute path if stale or empty (budget-limited per frame)
            if bb.path.is_empty() || bb.path_age > PATH_RECOMPUTE_INTERVAL {
                if can_pathfind {
                    if let Some(path) = nav.find_path(pos, target, MAX_PATH_STEPS) {
                        bb.path = path;
                        bb.path_age = 0.0;
                        bb.did_pathfind = true;
                    } else {
                        bb.did_pathfind = true;
                        // No path — try moving toward last known direction
                        let dir = Vec3::new(to_target.x, 0.0, to_target.z).normalize_or_zero();
                        return TickResult::running(dir * speed * 0.3);
                    }
                } else {
                    // Budget exhausted — use existing path or move directly
                    if bb.path.is_empty() {
                        let dir = Vec3::new(to_target.x, 0.0, to_target.z).normalize_or_zero();
                        return TickResult::running(dir * speed * 0.5);
                    }
                }
            }

            // Follow path: move toward first waypoint, pop when reached
            follow_path(bb, pos, speed)
        }

        BehaviorNode::ReturnHome => {
            let to_home = bb.home_pos - pos;
            if to_home.length() < 1.0 {
                bb.path.clear();
                return TickResult::success(Vec3::ZERO);
            }

            // Recompute path home if stale (uses pathfind budget)
            if bb.path.is_empty() || bb.path_age > PATH_RECOMPUTE_INTERVAL * 2.0 {
                if can_pathfind {
                    if let Some(path) = nav.find_path(pos, bb.home_pos, MAX_PATH_STEPS) {
                        bb.path = path;
                        bb.path_age = 0.0;
                        bb.did_pathfind = true;
                    }
                }
            }

            if !bb.path.is_empty() {
                follow_path(bb, pos, speed * 0.6)
            } else {
                let dir = Vec3::new(to_home.x, 0.0, to_home.z).normalize_or_zero();
                TickResult::running(dir * speed * 0.6)
            }
        }

        BehaviorNode::Wander { radius } => {
            // Pick a new wander target if we don't have one or reached it
            let need_new = match bb.wander_target {
                Some(wt) => (wt - pos).length() < 1.0,
                None => true,
            };

            if need_new {
                // Pick a random walkable position near home
                let mut attempts = 0;
                loop {
                    let offset = Vec3::new(rng.next() * radius, 0.0, rng.next() * radius);
                    let candidate = bb.home_pos + offset;
                    let (gx, gz) = nav.world_to_grid(candidate);
                    if nav.is_walkable(gx, gz) || attempts > 5 {
                        bb.wander_target = Some(candidate);
                        bb.path.clear(); // force recompute
                        break;
                    }
                    attempts += 1;
                }
            }

            if let Some(target) = bb.wander_target {
                // Use path to wander target
                if bb.path.is_empty() || bb.path_age > 1.0 {
                    if can_pathfind {
                        if let Some(path) = nav.find_path(pos, target, 200) {
                            bb.path = path;
                            bb.path_age = 0.0;
                            bb.did_pathfind = true;
                        }
                    }
                }

                if !bb.path.is_empty() {
                    follow_path(bb, pos, speed * 0.4)
                } else {
                    let dir = Vec3::new(target.x - pos.x, 0.0, target.z - pos.z).normalize_or_zero();
                    TickResult::running(dir * speed * 0.4)
                }
            } else {
                TickResult::success(Vec3::ZERO)
            }
        }

        BehaviorNode::Idle => {
            TickResult::success(Vec3::ZERO)
        }

        BehaviorNode::Charge { speed_mult } => {
            if let Some(target) = bb.target_pos {
                let to_target = target - pos;
                let dir = Vec3::new(to_target.x, 0.0, to_target.z).normalize_or_zero();
                bb.attack_count += 1;
                TickResult::success(dir * speed * speed_mult)
            } else {
                TickResult::failure()
            }
        }

        BehaviorNode::CircleStrafe { radius, direction } => {
            if let Some(target) = bb.target_pos {
                let to_entity = pos - target;
                let current_dist = to_entity.length();

                // Calculate tangent direction for circling
                let tangent = Vec3::new(-to_entity.z, 0.0, to_entity.x).normalize_or_zero() * *direction;

                // Also adjust radial distance toward desired radius
                let radial = if current_dist > *radius + 0.5 {
                    (target - pos).normalize_or_zero() * 0.5
                } else if current_dist < *radius - 0.5 {
                    (pos - target).normalize_or_zero() * 0.5
                } else {
                    Vec3::ZERO
                };

                let combined = (tangent + radial).normalize_or_zero();
                TickResult::running(combined * speed * 0.8)
            } else {
                TickResult::failure()
            }
        }

        BehaviorNode::SetCooldown(duration) => {
            bb.action_timer = *duration;
            TickResult::success(Vec3::ZERO)
        }

        BehaviorNode::WaitCooldown => {
            if bb.action_timer <= 0.0 {
                TickResult::success(Vec3::ZERO)
            } else {
                TickResult::running(Vec3::ZERO)
            }
        }

        // ─── Smart actions ────────────────────────────────────────
        BehaviorNode::Backpedal { speed_mult } => {
            let Some(target) = bb.target_pos else { return TickResult::failure(); };
            let away = pos - target;
            let dir = Vec3::new(away.x, 0.0, away.z).normalize_or_zero();
            if dir == Vec3::ZERO {
                return TickResult::success(Vec3::ZERO);
            }
            TickResult::running(dir * speed * *speed_mult)
        }

        BehaviorNode::KeepDistance { min, ideal, max } => {
            let Some(target) = bb.target_pos else { return TickResult::failure(); };
            let dist = bb.distance_to_target;
            let to_target = target - pos;
            let to_target_xz = Vec3::new(to_target.x, 0.0, to_target.z);

            // Pick a stable strafe direction once.
            if bb.strafe_dir.abs() < 0.5 {
                bb.strafe_dir = if (rng.next() as f32) > 0.0 { 1.0 } else { -1.0 };
            }

            if dist < *min {
                // Too close: backpedal hard, slight strafe to break line.
                let away = -to_target_xz.normalize_or_zero();
                let tangent = Vec3::new(-to_target_xz.z, 0.0, to_target_xz.x).normalize_or_zero() * bb.strafe_dir;
                let v = (away * 0.85 + tangent * 0.4).normalize_or_zero();
                TickResult::running(v * speed * 1.1)
            } else if dist > *max {
                // Too far: approach.
                let dir = to_target_xz.normalize_or_zero();
                TickResult::running(dir * speed * 0.9)
            } else {
                // In the band: strafe around the target, drift toward `ideal`.
                let tangent = Vec3::new(-to_target_xz.z, 0.0, to_target_xz.x).normalize_or_zero() * bb.strafe_dir;
                let radial = if dist > *ideal + 0.4 {
                    to_target_xz.normalize_or_zero() * 0.4
                } else if dist < *ideal - 0.4 {
                    -to_target_xz.normalize_or_zero() * 0.4
                } else {
                    Vec3::ZERO
                };
                let v = (tangent + radial).normalize_or_zero();
                TickResult::running(v * speed * 0.7)
            }
        }

        BehaviorNode::RangedAttack { damage } => {
            let Some(target) = bb.target_pos else { return TickResult::failure(); };
            // Need LoS and not currently in a leap.
            if !bb.can_see_target {
                return TickResult::failure();
            }
            // Spawn slightly above the agent so the bolt travels at chest height.
            let origin = pos + Vec3::Y * 0.6;
            let aim = target + Vec3::Y * 0.5;
            bb.pending_action = Some(PendingAction::RangedShot { origin, target: aim, damage: *damage });
            bb.attack_count += 1;
            TickResult::success(Vec3::ZERO)
        }

        BehaviorNode::LeapStrike { distance, duration } => {
            let Some(target) = bb.target_pos else { return TickResult::failure(); };
            let to_target = target - pos;
            let dir = Vec3::new(to_target.x, 0.0, to_target.z).normalize_or_zero();
            if dir == Vec3::ZERO {
                return TickResult::failure();
            }
            // Leap toward a point near the player, capped at `distance`.
            let leap_dist = to_target.length().min(*distance);
            let leap_to = pos + dir * leap_dist;
            bb.leap_target = leap_to;
            bb.leap_timer = *duration;
            bb.pending_action = Some(PendingAction::LeapStart { target: leap_to });
            bb.attack_count += 1;
            TickResult::success(dir * speed * 2.5)
        }

        BehaviorNode::SlamTelegraph { radius, damage, delay } => {
            // Stop, telegraph an AoE at our feet (or just ahead toward target).
            let center = if let Some(t) = bb.target_pos {
                let to_t = t - pos;
                let to_t_xz = Vec3::new(to_t.x, 0.0, to_t.z);
                pos + to_t_xz.normalize_or_zero() * (radius * 0.5)
            } else {
                pos
            };
            bb.pending_action = Some(PendingAction::SlamTelegraph {
                center,
                radius: *radius,
                damage: *damage,
                delay: *delay,
            });
            bb.attack_count += 1;
            TickResult::success(Vec3::ZERO)
        }
    }
}

/// Follow the current path in the blackboard: move toward the first waypoint,
/// pop it when reached, return Running velocity.
fn follow_path(bb: &mut Blackboard, pos: Vec3, speed: f32) -> TickResult {
    while !bb.path.is_empty() {
        let wp = bb.path[0];
        let to_wp = Vec3::new(wp.x - pos.x, 0.0, wp.z - pos.z);
        if to_wp.length() < WAYPOINT_REACH_DIST {
            bb.path.remove(0);
        } else {
            let dir = to_wp.normalize_or_zero();
            return TickResult::running(dir * speed);
        }
    }
    // Path exhausted
    TickResult::success(Vec3::ZERO)
}
