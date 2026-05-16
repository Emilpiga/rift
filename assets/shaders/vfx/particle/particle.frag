#version 450

// VFX particle fragment shader — modular layout under `vfx/particle/`.
// Sprite indices match `SpriteShape` in the engine.
//
//   0 SoftGlow  1 Spark  2 Smoke  3 Shard  4 Ring  5 Streak
//   6 Wisp  7 SilkStrand  8 GroundCrack  9 Flame  10 Hybrid
//
// Output is HDR pre-multiplied alpha (SRC = ONE for alpha/additive).

#include "uniforms.glsl"
#include "noise.glsl"
#include "shapes/soft_glow.glsl"
#include "shapes/spark.glsl"
#include "shapes/smoke.glsl"
#include "shapes/hybrid.glsl"
#include "shapes/shard.glsl"
#include "shapes/ring.glsl"
#include "shapes/streak.glsl"
#include "shapes/wisp.glsl"
#include "shapes/silk_strand.glsl"
#include "shapes/ground_crack.glsl"
#include "shapes/flame.glsl"
#include "evaluate_sprite.glsl"
#include "composite.glsl"
