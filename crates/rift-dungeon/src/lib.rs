pub mod bsp;
pub mod config;
pub mod nav;
pub mod rooms;

pub use config::FloorConfig;
pub use nav::NavGrid;
pub use rooms::{Room, RoomShape, RoomTheme, RoomType, SurfaceKind};

use glam::Vec3;

/// Cell types in the dungeon grid.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tile {
    Wall,
    Floor,
    /// A stair / ramp tile bridging two elevations. The
    /// elevation grid stores the *low* end's elevation; the
    /// `dir` field encodes which cardinal direction the stair
    /// rises in (the rise is to elevation `low + 1`). Walked
    /// the same as floor for collision; the renderer builds a
    /// slanted ramp quad in place of a flat tile here.
    Stair {
        dir: StairDir,
    },
}

impl Tile {
    /// `true` for any tile a character can stand on. Floor
    /// and stair tiles return true; walls return false. Used
    /// by pathfinding, wall-mesh adjacency tests, and prop
    /// placement so adding stairs (Phase 3+) doesn't break
    /// every consumer that previously checked `== Floor`.
    pub fn is_walkable(self) -> bool {
        matches!(self, Tile::Floor | Tile::Stair { .. })
    }
}

/// Cardinal direction a [`Tile::Stair`] ascends in. Ramp
/// geometry rises from the low edge (the side opposite this
/// direction) to the high edge (the side this direction
/// points to). E.g. `StairDir::PosX` means the +X edge is the
/// raised one and the -X edge sits at the base elevation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StairDir {
    PosX,
    NegX,
    PosZ,
    NegZ,
}

/// World-space height of one elevation step. Picked so a
/// human-scale character (~1.8 m tall) reads the change as
/// "I climbed a low platform" without making stairs feel
/// monumental. Two steps stack to ~one metre, the height of
/// a typical balcony rail.
pub const ELEVATION_STEP: f32 = 0.5;

/// Type used for elevation indices in the per-tile elevation
/// grid. Signed so we can represent sunken pits (negative
/// elevation) below the nominal Y=0 floor. `i8` gives us a
/// total elevation range of ~64 m up and 64 m down at our
/// 0.5 m step — vastly more than any realistic dungeon ever
/// needs.
pub type Elevation = i8;

/// A fully generated dungeon floor.
pub struct Floor {
    pub width: usize,
    pub depth: usize,
    pub tiles: Vec<Tile>,
    /// Per-tile elevation in [`ELEVATION_STEP`]-multiples.
    /// Wall tiles' values are unused (set to 0). Floor tiles
    /// store their level. Stair tiles store the *low end*'s
    /// elevation; the high end is `elev + 1`.
    ///
    /// Co-located with `tiles` rather than added to the
    /// [`Tile`] enum so existing `tiles[i] == Tile::Floor`
    /// checks keep working unchanged; consumers that care
    /// about elevation read this grid directly.
    pub elevation: Vec<Elevation>,
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
    /// Floor-wide ground-material fallback used by
    /// [`Floor::surface_at`] when the queried tile is
    /// outside any room (corridors, doorways, OOB).
    /// Per-room surfaces (driven by [`RoomTheme::default_surface`])
    /// take precedence whenever the lookup lands inside a
    /// room rectangle. The hub overrides this to
    /// [`SurfaceKind::Sand`]; rift floors leave it at
    /// [`SurfaceKind::Stone`].
    pub default_surface: SurfaceKind,
    pub config: FloorConfig,
}

impl Floor {
    /// World-space Y coordinate of the floor surface at tile
    /// `(x, z)`. Returns `0.0` for walls and out-of-bounds
    /// (the historical floor plane). For stair tiles, returns
    /// the *low* end's elevation — callers that need the slope
    /// surface (e.g. character ground-following) should
    /// interpolate using [`tile_floor_y_at`].
    pub fn tile_floor_y(&self, x: usize, z: usize) -> f32 {
        if x >= self.width || z >= self.depth {
            return 0.0;
        }
        let i = z * self.width + x;
        match self.tiles[i] {
            Tile::Wall => 0.0,
            Tile::Floor | Tile::Stair { .. } => self.elevation[i] as f32 * ELEVATION_STEP,
        }
    }

    /// World-space Y at the given world `(x, z)` position,
    /// honouring stair-tile slopes. The cheap path: integer
    /// tile lookup for flat floors. Stair tiles linearly
    /// interpolate Y across the step in their ascent direction
    /// using the fractional position inside the tile so a
    /// character walking up a ramp climbs continuously instead
    /// of teleporting at tile boundaries.
    ///
    /// Tile (ix, iz) is rendered as a 1 × 1 quad **centred**
    /// on the integer coordinate — it covers world
    /// `[ix-0.5, ix+0.5]` along each axis. To match the mesh
    /// (and the rest of the engine: physics, projectile,
    /// brute AI, etc.), we use the `(x + 0.5).floor()`
    /// tile-centre indexing convention. Using a plain
    /// `x.floor()` here would shift the heightfield 0.5 m
    /// in +X / +Z relative to the rendered tiles, which is
    /// exactly the visible mismatch players reported when
    /// stepping onto raised daises and sunken pits.
    pub fn tile_floor_y_at(&self, x: f32, z: f32) -> f32 {
        let gx = (x + 0.5).floor();
        let gz = (z + 0.5).floor();
        if gx < 0.0 || gz < 0.0 {
            return 0.0;
        }
        let (ix, iz) = (gx as usize, gz as usize);
        if ix >= self.width || iz >= self.depth {
            return 0.0;
        }
        let i = iz * self.width + ix;
        match self.tiles[i] {
            Tile::Wall => 0.0,
            Tile::Floor => self.elevation[i] as f32 * ELEVATION_STEP,
            Tile::Stair { dir } => {
                // Fractional position inside the tile, in
                // [0, 1] along each axis. The slope rises from
                // the low edge (opposite `dir`) to the high
                // edge (`dir`); we project the position onto
                // the slope axis and lerp between low and high
                // elevations. With centre-indexing, the tile
                // spans world `[ix-0.5, ix+0.5]`, so the local
                // [0, 1] axis is `(x + 0.5) - gx`.
                let fx = (x + 0.5) - gx;
                let fz = (z + 0.5) - gz;
                let t = match dir {
                    StairDir::PosX => fx,
                    StairDir::NegX => 1.0 - fx,
                    StairDir::PosZ => fz,
                    StairDir::NegZ => 1.0 - fz,
                };
                let low = self.elevation[i] as f32 * ELEVATION_STEP;
                low + t.clamp(0.0, 1.0) * ELEVATION_STEP
            }
        }
    }

    /// Ground-material classification at world `(x, z)`. Used
    /// by gameplay systems that key off of *physical surface*
    /// rather than visual theme — footstep audio is the
    /// canonical case. Lookup priority:
    ///
    /// 1. **Per-tile override** — reserved for future use
    ///    when [`Tile`] grows a per-tile material slot
    ///    (wooden bridges, metal grates inside a stone
    ///    room, bone piles in a prison cell).
    /// 2. **Room theme** — the room containing the queried
    ///    point dictates the surface via
    ///    [`RoomTheme::default_surface`].
    /// 3. **Floor default** — corridors and OOB queries
    ///    fall back to [`Floor::default_surface`].
    ///
    /// Out-of-bounds queries return the floor default rather
    /// than panic so callers (footstep audio sampling at the
    /// player's current frame) are robust to a one-frame
    /// teleport / spawn-correction landing them off the
    /// grid.
    pub fn surface_at(&self, x: f32, z: f32) -> SurfaceKind {
        let gx = (x + 0.5).floor();
        let gz = (z + 0.5).floor();
        if gx < 0.0 || gz < 0.0 {
            return self.default_surface;
        }
        let (ix, iz) = (gx as usize, gz as usize);
        if ix >= self.width || iz >= self.depth {
            return self.default_surface;
        }
        // (1) per-tile override — not yet on `Tile`; future
        // work will check it here before the room lookup.

        // (2) room lookup. Linear scan is fine: every floor
        // has well under 30 rooms and `surface_at` is called
        // at most a handful of times per frame (one per
        // player). Mirror of `room_at` in
        // `crates/rift-client/src/game/torches.rs`.
        for room in &self.rooms {
            if ix >= room.x && ix < room.x + room.width && iz >= room.z && iz < room.z + room.depth
            {
                // Per-room override (e.g. hub forcing Sand)
                // wins over the theme default.
                return room.surface.unwrap_or_else(|| room.theme.default_surface());
            }
        }

        // (3) corridors and dead space.
        self.default_surface
    }

    /// Surface normal at the given world `(x, z)` position. The
    /// companion to [`tile_floor_y_at`] for foot IK / character
    /// pose alignment: a flat floor returns `Vec3::Y`; a stair
    /// tile returns its analytic ramp normal (computed from the
    /// stair direction and `ELEVATION_STEP`); walls and
    /// out-of-bounds also return `Vec3::Y` (so a foot momentarily
    /// outside the grid stays upright instead of snapping
    /// horizontal).
    pub fn tile_floor_normal_at(&self, x: f32, z: f32) -> Vec3 {
        let gx = (x + 0.5).floor();
        let gz = (z + 0.5).floor();
        if gx < 0.0 || gz < 0.0 {
            return Vec3::Y;
        }
        let (ix, iz) = (gx as usize, gz as usize);
        if ix >= self.width || iz >= self.depth {
            return Vec3::Y;
        }
        let i = iz * self.width + ix;
        match self.tiles[i] {
            Tile::Wall | Tile::Floor => Vec3::Y,
            Tile::Stair { dir } => {
                // Slope rises one ELEVATION_STEP across one tile
                // (1.0 m) in the `dir` axis. Tangent along the
                // slope is `(axis, ELEVATION_STEP, 0)`; cross
                // with the perpendicular horizontal gives the
                // surface normal. Sign matches the mesh builder
                // in `Mesh::dungeon_stairs` so lighting and IK
                // agree.
                match dir {
                    StairDir::PosX => Vec3::new(-ELEVATION_STEP, 1.0, 0.0).normalize(),
                    StairDir::NegX => Vec3::new(ELEVATION_STEP, 1.0, 0.0).normalize(),
                    StairDir::PosZ => Vec3::new(0.0, 1.0, -ELEVATION_STEP).normalize(),
                    StairDir::NegZ => Vec3::new(0.0, 1.0, ELEVATION_STEP).normalize(),
                }
            }
        }
    }
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

        // Carve shape silhouettes back over the rectangles.
        // Done *before* corridors so corridor carving still
        // can punch through anything we re-walled (a corridor
        // entry point that lands on a freshly walled-off
        // alcove corner gets re-floored and the room remains
        // reachable). The carve step is purely additive: it
        // turns floor tiles back into walls or leaves them
        // alone, never the reverse.
        for room in &rooms {
            apply_room_shape(&mut tiles, width, depth, room, seed);
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

        // Phase 2: assign per-room elevation features
        // (raised dais, sunken pit, balcony band) and stitch
        // them into the tile grid via stair tiles. Every
        // feature is gated on the room's theme + size so
        // small rooms don't pile multi-tier geometry into
        // 4-tile spaces, and on the room being well-clear of
        // the corridor mouths so stairs don't land on a
        // doorway.
        let mut elevation = vec![0 as Elevation; width * depth];
        for room in &rooms {
            apply_room_elevation(&mut tiles, &mut elevation, width, depth, room, seed);
        }

        Self {
            width,
            depth,
            tiles,
            elevation,
            rooms,
            spawn_pos,
            boss_room_center,
            portal_anchors,
            default_surface: SurfaceKind::Stone,
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
                && self.get(x as usize, z as usize).is_walkable()
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
    ///
    /// Wall base Y stays fixed at 0 because every elevation
    /// feature we generate (raised dais, sunken pit) is an
    /// *interior* sub-region of a room, never touching a wall
    /// tile. Lowering the wall base to follow a neighbouring
    /// pit's elevation would just clip 0.5 m off the wall *top*
    /// for every pit-adjacent wall, which we don't have any of.
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
                    .any(|&(nx, nz)| {
                        if nx >= self.width || nz >= self.depth {
                            return false;
                        }
                        self.tiles[nz * self.width + nx].is_walkable()
                    });

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
        self.rooms
            .iter()
            .filter(|r| r.room_type == RoomType::Arena)
            .collect()
    }

    /// Index of the room whose BSP rectangle contains the given
    /// integer tile coords, or `None` for corridor / out-of-room
    /// tiles. Linear scan is fine — floors top out at a few
    /// dozen rooms and this is only called during mesh build.
    ///
    /// Note that interior `RoomShape` carving (pillars, alcoves,
    /// cross, round) may turn an in-rect tile into [`Tile::Wall`];
    /// this method still reports the room index for those tiles
    /// because the *room* still owns them visually (the wall sits
    /// inside the room's perimeter and should be themed with the
    /// room's wall pack, not the corridor default).
    pub fn tile_room(&self, x: usize, z: usize) -> Option<usize> {
        for (i, r) in self.rooms.iter().enumerate() {
            if x >= r.x && x < r.x + r.width && z >= r.z && z < r.z + r.depth {
                return Some(i);
            }
        }
        None
    }

    /// Theme of the room covering tile `(x, z)`, or
    /// [`RoomTheme::Generic`] for corridor / out-of-bounds tiles.
    /// Convenience wrapper over [`Self::tile_room`] for the
    /// client-side per-room texture-pack split — corridors share
    /// the generic palette so the seam between corridor and room
    /// reads as the room's interior changing materials.
    pub fn tile_theme(&self, x: usize, z: usize) -> RoomTheme {
        match self.tile_room(x, z) {
            Some(i) => self.rooms[i].theme,
            None => RoomTheme::Generic,
        }
    }

    /// Get all floor tile positions (for building the dungeon floor mesh).
    pub fn floor_positions(&self) -> Vec<Vec3> {
        let mut positions = Vec::new();
        for z in 0..self.depth {
            for x in 0..self.width {
                let i = z * self.width + x;
                if self.tiles[i] == Tile::Floor {
                    let y = self.elevation[i] as f32 * ELEVATION_STEP;
                    positions.push(Vec3::new(x as f32, y, z as f32));
                }
            }
        }
        positions
    }

    /// Get all stair tile positions for the dungeon ramp mesh.
    /// Each entry is `(base_pos, dir)` where `base_pos.y` is the
    /// *low* end's world Y (the high end sits one
    /// [`ELEVATION_STEP`] above).
    pub fn stair_positions(&self) -> Vec<(Vec3, StairDir)> {
        let mut out = Vec::new();
        for z in 0..self.depth {
            for x in 0..self.width {
                let i = z * self.width + x;
                if let Tile::Stair { dir } = self.tiles[i] {
                    let y = self.elevation[i] as f32 * ELEVATION_STEP;
                    out.push((Vec3::new(x as f32, y, z as f32), dir));
                }
            }
        }
        out
    }

    /// Get the boss room.
    pub fn boss_room(&self) -> Option<&Room> {
        self.rooms
            .iter()
            .find(|r| r.room_type == RoomType::BossRoom)
    }

    /// Build a synthetic, BSP-free single-room floor used for the hub /
    /// starting zone.  No enemies, no boss room — just a quiet square
    /// chamber with a fixed spawn at the south end and a "centre" point
    /// the caller can drop a return-to-rift portal on.
    pub fn hub() -> Self {
        const SIZE: usize = 36; // grid is SIZE × SIZE walls
        const ROOM: usize = 30; // inner walkable square
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
            theme: RoomTheme::Generic,
            shape: RoomShape::Rectangular,
            // Force Sand for the hub regardless of the
            // synthetic room's `Generic` theme — every
            // tile in the hub is the desert platform.
            surface: Some(SurfaceKind::Sand),
        };
        // Player spawns near the south wall, facing the portal in centre.
        let spawn_pos = Vec3::new((pad + ROOM / 2) as f32, 0.5, (pad + ROOM - 3) as f32);
        let mut config = FloorConfig::for_floor(1);
        config.width = width;
        config.depth = depth;
        config.base_enemy_count = 0;
        config.enemies_per_floor = 0;
        config.packs_per_room = 0;
        config.mobs_per_pack = 0;
        let elevation = vec![0 as Elevation; width * depth];
        Self {
            width,
            depth,
            tiles,
            elevation,
            rooms: vec![room],
            spawn_pos,
            boss_room_center: Vec3::ZERO,
            portal_anchors: None,
            default_surface: SurfaceKind::Sand,
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

/// Carve a room's [`RoomShape`] back into the tile grid.
///
/// The room rectangle has already been turned into floor;
/// this function selectively re-walls interior or perimeter
/// tiles to produce the requested silhouette. Border tiles
/// (one tile in from each room edge) are never touched so
/// every shape preserves a continuous floor band along the
/// walls — that's where wall-prop placement and corridor
/// connections live, and breaking it would orphan whole
/// rooms.
fn apply_room_shape(tiles: &mut [Tile], width: usize, _depth: usize, room: &Room, seed: u64) {
    use rooms::RoomShape::*;
    // Tile-level helpers scoped to this room.
    let set_wall = |tiles: &mut [Tile], x: usize, z: usize| {
        if x < room.x || x >= room.x + room.width {
            return;
        }
        if z < room.z || z >= room.z + room.depth {
            return;
        }
        // Don't re-wall the perimeter: that's where wall
        // props attach and corridors connect.
        if x == room.x || x + 1 == room.x + room.width {
            return;
        }
        if z == room.z || z + 1 == room.z + room.depth {
            return;
        }
        tiles[z * width + x] = Tile::Wall;
    };

    match room.shape {
        Rectangular => {}

        Pillared => {
            // Regular grid of single-tile pillars in the
            // interior. Step is calibrated to leave one
            // floor tile of clearance between each pillar
            // (so a 3-tile-wide character path always fits
            // between any two pillars). Min 2-tile margin
            // from any wall keeps the colonnade reading as
            // "interior decoration" rather than "blocked
            // doorway".
            let step = 3;
            let margin = 2;
            let z0 = room.z + margin;
            let z1 = room.z + room.depth - margin;
            let x0 = room.x + margin;
            let x1 = room.x + room.width - margin;
            let mut z = z0;
            while z < z1 {
                let mut x = x0;
                while x < x1 {
                    set_wall(tiles, x, z);
                    x += step;
                }
                z += step;
            }
        }

        Alcoved => {
            // Cut the four corners back into 2x2 walls so
            // the room reads as octagonal, then carve a
            // single 2-wide alcove into each long side
            // facing inward. A 2x2 corner cut is the
            // cheapest way to break "rectangular" silhouette
            // and gives wall-prop placement the alcove
            // niches it needs.
            let cw = 2.min(room.width / 4);
            let cd = 2.min(room.depth / 4);
            for dz in 0..cd {
                for dx in 0..cw {
                    set_wall(tiles, room.x + 1 + dx, room.z + 1 + dz);
                    set_wall(tiles, room.x + room.width - 2 - dx, room.z + 1 + dz);
                    set_wall(tiles, room.x + 1 + dx, room.z + room.depth - 2 - dz);
                    set_wall(
                        tiles,
                        room.x + room.width - 2 - dx,
                        room.z + room.depth - 2 - dz,
                    );
                }
            }
        }

        Cross => {
            // Wall off all four corners as larger blocks so
            // the remaining floor is a plus / cross shape.
            // Block size scales with the shorter dimension
            // so the cross arms always have room for the
            // 3-wide corridor entry.
            let arm_pad = (room.width.min(room.depth) / 3).max(2);
            for dz in 0..arm_pad {
                for dx in 0..arm_pad {
                    let x = room.x + 1 + dx;
                    let z = room.z + 1 + dz;
                    set_wall(tiles, x, z);
                    set_wall(tiles, room.x + room.width - 2 - dx, z);
                    set_wall(tiles, x, room.z + room.depth - 2 - dz);
                    set_wall(
                        tiles,
                        room.x + room.width - 2 - dx,
                        room.z + room.depth - 2 - dz,
                    );
                }
            }
        }

        Round => {
            // Stepped corner cuts that approximate a circle.
            // Inset = corner-tile distance from the diagonal.
            // Tiles whose distance from the room's axis-
            // aligned corner is less than `inset - manhattan`
            // get walled.
            let r = (room.width.min(room.depth) / 2) as isize;
            let cx = (room.x + room.width / 2) as isize;
            let cz = (room.z + room.depth / 2) as isize;
            // Walk a 1-tile-thick interior ring and any
            // tile whose Chebyshev distance to centre
            // exceeds `r-1` becomes a wall (that's the
            // outer corner band of the rectangle).
            for z in (room.z + 1)..(room.z + room.depth - 1) {
                for x in (room.x + 1)..(room.x + room.width - 1) {
                    let dx = (x as isize - cx).abs();
                    let dz = (z as isize - cz).abs();
                    // Approximate "outside the inscribed
                    // circle" by squared distance against r².
                    if dx * dx + dz * dz > (r - 1) * (r - 1) {
                        set_wall(tiles, x, z);
                    }
                }
            }
            // Touch the seed so a future variation can
            // randomise the radius without compiler warnings.
            let _ = seed;
        }
    }
}

/// Apply a per-room elevation feature (raised dais, sunken
/// pit, balcony band). Mutates both the tile grid (placing
/// stair tiles where ramps connect levels) and the elevation
/// grid. Runs after [`apply_room_shape`] so it observes the
/// final wall layout and never tries to stair into a wall
/// tile.
///
/// Selection rules:
/// * **BossRoom + Shrine** → raised central dais (climactic
///   "approach the altar" silhouette).
/// * **Crypt** → sunken central pit (oubliette feel).
/// * **Throne / Library** with enough size → ring balcony
///   along two opposite walls. (Phase 2 keeps it simple:
///   only the rooms tagged Library or Shrine boss room get
///   it, since wider applicability would need theme-aware
///   prop blocking that we don't have yet.)
/// * Everything else stays flat.
fn apply_room_elevation(
    tiles: &mut [Tile],
    elevation: &mut [Elevation],
    width: usize,
    _depth: usize,
    room: &Room,
    seed: u64,
) {
    use rooms::RoomTheme::*;

    let min_dim = room.width.min(room.depth);
    if min_dim < 6 {
        // Too small for any elevation feature to read as
        // intentional rather than as a navigation hazard.
        return;
    }

    // Per-room RNG so feature parameters (which side the
    // stairs go on, etc) decorrelate from theme assignment.
    let room_seed = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(room.x as u64)
        .wrapping_mul(0x100_0000_01b3)
        .wrapping_add(room.z as u64);
    let mut rng = SimpleRng::new(room_seed);

    let raise_inner =
        |tiles: &mut [Tile], elevation: &mut [Elevation], inset: usize, level: Elevation| {
            // Inset = how many tiles in from the wall the raised
            // region starts. The ring of tiles between the wall
            // and the raised region remains at base elevation
            // (so the player walks around the dais), and we
            // place stair tiles on a random subset of the dais's
            // border so the dais is reachable.
            let x0 = room.x + inset;
            let x1 = room.x + room.width - inset;
            let z0 = room.z + inset;
            let z1 = room.z + room.depth - inset;
            if x1 <= x0 + 1 || z1 <= z0 + 1 {
                return;
            }
            for z in z0..z1 {
                for x in x0..x1 {
                    let i = z * width + x;
                    if matches!(tiles[i], Tile::Floor) {
                        elevation[i] = level;
                    }
                }
            }
        };

    let lower_inner =
        |tiles: &mut [Tile], elevation: &mut [Elevation], inset: usize, level: Elevation| {
            let x0 = room.x + inset;
            let x1 = room.x + room.width - inset;
            let z0 = room.z + inset;
            let z1 = room.z + room.depth - inset;
            if x1 <= x0 + 1 || z1 <= z0 + 1 {
                return;
            }
            for z in z0..z1 {
                for x in x0..x1 {
                    let i = z * width + x;
                    if matches!(tiles[i], Tile::Floor) {
                        elevation[i] = level;
                    }
                }
            }
        };

    // Place a stair tile at `(sx, sz)` ascending in `dir`,
    // anchored to elevation `low`. We only place it if the
    // tile is currently a flat floor at `low + 1` adjacent
    // to a flat floor at `low`, which is the only place a
    // ramp belongs.
    let place_stair = |tiles: &mut [Tile],
                       elevation: &mut [Elevation],
                       sx: usize,
                       sz: usize,
                       dir: StairDir,
                       low: Elevation| {
        if sx >= width {
            return;
        }
        let i = sz * width + sx;
        if !matches!(tiles[i], Tile::Floor) {
            return;
        }
        // Set the stair tile to live at the *low* elevation;
        // the ramp's surface rises across it.
        tiles[i] = Tile::Stair { dir };
        elevation[i] = low;
    };

    match (room.room_type, room.theme) {
        // ---- Raised central dais (boss-room shrine) ----
        (RoomType::BossRoom, _) | (_, Shrine) if min_dim >= 7 => {
            // Square 3-or-5 wide dais at the centre, raised
            // 1 step. Stairs face whichever side has the
            // most clearance (cheap heuristic: pick a random
            // side from the seed and trust the room is
            // square-ish).
            let inset = if min_dim >= 9 { 2 } else { 2 };
            raise_inner(tiles, elevation, inset, 1);

            // Connect the dais to the surrounding floor with
            // ramp tiles on one cardinal side. The ramp tile
            // sits on the *border* row between low floor and
            // raised dais, ascending into the dais.
            let side = (rng.next() as usize) & 3;
            let cx = room.x + room.width / 2;
            let cz = room.z + room.depth / 2;
            match side {
                0 => place_stair(tiles, elevation, cx, room.z + inset - 1, StairDir::PosZ, 0),
                1 => place_stair(
                    tiles,
                    elevation,
                    cx,
                    room.z + room.depth - inset,
                    StairDir::NegZ,
                    0,
                ),
                2 => place_stair(tiles, elevation, room.x + inset - 1, cz, StairDir::PosX, 0),
                _ => place_stair(
                    tiles,
                    elevation,
                    room.x + room.width - inset,
                    cz,
                    StairDir::NegX,
                    0,
                ),
            }
        }

        // ---- Sunken pit (crypt arenas) ----
        (RoomType::Arena, Crypt) if min_dim >= 7 => {
            let inset = 2;
            lower_inner(tiles, elevation, inset, -1);

            // Two ramps on opposite sides so the player can
            // enter from either approach without committing
            // to a one-way drop.
            let cx = room.x + room.width / 2;
            let cz = room.z + room.depth / 2;
            // Ramps descend into the pit. The ramp tile is
            // the perimeter floor adjacent to the lowered
            // region; it ascends back *up* to elevation 0
            // when walking outward, which we model by
            // setting low=-1 and direction pointing *out*
            // of the pit.
            let _ = cz;
            place_stair(tiles, elevation, cx, room.z + inset - 1, StairDir::NegZ, -1);
            place_stair(
                tiles,
                elevation,
                cx,
                room.z + room.depth - inset,
                StairDir::PosZ,
                -1,
            );
        }

        // ---- Balcony band (libraries) ----
        (_, Library) if min_dim >= 9 => {
            // 2-tile wide raised perimeter on the long axis
            // only, leaving the centre + short-axis
            // approach floors at base elevation. Reads as a
            // mezzanine library where the floor is reading
            // tables and the raised side rows are the
            // bookshelves.
            let band_w = 2;
            if room.width >= room.depth {
                // Long axis is X — band runs the full width
                // along ±Z edges.
                for x in (room.x + 1)..(room.x + room.width - 1) {
                    for dz in 0..band_w {
                        let zlo = room.z + 1 + dz;
                        let zhi = room.z + room.depth - 2 - dz;
                        let ilo = zlo * width + x;
                        let ihi = zhi * width + x;
                        if matches!(tiles[ilo], Tile::Floor) {
                            elevation[ilo] = 1;
                        }
                        if matches!(tiles[ihi], Tile::Floor) {
                            elevation[ihi] = 1;
                        }
                    }
                }
                // One ramp on each side, near the centre,
                // pointing inward toward the bookshelves.
                let cx = room.x + room.width / 2;
                place_stair(tiles, elevation, cx, room.z + 1 + band_w, StairDir::NegZ, 0);
                place_stair(
                    tiles,
                    elevation,
                    cx,
                    room.z + room.depth - 2 - band_w,
                    StairDir::PosZ,
                    0,
                );
            } else {
                for z in (room.z + 1)..(room.z + room.depth - 1) {
                    for dx in 0..band_w {
                        let xlo = room.x + 1 + dx;
                        let xhi = room.x + room.width - 2 - dx;
                        let ilo = z * width + xlo;
                        let ihi = z * width + xhi;
                        if matches!(tiles[ilo], Tile::Floor) {
                            elevation[ilo] = 1;
                        }
                        if matches!(tiles[ihi], Tile::Floor) {
                            elevation[ihi] = 1;
                        }
                    }
                }
                let cz = room.z + room.depth / 2;
                place_stair(tiles, elevation, room.x + 1 + band_w, cz, StairDir::NegX, 0);
                place_stair(
                    tiles,
                    elevation,
                    room.x + room.width - 2 - band_w,
                    cz,
                    StairDir::PosX,
                    0,
                );
            }
        }

        _ => {}
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
