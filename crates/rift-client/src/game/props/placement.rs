//! Shared placement primitives + generic decorators.
//!
//! All prop packs use the same two strategies:
//!
//! 1. **Wall placement** — pick from a list of (floor tile, wall_dir)
//!    candidates and spawn one prop at each (or a sampled subset).
//!    Used by the fantasy dungeon (per-room with a count) and by the
//!    nature hub forest border (one per wall tile).
//! 2. **Scatter** — pick `count` random tiles from a list and spawn
//!    a free-standing prop at a sub-tile-jittered position with
//!    spacing + avoid-radius constraints.
//!
//! Both are driven by a small config struct so per-pack `decorate_*`
//! functions become a couple of declarative calls.

use glam::Vec3;
use rift_engine::dungeon::Tile;
use rift_engine::{Floor, Renderer};

use super::{PlacementHint, PropAsset, Props};

// ---------------------------------------------------------------------
// RNG
// ---------------------------------------------------------------------

/// Tiny seeded xorshift64. Deterministic per floor seed.
pub struct SmallRng {
    state: u64,
}

impl SmallRng {
    pub fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed },
        }
    }
    pub fn next(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }
    pub fn range(&mut self, lo: u32, hi: u32) -> u32 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next() % (hi - lo) as u64) as u32
    }
    /// Uniform random in `[lo, hi)`.
    pub fn frange(&mut self, lo: f32, hi: f32) -> f32 {
        if hi <= lo {
            return lo;
        }
        let t = (self.next() & 0xFFFF) as f32 / 65536.0;
        lo + (hi - lo) * t
    }
}

// ---------------------------------------------------------------------
// Tile classification + helpers
// ---------------------------------------------------------------------

/// `(tx, tz, wall_dir)` triple — a floor tile and the (ox,oz) offset
/// to its adjacent wall tile.
pub type WallTile = (i32, i32, (i32, i32));

const WALL_DIRS: [(i32, i32); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];

/// First wall direction (in scan order) from `(tx, tz)`, or `None` if
/// the tile is fully interior.
pub fn wall_dir_at(floor: &Floor, tx: i32, tz: i32) -> Option<(i32, i32)> {
    for &(ox, oz) in &WALL_DIRS {
        let nx = tx + ox;
        let nz = tz + oz;
        if nx < 0 || nz < 0 {
            continue;
        }
        if floor.get(nx as usize, nz as usize) == Tile::Wall {
            return Some((ox, oz));
        }
    }
    None
}

/// Split every walkable tile in `floor` into wall-adjacent and
/// interior groups.
pub fn collect_floor_tiles(floor: &Floor) -> (Vec<WallTile>, Vec<(i32, i32)>) {
    let mut border = Vec::new();
    let mut interior = Vec::new();
    for tz in 0..floor.depth as i32 {
        for tx in 0..floor.width as i32 {
            if floor.get(tx as usize, tz as usize) != Tile::Floor {
                continue;
            }
            match wall_dir_at(floor, tx, tz) {
                Some(d) => border.push((tx, tz, d)),
                None => interior.push((tx, tz)),
            }
        }
    }
    (border, interior)
}

/// Wall-adjacent floor tiles within the bounding box of `room`.
pub fn collect_room_wall_tiles(
    floor: &Floor,
    room: &rift_engine::dungeon::Room,
) -> Vec<WallTile> {
    let mut out = Vec::new();
    for dx in 0..room.width as i32 {
        for dz in 0..room.depth as i32 {
            let tx = room.x as i32 + dx;
            let tz = room.z as i32 + dz;
            if tx < 0 || tz < 0 {
                continue;
            }
            if floor.get(tx as usize, tz as usize) != Tile::Floor {
                continue;
            }
            if let Some(d) = wall_dir_at(floor, tx, tz) {
                out.push((tx, tz, d));
            }
        }
    }
    out
}

/// World position at the centre of grid tile `(tx, tz)`.
pub fn tile_centre(tx: i32, tz: i32) -> Vec3 {
    Vec3::new(tx as f32, 0.0, tz as f32)
}

/// `true` when tile `(tx, tz)` is a walkable floor tile whose
/// 4-cardinal neighbours all share the same authored elevation
/// (or are out-of-bounds / walls). Used to gate prop spawns so
/// barrels, candles, statues, etc. only land on flat plateaus
/// — never on the lip of a raised dais, the crest of a sunken
/// pit, or a stair tile (whose surface is sloped). Without
/// this gate a prop placed at the *centre* of an edge tile
/// reads as "lifted to the right Y", but its rendered footprint
/// straddles the elevation step and visually clips into / hovers
/// over the adjacent dais wall skirt.
fn is_flat_plateau(floor: &Floor, tx: i32, tz: i32) -> bool {
    if tx < 0 || tz < 0 {
        return false;
    }
    let (ix, iz) = (tx as usize, tz as usize);
    if ix >= floor.width || iz >= floor.depth {
        return false;
    }
    let i = iz * floor.width + ix;
    // Stair tiles are sloped surfaces — props on them would
    // tilt or hover. Always reject.
    if !matches!(floor.tiles[i], Tile::Floor) {
        return false;
    }
    let my_elev = floor.elevation[i];
    for &(ox, oz) in &WALL_DIRS {
        let nx = tx + ox;
        let nz = tz + oz;
        if nx < 0 || nz < 0 {
            continue;
        }
        let (nix, niz) = (nx as usize, nz as usize);
        if nix >= floor.width || niz >= floor.depth {
            continue;
        }
        let ni = niz * floor.width + nix;
        match floor.tiles[ni] {
            // Walls are fine — the prop's wall-side edge is
            // already snapped flush in `Props::spawn`.
            Tile::Wall => {}
            // A neighbour stair leans up or down from this
            // tile; placing a prop here puts its footprint
            // on the lip of the ramp.
            Tile::Stair { .. } => return false,
            Tile::Floor => {
                if floor.elevation[ni] != my_elev {
                    return false;
                }
            }
        }
    }
    true
}

/// Pick one asset from `assets` by `weight`.
pub fn weighted_pick<'a>(assets: &'a [PropAsset], rng: &mut SmallRng) -> &'a PropAsset {
    let total: u32 = assets.iter().map(|a| a.weight).sum();
    if total == 0 || assets.is_empty() {
        return &assets[0];
    }
    let mut pick = rng.range(0, total);
    for a in assets {
        if pick < a.weight {
            return a;
        }
        pick -= a.weight;
    }
    &assets[0]
}

// ---------------------------------------------------------------------
// Generic decorators
// ---------------------------------------------------------------------

/// Where the wall-placed prop's anchor sits.
#[derive(Clone, Copy, Debug)]
pub enum WallAnchor {
    /// Anchor at the floor tile centre. The wall-snap math in
    /// `Props::spawn` then pushes the prop's back face to the wall.
    /// Used by interior dungeon furniture.
    OnFloorTile,
    /// Anchor at the neighbouring wall tile centre — i.e. *where the
    /// wall used to be*. Used for the hub's forest border (the wall
    /// is gone; trees replace it).
    OnWallTile,
}

/// Config for [`place_on_walls`].
pub struct WallPlacement<'a> {
    pub assets: &'a [PropAsset],
    /// `Some(n)` = sample `n` candidates; `None` = one prop per candidate.
    pub count: Option<usize>,
    pub anchor: WallAnchor,
    /// Random jitter (degrees) added to the wall-aligned yaw of
    /// `PlacementHint::WallAligned` assets. Free-hint assets always
    /// get a fully random yaw regardless.
    pub wall_yaw_jitter_deg: f32,
    /// Don't place if any other prop sits within this distance.
    pub min_spacing: f32,
    /// `(point, radius)` exclusion zones (e.g. spawn, room centre).
    pub avoid: &'a [(Vec3, f32)],
    /// Per-instance scale multiplier range. `(1.0, 1.0)` = use the
    /// asset's authored scale exactly.
    pub scale_jitter: (f32, f32),
}

/// Spawn props along the wall-edge `candidates`. Consumes the vec.
///
/// `floor` is consulted per-spawn to lift the prop's Y to the
/// tile's authored elevation (raised daises, sunken pits) so a
/// prop lands on the visible floor surface rather than always
/// at y=0. Without this, props placed inside a pit or on a
/// dais hover above (or sink into) the slab.
pub fn place_on_walls(
    props: &mut Props,
    world: &mut hecs::World,
    renderer: &mut Renderer,
    floor: &Floor,
    mut candidates: Vec<WallTile>,
    rng: &mut SmallRng,
    p: &WallPlacement,
) {
    candidates.retain(|(tx, tz, _)| {
        // Reject tiles that border an elevation step or stair
        // — props placed on the dais lip end up half-buried
        // in the dais skirt or hovering over the lower floor.
        if !is_flat_plateau(floor, *tx, *tz) {
            return false;
        }
        let pos = tile_centre(*tx, *tz);
        !p.avoid.iter().any(|(c, r)| pos.distance(*c) < *r)
    });

    let target = p.count.unwrap_or(candidates.len());
    let mut placed: Vec<Vec3> = Vec::new();
    let min_sq = p.min_spacing * p.min_spacing;

    for _ in 0..target {
        if candidates.is_empty() {
            break;
        }
        let i = (rng.next() as usize) % candidates.len();
        let (tx, tz, wall_dir) = candidates.swap_remove(i);
        let (ox, oz) = wall_dir;

        let mut pos = match p.anchor {
            WallAnchor::OnFloorTile => tile_centre(tx, tz),
            WallAnchor::OnWallTile => tile_centre(tx + ox, tz + oz),
        };
        // Lift to the tile's authored elevation. For
        // `OnFloorTile` we sample the floor tile itself;
        // for `OnWallTile` (hub forest border) the wall
        // tile reports y=0, so we sample the *adjacent*
        // floor tile instead so trees that replace the
        // wall sit on the same step as the floor they
        // border.
        let (sx, sz) = match p.anchor {
            WallAnchor::OnFloorTile => (pos.x, pos.z),
            WallAnchor::OnWallTile => (tx as f32, tz as f32),
        };
        pos.y = floor.tile_floor_y_at(sx, sz);

        if p.min_spacing > 0.0
            && placed.iter().any(|q| q.distance_squared(pos) < min_sq)
        {
            continue;
        }

        let asset = weighted_pick(p.assets, rng);
        let yaw = match asset.placement {
            PlacementHint::WallAligned => {
                let face = Vec3::new(-ox as f32, 0.0, -oz as f32);
                let base = face.x.atan2(face.z);
                let j = p.wall_yaw_jitter_deg;
                base + rng.frange(-j, j).to_radians()
            }
            PlacementHint::Free => rng.frange(0.0, std::f32::consts::TAU),
        };
        let scale = asset.scale * rng.frange(p.scale_jitter.0, p.scale_jitter.1);

        props.spawn(world, renderer, asset, pos, yaw, wall_dir, Some(scale));
        placed.push(pos);
    }
}

/// Config for [`scatter_on_tiles`].
pub struct ScatterPlacement<'a> {
    pub assets: &'a [PropAsset],
    pub count: usize,
    pub min_spacing: f32,
    pub avoid: &'a [(Vec3, f32)],
    /// Half-range of the per-prop sub-tile XZ offset (0.0 = exact tile centre).
    pub sub_tile_jitter: f32,
    pub scale_jitter: (f32, f32),
}

/// Scatter free-standing props across `tiles`.
///
/// `floor` is sampled per-spawn to lift the prop's Y to the
/// tile's authored elevation — see [`place_on_walls`].
pub fn scatter_on_tiles(
    props: &mut Props,
    world: &mut hecs::World,
    renderer: &mut Renderer,
    floor: &Floor,
    tiles: &[(i32, i32)],
    rng: &mut SmallRng,
    p: &ScatterPlacement,
) {
    if tiles.is_empty() {
        return;
    }
    let mut placed: Vec<Vec3> = Vec::new();
    let min_sq = p.min_spacing * p.min_spacing;
    let j = p.sub_tile_jitter;

    for _ in 0..p.count {
        let (tx, tz) = tiles[(rng.next() as usize) % tiles.len()];
        // Same flat-plateau gate as `place_on_walls`: no
        // scatter on dais lips, pit edges, or stair tiles.
        if !is_flat_plateau(floor, tx, tz) {
            continue;
        }
        let mut pos = Vec3::new(
            tx as f32 + rng.frange(-j, j),
            0.0,
            tz as f32 + rng.frange(-j, j),
        );
        // Sample the elevation under the (jittered) world
        // position so a scatter that crosses a tile
        // boundary still lands on the right step.
        pos.y = floor.tile_floor_y_at(pos.x, pos.z);
        if p.avoid.iter().any(|(c, r)| pos.distance(*c) < *r) {
            continue;
        }
        if p.min_spacing > 0.0
            && placed.iter().any(|q| q.distance_squared(pos) < min_sq)
        {
            continue;
        }
        let asset = weighted_pick(p.assets, rng);
        let yaw = rng.frange(0.0, std::f32::consts::TAU);
        let scale = asset.scale * rng.frange(p.scale_jitter.0, p.scale_jitter.1);
        props.spawn(world, renderer, asset, pos, yaw, (0, 0), Some(scale));
        placed.push(pos);
    }
}
