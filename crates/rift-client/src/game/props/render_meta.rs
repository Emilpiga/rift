//! Per-[`PropId`] render metadata: gltf path, material kind,
//! authored asset scale.
//!
//! Pure rendering data — the dungeon doesn't see it. Resolved
//! via an exhaustive `match` in [`render_meta`] so the
//! compiler enforces that every variant has an entry.

use rift_dungeon::props::PropId;

/// How the prop is shaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RenderMaterial {
    /// Engine default 1×1 white sampler. Vertex colours from
    /// the gltf supply all the look. Used by the nature pack.
    Default,
    /// Bind a shared descriptor set sampled from `path`. The
    /// first prop that requests this path uploads the texture
    /// and caches the descriptor set; subsequent props share
    /// it.
    SharedTexture(&'static str),
}

#[derive(Clone, Copy, Debug)]
pub struct RenderMeta {
    pub gltf: &'static str,
    pub material: RenderMaterial,
    /// Authored asset scale (multiplied with
    /// `PlacedProp::scale` at render time). 1.0 for most
    /// props; 0.45 for grass, 0.50 for pebbles, 0.9 for
    /// the stash chest.
    pub asset_scale: f32,
}

const TRIM_FURNITURE: &str = "assets/models/fantasy-props/Textures/T_Trim_Furniture_BaseColor.png";
const TRIM_METAL: &str = "assets/models/fantasy-props/Textures/T_Trim_Metal_BaseColor.png";
const TRIM_CLOTH: &str = "assets/models/fantasy-props/Textures/T_Trim_Cloth_BaseColor.png";
const TRIM_PROPS: &str = "assets/models/fantasy-props/Textures/T_Trim_Props_BaseColor.png";

const fn meta(gltf: &'static str, material: RenderMaterial, asset_scale: f32) -> RenderMeta {
    RenderMeta {
        gltf,
        material,
        asset_scale,
    }
}
const fn furn(gltf: &'static str) -> RenderMeta {
    meta(gltf, RenderMaterial::SharedTexture(TRIM_FURNITURE), 1.0)
}
const fn metal(gltf: &'static str) -> RenderMeta {
    meta(gltf, RenderMaterial::SharedTexture(TRIM_METAL), 1.0)
}
const fn cloth(gltf: &'static str) -> RenderMeta {
    meta(gltf, RenderMaterial::SharedTexture(TRIM_CLOTH), 1.0)
}
const fn small(gltf: &'static str) -> RenderMeta {
    meta(gltf, RenderMaterial::SharedTexture(TRIM_PROPS), 1.0)
}
const fn nature(gltf: &'static str, asset_scale: f32) -> RenderMeta {
    meta(gltf, RenderMaterial::Default, asset_scale)
}

/// Look up the render metadata for `id`.
///
/// Implemented as an exhaustive `match` so the compiler
/// enforces that every [`PropId`] variant has an entry —
/// adding a new variant produces a non-exhaustive error
/// here until you fill it in. This avoids the parallel-table
/// drift that an indexed slice would invite.
pub const fn render_meta(id: PropId) -> RenderMeta {
    use PropId::*;
    match id {
        // ---- Fantasy / dungeon ----
        Cauldron => metal("assets/models/fantasy-props/Exports/glTF/Cauldron.gltf"),
        CandleStickTriple => {
            metal("assets/models/fantasy-props/Exports/glTF/CandleStick_Triple.gltf")
        }
        CandleStick => metal("assets/models/fantasy-props/Exports/glTF/CandleStick.gltf"),
        CandleStickStand => {
            metal("assets/models/fantasy-props/Exports/glTF/CandleStick_Stand.gltf")
        }
        CageSmall => metal("assets/models/fantasy-props/Exports/glTF/Cage_Small.gltf"),
        ChainCoil => metal("assets/models/fantasy-props/Exports/glTF/Chain_Coil.gltf"),
        Anvil => metal("assets/models/fantasy-props/Exports/glTF/Anvil.gltf"),
        AnvilLog => metal("assets/models/fantasy-props/Exports/glTF/Anvil_Log.gltf"),
        AxeBronze => metal("assets/models/fantasy-props/Exports/glTF/Axe_Bronze.gltf"),
        BookStand => furn("assets/models/fantasy-props/Exports/glTF/BookStand.gltf"),
        BookStack1 => small("assets/models/fantasy-props/Exports/glTF/Book_Stack_1.gltf"),
        BookStack2 => small("assets/models/fantasy-props/Exports/glTF/Book_Stack_2.gltf"),
        BookGroupMedium1 => {
            small("assets/models/fantasy-props/Exports/glTF/BookGroup_Medium_1.gltf")
        }
        BookGroupMedium2 => {
            small("assets/models/fantasy-props/Exports/glTF/BookGroup_Medium_2.gltf")
        }
        BookGroupMedium3 => {
            small("assets/models/fantasy-props/Exports/glTF/BookGroup_Medium_3.gltf")
        }
        BookGroupSmall1 => small("assets/models/fantasy-props/Exports/glTF/BookGroup_Small_1.gltf"),
        Bookcase2 => furn("assets/models/fantasy-props/Exports/glTF/Bookcase_2.gltf"),
        Cabinet => furn("assets/models/fantasy-props/Exports/glTF/Cabinet.gltf"),
        Bench => furn("assets/models/fantasy-props/Exports/glTF/Bench.gltf"),
        BedTwin1 => furn("assets/models/fantasy-props/Exports/glTF/Bed_Twin1.gltf"),
        BedTwin2 => furn("assets/models/fantasy-props/Exports/glTF/Bed_Twin2.gltf"),
        Barrel => furn("assets/models/fantasy-props/Exports/glTF/Barrel.gltf"),
        BarrelApples => furn("assets/models/fantasy-props/Exports/glTF/Barrel_Apples.gltf"),
        BarrelHolder => furn("assets/models/fantasy-props/Exports/glTF/Barrel_Holder.gltf"),
        BucketWooden1 => furn("assets/models/fantasy-props/Exports/glTF/Bucket_Wooden_1.gltf"),
        BucketMetal => metal("assets/models/fantasy-props/Exports/glTF/Bucket_Metal.gltf"),
        Bag => cloth("assets/models/fantasy-props/Exports/glTF/Bag.gltf"),
        Banner1 => cloth("assets/models/fantasy-props/Exports/glTF/Banner_1.gltf"),
        Banner2 => cloth("assets/models/fantasy-props/Exports/glTF/Banner_2.gltf"),
        Banner1Cloth => cloth("assets/models/fantasy-props/Exports/glTF/Banner_1_Cloth.gltf"),
        Banner2Cloth => cloth("assets/models/fantasy-props/Exports/glTF/Banner_2_Cloth.gltf"),
        Bottle1 => small("assets/models/fantasy-props/Exports/glTF/Bottle_1.gltf"),
        Candle1 => small("assets/models/fantasy-props/Exports/glTF/Candle_1.gltf"),
        Candle2 => small("assets/models/fantasy-props/Exports/glTF/Candle_2.gltf"),
        Carrot => small("assets/models/fantasy-props/Exports/glTF/Carrot.gltf"),
        // ---- Nature / hub ----
        GrassCommonShort => nature(
            "assets/models/nature-props/glTF/Grass_Common_Short.gltf",
            0.45,
        ),
        GrassWispyShort => nature(
            "assets/models/nature-props/glTF/Grass_Wispy_Short.gltf",
            0.45,
        ),
        PebbleRound2 => nature("assets/models/nature-props/glTF/Pebble_Round_2.gltf", 0.50),
        PebbleRound4 => nature("assets/models/nature-props/glTF/Pebble_Round_4.gltf", 0.50),
        // ---- Special ----
        StashChest => RenderMeta {
            gltf: "assets/models/chest/Low_Poly_Chest [2.0] GLTF.glb",
            material: RenderMaterial::SharedTexture(
                "assets/models/chest/Low_Poly_Chest [2.0] Textures/Low_Poly_Chest [2.0] Color.png",
            ),
            asset_scale: 1.0,
        },
        VoidForge => metal("assets/models/fantasy-props/Exports/glTF/Anvil.gltf"),
    }
}
