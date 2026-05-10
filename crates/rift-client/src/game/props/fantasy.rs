//! Fantasy / dungeon prop pack.
//!
//! Per-theme curated palettes from the 40-prop fantasy pack.
//! Each [`RoomTheme`] maps to a [`ThemePalette`] specifying:
//!
//! * a list of `PropAsset`s with theme-appropriate weights,
//! * a target prop density (per tile of room area),
//! * spacing to avoid the "barrel mid-doorway" mid-mush feel.
//!
//! [`decorate_dungeon`] iterates rooms and dispatches the
//! correct palette per room. The boss room always reads as
//! Shrine, the portal room as Generic; every other room got
//! its theme assigned deterministically at floor-generation
//! time so client and server agree purely from the seed.

use rift_engine::dungeon::{RoomTheme, RoomType};
use rift_engine::{Floor, Renderer};

use super::placement::{
    collect_room_wall_tiles, place_on_walls, SmallRng, WallAnchor, WallPlacement,
};
use super::{ColliderShape, Material, PlacementHint, PropAsset};

// Trim sheets — `Material::SharedTexture` keys the descriptor cache
// in `Props` on these strings.
const TRIM_FURNITURE: &str = "assets/models/fantasy-props/Textures/T_Trim_Furniture_BaseColor.png";
const TRIM_METAL:     &str = "assets/models/fantasy-props/Textures/T_Trim_Metal_BaseColor.png";
const TRIM_CLOTH:     &str = "assets/models/fantasy-props/Textures/T_Trim_Cloth_BaseColor.png";
const TRIM_PROPS:     &str = "assets/models/fantasy-props/Textures/T_Trim_Props_BaseColor.png";

const fn furniture_wall(gltf: &'static str, weight: u32, solid: bool) -> PropAsset {
    PropAsset {
        gltf, scale: 1.0,
        material: Material::SharedTexture(TRIM_FURNITURE),
        collider: if solid { ColliderShape::Aabb { shrink: 0.85 } } else { ColliderShape::None },
        placement: PlacementHint::WallAligned,
        weight,
    }
}
const fn metal(gltf: &'static str, weight: u32, against_wall: bool, solid: bool) -> PropAsset {
    PropAsset {
        gltf, scale: 1.0,
        material: Material::SharedTexture(TRIM_METAL),
        collider: if solid { ColliderShape::Aabb { shrink: 0.85 } } else { ColliderShape::None },
        placement: if against_wall { PlacementHint::WallAligned } else { PlacementHint::Free },
        weight,
    }
}
const fn cloth_wall(gltf: &'static str, weight: u32) -> PropAsset {
    PropAsset {
        gltf, scale: 1.0,
        material: Material::SharedTexture(TRIM_CLOTH),
        collider: ColliderShape::None,
        placement: PlacementHint::WallAligned,
        weight,
    }
}
const fn small_prop(gltf: &'static str, mat: &'static str, weight: u32, solid: bool) -> PropAsset {
    PropAsset {
        gltf, scale: 1.0,
        material: Material::SharedTexture(mat),
        collider: if solid { ColliderShape::Aabb { shrink: 0.85 } } else { ColliderShape::None },
        placement: PlacementHint::Free,
        weight,
    }
}

/// Candlestick stand — used by the wall-torch system to give
/// every torch a physical model to anchor its flame VFX to.
/// Exported separately from the theme palettes so the torch
/// placer can reference it without adding it to the random
/// scatter pool.
pub const CANDLESTICK_STAND: PropAsset = metal(
    "assets/models/fantasy-props/Exports/glTF/CandleStick_Stand.gltf",
    1, true, false,
);

// =====================================================================
// Theme palettes
// =====================================================================
//
// Each palette is a curated subset of the 40 props in the
// fantasy-props pack, weighted to give the room its thematic
// signature. The density and spacing fields tune *how* the
// props are placed: ceremonial themes (Library, Shrine) want
// a higher density and tighter spacing for that "lived-in"
// feel; functional themes (Storage, Prison) want bigger gaps
// so each prop reads as a distinct object.

/// One theme's prop palette plus placement parameters.
pub struct ThemePalette {
    /// Asset table the picker draws from.
    pub assets: &'static [PropAsset],
    /// Target prop count is `room_area * density` (clamped to
    /// `[min_count, max_count]`). 0.07 = ~1 prop per 14 tiles
    /// of room area.
    pub density: f32,
    pub min_count: usize,
    pub max_count: usize,
    /// Minimum world-space distance between any two props of
    /// this palette. Larger = sparser, props read individually.
    pub min_spacing: f32,
    /// Optional centerpiece placed at the room's geometric
    /// centre. Skipped on rooms smaller than
    /// `min_centerpiece_area` so it doesn't crowd corridors.
    /// The wall-scatter pass keeps a clearance around the
    /// centre via its `avoid` list, so the centerpiece always
    /// has breathing room.
    pub centerpiece: Option<&'static PropAsset>,
    /// Optional pair of flanker props placed symmetrically on
    /// either side of the centerpiece (~1.5 m offset). Builds
    /// arrangements like "altar + two candle stands" without
    /// needing a full slot-graph cluster system.
    pub flanker: Option<&'static PropAsset>,
    /// Minimum room area (in tiles) for the centerpiece to
    /// spawn. 24 = 6×4 floor — anything smaller would see the
    /// centerpiece dominate the playable space.
    pub min_centerpiece_area: usize,
}

// =====================================================================
// Centerpiece / flanker assets
// =====================================================================
//
// Per-theme anchor props. Expressed as `static` items so the
// `&'static PropAsset` references in `ThemePalette` stay
// const-eval-friendly. All of these are also (deliberately)
// included in the matching theme's wall-scatter pool with low
// weights, so the centerpiece reads as "a bigger version of
// the room's normal stuff" rather than feeling parachuted in.

// Crypt: a great cauldron at centre, flanked by triple
// candle stands. Reads as a funerary urn flanked by ritual
// candles.
static CRYPT_CENTER:  PropAsset = metal("assets/models/fantasy-props/Exports/glTF/Cauldron.gltf",           1, false, true);
static CRYPT_FLANKER: PropAsset = metal("assets/models/fantasy-props/Exports/glTF/CandleStick_Triple.gltf", 1, false, false);

// Barracks: anvil at centre, axes leaning in pairs. Workshop
// vibe.
static BARRACKS_CENTER:  PropAsset = metal("assets/models/fantasy-props/Exports/glTF/Anvil.gltf",      1, false, true);
static BARRACKS_FLANKER: PropAsset = metal("assets/models/fantasy-props/Exports/glTF/Anvil_Log.gltf",  1, false, true);

// Library: book stand for the open tome, candle stands either
// side for the late-night reader.
static LIBRARY_CENTER:  PropAsset = small_prop("assets/models/fantasy-props/Exports/glTF/BookStand.gltf", TRIM_FURNITURE, 1, true);
static LIBRARY_FLANKER: PropAsset = metal("assets/models/fantasy-props/Exports/glTF/CandleStick.gltf",     1, false, false);

// Shrine: the climactic centerpiece for boss rooms. The
// cauldron-as-altar reads as the ritual focal point; triple
// candle stands flank it.
static SHRINE_CENTER:  PropAsset = metal("assets/models/fantasy-props/Exports/glTF/Cauldron.gltf",           1, false, true);
static SHRINE_FLANKER: PropAsset = metal("assets/models/fantasy-props/Exports/glTF/CandleStick_Triple.gltf", 1, false, false);

// Storage: barrel holder centre with bracketed barrels
// flanking — reads as a stocked shelf.
static STORAGE_CENTER:  PropAsset = furniture_wall("assets/models/fantasy-props/Exports/glTF/Barrel_Holder.gltf", 1, true);
static STORAGE_FLANKER: PropAsset = furniture_wall("assets/models/fantasy-props/Exports/glTF/Barrel.gltf",        1, true);

// Prison: cage at centre with chain coils flanking.
static PRISON_CENTER:  PropAsset = metal("assets/models/fantasy-props/Exports/glTF/Cage_Small.gltf", 1, false, true);
static PRISON_FLANKER: PropAsset = metal("assets/models/fantasy-props/Exports/glTF/Chain_Coil.gltf", 1, false, false);

// ---- Crypt ------------------------------------------------
// Cold burial chamber. Cauldrons stand in for funerary urns,
// chained cages for bone reliquaries, tattered cloth banners
// for shrouded tombstones. Sparse, low-density — empty space
// is part of the mood.
pub const CRYPT_PALETTE: ThemePalette = ThemePalette {
    assets: &[
        metal("assets/models/fantasy-props/Exports/glTF/Cauldron.gltf",           4, true,  true),
        metal("assets/models/fantasy-props/Exports/glTF/CandleStick_Triple.gltf", 3, false, false),
        metal("assets/models/fantasy-props/Exports/glTF/Chain_Coil.gltf",         2, true,  false),
        metal("assets/models/fantasy-props/Exports/glTF/Cage_Small.gltf",         2, true,  true),
        cloth_wall("assets/models/fantasy-props/Exports/glTF/Banner_1_Cloth.gltf", 2),
        cloth_wall("assets/models/fantasy-props/Exports/glTF/Banner_2_Cloth.gltf", 2),
        small_prop("assets/models/fantasy-props/Exports/glTF/Bottle_1.gltf",       TRIM_PROPS, 1, false),
    ],
    density: 0.04,
    min_count: 2,
    max_count: 6,
    min_spacing: 2.4,
    centerpiece: Some(&CRYPT_CENTER),
    flanker:     Some(&CRYPT_FLANKER),
    min_centerpiece_area: 24,
};

// ---- Barracks ---------------------------------------------
// Soldiers' quarters. Beds and weapon racks (anvils stand
// in for armor stands), banners on the walls.
pub const BARRACKS_PALETTE: ThemePalette = ThemePalette {
    assets: &[
        furniture_wall("assets/models/fantasy-props/Exports/glTF/Bed_Twin1.gltf", 4, true),
        furniture_wall("assets/models/fantasy-props/Exports/glTF/Bed_Twin2.gltf", 4, true),
        furniture_wall("assets/models/fantasy-props/Exports/glTF/Bench.gltf",     3, true),
        furniture_wall("assets/models/fantasy-props/Exports/glTF/Barrel.gltf",    3, true),
        metal("assets/models/fantasy-props/Exports/glTF/Anvil.gltf",     2, false, true),
        metal("assets/models/fantasy-props/Exports/glTF/Anvil_Log.gltf", 2, true,  true),
        metal("assets/models/fantasy-props/Exports/glTF/Axe_Bronze.gltf", 1, true, false),
        cloth_wall("assets/models/fantasy-props/Exports/glTF/Bag.gltf",      2),
        cloth_wall("assets/models/fantasy-props/Exports/glTF/Banner_1.gltf", 2),
        cloth_wall("assets/models/fantasy-props/Exports/glTF/Banner_2.gltf", 2),
    ],
    density: 0.07,
    min_count: 3,
    max_count: 9,
    min_spacing: 1.7,
    centerpiece: Some(&BARRACKS_CENTER),
    flanker:     Some(&BARRACKS_FLANKER),
    min_centerpiece_area: 28,
};

// ---- Library ----------------------------------------------
// Scriptorium. Bookcases dominate the walls, reading benches
// with stacks of books and candles for night study.
pub const LIBRARY_PALETTE: ThemePalette = ThemePalette {
    assets: &[
        furniture_wall("assets/models/fantasy-props/Exports/glTF/Bookcase_2.gltf", 6, true),
        furniture_wall("assets/models/fantasy-props/Exports/glTF/Cabinet.gltf",    3, true),
        furniture_wall("assets/models/fantasy-props/Exports/glTF/Bench.gltf",      2, true),
        small_prop("assets/models/fantasy-props/Exports/glTF/BookStand.gltf",          TRIM_FURNITURE, 2, true),
        small_prop("assets/models/fantasy-props/Exports/glTF/Book_Stack_1.gltf",       TRIM_PROPS, 2, false),
        small_prop("assets/models/fantasy-props/Exports/glTF/Book_Stack_2.gltf",       TRIM_PROPS, 2, false),
        small_prop("assets/models/fantasy-props/Exports/glTF/BookGroup_Medium_1.gltf", TRIM_PROPS, 1, false),
        small_prop("assets/models/fantasy-props/Exports/glTF/BookGroup_Medium_2.gltf", TRIM_PROPS, 1, false),
        small_prop("assets/models/fantasy-props/Exports/glTF/BookGroup_Medium_3.gltf", TRIM_PROPS, 1, false),
        small_prop("assets/models/fantasy-props/Exports/glTF/BookGroup_Small_1.gltf",  TRIM_PROPS, 1, false),
        metal("assets/models/fantasy-props/Exports/glTF/CandleStick.gltf",        2, false, false),
        metal("assets/models/fantasy-props/Exports/glTF/CandleStick_Triple.gltf", 1, false, false),
    ],
    density: 0.09,
    min_count: 4,
    max_count: 12,
    min_spacing: 1.4,
    centerpiece: Some(&LIBRARY_CENTER),
    flanker:     Some(&LIBRARY_FLANKER),
    min_centerpiece_area: 28,
};

// ---- Shrine -----------------------------------------------
// Sanctum / ritual chamber. The boss room is always Shrine —
// big space, ceremonial layout. Cauldrons and candle stands
// flank a cleared centre; benches face inward.
pub const SHRINE_PALETTE: ThemePalette = ThemePalette {
    assets: &[
        metal("assets/models/fantasy-props/Exports/glTF/CandleStick_Triple.gltf", 6, false, false),
        metal("assets/models/fantasy-props/Exports/glTF/CandleStick.gltf",        4, false, false),
        metal("assets/models/fantasy-props/Exports/glTF/Cauldron.gltf",           3, false, true),
        furniture_wall("assets/models/fantasy-props/Exports/glTF/Bench.gltf",     3, true),
        cloth_wall("assets/models/fantasy-props/Exports/glTF/Banner_1_Cloth.gltf", 3),
        cloth_wall("assets/models/fantasy-props/Exports/glTF/Banner_2_Cloth.gltf", 3),
        small_prop("assets/models/fantasy-props/Exports/glTF/Candle_1.gltf", TRIM_PROPS, 2, false),
        small_prop("assets/models/fantasy-props/Exports/glTF/Candle_2.gltf", TRIM_PROPS, 2, false),
    ],
    density: 0.06,
    min_count: 4,
    max_count: 10,
    min_spacing: 2.0,
    centerpiece: Some(&SHRINE_CENTER),
    flanker:     Some(&SHRINE_FLANKER),
    // Boss rooms (always Shrine) are the largest rooms on a
    // floor by construction, so this lower bound just gates
    // out small Arena rooms that happened to roll Shrine.
    min_centerpiece_area: 30,
};

// ---- Storage ----------------------------------------------
// Cellar. Rows of barrels and crates, sacks, occasional
// cages.
pub const STORAGE_PALETTE: ThemePalette = ThemePalette {
    assets: &[
        furniture_wall("assets/models/fantasy-props/Exports/glTF/Barrel.gltf",        6, true),
        furniture_wall("assets/models/fantasy-props/Exports/glTF/Barrel_Apples.gltf", 4, true),
        furniture_wall("assets/models/fantasy-props/Exports/glTF/Barrel_Holder.gltf", 3, true),
        furniture_wall("assets/models/fantasy-props/Exports/glTF/Cabinet.gltf",       2, true),
        furniture_wall("assets/models/fantasy-props/Exports/glTF/Bucket_Wooden_1.gltf", 2, true),
        metal("assets/models/fantasy-props/Exports/glTF/Bucket_Metal.gltf", 2, true, true),
        metal("assets/models/fantasy-props/Exports/glTF/Cage_Small.gltf",   1, true, true),
        cloth_wall("assets/models/fantasy-props/Exports/glTF/Bag.gltf", 3),
        small_prop("assets/models/fantasy-props/Exports/glTF/Carrot.gltf", TRIM_PROPS, 1, false),
    ],
    density: 0.10,
    min_count: 4,
    max_count: 14,
    min_spacing: 1.3,
    centerpiece: Some(&STORAGE_CENTER),
    flanker:     Some(&STORAGE_FLANKER),
    min_centerpiece_area: 24,
};

// ---- Prison -----------------------------------------------
// Holding cells. Cages, cot beds, buckets, chains.
pub const PRISON_PALETTE: ThemePalette = ThemePalette {
    assets: &[
        metal("assets/models/fantasy-props/Exports/glTF/Cage_Small.gltf", 6, true, true),
        metal("assets/models/fantasy-props/Exports/glTF/Chain_Coil.gltf", 4, true, false),
        furniture_wall("assets/models/fantasy-props/Exports/glTF/Bed_Twin1.gltf", 3, true),
        furniture_wall("assets/models/fantasy-props/Exports/glTF/Bucket_Wooden_1.gltf", 3, true),
        metal("assets/models/fantasy-props/Exports/glTF/Bucket_Metal.gltf", 2, true, true),
        cloth_wall("assets/models/fantasy-props/Exports/glTF/Bag.gltf", 2),
        cloth_wall("assets/models/fantasy-props/Exports/glTF/Banner_1_Cloth.gltf", 1),
    ],
    density: 0.06,
    min_count: 3,
    max_count: 8,
    min_spacing: 1.8,
    centerpiece: Some(&PRISON_CENTER),
    flanker:     Some(&PRISON_FLANKER),
    min_centerpiece_area: 24,
};

// ---- Generic (fallback) -----------------------------------
// Mixed palette for `Generic`-themed rooms (e.g. portal room
// transit space). Lower density: the portal room intentionally
// stays uncluttered so the two portals read clearly.
pub const GENERIC_PALETTE: ThemePalette = ThemePalette {
    assets: &[
        furniture_wall("assets/models/fantasy-props/Exports/glTF/Barrel.gltf",          3, true),
        furniture_wall("assets/models/fantasy-props/Exports/glTF/Bench.gltf",           2, true),
        furniture_wall("assets/models/fantasy-props/Exports/glTF/Bucket_Wooden_1.gltf", 2, true),
        cloth_wall("assets/models/fantasy-props/Exports/glTF/Bag.gltf", 1),
    ],
    density: 0.03,
    min_count: 1,
    max_count: 4,
    min_spacing: 2.2,
    centerpiece: None,
    flanker:     None,
    min_centerpiece_area: 0,
};

/// Pick the palette for a [`RoomTheme`].
pub fn palette_for(theme: RoomTheme) -> &'static ThemePalette {
    match theme {
        RoomTheme::Crypt    => &CRYPT_PALETTE,
        RoomTheme::Barracks => &BARRACKS_PALETTE,
        RoomTheme::Library  => &LIBRARY_PALETTE,
        RoomTheme::Shrine   => &SHRINE_PALETTE,
        RoomTheme::Storage  => &STORAGE_PALETTE,
        RoomTheme::Prison   => &PRISON_PALETTE,
        RoomTheme::Generic  => &GENERIC_PALETTE,
    }
}

/// Decorate every Arena/BossRoom/PortalRoom on `floor` with
/// theme-appropriate props. Each room dispatches to its own
/// palette so a Library reads visually distinct from a
/// Crypt or a Barracks even when the rooms have the same
/// shape.
///
/// `extra_avoid` is a slice of `(point, radius)` exclusion
/// zones layered on top of the per-room defaults. The
/// caller passes wall-torch positions here so barrels /
/// benches / etc. don't spawn clipping into a candlestick
/// the torch system already placed on the same wall tile.
pub fn decorate_dungeon(
    props: &mut super::Props,
    world: &mut hecs::World,
    renderer: &mut Renderer,
    floor: &Floor,
    seed: u64,
    extra_avoid: &[(glam::Vec3, f32)],
) {
    let mut rng = SmallRng::new(seed.wrapping_add(0xC1A0_5EED));

    for room in &floor.rooms {
        // Skip pure transit spaces. PortalRoom keeps its
        // theme-aware decoration (Generic palette = light
        // touch) so it doesn't look totally bare.
        match room.room_type {
            RoomType::Arena | RoomType::BossRoom | RoomType::PortalRoom => {}
            RoomType::Corridor => continue,
        }

        let palette = palette_for(room.theme);

        // ---- Centerpiece ----
        // Spawned first so its world position is registered
        // before the wall-scatter pass. The wall pass already
        // avoids the room centre via its `avoid` list, so the
        // two passes never conflict.
        let mut centre_yaw = 0.0_f32;
        // Boss rooms are deliberately left bare around the
        // centre: the boss arena needs a clear pivot point
        // for kiting, and a centerpiece prop in the middle
        // of the fight ring reads as a random obstacle
        // rather than scenery. Loot drops still land at
        // the centre, but the *static* centerpiece is
        // skipped here so the floor stays open.
        let allow_centerpiece = room.room_type != RoomType::BossRoom;
        let centerpiece_spawned = if let (true, Some(asset)) =
            (allow_centerpiece, palette.centerpiece)
        {
            if room.area() >= palette.min_centerpiece_area
                && floor.spawn_pos.distance(room.center_world()) > 4.0
            {
                // Yaw faces the player's most likely entry —
                // the door direction is hard to derive
                // cheaply, so we instead use a deterministic
                // per-room yaw drawn from the seed. Picks
                // one of 4 cardinal-aligned rotations so the
                // centerpiece always sits flush with the
                // grid (boss-room flankers below depend on
                // this).
                let yaw_quad = (rng.next() as usize) & 3;
                centre_yaw = std::f32::consts::FRAC_PI_2 * yaw_quad as f32;
                // Lift the centerpiece to the room
                // centre's authored elevation so a sunken
                // pit room or raised dais carries the
                // statue / altar at the right step.
                let mut centre_pos = room.center_world();
                centre_pos.y = floor.tile_floor_y_at(centre_pos.x, centre_pos.z);
                props.spawn(
                    world, renderer, asset,
                    centre_pos,
                    centre_yaw,
                    (0, 0),
                    None,
                ).is_some()
            } else {
                false
            }
        } else {
            false
        };

        // ---- Flankers ----
        // Only when a centerpiece actually spawned. Flankers
        // sit at ±1.5 m along the centerpiece's right axis
        // (perpendicular to its facing yaw), which puts them
        // visually beside it rather than in front / behind.
        if centerpiece_spawned {
            if let Some(asset) = palette.flanker {
                let (sin_y, cos_y) = centre_yaw.sin_cos();
                // Right axis = (cos, 0, -sin) for +Y rotation.
                let right = glam::Vec3::new(cos_y, 0.0, -sin_y);
                let centre = room.center_world();
                for sign in [-1.0_f32, 1.0_f32] {
                    let mut flanker_pos = centre + right * (1.5 * sign);
                    // Skip if the flanker would land on a
                    // wall tile (small rooms with shape
                    // carving may have walls right next to
                    // the centre).
                    let gx = flanker_pos.x.round() as i32;
                    let gz = flanker_pos.z.round() as i32;
                    if gx < 0 || gz < 0 { continue; }
                    if floor.get(gx as usize, gz as usize)
                        != rift_engine::dungeon::Tile::Floor
                    {
                        continue;
                    }
                    // Lift to the tile elevation under
                    // each flanker independently — a
                    // centerpiece on the lip of a sunken
                    // pit can have one flanker on the
                    // dais and one in the pit.
                    flanker_pos.y =
                        floor.tile_floor_y_at(flanker_pos.x, flanker_pos.z);
                    props.spawn(
                        world, renderer, asset,
                        flanker_pos,
                        centre_yaw,
                        (0, 0),
                        None,
                    );
                }
            }
        }

        // ---- Wall scatter ----
        let area_props = (room.area() as f32 * palette.density) as usize;
        let count = area_props.clamp(palette.min_count, palette.max_count);

        // Centerpiece avoid radius scales with whether one
        // actually spawned — when it didn't, we still want a
        // small clearance around the centre so play space
        // stays open.
        let centre_clear = if centerpiece_spawned {
            // Big enough to cover both flankers (~1.5 m
            // offset + ~0.7 m flanker half-extent + 0.4 m
            // breathing room).
            2.8
        } else if room.room_type == RoomType::BossRoom {
            3.5
        } else {
            2.5
        };
        // Build the per-room avoid list: room centre +
        // player spawn + every external avoid (e.g. wall
        // torch positions). Done as a Vec because
        // `extra_avoid` length is dynamic.
        let mut avoid: Vec<(glam::Vec3, f32)> = Vec::with_capacity(2 + extra_avoid.len());
        avoid.push((room.center_world(), centre_clear));
        // Stay clear of the player spawn so the first
        // step into the dungeon doesn't trip over a
        // barrel.
        avoid.push((floor.spawn_pos, 4.5));
        avoid.extend_from_slice(extra_avoid);
        place_on_walls(
            props, world, renderer, floor,
            collect_room_wall_tiles(floor, room),
            &mut rng,
            &WallPlacement {
                assets: palette.assets,
                count: Some(count),
                anchor: WallAnchor::OnFloorTile,
                wall_yaw_jitter_deg: 10.0,
                min_spacing: palette.min_spacing,
                avoid: &avoid,
                scale_jitter: (1.0, 1.0),
            },
        );
    }
}
