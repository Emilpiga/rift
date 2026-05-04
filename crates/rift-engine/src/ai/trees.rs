use super::behavior::BehaviorNode;

/// Generic enemy fallback: same as Brute. Kept for backwards compat.
pub fn enemy_behavior() -> BehaviorNode {
    brute_behavior()
}

/// Brute: heavy melee. Charges into range, slams when surrounded by short cooldowns,
/// and leaps at medium range to close gaps. Slow but committed.
pub fn brute_behavior() -> BehaviorNode {
    BehaviorNode::Selector(vec![
        // 1. Adjacent + cooldown ready: heavy melee charge (existing system applies dmg on touch).
        BehaviorNode::Sequence(vec![
            BehaviorNode::InRange(1.8),
            BehaviorNode::CooldownReady,
            BehaviorNode::Charge { speed_mult: 2.2 },
            BehaviorNode::SetCooldown(0.7),
        ]),
        // 2. Medium range + cooldown: leap to close.
        BehaviorNode::Sequence(vec![
            BehaviorNode::CanSeeTarget,
            BehaviorNode::InRange(7.0),
            BehaviorNode::Inverter(Box::new(BehaviorNode::InRange(2.5))),
            BehaviorNode::CooldownReady,
            BehaviorNode::LeapStrike { distance: 5.5, duration: 0.45 },
            BehaviorNode::SetCooldown(3.5),
        ]),
        // 3. In detection range: chase.
        BehaviorNode::Sequence(vec![
            BehaviorNode::InRange(13.0),
            BehaviorNode::ChaseTarget,
        ]),
        // 4. Wander.
        BehaviorNode::Wander { radius: 4.0 },
    ])
}

/// Stalker: fast, evasive flanker. Strafes around the player at medium range,
/// occasionally lunging in for a quick strike.
pub fn stalker_behavior() -> BehaviorNode {
    BehaviorNode::Selector(vec![
        // 1. In melee + ready: slash and skip back via cooldown.
        BehaviorNode::Sequence(vec![
            BehaviorNode::InRange(1.6),
            BehaviorNode::CooldownReady,
            BehaviorNode::Charge { speed_mult: 1.6 },
            BehaviorNode::SetCooldown(0.6),
        ]),
        // 2. Just outside melee: rapid lunge.
        BehaviorNode::Sequence(vec![
            BehaviorNode::CanSeeTarget,
            BehaviorNode::InRange(4.5),
            BehaviorNode::Inverter(Box::new(BehaviorNode::InRange(1.8))),
            BehaviorNode::CooldownReady,
            BehaviorNode::LeapStrike { distance: 3.5, duration: 0.3 },
            BehaviorNode::SetCooldown(2.0),
        ]),
        // 3. In sight at medium range: strafe and weave.
        BehaviorNode::Sequence(vec![
            BehaviorNode::CanSeeTarget,
            BehaviorNode::InRange(8.0),
            BehaviorNode::KeepDistance { min: 2.5, ideal: 4.0, max: 6.0 },
        ]),
        // 4. Out of sight / far: chase.
        BehaviorNode::Sequence(vec![
            BehaviorNode::InRange(15.0),
            BehaviorNode::ChaseTarget,
        ]),
        BehaviorNode::Wander { radius: 5.0 },
    ])
}

/// Caster: ranged attacker. Keeps distance, fires bolts on a cooldown,
/// kites away when player closes the gap.
pub fn caster_behavior() -> BehaviorNode {
    BehaviorNode::Selector(vec![
        // 1. In range + LoS + cooldown ready: fire bolts at a measured pace.
        BehaviorNode::Sequence(vec![
            BehaviorNode::CanSeeTarget,
            BehaviorNode::InRange(11.0),
            BehaviorNode::CooldownReady,
            BehaviorNode::RangedAttack { damage: 5.0 },
            BehaviorNode::SetCooldown(2.0),
        ]),
        // 2. Player too close: backpedal aggressively.
        BehaviorNode::Sequence(vec![
            BehaviorNode::InRange(4.5),
            BehaviorNode::Backpedal { speed_mult: 1.4 },
        ]),
        // 3. In sight: keep distance and strafe.
        BehaviorNode::Sequence(vec![
            BehaviorNode::CanSeeTarget,
            BehaviorNode::InRange(13.0),
            BehaviorNode::KeepDistance { min: 5.0, ideal: 8.0, max: 11.0 },
        ]),
        // 4. Reposition.
        BehaviorNode::Sequence(vec![
            BehaviorNode::InRange(15.0),
            BehaviorNode::ChaseTarget,
        ]),
        BehaviorNode::Wander { radius: 4.0 },
    ])
}

/// Elite: pack leader. Brute-like but with a telegraphed AoE slam and a long-range leap.
pub fn elite_behavior() -> BehaviorNode {
    BehaviorNode::Selector(vec![
        // 1. Low HP enrage: aggressive leap then melee.
        BehaviorNode::Sequence(vec![
            BehaviorNode::HealthBelow(0.35),
            BehaviorNode::CanSeeTarget,
            BehaviorNode::InRange(9.0),
            BehaviorNode::CooldownReady,
            BehaviorNode::LeapStrike { distance: 7.0, duration: 0.45 },
            BehaviorNode::SetCooldown(2.5),
        ]),
        // 2. Adjacent + cooldown: AoE slam (telegraphed).
        BehaviorNode::Sequence(vec![
            BehaviorNode::InRange(3.0),
            BehaviorNode::CooldownReady,
            BehaviorNode::SlamTelegraph { radius: 3.2, damage: 18.0, delay: 0.85 },
            BehaviorNode::SetCooldown(4.0),
        ]),
        // 3. Adjacent: heavy strike.
        BehaviorNode::Sequence(vec![
            BehaviorNode::InRange(2.0),
            BehaviorNode::Charge { speed_mult: 1.8 },
            BehaviorNode::SetCooldown(0.6),
        ]),
        // 4. Medium range: leap close.
        BehaviorNode::Sequence(vec![
            BehaviorNode::CanSeeTarget,
            BehaviorNode::InRange(8.0),
            BehaviorNode::Inverter(Box::new(BehaviorNode::InRange(2.5))),
            BehaviorNode::CooldownReady,
            BehaviorNode::LeapStrike { distance: 6.0, duration: 0.5 },
            BehaviorNode::SetCooldown(3.0),
        ]),
        // 5. Chase.
        BehaviorNode::Sequence(vec![
            BehaviorNode::InRange(18.0),
            BehaviorNode::ChaseTarget,
        ]),
        BehaviorNode::Idle,
    ])
}

/// Boss: aggressive with charge attacks, leaps, and a telegraphed AoE.
pub fn boss_behavior() -> BehaviorNode {
    BehaviorNode::Selector(vec![
        // 1. Enrage below 30% HP — immediate leap-strike on cooldown.
        BehaviorNode::Sequence(vec![
            BehaviorNode::HealthBelow(0.3),
            BehaviorNode::CanSeeTarget,
            BehaviorNode::InRange(15.0),
            BehaviorNode::CooldownReady,
            BehaviorNode::LeapStrike { distance: 9.0, duration: 0.5 },
            BehaviorNode::SetCooldown(1.8),
        ]),
        // 2. AoE slam when player is adjacent.
        BehaviorNode::Sequence(vec![
            BehaviorNode::InRange(3.5),
            BehaviorNode::CooldownReady,
            BehaviorNode::SlamTelegraph { radius: 4.0, damage: 25.0, delay: 0.9 },
            BehaviorNode::SetCooldown(3.5),
        ]),
        // 3. Charge attack at medium range.
        BehaviorNode::Sequence(vec![
            BehaviorNode::InRange(8.0),
            BehaviorNode::CooldownReady,
            BehaviorNode::Charge { speed_mult: 2.5 },
            BehaviorNode::SetCooldown(1.3),
        ]),
        // 4. Circle-strafe when close.
        BehaviorNode::Sequence(vec![
            BehaviorNode::InRange(5.0),
            BehaviorNode::CircleStrafe { radius: 3.0, direction: 1.0 },
        ]),
        // 5. Chase.
        BehaviorNode::Sequence(vec![
            BehaviorNode::InRange(20.0),
            BehaviorNode::ChaseTarget,
        ]),
        BehaviorNode::Idle,
    ])
}

/// Ranged/cautious enemy variant: alias for caster_behavior (kept for backwards compat).
pub fn ranged_behavior() -> BehaviorNode {
    caster_behavior()
}
