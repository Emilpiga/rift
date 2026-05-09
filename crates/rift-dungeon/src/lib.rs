pub mod bsp;
pub mod config;
pub mod nav;
pub mod rooms;

pub use config::FloorConfig;
pub use nav::NavGrid;
pub use rooms::{Room, RoomType};

use glam::Vec3;

/// Cell types in the dungeon grid.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tile {
    Wall,
    Floor,
}

/// A fully generated dungeon floor.
pub struct Floor {
    pub width: usize,
    pub depth: usize,
    pub tiles: Vec<Tile>,
    pub rooms: Vec<Room>,
    pub spawn_pos: Vec3,
    pub boss_room_center: Vec3,
    /// World positions of the two post-boss portals (descend
    /// + return-to-hub), pre-baked from the dedicated
    /// [`RoomType::PortalRoom`]. `None` only on degenerate
    /// floors where no second room could be found (the
    /// hub-style synthetic floor or a tiny floor where every
    /// other room got a different role). Callers that don't
    /// find this should fall back to spawning at
    /// `boss_room_center` so the portals still appear, but
    /// the normal rift path always populates this.
    pub portal_anchors: Option<(Vec3, Vec3)>,
    pub config: FloorConfig,
}

impl Floor {
    /// Generate a floor using BSP partitioning.
    pub fn generate(config: FloorConfig, seed: u64) -> Self {
        let width = config.width;
        let depth = config.depth;
        let mut tiles = vec![Tile::Wall; width * depth];

        let (rooms, corridors) = bsp::generate_bsp(&config, seed);

        // Carve rooms into the tile grid
        for room in &rooms {
            for z in room.z..room.z + room.depth {
                for x in room.x..room.x + room.width {
                    if x < width && z < depth {
                        tiles[z * width + x] = Tile::Floor;
                    }
                }
            }
        }

        // Carve corridors (L-shaped: horizontal then vertical, 3-wide)
        for &(x1, z1, x2, z2) in &corridors {
            // Horizontal segment
            let (sx, ex) = if x1 < x2 { (x1, x2) } else { (x2, x1) };
            for x in sx..=ex {
                for dz in 0..3 {
                    let cz = z1 + dz;
                    if x < width && cz < depth {
                        tiles[cz * width + x] = Tile::Floor;
                    }
                }
            }
            // Vertical segment
            let (sz, ez) = if z1 < z2 { (z1, z2) } else { (z2, z1) };
            for z in sz..=ez {
                for dx in 0..3 {
                    let cx = x2 + dx;
                    if cx < width && z < depth {
                        tiles[z * width + cx] = Tile::Floor;
                    }
                }
            }
        }

        // Spawn in center of first (non-boss) room.
        // Excludes the portal room too — landing on top of
        // the post-boss portals on every floor entry would
        // defeat the "fight your way through, then choose"
        // pacing.
        let spawn_pos = rooms
            .iter()
            .find(|r| r.room_type == RoomType::Arena)
            .map(|r| r.center_world() + Vec3::new(0.0, 0.5, 0.0))
            .or_else(|| {
                // Fallback: if no Arena room (tiny BSP), any
                // non-boss room works.
                rooms
                    .iter()
                    .find(|r| r.room_type != RoomType::BossRoom)
                    .map(|r| r.center_world() + Vec3::new(0.0, 0.5, 0.0))
            })
            .unwrap_or(Vec3::new(width as f32 / 2.0, 0.5, depth as f32 / 2.0));

        // Boss room center
        let boss_room_center = rooms
            .iter()
            .find(|r| r.room_type == RoomType::BossRoom)
            .map(|r| r.center_world())
            .unwrap_or(Vec3::ZERO);

        // Pre-bake portal anchors from the portal room. Done
        // once at floor-gen time so the runtime portal-spawn
        // path is a constant-time lookup, and so changing the
        // anchor formula doesn't drift between client and any
        // future server-side proximity check.
        let portal_anchors = rooms
            .iter()
            .find(|r| r.room_type == RoomType::PortalRoom)
            .map(|r| r.portal_anchors());

        Self {
            width,
            depth,
            tiles,
            rooms,
            spawn_pos,
            boss_room_center,
            portal_anchors,
            config,
        }
    }

    pub fn get(&self, x: usize, z: usize) -> Tile {
        if x >= self.width || z >= self.depth {
            return Tile::Wall;
        }
        self.tiles[z * self.width + x]
    }

    /// Returns `true` if a horizontal line from `a` to `b` is
    /// not blocked by any [`Tile::Wall`]. Y is ignored.
    ///
    /// Thin wrapper over
    /// [`rift_math::physics::line_of_sight_grid`] — the
    /// algorithm (segment sampling, step cadence, world→grid
    /// rounding) lives in `rift-math` so every LOS user in the
    /// workspace shares one implementation. This binding only
    /// supplies the "is this tile a wall?" predicate, which is
    /// the part that depends on [`Floor`]'s tile storage.
    ///
    /// Negative or out-of-bounds samples count as blocked,
    /// matching [`Floor::get`]'s behaviour for those cases.
    pub fn line_of_sight(&self, a: Vec3, b: Vec3) -> bool {
        rift_math::physics::line_of_sight_grid(a, b, |gx, gz| {
            if gx < 0 || gz < 0 {
                return true;
            }
            self.get(gx as usize, gz as usize) == Tile::Wall
        })
    }

    /// 4-way A* on the floor's tile grid. Thin wrapper over
    /// [`rift_math::physics::astar_grid`] with the "tile is
    /// walkable iff in-bounds and [`Tile::Floor`]" rule baked
    /// in. See the underlying fn for path-format and budget
    /// semantics.
    ///
    /// Used by enemy AI to navigate around walls when
    /// straight-line LOS to the target is blocked.
    pub fn path(
        &self,
        from: (i32, i32),
        goal: (i32, i32),
        max_expanded: usize,
    ) -> Option<Vec<(i32, i32)>> {
        rift_math::physics::astar_grid(from, goal, max_expanded, |x, z| {
            x >= 0
                && z >= 0
                && (x as usize) < self.width
                && (z as usize) < self.depth
                && self.get(x as usize, z as usize) == Tile::Floor
        })
    }

    /// Get wall positions (only walls adjacent to floor tiles).
    ///
    /// Includes diagonal neighbors so inside-corner cells aren't
    /// culled. With orthogonal-only adjacency, an L-bend like:
    ///
    /// ```text
    ///   W W F
    ///   W F F
    ///   F F F
    /// ```
    ///
    /// would drop the top-left wall (no N/E/S/W floor neighbor)
    /// and leave a visible 1-tile gap at the inside corner. The
    /// 8-way check fills those corners while still pruning walls
    /// buried deep in the rock.
    pub fn wall_positions(&self) -> Vec<Vec3> {
        let mut positions = Vec::new();
        for z in 0..self.depth {
            for x in 0..self.width {
                if self.tiles[z * self.width + x] == Tile::Wall {
                    let adjacent_floor = [
                        (x.wrapping_sub(1), z),
                        (x + 1, z),
                        (x, z.wrapping_sub(1)),
                        (x, z + 1),
                        (x.wrapping_sub(1), z.wrapping_sub(1)),
                        (x + 1, z.wrapping_sub(1)),
                        (x.wrapping_sub(1), z + 1),
                        (x + 1, z + 1),
                    ]
                    .iter()
                    .any(|&(nx, nz)| self.get(nx, nz) == Tile::Floor);

                    if adjacent_floor {
                        positions.push(Vec3::new(x as f32, 0.0, z as f32));
                    }
                }
            }
        }
        positions
    }

    /// Get arena rooms (for enemy spawning).
    pub fn arena_rooms(&self) -> Vec<&Room> {
        self.rooms.iter().filter(|r| r.room_type == RoomType::Arena).collect()
    }

    /// Get all floor tile positions (for building the dungeon floor mesh).
    pub fn floor_positions(&self) -> Vec<Vec3> {
        let mut positions = Vec::new();
        for z in 0..self.depth {
            for x in 0..self.width {
                if self.tiles[z * self.width + x] == Tile::Floor {
                    positions.push(Vec3::new(x as f32, 0.0, z as f32));
                }
            }
        }
        positions
    }

    /// Get the boss room.
    pub fn boss_room(&self) -> Option<&Room> {
        self.rooms.iter().find(|r| r.room_type == RoomType::BossRoom)
    }

    /// Build a synthetic, BSP-free single-room floor used for the hub /
    /// starting zone.  No enemies, no boss room — just a quiet square
    /// chamber with a fixed spawn at the south end and a "centre" point
    /// the caller can drop a return-to-rift portal on.
    pub fn hub() -> Self {
        const SIZE: usize = 36;     // grid is SIZE × SIZE walls
        const ROOM: usize = 30;     // inner walkable square
        let pad = (SIZE - ROOM) / 2;
        let width = SIZE;
        let depth = SIZE;
        let mut tiles = vec![Tile::Wall; width * depth];
        for z in pad..pad + ROOM {
            for x in pad..pad + ROOM {
                tiles[z * width + x] = Tile::Floor;
            }
        }
        let room = Room {
            x: pad,
            z: pad,
            width: ROOM,
            depth: ROOM,
            room_type: RoomType::Arena,
        };
        // Player spawns near the south wall, facing the portal in centre.
        let spawn_pos = Vec3::new(
            (pad + ROOM / 2) as f32,
            0.5,
            (pad + ROOM - 3) as f32,
        );
        let mut config = FloorConfig::for_floor(1);
        config.width = width;
        config.depth = depth;
        config.base_enemy_count = 0;
        config.enemies_per_floor = 0;
        config.packs_per_room = 0;
        config.mobs_per_pack = 0;
        Self {
            width,
            depth,
            tiles,
            rooms: vec![room],
            spawn_pos,
            boss_room_center: Vec3::ZERO,
            portal_anchors: None,
            config,
        }
    }

    /// Centre of the first arena-style room — useful for hub layouts to
    /// position a return portal.
    pub fn first_room_center(&self) -> Vec3 {
        self.rooms
            .iter()
            .find(|r| r.room_type == RoomType::Arena)
            .map(|r| r.center_world() + Vec3::new(0.0, 0.0, 0.0))
            .unwrap_or(Vec3::ZERO)
    }
}

/// Minimal seeded RNG (xorshift64).
pub(crate) struct SimpleRng {
    state: u64,
}

impl SimpleRng {
    pub fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 1 } else { seed },
        }
    }

    pub fn next(&mut self) -> u64 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        self.state
    }

    pub fn range(&mut self, min: u32, max: u32) -> u32 {
        if max <= min {
            return min;
        }
        min + (self.next() % (max - min) as u64) as u32
    }
}
