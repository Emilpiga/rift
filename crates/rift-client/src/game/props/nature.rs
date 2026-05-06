//! Outdoor / nature prop pack.
//!
//! Vertex-coloured (no shared material) — the `gltf` baseColorTexture
//! is sampled per-vertex at load time by the asset server, so all
//! entries use [`Material::Default`].
//!
//! Layout: trees + boulders ring the hub on every wall-edge tile;
//! interior is sprinkled with low scatter (grass, flowers, mushrooms,
//! ferns, plants, pebbles, bushes).

use glam::Vec3;
use rift_engine::{Floor, Renderer};

use super::placement::{
    collect_floor_tiles, place_on_walls, scatter_on_tiles,
    ScatterPlacement, SmallRng, WallAnchor, WallPlacement,
};
use super::{ColliderShape, Material, PlacementHint, PropAsset};

const fn tree(gltf: &'static str, weight: u32) -> PropAsset {
    PropAsset {
        gltf, scale: 0.55, material: Material::Default,
        collider: ColliderShape::Trunk { half_extent: 0.25 },
        placement: PlacementHint::Free, weight,
    }
}
const fn boulder(gltf: &'static str, weight: u32) -> PropAsset {
    PropAsset {
        gltf, scale: 1.0, material: Material::Default,
        collider: ColliderShape::Aabb { shrink: 0.85 },
        placement: PlacementHint::Free, weight,
    }
}
const fn ground(gltf: &'static str, scale: f32, weight: u32) -> PropAsset {
    PropAsset {
        gltf, scale, material: Material::Default,
        collider: ColliderShape::None,
        placement: PlacementHint::Free, weight,
    }
}

/// Forest border: one prop per wall-adjacent tile.
pub const PERIMETER: &[PropAsset] = &[
    tree("assets/models/nature-props/glTF/CommonTree_1.gltf",  5),
    tree("assets/models/nature-props/glTF/CommonTree_2.gltf",  5),
    tree("assets/models/nature-props/glTF/CommonTree_3.gltf",  5),
    tree("assets/models/nature-props/glTF/CommonTree_4.gltf",  4),
    tree("assets/models/nature-props/glTF/CommonTree_5.gltf",  4),
    tree("assets/models/nature-props/glTF/Pine_1.gltf",        2),
    tree("assets/models/nature-props/glTF/Pine_3.gltf",        2),
    tree("assets/models/nature-props/glTF/Pine_5.gltf",        2),
    tree("assets/models/nature-props/glTF/TwistedTree_2.gltf", 1),
    tree("assets/models/nature-props/glTF/DeadTree_3.gltf",    1),
    boulder("assets/models/nature-props/glTF/Rock_Medium_1.gltf", 1),
    boulder("assets/models/nature-props/glTF/Rock_Medium_2.gltf", 1),
    boulder("assets/models/nature-props/glTF/Rock_Medium_3.gltf", 1),
];

/// Inner-room scatter: never solid.
pub const SCATTER: &[PropAsset] = &[
    ground("assets/models/nature-props/glTF/Grass_Common_Short.gltf", 0.45, 8),
    ground("assets/models/nature-props/glTF/Grass_Common_Tall.gltf",  0.45, 6),
    ground("assets/models/nature-props/glTF/Grass_Wispy_Short.gltf",  0.45, 5),
    ground("assets/models/nature-props/glTF/Grass_Wispy_Tall.gltf",   0.45, 4),
    ground("assets/models/nature-props/glTF/Clover_1.gltf",           0.45, 4),
    ground("assets/models/nature-props/glTF/Clover_2.gltf",           0.45, 4),
    ground("assets/models/nature-props/glTF/Flower_3_Group.gltf",     0.45, 3),
    ground("assets/models/nature-props/glTF/Flower_3_Single.gltf",    0.45, 2),
    ground("assets/models/nature-props/glTF/Flower_4_Group.gltf",     0.45, 3),
    ground("assets/models/nature-props/glTF/Flower_4_Single.gltf",    0.45, 2),
    ground("assets/models/nature-props/glTF/Mushroom_Common.gltf",    0.45, 2),
    ground("assets/models/nature-props/glTF/Mushroom_Laetiporus.gltf",0.45, 1),
    ground("assets/models/nature-props/glTF/Fern_1.gltf",             0.45, 3),
    ground("assets/models/nature-props/glTF/Plant_1.gltf",            0.45, 2),
    ground("assets/models/nature-props/glTF/Plant_7.gltf",            0.45, 2),
    ground("assets/models/nature-props/glTF/Bush_Common.gltf",        0.50, 2),
    ground("assets/models/nature-props/glTF/Bush_Common_Flowers.gltf",0.50, 2),
    ground("assets/models/nature-props/glTF/Pebble_Round_2.gltf",     0.50, 2),
    ground("assets/models/nature-props/glTF/Pebble_Round_4.gltf",     0.50, 2),
    ground("assets/models/nature-props/glTF/Pebble_Square_2.gltf",    0.50, 2),
    ground("assets/models/nature-props/glTF/Pebble_Square_5.gltf",    0.50, 2),
];

/// Every gltf path the hub references, flat — for the preload phase.
pub fn hub_asset_paths() -> Vec<&'static str> {
    PERIMETER.iter().chain(SCATTER.iter()).map(|a| a.gltf).collect()
}

/// Number of distinct hub gltfs.
pub fn hub_total_assets() -> usize {
    PERIMETER.len() + SCATTER.len()
}

/// Decorate the hub: forest border + ground scatter.
pub fn decorate_hub(
    props: &mut super::Props,
    world: &mut hecs::World,
    renderer: &mut Renderer,
    floor: &Floor,
    seed: u64,
) {
    let mut rng = SmallRng::new(seed.wrapping_add(0x600D_F00D));
    let (border, interior) = collect_floor_tiles(floor);
    let centre = Vec3::new((floor.width / 2) as f32, 0.0, (floor.depth / 2) as f32);
    let avoid = [(floor.spawn_pos, 1.6), (centre, 1.6)];

    place_on_walls(
        props, world, renderer, border, &mut rng,
        &WallPlacement {
            assets: PERIMETER,
            count: None,
            anchor: WallAnchor::OnWallTile,
            wall_yaw_jitter_deg: 0.0,
            min_spacing: 0.0,
            avoid: &[],
            scale_jitter: (0.85, 1.15),
        },
    );

    let scatter_count = (interior.len() * 4).max(8);
    scatter_on_tiles(
        props, world, renderer, &interior, &mut rng,
        &ScatterPlacement {
            assets: SCATTER,
            count: scatter_count,
            min_spacing: 0.35,
            avoid: &avoid,
            sub_tile_jitter: 0.40,
            scale_jitter: (0.80, 1.40),
        },
    );
}
