//! Authoritative prop catalog + placement.
//!
//! Both the server (kinematic integration) and the client
//! (rendering) need to agree on *where* props live and *how
//! big* they are — the server so it can block player capsules
//! against them, the client so it can render the same world
//! that the player is colliding with. Two paths used to exist:
//! the server knew nothing about props at all, and the client
//! ran its own random scatter. The result was that prop
//! colliders on the client were dead weight against the
//! `NetControlled` player and the player walked straight
//! through everything.
//!
//! This module unifies both sides:
//!
//! * [`PropId`] — stable, compact identifier for every prop
//!   the game can place. Both sides switch on this; the
//!   client maps it to gltf + material, the server maps it
//!   to a footprint.
//! * [`meta`] — per-id placement hint + collision footprint,
//!   resolved via an exhaustive `match` so the compiler
//!   guarantees every [`PropId`] variant has an entry.
//!   Footprints are hand-authored (the server never loads
//!   gltfs) — see the function for the dimensions and the
//!   rationale for each.
//! * [`Floor.props`] (populated in `Floor::generate` /
//!   `Floor::hub`) — the resolved world placements as
//!   [`PlacedProp`] entries. Identical on both sides for the
//!   same `(seed, floor_index)` pair.
//!
//! Prop *materials* (textures, shaders, render-time bbox
//! offsets) live entirely on the client. The dungeon doesn't
//! know what a prop *looks like*, only where it sits and
//! how much space it takes up.

use glam::Vec3;

/// Stable identifier for every prop the game can place.
///
/// Resolved to a [`PropMeta`] via [`meta`]; that function is
/// an exhaustive `match`, so adding a new variant fails to
/// compile until the catalog has an entry for it.
#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PropId {
    // ---- Fantasy / dungeon ----
    Cauldron,
    CandleStickTriple,
    CandleStick,
    CandleStickStand,
    CageSmall,
    ChainCoil,
    Anvil,
    AnvilLog,
    AxeBronze,
    BookStand,
    BookStack1,
    BookStack2,
    BookGroupMedium1,
    BookGroupMedium2,
    BookGroupMedium3,
    BookGroupSmall1,
    Bookcase2,
    Cabinet,
    Bench,
    BedTwin1,
    BedTwin2,
    Barrel,
    BarrelApples,
    BarrelHolder,
    BucketWooden1,
    BucketMetal,
    Bag,
    Banner1,
    Banner2,
    Banner1Cloth,
    Banner2Cloth,
    Bottle1,
    Candle1,
    Candle2,
    Carrot,

    // ---- Nature / hub ----
    GrassCommonShort,
    GrassWispyShort,
    PebbleRound2,
    PebbleRound4,

    // ---- Special ----
    StashChest,
    /// Hub-only forge: same footprint as [`Anvil`]; client gives it void styling.
    VoidForge,
}

/// How the placement algorithm should choose this prop's yaw
/// and tile relationship.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacementHint {
    /// Yaw faces away from the adjacent wall; the prop's back
    /// edge sits flush with the wall surface.
    WallAligned,
    /// Free-standing — random yaw, tile centre.
    Free,
}

/// Authoritative collision footprint for a placed prop.
///
/// Half-extents are in the prop's *local* frame at scale 1.0.
/// The placement step scales them by `PlacedProp::scale` and
/// the collision query rotates by `PlacedProp::yaw` (or for
/// `trunk` props, ignores yaw and uses an XZ-square test).
///
/// All footprints are hand-authored: the server never loads
/// gltfs, so we can't derive these from the mesh. Erring
/// slightly small is preferable to slightly large — a 5 cm
/// gap reads as "tight squeeze", a 5 cm overlap reads as
/// "stuck on invisible geometry".
#[derive(Clone, Copy, Debug)]
pub struct PropFootprint {
    pub half_x: f32,
    pub half_y: f32,
    pub half_z: f32,
    /// Square-XZ trunk (used for tree colliders where the
    /// visual canopy is wide but the trunk is narrow). When
    /// `true`, `half_z` is ignored and `half_x` is used as
    /// the trunk radius regardless of yaw.
    pub trunk: bool,
}

impl PropFootprint {
    pub const fn aabb(half_x: f32, half_y: f32, half_z: f32) -> Self {
        Self {
            half_x,
            half_y,
            half_z,
            trunk: false,
        }
    }
    #[allow(dead_code)]
    pub const fn trunk(radius: f32, half_y: f32) -> Self {
        Self {
            half_x: radius,
            half_y,
            half_z: radius,
            trunk: true,
        }
    }
}

/// Per-id catalog entry. `footprint = None` means the prop is
/// passable (pure decoration — books, banners, candles,
/// grass).
#[derive(Clone, Copy, Debug)]
pub struct PropMeta {
    pub placement: PlacementHint,
    pub footprint: Option<PropFootprint>,
}

const fn solid(placement: PlacementHint, hx: f32, hy: f32, hz: f32) -> PropMeta {
    PropMeta {
        placement,
        footprint: Some(PropFootprint::aabb(hx, hy, hz)),
    }
}
const fn passable(placement: PlacementHint) -> PropMeta {
    PropMeta {
        placement,
        footprint: None,
    }
}

use PlacementHint::*;

/// Per-id catalog lookup.
///
/// Implemented as an exhaustive `match` rather than a parallel
/// slice indexed by `PropId as usize` so the compiler enforces
/// that every variant has an entry — adding a new [`PropId`]
/// produces a "non-exhaustive" error here until you fill it
/// in. This is friendlier than the previous `&[PropMeta]`
/// table where a misordered or missing entry only surfaced
/// at runtime via a debug-assert.
///
/// Footprints were hand-authored from a quick visual
/// inspection of each gltf in Blender — they're approximate
/// silhouette boxes, deliberately erring on the small side
/// (the existing client code applied a 0.85 shrink factor
/// to AABB colliders for the same reason: a tight squeeze
/// reads better than a phantom snag).
pub const fn meta(id: PropId) -> PropMeta {
    use PropId::*;
    match id {
        // ---- Fantasy / dungeon ----
        Cauldron => solid(Free, 0.30, 0.40, 0.30),
        CandleStickTriple => passable(Free),
        CandleStick => passable(Free),
        CandleStickStand => passable(WallAligned),
        CageSmall => solid(WallAligned, 0.30, 0.50, 0.30),
        ChainCoil => passable(WallAligned),
        Anvil => solid(Free, 0.40, 0.35, 0.20),
        AnvilLog => solid(WallAligned, 0.25, 0.40, 0.25),
        AxeBronze => passable(WallAligned),
        BookStand => solid(Free, 0.20, 0.50, 0.20),
        BookStack1 => passable(Free),
        BookStack2 => passable(Free),
        BookGroupMedium1 => passable(Free),
        BookGroupMedium2 => passable(Free),
        BookGroupMedium3 => passable(Free),
        BookGroupSmall1 => passable(Free),
        Bookcase2 => solid(WallAligned, 0.45, 0.95, 0.20),
        Cabinet => solid(WallAligned, 0.40, 0.55, 0.25),
        Bench => solid(WallAligned, 0.55, 0.25, 0.20),
        BedTwin1 => solid(WallAligned, 0.40, 0.30, 0.65),
        BedTwin2 => solid(WallAligned, 0.40, 0.30, 0.65),
        Barrel => solid(WallAligned, 0.25, 0.40, 0.25),
        BarrelApples => solid(WallAligned, 0.25, 0.40, 0.25),
        BarrelHolder => solid(WallAligned, 0.45, 0.45, 0.30),
        BucketWooden1 => solid(WallAligned, 0.18, 0.20, 0.18),
        BucketMetal => solid(WallAligned, 0.18, 0.20, 0.18),
        Bag => passable(WallAligned),
        Banner1 => passable(WallAligned),
        Banner2 => passable(WallAligned),
        Banner1Cloth => passable(WallAligned),
        Banner2Cloth => passable(WallAligned),
        Bottle1 => passable(Free),
        Candle1 => passable(Free),
        Candle2 => passable(Free),
        Carrot => passable(Free),
        // ---- Nature / hub ----
        GrassCommonShort => passable(Free),
        GrassWispyShort => passable(Free),
        PebbleRound2 => passable(Free),
        PebbleRound4 => passable(Free),
        // ---- Special ----
        StashChest => solid(Free, 0.40, 0.30, 0.30),
        VoidForge => solid(Free, 0.40, 0.35, 0.20),
    }
}

/// World-space placement record for one prop. Emitted by the
/// dungeon generator and consumed by both server (collision)
/// and client (rendering).
#[derive(Clone, Copy, Debug)]
pub struct PlacedProp {
    pub id: PropId,
    pub pos: Vec3,
    pub yaw: f32,
    pub scale: f32,
    /// For [`PlacementHint::WallAligned`] props placed by the
    /// room scatter pass: the (ox, oz) tile offset toward the
    /// adjacent wall tile. Lets the client push the prop's
    /// back face flush against the wall surface using the
    /// gltf-derived mesh bounds (which the server doesn't
    /// have). `None` for free-standing / centerpiece /
    /// hub-scatter props.
    pub wall_dir: Option<(i8, i8)>,
    /// `true` when the client should attach a flickering
    /// point light + flame VFX + audio emitter to this prop.
    /// Currently only the wall-mounted candlesticks placed by
    /// `props_placement::place_torches` set this. The server
    /// doesn't care about the flag; it exists purely to give
    /// the client a deterministic torch list (driven by the
    /// same `(seed, floor_index)` as everything else) without
    /// the client running its own placement pass.
    pub light: bool,
}

impl PlacedProp {
    /// World-space AABB (`(min, max)`) for the prop's
    /// collider, or `None` if the prop is passable. Used by
    /// `kinematic::integrate` to block the player capsule.
    pub fn collider_aabb(&self) -> Option<(Vec3, Vec3)> {
        let fp = meta(self.id).footprint?;
        let (sin_y, cos_y) = self.yaw.sin_cos();
        // Rotated half-extents (yaw is the only rotation —
        // props don't tilt, the floor's elevation steps stay
        // axis-aligned). Trunks are rotation-invariant so we
        // skip the rotation entirely.
        let (hx, hz) = if fp.trunk {
            (fp.half_x * self.scale, fp.half_x * self.scale)
        } else {
            let hx = (cos_y.abs() * fp.half_x + sin_y.abs() * fp.half_z) * self.scale;
            let hz = (sin_y.abs() * fp.half_x + cos_y.abs() * fp.half_z) * self.scale;
            (hx, hz)
        };
        let hy = fp.half_y * self.scale;
        // The footprint's *origin* is the prop's anchor
        // (sole / base). Lift the AABB centre by half the
        // prop's height so the box covers `[pos.y, pos.y +
        // 2*hy]`; collision queries then test the player
        // capsule against the visible silhouette.
        let centre = Vec3::new(self.pos.x, self.pos.y + hy, self.pos.z);
        Some((
            centre - Vec3::new(hx, hy, hz),
            centre + Vec3::new(hx, hy, hz),
        ))
    }

    /// True when a horizontal capsule of `radius` centred at
    /// `(x, _, z)` would overlap this prop's footprint
    /// (regardless of vertical position — props in pits
    /// still block players who can't drop into the pit, and
    /// players standing on a dais can't clip into a prop on
    /// the lower floor because the kinematic only generates
    /// motion within reachable elevations).
    pub fn blocks_capsule_xz(&self, x: f32, z: f32, radius: f32) -> bool {
        let Some((min, max)) = self.collider_aabb() else {
            return false;
        };
        let cx = x.clamp(min.x, max.x);
        let cz = z.clamp(min.z, max.z);
        let dx = x - cx;
        let dz = z - cz;
        dx * dx + dz * dz < radius * radius
    }

    /// Minimum-translation depenetration vector for a
    /// horizontal capsule of `radius` centred at `(x, _, z)`
    /// against this prop's footprint, or `None` if there is
    /// no overlap.
    ///
    /// Two regimes:
    ///
    /// * **Inside the AABB** (`(x, z)` is within the box) —
    ///   pick the smallest of the four face penetrations
    ///   (left / right / front / back) and push out along
    ///   that axis. This is what handles "spawned inside a
    ///   prop" or "two props overlap and the player landed
    ///   between them" — the player slides out the nearest
    ///   face in one frame instead of getting stuck.
    /// * **Grazing an edge or corner** (`(x, z)` is outside
    ///   the box but the closest box point is within `radius`)
    ///   — push along the box→player vector by
    ///   `radius - dist`. This is the slide-along path:
    ///   moving tangent to the face leaves the push vector
    ///   purely normal so motion isn't damped, but moving
    ///   *into* the face produces a clean perpendicular
    ///   pushback that tracks the velocity direction with
    ///   no float-edge oscillation.
    ///
    /// Returns the (dx, dz) offset to add to the player's
    /// position. The magnitude is the exact penetration
    /// depth, so a single application clears the overlap.
    pub fn depenetrate_capsule_xz(&self, x: f32, z: f32, radius: f32) -> Option<(f32, f32)> {
        let (min, max) = self.collider_aabb()?;
        let cx = x.clamp(min.x, max.x);
        let cz = z.clamp(min.z, max.z);
        let dx = x - cx;
        let dz = z - cz;
        let d2 = dx * dx + dz * dz;
        let r2 = radius * radius;
        if d2 >= r2 {
            return None;
        }
        // Edge / corner case: grazing from outside.
        if d2 > 1e-12 {
            let d = d2.sqrt();
            let push = radius - d;
            return Some((dx / d * push, dz / d * push));
        }
        // Inside the AABB (or exactly on a face): pick the
        // shortest face escape and push outward along it.
        // Including the radius so the capsule edge clears
        // the face, not just the centre.
        let left = (x - min.x) + radius;
        let right = (max.x - x) + radius;
        let back = (z - min.z) + radius;
        let front = (max.z - z) + radius;
        let m = left.min(right).min(back).min(front);
        if m == left {
            Some((-left, 0.0))
        } else if m == right {
            Some((right, 0.0))
        } else if m == back {
            Some((0.0, -back))
        } else {
            Some((0.0, front))
        }
    }
}
