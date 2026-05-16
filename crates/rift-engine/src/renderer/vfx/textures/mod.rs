//! VFX texture registry — hybrid particle sampling.

mod library;

pub use library::{
    pack_hybrid_instance, HybridProfileGpu, VfxTextureId, VfxTextureLibrary, MAX_VFX_TEXTURES,
    SMOKE_BILLOW_PATH,
};
