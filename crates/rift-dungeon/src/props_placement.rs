//! Deterministic per-floor prop placement.
//!
//! Lives here (rather than client-side) so the server's
//! authoritative kinematic integration can collide the player
//! against the same props the client renders. Both sides
//! re-run this code from `(floor_seed, floor_index)` and
//! produce identical [`PlacedProp`] vectors — no replication
//! traffic needed.
//!
//! The algorithm is split into two phases per room:
//!
//! 1. **Centerpiece + flankers** — at the room's centre,
//!    one big prop (cauldron, anvil, bookstand) plus a pair
//!    of flankers offset along its right axis. Skipped on
//!    boss rooms (the arena needs a clear pivot) and on
//!    rooms below `min_centerpiece_area`.
//! 2. **Wall scatter** — `count` weighted-random picks from
//!    the theme palette placed at room-wall-adjacent floor
//!    tiles, with `min_spacing` rejection sampling.
//!
//! The hub uses a single `scatter` phase across interior
//! tiles for grass / pebbles, plus a fixed-position
//! [`PropId::StashChest`].
//!
//! Visual concerns (gltf paths, materials, render-time
//! bbox-centre offsets, wall-snap push) live entirely on
//! the client — see `crates/rift-client/src/game/props/`.

use glam::Vec3;

use crate::props::{meta, PlacedProp, PlacementHint, PropId};
use crate::rooms::{Room, RoomTheme, RoomType};
use crate::{Floor, Tile};

// =====================================================================
// RNG
// =====================================================================

/// Tiny seeded xorshift64. Identical to the one the client
/// previously used (moved here verbatim) so determinism is
/// preserved across the refactor — a floor seeded with
/// `(seed, index)` produces the same prop layout pre- and
/// post-refactor.
pub struct SmallRng {
    state: u64,
}

impl SmallRng {
    pub fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0x9E37_79B9_7F4A_7C15
            } else {
                seed
            },
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
    pub fn frange(&mut self, lo: f32, hi: f32) -> f32 {
        if hi <= lo {
            return lo;
        }
        let t = (self.next() & 0xFFFF) as f32 / 65536.0;
        lo + (hi - lo) * t
    }
}

// =====================================================================
// Tile classification
// =====================================================================

/// `(tx, tz, wall_dir)`: floor tile + (ox, oz) toward adjacent
/// wall tile.
pub type WallTile = (i32, i32, (i32, i32));

const WALL_DIRS: [(i32, i32); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];

fn wall_dir_at(floor: &Floor, tx: i32, tz: i32) -> Option<(i32, i32)> {
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

fn collect_room_wall_tiles(floor: &Floor, room: &Room) -> Vec<WallTile> {
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

pub fn tile_centre(tx: i32, tz: i32) -> Vec3 {
    Vec3::new(tx as f32, 0.0, tz as f32)
}

/// Walkable tile whose 4-cardinal neighbours all share the
/// same authored elevation. Props placed on the lip of an
/// elevation step otherwise visually clip into / hover over
/// the adjacent dais skirt.
fn is_flat_plateau(floor: &Floor, tx: i32, tz: i32) -> bool {
    if tx < 0 || tz < 0 {
        return false;
    }
    let (ix, iz) = (tx as usize, tz as usize);
    if ix >= floor.width || iz >= floor.depth {
        return false;
    }
    let i = iz * floor.width + ix;
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
            Tile::Wall => {}
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

// =====================================================================
// Palette tables
// =====================================================================

/// Weighted catalog entry — `(id, weight)`.
type PaletteEntry = (PropId, u32);

/// One theme's prop palette + placement parameters. Identical
/// shape to the client's old `ThemePalette`, but parameterised
/// over [`PropId`] instead of gltf-bearing `PropAsset`.
struct ThemePalette {
    assets: &'static [PaletteEntry],
    density: f32,
    min_count: usize,
    max_count: usize,
    min_spacing: f32,
    centerpiece: Option<PropId>,
    flanker: Option<PropId>,
    min_centerpiece_area: usize,
}

fn weighted_pick(assets: &[PaletteEntry], rng: &mut SmallRng) -> PropId {
    let total: u32 = assets.iter().map(|(_, w)| *w).sum();
    if total == 0 || assets.is_empty() {
        return assets[0].0;
    }
    let mut pick = rng.range(0, total);
    for &(id, w) in assets {
        if pick < w {
            return id;
        }
        pick -= w;
    }
    assets[0].0
}

use PropId::*;

// Palettes deliberately kept verbatim from the previous
// client-side tables (asset selection, weights, density,
// spacing, centerpiece + flanker pairs). The only change is
// the asset reference: `PropAsset` constants → `PropId`s.

const CRYPT_PALETTE: ThemePalette = ThemePalette {
    assets: &[
        (Cauldron, 4),
        (CandleStickTriple, 3),
        (ChainCoil, 2),
        (CageSmall, 2),
        (Banner1Cloth, 2),
        (Banner2Cloth, 2),
        (Bottle1, 1),
    ],
    density: 0.04,
    min_count: 2,
    max_count: 6,
    min_spacing: 2.4,
    centerpiece: Some(Cauldron),
    flanker: Some(CandleStickTriple),
    min_centerpiece_area: 24,
};

const BARRACKS_PALETTE: ThemePalette = ThemePalette {
    assets: &[
        (BedTwin1, 4),
        (BedTwin2, 4),
        (Bench, 3),
        (Barrel, 3),
        (Anvil, 2),
        (AnvilLog, 2),
        (AxeBronze, 1),
        (Bag, 2),
        (Banner1, 2),
        (Banner2, 2),
    ],
    density: 0.07,
    min_count: 3,
    max_count: 9,
    min_spacing: 1.7,
    centerpiece: Some(Anvil),
    flanker: Some(AnvilLog),
    min_centerpiece_area: 28,
};

const LIBRARY_PALETTE: ThemePalette = ThemePalette {
    assets: &[
        (Bookcase2, 6),
        (Cabinet, 3),
        (Bench, 2),
        (BookStand, 2),
        (BookStack1, 2),
        (BookStack2, 2),
        (BookGroupMedium1, 1),
        (BookGroupMedium2, 1),
        (BookGroupMedium3, 1),
        (BookGroupSmall1, 1),
        (CandleStick, 2),
        (CandleStickTriple, 1),
    ],
    density: 0.09,
    min_count: 4,
    max_count: 12,
    min_spacing: 1.4,
    centerpiece: Some(BookStand),
    flanker: Some(CandleStick),
    min_centerpiece_area: 28,
};

const SHRINE_PALETTE: ThemePalette = ThemePalette {
    assets: &[
        (CandleStickTriple, 6),
        (CandleStick, 4),
        (Cauldron, 3),
        (Bench, 3),
        (Banner1Cloth, 3),
        (Banner2Cloth, 3),
        (Candle1, 2),
        (Candle2, 2),
    ],
    density: 0.06,
    min_count: 4,
    max_count: 10,
    min_spacing: 2.0,
    centerpiece: Some(Cauldron),
    flanker: Some(CandleStickTriple),
    min_centerpiece_area: 30,
};

const STORAGE_PALETTE: ThemePalette = ThemePalette {
    assets: &[
        (Barrel, 6),
        (BarrelApples, 4),
        (BarrelHolder, 3),
        (Cabinet, 2),
        (BucketWooden1, 2),
        (BucketMetal, 2),
        (CageSmall, 1),
        (Bag, 3),
        (Carrot, 1),
    ],
    density: 0.10,
    min_count: 4,
    max_count: 14,
    min_spacing: 1.3,
    centerpiece: Some(BarrelHolder),
    flanker: Some(Barrel),
    min_centerpiece_area: 24,
};

const PRISON_PALETTE: ThemePalette = ThemePalette {
    assets: &[
        (CageSmall, 6),
        (ChainCoil, 4),
        (BedTwin1, 3),
        (BucketWooden1, 3),
        (BucketMetal, 2),
        (Bag, 2),
        (Banner1Cloth, 1),
    ],
    density: 0.06,
    min_count: 3,
    max_count: 8,
    min_spacing: 1.8,
    centerpiece: Some(CageSmall),
    flanker: Some(ChainCoil),
    min_centerpiece_area: 24,
};

const GENERIC_PALETTE: ThemePalette = ThemePalette {
    assets: &[(Barrel, 3), (Bench, 2), (BucketWooden1, 2), (Bag, 1)],
    density: 0.03,
    min_count: 1,
    max_count: 4,
    min_spacing: 2.2,
    centerpiece: None,
    flanker: None,
    min_centerpiece_area: 0,
};

fn palette_for(theme: RoomTheme) -> &'static ThemePalette {
    match theme {
        RoomTheme::Crypt => &CRYPT_PALETTE,
        RoomTheme::Barracks => &BARRACKS_PALETTE,
        RoomTheme::Library => &LIBRARY_PALETTE,
        RoomTheme::Shrine => &SHRINE_PALETTE,
        RoomTheme::Storage => &STORAGE_PALETTE,
        RoomTheme::Prison => &PRISON_PALETTE,
        RoomTheme::Generic => &GENERIC_PALETTE,
    }
}

// =====================================================================
// Per-room placement
// =====================================================================

/// Wall-aligned yaw with small jitter (degrees).
const WALL_YAW_JITTER_DEG: f32 = 10.0;

fn wall_aligned_yaw(rng: &mut SmallRng, ox: i32, oz: i32) -> f32 {
    let face = Vec3::new(-ox as f32, 0.0, -oz as f32);
    let base = face.x.atan2(face.z);
    base + rng
        .frange(-WALL_YAW_JITTER_DEG, WALL_YAW_JITTER_DEG)
        .to_radians()
}

/// Place props along `candidates` (room wall tiles).
fn place_on_walls(
    floor: &Floor,
    palette: &ThemePalette,
    mut candidates: Vec<WallTile>,
    avoid: &[(Vec3, f32)],
    target: usize,
    rng: &mut SmallRng,
    out: &mut Vec<PlacedProp>,
) {
    candidates.retain(|(tx, tz, _)| {
        if !is_flat_plateau(floor, *tx, *tz) {
            return false;
        }
        let pos = tile_centre(*tx, *tz);
        !avoid.iter().any(|(c, r)| pos.distance(*c) < *r)
    });

    let mut placed: Vec<Vec3> = Vec::new();
    let min_sq = palette.min_spacing * palette.min_spacing;

    for _ in 0..target {
        if candidates.is_empty() {
            break;
        }
        let i = (rng.next() as usize) % candidates.len();
        let (tx, tz, (ox, oz)) = candidates.swap_remove(i);

        let mut pos = tile_centre(tx, tz);
        pos.y = floor.tile_floor_y_at(pos.x, pos.z);

        if palette.min_spacing > 0.0 && placed.iter().any(|q| q.distance_squared(pos) < min_sq) {
            continue;
        }

        let id = weighted_pick(palette.assets, rng);
        let yaw = match meta(id).placement {
            PlacementHint::WallAligned => wall_aligned_yaw(rng, ox, oz),
            PlacementHint::Free => rng.frange(0.0, std::f32::consts::TAU),
        };
        // Track wall_dir only for actually wall-aligned props
        // — free-standing picks from the same palette (e.g.
        // a candlestick rolled into a wall slot) read better
        // standing slightly off the wall than snapped flush.
        let wall_dir = if matches!(meta(id).placement, PlacementHint::WallAligned) {
            Some((ox as i8, oz as i8))
        } else {
            None
        };
        out.push(PlacedProp {
            id,
            pos,
            yaw,
            scale: 1.0,
            wall_dir,
            light: false,
        });
        placed.push(pos);
    }
}

/// Decorate one room with theme-appropriate props.
fn decorate_room(floor: &Floor, room: &Room, rng: &mut SmallRng, out: &mut Vec<PlacedProp>) {
    match room.room_type {
        RoomType::Arena | RoomType::BossRoom | RoomType::PortalRoom => {}
        RoomType::Corridor => return,
    }

    let palette = palette_for(room.theme);

    // ---- Centerpiece ----
    let mut centre_yaw = 0.0_f32;
    let allow_centerpiece = room.room_type != RoomType::BossRoom;
    let centerpiece_spawned = if let (true, Some(id)) = (allow_centerpiece, palette.centerpiece) {
        if room.area() >= palette.min_centerpiece_area
            && floor.spawn_pos.distance(room.center_world()) > 4.0
        {
            let yaw_quad = (rng.next() as usize) & 3;
            centre_yaw = std::f32::consts::FRAC_PI_2 * yaw_quad as f32;
            let mut centre_pos = room.center_world();
            centre_pos.y = floor.tile_floor_y_at(centre_pos.x, centre_pos.z);
            out.push(PlacedProp {
                id,
                pos: centre_pos,
                yaw: centre_yaw,
                scale: 1.0,
                wall_dir: None,
                light: false,
            });
            true
        } else {
            false
        }
    } else {
        false
    };

    // ---- Flankers ----
    if centerpiece_spawned {
        if let Some(id) = palette.flanker {
            let (sin_y, cos_y) = centre_yaw.sin_cos();
            let right = Vec3::new(cos_y, 0.0, -sin_y);
            let centre = room.center_world();
            for sign in [-1.0_f32, 1.0_f32] {
                let mut flanker_pos = centre + right * (1.5 * sign);
                let gx = flanker_pos.x.round() as i32;
                let gz = flanker_pos.z.round() as i32;
                if gx < 0 || gz < 0 {
                    continue;
                }
                if floor.get(gx as usize, gz as usize) != Tile::Floor {
                    continue;
                }
                flanker_pos.y = floor.tile_floor_y_at(flanker_pos.x, flanker_pos.z);
                out.push(PlacedProp {
                    id,
                    pos: flanker_pos,
                    yaw: centre_yaw,
                    scale: 1.0,
                    wall_dir: None,
                    light: false,
                });
            }
        }
    }

    // ---- Wall scatter ----
    let area_props = (room.area() as f32 * palette.density) as usize;
    let count = area_props.clamp(palette.min_count, palette.max_count);

    let centre_clear = if centerpiece_spawned {
        2.8
    } else if room.room_type == RoomType::BossRoom {
        3.5
    } else {
        2.5
    };
    let avoid: [(Vec3, f32); 2] = [(room.center_world(), centre_clear), (floor.spawn_pos, 4.5)];

    place_on_walls(
        floor,
        palette,
        collect_room_wall_tiles(floor, room),
        &avoid,
        count,
        rng,
        out,
    );
}

// =====================================================================
// Public entry points
// =====================================================================

/// Compute the deterministic prop placement for a generated
/// rift floor. Called from [`Floor::generate`].
pub fn decorate_rift(floor: &Floor, seed: u64) -> Vec<PlacedProp> {
    let mut rng = SmallRng::new(seed.wrapping_add(0xC1A0_5EED));
    let mut out = Vec::new();
    // Rooms borrowed by reference — no clone — but we iterate
    // the slice and copy each `Room` since `decorate_room`
    // needs an immutable borrow. The Room itself is small
    // (`Copy`-cheap).
    let rooms_snapshot: Vec<Room> = floor.rooms.clone();
    for room in &rooms_snapshot {
        decorate_room(floor, room, &mut rng, &mut out);
    }
    place_torches(floor, &mut out, seed);
    out
}

/// Compute the deterministic prop placement for the hub.
/// Called from [`Floor::hub`]. The hub gets a stash chest at
/// the player's spawn-adjacent stash anchor (passed in by the
/// caller — the chest's position is gameplay-driven, not
/// derived from the dungeon grid) plus a void-forge station
/// beside it (same footprint as a dungeon [`PropId::Anvil`]).
pub fn decorate_hub(floor: &Floor, seed: u64, stash_pos: Vec3) -> Vec<PlacedProp> {
    let _ = (floor, seed);
    let mut out = Vec::new();

    // Stash chest at the fixed gameplay anchor. Yaw rotates
    // the chest ~30° so the lid faces the spawn approach
    // (matches the previous client-side hard-coded value).
    out.push(PlacedProp {
        id: StashChest,
        pos: stash_pos,
        yaw: -std::f32::consts::FRAC_PI_6,
        scale: 0.9,
        wall_dir: None,
        light: false,
    });
    out.push(PlacedProp {
        id: VoidForge,
        pos: stash_pos + Vec3::new(1.6, 0.0, -0.35),
        yaw: std::f32::consts::FRAC_PI_4,
        scale: 1.0,
        wall_dir: None,
        light: false,
    });

    out
}

/// Sparse wall-mounted torch placement.
///
/// Torches are just regular [`PlacedProp`] entries with
/// `id = CandleStickStand` and `light = true` so the client
/// can spawn the flame VFX, point light, and audio emitter
/// for each one. Keeping placement here means the torch list
/// is deterministic from `(seed, floor_index)` like every
/// other prop, so a server with no rendering can still
/// describe exactly where the torches are.
///
/// Called *after* [`decorate_room`] for every room so we
/// can skip wall tiles already taken by the room scatter
/// (barrels, bookcases, weapon racks). Otherwise a candle
/// would clip into whatever was placed there first.
fn place_torches(floor: &Floor, props: &mut Vec<PlacedProp>, seed: u64) {
    // Min spacing between torches, in metres squared. The
    // forward shader caps active point lights at 8, so we
    // intentionally place torches sparsely enough that a
    // typical room has ~2–4 of them. Otherwise the per-frame
    // nearest-8 selection has to swap lights in and out as
    // the player walks, which reads as a halo tracking the
    // player rather than static fixtures anchored to the
    // walls. ~11 m feels right: each torch's lit area
    // (radius 11) just kisses its neighbour's, leaving no
    // obvious dark gaps but also no overlap that would force
    // a swap.
    const MIN_SPACING_SQ: f32 = 11.0 * 11.0;

    // Tiles already occupied by a wall-aligned scatter prop
    // — skip them so a candle doesn't clip into the barrel
    // already standing there. We only need an XZ proximity
    // test (radius 0.7 m, slightly bigger than a tile half
    // because the scatter pass may have nudged the prop a
    // few cm along the wall to clear a corner). Free-standing
    // props (centerpiece, flanker, scatter) live in the room
    // interior, far from wall tiles, so we don't filter
    // against those.
    let occupied: Vec<Vec3> = props
        .iter()
        .filter(|p| p.wall_dir.is_some())
        .map(|p| p.pos)
        .collect();
    const OCCUPIED_R2: f32 = 0.7 * 0.7;

    let (border, _interior) = collect_floor_tiles(floor);
    if border.is_empty() {
        return;
    }

    // Seed-fold so torch placement is stable for a given
    // floor seed but doesn't collide with the room scatter
    // RNG (which uses `0xC1A0_5EED`).
    let mut rng = SmallRng::new(seed ^ 0xA1B2_C3D4_E5F6_0789_u64);

    // Fisher-Yates shuffle so the spacing pruning doesn't
    // bias to one corner of the room.
    let mut order: Vec<usize> = (0..border.len()).collect();
    for i in (1..order.len()).rev() {
        let j = rng.range(0, (i as u32) + 1) as usize;
        order.swap(i, j);
    }

    let mut placed: Vec<Vec3> = Vec::new();

    for idx in order {
        let (tx, tz, (ox, oz)) = border[idx];
        let centre = tile_centre(tx, tz);

        // Skip if a wall-aligned prop already occupies this
        // tile (or the one immediately adjacent along the
        // wall). The 0.7 m radius covers any plausible wall
        // prop's footprint plus a little air gap.
        if occupied
            .iter()
            .any(|q| q.distance_squared(centre) < OCCUPIED_R2)
        {
            continue;
        }

        // Spacing check against already-placed torches. We
        // compare in tile-centre space rather than the
        // wall-snapped flame position because the snap is
        // mesh-bounds-driven on the client (the server has
        // no meshes) and the difference is at most ~0.3 m,
        // which is dwarfed by the 11 m spacing radius.
        if placed
            .iter()
            .any(|q| q.distance_squared(centre) < MIN_SPACING_SQ)
        {
            continue;
        }

        // Lift to the tile's authored elevation so a torch on
        // a raised dais or sunken-pit wall sits on the room's
        // floor surface. Without this the candle hovers above
        // (or sinks below) every non-base-elevation tile and
        // its flame VFX renders detached from the model.
        let pos = Vec3::new(
            centre.x,
            floor.tile_floor_y_at(centre.x, centre.z),
            centre.z,
        );

        // Yaw faces the candle away from the wall (toward
        // the room) so the wick + sculpted detail reads at
        // viewer angle.
        let yaw = (ox as f32).atan2(oz as f32);

        props.push(PlacedProp {
            id: PropId::CandleStickStand,
            pos,
            yaw,
            scale: 1.0,
            wall_dir: Some((ox as i8, oz as i8)),
            light: true,
        });
        placed.push(centre);
    }
}
