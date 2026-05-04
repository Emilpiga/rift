/// Experience and leveling system.
#[derive(Clone, Debug)]
pub struct Experience {
    pub level: u32,
    pub current_xp: u64,
    pub total_xp: u64,
}

impl Experience {
    pub fn new() -> Self {
        Self {
            level: 1,
            current_xp: 0,
            total_xp: 0,
        }
    }

    /// XP required to reach next level from current level.
    pub fn xp_to_next_level(&self) -> u64 {
        xp_for_level(self.level + 1)
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
                rewards.push(LevelUpReward {
                    new_level: self.level,
                    attribute_points: attribute_points_for_level(self.level),
                    talent_points: 1, // 1 talent point per level
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
        // Reduced XP if you outlevel the monster
        let level_diff = player_level as i32 - monster_level as i32;
        let mult = if level_diff > 5 {
            0.1
        } else if level_diff > 0 {
            1.0 - (level_diff as f32 * 0.15)
        } else {
            // Bonus for killing higher-level monsters
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
fn xp_for_level(level: u32) -> u64 {
    (100.0 * (level as f32).powf(1.5)) as u64
}

/// Attribute points granted at a specific level.
fn attribute_points_for_level(level: u32) -> u32 {
    // 2 points every level, bonus 3 at milestone levels (10, 20, 30...)
    if level % 10 == 0 {
        5
    } else {
        2
    }
}
