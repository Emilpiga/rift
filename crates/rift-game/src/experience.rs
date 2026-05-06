//! Experience and leveling.

#[derive(Clone, Debug)]
pub struct Experience {
    pub level: u32,
    pub current_xp: u64,
    pub total_xp: u64,
    /// Authoritative XP threshold for the next level. Mirrored
    /// from the server's `ServerMsg::CharacterStats` so the bar
    /// can never disagree with the server's level-up trigger.
    /// `None` falls back to the local
    /// `xp_for_level(level + 1)` formula \u2014 used in the
    /// pre-connect / character-select preview where there's no
    /// server to ask.
    server_xp_to_next: Option<u64>,
}

impl Experience {
    pub fn new() -> Self {
        Self {
            level: 1,
            current_xp: 0,
            total_xp: 0,
            server_xp_to_next: None,
        }
    }

    /// Override the local `xp_for_level` formula with the value
    /// the server just sent. Cleared by [`Self::grant_xp`] on a
    /// level-up so the next bar segment falls back to the local
    /// estimate until the next server message arrives.
    pub fn set_xp_to_next(&mut self, value: u64) {
        self.server_xp_to_next = Some(value);
    }

    /// XP required to reach next level from current level.
    pub fn xp_to_next_level(&self) -> u64 {
        self.server_xp_to_next.unwrap_or_else(|| xp_for_level(self.level + 1))
    }

    /// Progress to next level as 0.0–1.0.
    pub fn progress(&self) -> f32 {
        let needed = self.xp_to_next_level();
        if needed == 0 { return 1.0; }
        self.current_xp as f32 / needed as f32
    }

    /// Grant XP. Returns a list of level-up rewards if any levels were gained.
    pub fn grant_xp(&mut self, amount: u64) -> Vec<LevelUpReward> {
        self.current_xp += amount;
        self.total_xp += amount;

        let mut rewards = Vec::new();
        loop {
            let needed = self.xp_to_next_level();
            if self.current_xp >= needed {
                self.current_xp -= needed;
                self.level += 1;
                // The server will push a fresh threshold next
                // tick; clear the cached value so the bar uses
                // the local formula in the meantime.
                self.server_xp_to_next = None;
                rewards.push(LevelUpReward {
                    new_level: self.level,
                    attribute_points: attribute_points_for_level(self.level),
                    talent_points: 1,
                });
            } else {
                break;
            }
        }

        rewards
    }

    /// XP reward for killing a monster at a given level.
    pub fn xp_for_kill(monster_level: u32, player_level: u32) -> u64 {
        let base = 20 + monster_level as u64 * 5;
        let level_diff = player_level as i32 - monster_level as i32;
        let mult = if level_diff > 5 {
            0.1
        } else if level_diff > 0 {
            1.0 - (level_diff as f32 * 0.15)
        } else {
            1.0 + (-level_diff as f32 * 0.1)
        };
        (base as f32 * mult.max(0.1)) as u64
    }
}

/// Reward granted on level up.
#[derive(Clone, Debug)]
pub struct LevelUpReward {
    pub new_level: u32,
    pub attribute_points: u32,
    pub talent_points: u32,
}

/// XP required to go from (level-1) to (level).
/// Formula: 100 * level^1.5 (accelerating curve).
///
/// Public so the server can derive `current_xp` from a stored
/// `(total_xp, level)` pair without re-implementing the curve.
pub fn xp_for_level(level: u32) -> u64 {
    (100.0 * (level as f32).powf(1.5)) as u64
}

/// Cumulative XP required to reach `level` from level 1. Sum of
/// [`xp_for_level`] over `2..=level`. Returns 0 for levels ≤ 1.
pub fn total_xp_for_level(level: u32) -> u64 {
    if level <= 1 {
        return 0;
    }
    (2..=level).map(xp_for_level).sum()
}

/// Attribute points granted at a specific level.
fn attribute_points_for_level(level: u32) -> u32 {
    if level % 10 == 0 {
        5
    } else {
        2
    }
}
