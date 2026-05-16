// Must match `SpriteShape` / GPU sprite index in `runtime.rs`.
const uint SPRITE_SOFT_GLOW   = 0u;
const uint SPRITE_SPARK       = 1u;
const uint SPRITE_SMOKE       = 2u;
const uint SPRITE_SHARD       = 3u;
const uint SPRITE_RING        = 4u;
const uint SPRITE_STREAK      = 5u;
const uint SPRITE_WISP        = 6u;
const uint SPRITE_SILK_STRAND = 7u;
const uint SPRITE_GROUND_CRACK = 8u;
const uint SPRITE_FLAME       = 9u;
const uint SPRITE_HYBRID      = 10u;

// Semantic roles — match `VfxRole::gpu_role_id` in Rust.
const float ROLE_CORE      = 1.0;
const float ROLE_FILAMENT  = 2.0;
const float ROLE_RUPTURE   = 3.0;
const float ROLE_VAPOR     = 4.0;
const float ROLE_IMPACT    = 5.0;
const float ROLE_RESIDUE   = 6.0;
