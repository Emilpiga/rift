//! Fantasy / dungeon prop pack.
//!
//! Trim-sheet textured props placed along the inner perimeter of
//! arena and boss rooms. The decorate function is just two lines of
//! per-room loop driving the generic [`place_on_walls`] helper.

use rift_engine::dungeon::RoomType;
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
/// Exported separately from `ASSETS` so the torch placer can
/// reference it without adding it to the random scatter pool.
pub const CANDLESTICK_STAND: PropAsset = metal(
    "assets/models/fantasy-props/Exports/glTF/CandleStick_Stand.gltf",
    1, true, false,
);

/// Curated subset of the fantasy-props pack.
pub const ASSETS: &[PropAsset] = &[
    furniture_wall("assets/models/fantasy-props/Exports/glTF/Barrel.gltf",          6, true),
    furniture_wall("assets/models/fantasy-props/Exports/glTF/Barrel_Apples.gltf",   2, true),
    furniture_wall("assets/models/fantasy-props/Exports/glTF/Barrel_Holder.gltf",   2, true),
    furniture_wall("assets/models/fantasy-props/Exports/glTF/Bench.gltf",           3, true),
    furniture_wall("assets/models/fantasy-props/Exports/glTF/Bookcase_2.gltf",      2, true),
    furniture_wall("assets/models/fantasy-props/Exports/glTF/Cabinet.gltf",         2, true),
    furniture_wall("assets/models/fantasy-props/Exports/glTF/Bed_Twin1.gltf",       1, true),
    furniture_wall("assets/models/fantasy-props/Exports/glTF/Bed_Twin2.gltf",       1, true),
    furniture_wall("assets/models/fantasy-props/Exports/glTF/Anvil_Log.gltf",       1, true),
    furniture_wall("assets/models/fantasy-props/Exports/glTF/Bucket_Wooden_1.gltf", 2, true),

    metal("assets/models/fantasy-props/Exports/glTF/Anvil.gltf",              1, false, true),
    metal("assets/models/fantasy-props/Exports/glTF/Cauldron.gltf",           2, false, true),
    metal("assets/models/fantasy-props/Exports/glTF/Bucket_Metal.gltf",       2, true,  true),
    metal("assets/models/fantasy-props/Exports/glTF/Cage_Small.gltf",         1, true,  true),
    metal("assets/models/fantasy-props/Exports/glTF/CandleStick_Triple.gltf", 2, false, false),
    // Note: `CandleStick_Stand` is *not* in the scatter pool — it
    // is placed deterministically by the wall-torch system so
    // every torch gets a physical candle to anchor its flame.
    metal("assets/models/fantasy-props/Exports/glTF/Chain_Coil.gltf",         1, true,  false),

    cloth_wall("assets/models/fantasy-props/Exports/glTF/Bag.gltf",      2),
    cloth_wall("assets/models/fantasy-props/Exports/glTF/Banner_1.gltf", 1),
    cloth_wall("assets/models/fantasy-props/Exports/glTF/Banner_2.gltf", 1),

    small_prop("assets/models/fantasy-props/Exports/glTF/Book_Stack_1.gltf",       TRIM_PROPS,     1, false),
    small_prop("assets/models/fantasy-props/Exports/glTF/Book_Stack_2.gltf",       TRIM_PROPS,     1, false),
    small_prop("assets/models/fantasy-props/Exports/glTF/BookGroup_Medium_1.gltf", TRIM_PROPS,     1, false),
    small_prop("assets/models/fantasy-props/Exports/glTF/BookStand.gltf",          TRIM_FURNITURE, 1, true),
];

/// Scatter fantasy props through every Arena/BossRoom on `floor`.
pub fn decorate_dungeon(
    props: &mut super::Props,
    world: &mut hecs::World,
    renderer: &mut Renderer,
    floor: &Floor,
    seed: u64,
) {
    let mut rng = SmallRng::new(seed.wrapping_add(0xC1A0_5EED));

    for room in &floor.rooms {
        let count = match room.room_type {
            RoomType::BossRoom => (room.area() / 18).clamp(4, 10),
            RoomType::Arena    => (room.area() / 22).clamp(2, 6),
            _ => continue,
        };
        let avoid = [
            (room.center_world(), 2.5),
            (floor.spawn_pos,     4.5),
        ];
        place_on_walls(
            props, world, renderer,
            collect_room_wall_tiles(floor, room),
            &mut rng,
            &WallPlacement {
                assets: ASSETS,
                count: Some(count),
                anchor: WallAnchor::OnFloorTile,
                wall_yaw_jitter_deg: 10.0,
                min_spacing: 1.6,
                avoid: &avoid,
                scale_jitter: (1.0, 1.0),
            },
        );
    }
}
