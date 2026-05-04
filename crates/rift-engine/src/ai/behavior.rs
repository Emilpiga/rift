/// Result of ticking a behavior node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Node completed successfully.
    Success,
    /// Node failed.
    Failure,
    /// Node is still running (multi-frame action).
    Running,
}

/// A composable behavior tree node.
#[derive(Clone, Debug)]
pub enum BehaviorNode {
    // ─── Composites ───────────────────────────────────────────────
    /// Try children in order until one succeeds (OR logic).
    Selector(Vec<BehaviorNode>),
    /// Run children in order until one fails (AND logic).
    Sequence(Vec<BehaviorNode>),

    // ─── Decorators ──────────────────────────────────────────────
    /// Invert the child's result (Success↔Failure, Running stays).
    Inverter(Box<BehaviorNode>),
    /// Repeat child until it fails (useful for patrol loops).
    RepeatUntilFail(Box<BehaviorNode>),

    // ─── Conditions (leaf) ───────────────────────────────────────
    /// True if distance to target < range.
    InRange(f32),
    /// True if we can see the target.
    CanSeeTarget,
    /// True if action_timer <= 0 (cooldown expired).
    CooldownReady,
    /// True if entity health < threshold percent (0.0–1.0).
    HealthBelow(f32),

    // ─── Actions (leaf) ──────────────────────────────────────────
    /// Move toward the target (player).
    ChaseTarget,
    /// Move toward home position (leash).
    ReturnHome,
    /// Wander randomly near home.
    Wander { radius: f32 },
    /// Stand still.
    Idle,
    /// Charge at the target at a speed multiplier.
    Charge { speed_mult: f32 },
    /// Circle-strafe around the target.
    CircleStrafe { radius: f32, direction: f32 },
    /// Set the action timer (start a cooldown).
    SetCooldown(f32),
    /// Wait (returns Running) until action_timer expires.
    WaitCooldown,

    // ─── Smart actions (request game-side effects) ───────────────
    /// Move directly away from the target while keeping aim.
    Backpedal { speed_mult: f32 },
    /// Maintain a comfortable distance band from target.
    /// Too close → backpedal; too far → approach; in band → strafe.
    KeepDistance { min: f32, ideal: f32, max: f32 },
    /// Fire a ranged bolt at the target. Sets a PendingAction::RangedShot
    /// for the game layer to spawn the projectile.
    RangedAttack { damage: f32 },
    /// Begin a leap-strike toward the target. Sets PendingAction::LeapStart
    /// and engages a short movement-locked leap.
    LeapStrike { distance: f32, duration: f32 },
    /// Wind up a slam: the agent stops, then telegraphs an AoE.
    SlamTelegraph { radius: f32, damage: f32, delay: f32 },
}

impl BehaviorNode {
    /// Helper to wrap in an Inverter.
    pub fn invert(self) -> Self {
        BehaviorNode::Inverter(Box::new(self))
    }
}
