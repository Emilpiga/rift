use glam::Vec3;

/// An action requested by the AI tick that the game layer must process
/// (e.g., spawn a projectile, perform a leap, telegraph an AoE).
#[derive(Clone, Copy, Debug)]
pub enum PendingAction {
    /// Fire a ranged bolt toward `target` with `damage`.
    RangedShot { origin: Vec3, target: Vec3, damage: f32 },
    /// Begin a leap toward `target`. The AI will commit movement; the game layer
    /// just spawns FX. `arrival_time` is when the impact damage triggers.
    LeapStart { target: Vec3 },
    /// AoE slam telegraph: the game layer should display a warning circle at
    /// `center` of `radius` and apply `damage` after `delay` seconds.
    SlamTelegraph { center: Vec3, radius: f32, damage: f32, delay: f32 },
}

/// Per-entity runtime state for AI decisions.
#[derive(Clone, Debug)]
pub struct Blackboard {
    /// Last known player position.
    pub target_pos: Option<Vec3>,
    /// Current wander destination.
    pub wander_target: Option<Vec3>,
    /// Home position (spawn point).
    pub home_pos: Vec3,
    /// Generic timer for pacing actions (cooldowns, waits).
    pub action_timer: f32,
    /// How long since we last saw the player.
    pub lost_sight_timer: f32,
    /// Distance to player (updated each tick).
    pub distance_to_target: f32,
    /// Whether the entity currently has line-of-sight to target.
    pub can_see_target: bool,
    /// Number of times this entity has attacked (for pattern variation).
    pub attack_count: u32,
    /// Current A* path waypoints (world positions, next waypoint first).
    pub path: Vec<Vec3>,
    /// Time since last path recomputation.
    pub path_age: f32,
    /// Flag: did this entity pathfind this frame (for budget tracking).
    pub did_pathfind: bool,
    /// Strafe direction: +1 (CCW) or -1 (CW). Picked once per agent.
    pub strafe_dir: f32,
    /// One-shot action requests for the game layer (cleared after read).
    pub pending_action: Option<PendingAction>,
    /// Active leap state: while >0 we are mid-leap and movement is locked
    /// to the leap target.
    pub leap_timer: f32,
    /// Where we're leaping to.
    pub leap_target: Vec3,
}

impl Blackboard {
    pub fn new(home_pos: Vec3) -> Self {
        Self {
            target_pos: None,
            wander_target: None,
            home_pos,
            action_timer: 0.0,
            lost_sight_timer: 0.0,
            distance_to_target: f32::MAX,
            can_see_target: false,
            attack_count: 0,
            path: Vec::new(),
            path_age: 0.0,
            did_pathfind: false,
            strafe_dir: 1.0,
            pending_action: None,
            leap_timer: 0.0,
            leap_target: Vec3::ZERO,
        }
    }
}
