use glam::Vec3;

/// The type of room placed in a BSP leaf.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomType {
    /// Standard combat encounter room.
    Arena,
    /// Large room for end-of-floor boss fight.
    BossRoom,
    /// Narrow connecting passage between rooms.
    Corridor,
}

/// A room with position, dimensions, type, and spawn metadata.
#[derive(Clone, Debug)]
pub struct Room {
    pub x: usize,
    pub z: usize,
    pub width: usize,
    pub depth: usize,
    pub room_type: RoomType,
}

impl Room {
    pub fn center(&self) -> (usize, usize) {
        (self.x + self.width / 2, self.z + self.depth / 2)
    }

    pub fn center_world(&self) -> Vec3 {
        let (cx, cz) = self.center();
        Vec3::new(cx as f32, 0.0, cz as f32)
    }

    pub fn area(&self) -> usize {
        self.width * self.depth
    }

    /// Get random floor positions inside the room for spawning entities.
    pub fn spawn_positions(&self, count: usize, seed: u64) -> Vec<Vec3> {
        let mut rng = super::SimpleRng::new(seed);
        let mut positions = Vec::with_capacity(count);

        for _ in 0..count {
            let px = self.x as f32 + 1.0 + rng.range(0, (self.width - 2).max(1) as u32) as f32;
            let pz = self.z as f32 + 1.0 + rng.range(0, (self.depth - 2).max(1) as u32) as f32;
            positions.push(Vec3::new(px, 0.5, pz));
        }

        positions
    }

    /// Spawn clustered packs of enemies. Returns (pack_center, positions) pairs.
    /// Each pack has `mobs_per_pack` enemies clustered within ~2 tiles of the pack center.
    pub fn spawn_packs(&self, num_packs: u32, mobs_per_pack: u32, seed: u64) -> Vec<(Vec3, Vec<Vec3>)> {
        let mut rng = super::SimpleRng::new(seed);
        let mut packs = Vec::new();

        for _ in 0..num_packs {
            // Pick a pack center somewhere inside the room (with margin)
            let margin = 2.0;
            let cx = self.x as f32 + margin
                + rng.range(0, ((self.width as f32 - margin * 2.0).max(1.0)) as u32) as f32;
            let cz = self.z as f32 + margin
                + rng.range(0, ((self.depth as f32 - margin * 2.0).max(1.0)) as u32) as f32;
            let center = Vec3::new(cx, 0.5, cz);

            let mut positions = Vec::new();
            for _ in 0..mobs_per_pack {
                // Scatter within 1.5 tiles of center
                let dx = (rng.range(0, 30) as f32 / 10.0) - 1.5;
                let dz = (rng.range(0, 30) as f32 / 10.0) - 1.5;
                let px = (cx + dx).clamp(self.x as f32 + 0.5, (self.x + self.width) as f32 - 0.5);
                let pz = (cz + dz).clamp(self.z as f32 + 0.5, (self.z + self.depth) as f32 - 0.5);
                positions.push(Vec3::new(px, 0.5, pz));
            }

            packs.push((center, positions));
        }

        packs
    }
}
