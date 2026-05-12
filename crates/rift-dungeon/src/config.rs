/// Floor generation configuration. Controls size, room count, and difficulty scaling.
#[derive(Clone, Debug)]
pub struct FloorConfig {
    /// Grid width in tiles.
    pub width: usize,
    /// Grid depth in tiles.
    pub depth: usize,
    /// Minimum BSP leaf size (rooms won't be smaller than this).
    pub min_leaf_size: usize,
    /// Maximum BSP leaf size before forced split.
    pub max_leaf_size: usize,
    /// Minimum room padding inside a leaf.
    pub room_padding: usize,
    /// Current floor number (1-indexed).
    pub floor: u32,
    /// Base enemy count.
    pub base_enemy_count: u32,
    /// Enemy count added per floor.
    pub enemies_per_floor: u32,
    /// Enemy speed multiplier (scales with floor, capped).
    pub enemy_speed: f32,
    /// Enemy health (scales quadratically with floor).
    pub enemy_health: f32,
    /// Multiplier on every enemy damage instance (melee, bolts,
    /// dash, slam, fan). Scales linearly so deep rifts hit hard
    /// even though HP-pool scaling is what slows the kill rate.
    pub enemy_damage_mult: f32,
    /// Number of mob packs per arena room.
    pub packs_per_room: u32,
    /// Mobs per pack (base).
    pub mobs_per_pack: u32,
    /// Chance (0.0-1.0) for a pack to have an elite leader.
    pub elite_chance: f32,
    /// Elite HP multiplier over normal enemy HP.
    pub elite_hp_mult: f32,
}

impl FloorConfig {
    /// Create config for a given floor number with default scaling.
    ///
    /// Scaling philosophy: density grows fast at low floors so
    /// the player feels the rift filling up, then plateaus while
    /// HP and damage take over as the difficulty drivers in the
    /// mid-game. Past floor ~12 the practical cap on density is
    /// hit (room sizes don't grow), and quadratic HP + linear
    /// damage scaling carry the curve.
    pub fn for_floor(floor: u32) -> Self {
        let f = floor as f32;
        Self {
            // Grid doubled (80\u2192160) and the BSP leaf bounds
            // doubled in lock-step so rooms come out roughly
            // twice as wide and twice as deep without changing
            // the typical room count per floor. Enemy density
            // (packs_per_room / mobs_per_pack) intentionally
            // stays the same — bigger rooms with the same pack
            // count means more breathing room between fights,
            // which is the gameplay change we want.
            width: 160,
            depth: 160,
            min_leaf_size: 24,
            max_leaf_size: 56,
            room_padding: 1,
            floor,
            base_enemy_count: 15,
            enemies_per_floor: 5,
            // Capped — past ~floor 9 enemies are at max chase speed.
            enemy_speed: (2.0 + f * 0.18).min(3.6),
            // Quadratic — kill time grows noticeably at depth.
            // f1: 25, f5: 78, f10: 198, f15: 378, f20: 618.
            enemy_health: 18.0 + f * 6.0 + f * f * 1.2,
            // Linear — keeps deep-rift hits scary even though
            // HP scaling biases toward longer fights.
            // f1: 1.15, f5: 1.75, f10: 2.5, f15: 3.25, f20: 4.0.
            enemy_damage_mult: 1.0 + f * 0.15,
            packs_per_room: 2 + floor.min(3),
            mobs_per_pack: 4 + floor.min(4),
            elite_chance: 0.3 + (f * 0.05).min(0.2),
            elite_hp_mult: 3.0,
        }
    }

    /// Total enemy count for this floor.
    pub fn enemy_count(&self) -> u32 {
        self.base_enemy_count + self.enemies_per_floor * (self.floor - 1)
    }
}
