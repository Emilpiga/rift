use glam::Vec3;

/// The type of room placed in a BSP leaf.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomType {
    /// Standard combat encounter room.
    Arena,
    /// Large room for end-of-floor boss fight.
    BossRoom,
    /// Quiet room placed adjacent to the boss room — holds the
    /// pair of post-boss portals (descend / return-to-hub) so
    /// the choice is physically separated from the boss-fight
    /// loot pile. Tagged here purely for downstream lookup; the
    /// client just renders it as floor like any other room.
    PortalRoom,
    /// Narrow connecting passage between rooms.
    Corridor,
}

/// The narrative / decoration character of a room. Distinct
/// from [`RoomType`] (which is a layout role): a room can be
/// an `Arena` *and* a `Crypt`, or a `BossRoom` *and* a
/// `Shrine`. Drives prop-palette selection on the client and
/// optional theme-specific lighting tints.
///
/// Themes are assigned deterministically at floor-gen time
/// from the floor seed + room index so client and server
/// agree without any extra wire data — reconstructing the
/// floor from the seed yields the same theme assignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomTheme {
    /// Default / unthemed. Mixed prop palette, used as a
    /// fallback when no specific theme fits a room's
    /// dimensions.
    Generic,
    /// Burial chamber. Sarcophagi, urns, candle stands,
    /// dead trees, scattered bones; cold lighting.
    Crypt,
    /// Soldiers' quarters / armory. Beds, weapon racks,
    /// anvils, barrels, training dummies.
    Barracks,
    /// Books, scrolls, scriptorium props. Bookcases lining
    /// walls, reading tables with chairs and candles.
    Library,
    /// Altar room / sanctum. A central altar centerpiece,
    /// candle stands flanking it, benches facing inward,
    /// warm bright lighting.
    Shrine,
    /// Cellar / storeroom. Rows of barrels and crates,
    /// cabinets, sacks, cages.
    Storage,
    /// Holding cells. Cages, cot beds, buckets, chains.
    /// Dim lighting.
    Prison,
}

/// Ground-material classification for a tile, used by
/// gameplay systems that key off of surface type rather than
/// visual theme — footstep audio is the canonical case
/// (sand vs stone vs wood vs metal grate), but the same
/// query is the right hook for surface-aware blood splat
/// colour, fall-impact one-shots, and any future "what am I
/// standing on?" lookup.
///
/// Distinct from [`RoomTheme`] because theme is about *visual
/// dressing* (palette, prop set, lighting tint) while surface
/// is about *physical material* — two themes can share a
/// surface (Library and Crypt are both Stone), and one theme
/// can have multiple surfaces if a room ever carries a
/// per-tile override (a wooden bridge across a stone floor).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SurfaceKind {
    /// Loose sand / dust. Hub default. Soft, scuffing
    /// footfalls.
    Sand,
    /// Cut stone, flagstones, dressed masonry. The default
    /// for rift floors.
    #[default]
    Stone,
    /// Wooden planks / boards. Hollow, resonant footfalls.
    Wood,
    /// Metal grates, plates, chain mail underfoot. Sharp,
    /// ringing footfalls.
    Metal,
    /// Soft turf / damp moss. Muffled footfalls.
    Grass,
    /// Bone-strewn / loose-rubble. Crunchy, brittle
    /// footfalls.
    Bone,
}

impl RoomTheme {
    /// Default surface material for a room of this theme.
    /// Per-tile overrides (if/when [`crate::Tile`] carries
    /// them) take precedence over this; this is the room-
    /// level fallback.
    pub fn default_surface(self) -> SurfaceKind {
        match self {
            // Generic fall-back: every rift floor is built
            // on dressed stone unless something more
            // specific overrides it.
            RoomTheme::Generic => SurfaceKind::Stone,
            // Cold burial chambers: stone slabs.
            RoomTheme::Crypt => SurfaceKind::Stone,
            // Soldier quarters tend to have wooden plank
            // mezzanines; reads warmer than the corridors.
            RoomTheme::Barracks => SurfaceKind::Wood,
            // Libraries: parquet/wood reading floors.
            RoomTheme::Library => SurfaceKind::Wood,
            // Altar rooms: polished stone.
            RoomTheme::Shrine => SurfaceKind::Stone,
            // Cellars: stone over packed earth, but stone
            // is the dominant audible surface for booted
            // feet.
            RoomTheme::Storage => SurfaceKind::Stone,
            // Prison: dirty stone, with bone debris in
            // many cells. Future floors can override per-
            // tile to Bone if we want crunchier audio in
            // the cells themselves.
            RoomTheme::Prison => SurfaceKind::Stone,
        }
    }
}

/// The interior silhouette of a room. Pure tile-grid edits on
/// top of the BSP rectangle, applied during floor generation
/// before any prop placement so wall/floor mesh building and
/// prop placement both see the carved tiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomShape {
    /// Plain BSP rectangle (the historical default).
    Rectangular,
    /// Regular grid of interior pillars (4-9 wall tiles in
    /// the room's interior). Reads as a colonnade.
    Pillared,
    /// Recessed alcoves cut into the perimeter walls. Wider
    /// rooms get more alcoves; each alcove is 2-3 tiles deep
    /// and 2 tiles wide.
    Alcoved,
    /// Cross/plus-shaped: corner squares walled off. Only
    /// applied to rooms where both dimensions are >= 9.
    Cross,
    /// Approximately round: corner tiles walled off in a
    /// stepped pattern. Reads as a rotunda.
    Round,
}

/// A room with position, dimensions, type, and spawn metadata.
#[derive(Clone, Debug)]
pub struct Room {
    pub x: usize,
    pub z: usize,
    pub width: usize,
    pub depth: usize,
    pub room_type: RoomType,
    /// Decorative theme, see [`RoomTheme`]. Defaults to
    /// `Generic` until the theme assignment pass runs in
    /// [`generate_bsp`]; clients should treat that as
    /// "use mixed palette".
    pub theme: RoomTheme,
    /// Interior silhouette, see [`RoomShape`]. Defaults to
    /// `Rectangular`. Carving happens in `Floor::generate`
    /// after the BSP rectangle is laid down but before the
    /// floor is finalised, so the resulting tile grid is
    /// authoritative.
    pub shape: RoomShape,
    /// Optional ground-material override. When `Some`, wins
    /// over [`RoomTheme::default_surface`] in
    /// [`crate::Floor::surface_at`]. Used by
    /// [`crate::Floor::hub`] to force `Sand` regardless of
    /// the synthetic hub room's `Generic` theme; rift-floor
    /// rooms leave this `None` and let the theme dispatch.
    pub surface: Option<SurfaceKind>,
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

    /// Two interior anchor points for the post-boss portal
    /// pair, returned as `(left, right)` in world coordinates.
    /// Both sit on the room centre's Z axis with a symmetric
    /// X offset, clamped to keep at least one tile of border
    /// between each anchor and the wall so portal meshes can't
    /// poke through. The minimum offset (1.5 m) is enough that
    /// the loot pile dropped at the boss centre — which sits
    /// in a *different* room — never overlaps either anchor's
    /// pickup / interact radius even when the corridor is
    /// short. The widest a room ever gets bounds the maximum
    /// at `width/2 - 1`, so callers can rely on the anchors
    /// being inside `Tile::Floor`.
    pub fn portal_anchors(&self) -> (Vec3, Vec3) {
        let (cx, cz) = self.center();
        // Half-width minus one tile of border, capped so giant
        // rooms don't push the portals into the corners.
        let max_off = (self.width as f32 / 2.0 - 1.0).max(1.0);
        let dx = max_off.min(3.0).max(1.5);
        let cx = cx as f32;
        let cz = cz as f32;
        (Vec3::new(cx - dx, 0.0, cz), Vec3::new(cx + dx, 0.0, cz))
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
    pub fn spawn_packs(
        &self,
        num_packs: u32,
        mobs_per_pack: u32,
        seed: u64,
    ) -> Vec<(Vec3, Vec<Vec3>)> {
        let mut rng = super::SimpleRng::new(seed);
        let mut packs = Vec::new();

        for _ in 0..num_packs {
            // Pick a pack center somewhere inside the room (with margin)
            let margin = 2.0;
            let cx = self.x as f32
                + margin
                + rng.range(0, ((self.width as f32 - margin * 2.0).max(1.0)) as u32) as f32;
            let cz = self.z as f32
                + margin
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
