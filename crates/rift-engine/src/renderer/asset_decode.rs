//! Background PNG decode + GPU upload split for authored
//! materials.
//!
//! Authored 2 k PBR packs are ~5–6 large PNGs each. Decoding
//! them takes the bulk of the wall-clock cost (the deflate
//! pass is purely CPU); the GPU upload itself is a buffer copy
//! plus a queue submit and runs in the tens of milliseconds.
//!
//! This module separates the two halves so the slow CPU work
//! can run on a worker thread while the netcode loop keeps
//! pumping on the main thread:
//!
//! 1. **CPU side, off-thread.** [`decode_srgb`], [`decode_linear`],
//!    and [`decode_mr_atlas`] read a PNG / JPG from disk and
//!    return a [`DecodedTexture`] with the raw RGBA bytes plus
//!    the Vulkan format the GPU side should use. They never
//!    touch any Vulkan handle, so they're trivially `Send` and
//!    safe to run on any worker thread.
//! 2. **GPU side, main thread.** The [`Renderer`] takes the
//!    [`DecodedTexture`] and runs only the buffer-copy + image-
//!    create + descriptor-set steps. No PNG work runs on the
//!    main thread once a pack has been pre-decoded.
//!
//! The pair of functions here is what the asset / environment
//! code uses to turn a list of paths into a list of
//! [`DecodedPbrPack`] / [`DecodedTexture`] values that can be
//! shipped over a `mpsc` channel and consumed each frame.

use std::path::{Path, PathBuf};

use anyhow::Result;
use ash::vk;

/// One fully-decoded RGBA8 image plus the GPU format it should
/// be uploaded with. Held in CPU memory only — there's no
/// Vulkan handle inside, so values of this type are safe to
/// send across threads via `mpsc` channels.
pub struct DecodedTexture {
    pub width: u32,
    pub height: u32,
    /// Tightly-packed `width * height * 4` bytes of RGBA8.
    pub pixels: Vec<u8>,
    /// Format the renderer should use when uploading. Picked
    /// at decode time so the worker thread can pre-classify
    /// each map's colour-space (SRGB for basecolor, UNORM for
    /// numeric data textures).
    pub format: vk::Format,
}

/// Fully-decoded set of PBR-pack maps, ready to be uploaded
/// into a single per-object descriptor set.
///
/// The metallic + roughness inputs are pre-merged on the
/// worker thread into a single packed UNORM atlas
/// (`R = metallic`, `G = roughness`, BA unused), matching what
/// `Renderer::upload_shared_pbr_material_split_mr` produces
/// internally. This keeps the main-thread upload step
/// straight-line.
pub struct DecodedPbrPack {
    /// Friendly name for log messages (`"cliff_rocks"`,
    /// `"ground_tiles"`, …). Not used by the renderer.
    pub name: String,
    /// Base colour map. SRGB.
    pub basecolor: DecodedTexture,
    /// Tangent-space normal map. UNORM.
    pub normal: Option<DecodedTexture>,
    /// Pre-packed metallic + roughness atlas. UNORM,
    /// `R = metallic`, `G = roughness`.
    pub mr: Option<DecodedTexture>,
    /// Ambient-occlusion map. UNORM.
    pub ao: Option<DecodedTexture>,
    /// Parallax / height map. UNORM.
    pub height: Option<DecodedTexture>,
}

/// Resolve `path` against the same set of cwd-relative
/// candidates the renderer's synchronous loaders use, so
/// callers can pass `assets/...` from any cwd. Returns the
/// first existing candidate.
pub fn resolve_asset_path(path: &Path) -> Result<PathBuf> {
    let candidates = [
        path.to_path_buf(),
        PathBuf::from("..").join(path),
        PathBuf::from("../..").join(path),
        PathBuf::from("../../..").join(path),
    ];
    candidates
        .iter()
        .find(|c| c.exists())
        .cloned()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "asset not found in any candidate path (cwd={:?}): {:?}",
                std::env::current_dir().ok(),
                path,
            )
        })
}

/// Decode a colour-space PNG/JPG (one that should be sampled
/// in SRGB by the GPU). Suitable for basecolor / albedo maps.
pub fn decode_srgb(path: &Path) -> Result<DecodedTexture> {
    decode_with_format(path, vk::Format::R8G8B8A8_SRGB)
}

/// Decode a numeric PNG/JPG (normal map, AO, height — anything
/// that is not perceptual colour). Sampled UNORM so the GPU
/// doesn't apply an sRGB → linear curve to data.
pub fn decode_linear(path: &Path) -> Result<DecodedTexture> {
    decode_with_format(path, vk::Format::R8G8B8A8_UNORM)
}

fn decode_with_format(path: &Path, format: vk::Format) -> Result<DecodedTexture> {
    let resolved = resolve_asset_path(path)?;
    let img = image::open(&resolved)
        .map_err(|e| anyhow::anyhow!("texture decode failed for {:?}: {}", resolved, e))?
        .to_rgba8();
    let (w, h) = (img.width(), img.height());
    Ok(DecodedTexture {
        width: w,
        height: h,
        pixels: img.into_raw(),
        format,
    })
}

/// Decode the metallic + roughness PNG pair into a single
/// packed UNORM atlas. Either side may be `None` (in which
/// case the missing channel falls back to the pool's neutral
/// default downstream — metallic→0, roughness→255). At least
/// one side must be `Some` or the function returns `Ok(None)`.
pub fn decode_mr_atlas(
    metallic_path: Option<&Path>,
    roughness_path: Option<&Path>,
) -> Result<Option<DecodedTexture>> {
    if metallic_path.is_none() && roughness_path.is_none() {
        return Ok(None);
    }
    let metallic = match metallic_path {
        Some(p) => Some(image::open(resolve_asset_path(p)?)?.to_luma8()),
        None => None,
    };
    let roughness = match roughness_path {
        Some(p) => Some(image::open(resolve_asset_path(p)?)?.to_luma8()),
        None => None,
    };
    let (w, h) = match (&metallic, &roughness) {
        (Some(m), Some(r)) => {
            if m.dimensions() != r.dimensions() {
                return Err(anyhow::anyhow!(
                    "metallic and roughness map dimensions differ: {:?} vs {:?}",
                    m.dimensions(),
                    r.dimensions()
                ));
            }
            m.dimensions()
        }
        (Some(m), None) => m.dimensions(),
        (None, Some(r)) => r.dimensions(),
        (None, None) => unreachable!(),
    };
    let mut packed = vec![0u8; (w * h * 4) as usize];
    for i in 0..(w * h) as usize {
        packed[i * 4] = metallic.as_ref().map(|m| m.as_raw()[i]).unwrap_or(0);
        packed[i * 4 + 1] = roughness.as_ref().map(|r| r.as_raw()[i]).unwrap_or(255);
        packed[i * 4 + 2] = 0;
        packed[i * 4 + 3] = 255;
    }
    Ok(Some(DecodedTexture {
        width: w,
        height: h,
        pixels: packed,
        format: vk::Format::R8G8B8A8_UNORM,
    }))
}
