//! Outdoor / nature prop pack.
//!
//! Vertex-coloured (no shared material) — the `gltf` baseColorTexture
//! is sampled per-vertex at load time by the asset server, so all
//! entries use [`Material::Default`].
//!
//! Layout: the hub is a "mysterious circular platform" — no perimeter
//! trees or boulders. The interior is sprinkled with low scatter
//! (grass, flowers, mushrooms, ferns, plants, pebbles, bushes). The
//! illusion of a bigger world is sold by the fog horizon and the
//! oversized grass apron disc rendered by `floor::generate_hub`.

use glam::Vec3;
use rift_engine::{Floor, Renderer};

use super::placement::{collect_floor_tiles, scatter_on_tiles, ScatterPlacement, SmallRng};
use super::{ColliderShape, Material, PlacementHint, PropAsset};

const fn ground(gltf: &'static str, scale: f32, weight: u32) -> PropAsset {
    PropAsset {
        gltf, scale, material: Material::Default,
        collider: ColliderShape::None,
        placement: PlacementHint::Free, weight,
    }
}

/// Inner-room scatter: never solid.
pub const SCATTER: &[PropAsset] = &[
    ground("assets/models/nature-props/glTF/Grass_Common_Short.gltf", 0.45, 8),
    ground("assets/models/nature-props/glTF/Grass_Common_Tall.gltf",  0.45, 6),
    ground("assets/models/nature-props/glTF/Grass_Wispy_Short.gltf",  0.45, 5),
    ground("assets/models/nature-props/glTF/Grass_Wispy_Tall.gltf",   0.45, 4),
    ground("assets/models/nature-props/glTF/Clover_1.gltf",           0.45, 4),
    ground("assets/models/nature-props/glTF/Clover_2.gltf",           0.45, 4),
    ground("assets/models/nature-props/glTF/Flower_3_Single.gltf",    0.45, 2),
    ground("assets/models/nature-props/glTF/Flower_4_Single.gltf",    0.45, 2),
    ground("assets/models/nature-props/glTF/Pebble_Round_2.gltf",     0.50, 2),
    ground("assets/models/nature-props/glTF/Pebble_Round_4.gltf",     0.50, 2),
    ground("assets/models/nature-props/glTF/Pebble_Square_2.gltf",    0.50, 2),
    ground("assets/models/nature-props/glTF/Pebble_Square_5.gltf",    0.50, 2),
];

/// Player stash chest, placed once at a fixed spot in the hub. Solid
/// AABB collider so the player can't walk through it. Loaded as a
/// `.glb` from the dedicated `assets/models/chest/` folder. The
/// `.glb` embeds its baseColorTexture as a buffer view, which the
/// static prop loader can't bake into vertex colours — so we
/// bind the sidecar PNG directly via `SharedTexture` instead.
pub const STASH_CHEST: PropAsset = PropAsset {
    gltf: "assets/models/chest/Low_Poly_Chest [2.0] GLTF.glb",
    scale: 0.9,
    material: Material::SharedTexture(
        "assets/models/chest/Low_Poly_Chest [2.0] Textures/Low_Poly_Chest [2.0] Color.png",
    ),
    collider: ColliderShape::Aabb { shrink: 0.9 },
    placement: PlacementHint::Free,
    weight: 1,
};

/// Every gltf path the hub references, flat — for the preload phase.
pub fn hub_asset_paths() -> Vec<&'static str> {
    let mut paths: Vec<&'static str> = SCATTER.iter().map(|a| a.gltf).collect();
    paths.push(STASH_CHEST.gltf);
    paths
}

/// Number of distinct hub gltfs.
pub fn hub_total_assets() -> usize {
    SCATTER.len() + 1
}

/// Decorate the hub: ground scatter only (no trees / boulders).
pub fn decorate_hub(
    props: &mut super::Props,
    world: &mut hecs::World,
    renderer: &mut Renderer,
    floor: &Floor,
    seed: u64,
) {
    let mut rng = SmallRng::new(seed.wrapping_add(0x600D_F00D));
    let (_border, interior) = collect_floor_tiles(floor);
    let centre = Vec3::new((floor.width / 2) as f32, 0.0, (floor.depth / 2) as f32);
    let avoid = [(floor.spawn_pos, 1.6), (centre, 1.6)];

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
