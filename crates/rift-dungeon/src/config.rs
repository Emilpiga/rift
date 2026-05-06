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
    /// Enemy speed multiplier (scales with floor).
    pub enemy_speed: f32,
    /// Enemy health multiplier (scales with floor).
    pub enemy_health: f32,
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
    pub fn for_floor(floor: u32) -> Self {
        Self {
            width: 80,
            depth: 80,
            min_leaf_size: 12,
            max_leaf_size: 28,
            room_padding: 1,
            floor,
            base_enemy_count: 15,
            enemies_per_floor: 5,
            enemy_speed: 2.0 + floor as f32 * 0.3,
            enemy_health: 15.0 + floor as f32 * 8.0,
            packs_per_room: 2 + floor.min(3),
            mobs_per_pack: 4 + floor.min(4),
            elite_chance: 0.3 + (floor as f32 * 0.05).min(0.2),
            elite_hp_mult: 3.0,
        }
    }

    /// Total enemy count for this floor.
    pub fn enemy_count(&self) -> u32 {
        self.base_enemy_count + self.enemies_per_floor * (self.floor - 1)
    }
}
