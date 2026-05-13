layout(binding = 0) uniform UniformData {
    mat4 view;
    mat4 proj;
    vec4 cameraPos;
    vec4 lightDir;
    vec4 lightColor;
    vec4 fogColor;
    vec4 fogParams; // x = start, y = end
    vec4 fogOrigin; // xyz = world-space anchor (player) for fog distance
    vec4 pointLightPos[16];   // xyz = position, w = radius
    vec4 pointLightColor[16]; // xyz = color, w = intensity
    vec4 pointLightCount;    // x = count
    mat4 lightVP;            // directional light view-projection (for shadow map)
    // Per-face VPs for the cube shadow atlas. Unused by the main pass
    // (only the shadow_point pipeline reads them) but must be present
    // here so the UBO layout matches across all pipelines that bind
    // descriptor set 0.
    mat4 pointShadowFaceVP[48];
    // x = active point-shadow caster count (0..=4). The point-light
    // loop below uses this to decide which point lights have a cube
    // atlas slot (and so should be sampled for occlusion) vs. which
    // are shadowless additive fill.
    vec4 pointShadowMeta;
    // x = seconds since renderer start. Used by the blood-field
    // composite to compute splat age (`u_time - splat_spawn_time`)
    // and drive the wet→dry tween.
    // y = floor-plane world Y. The blood field is a top-down 2D
    // accumulation texture, so any horizontal surface (wall caps,
    // pillar tops, raised platforms) above the floor plane would
    // otherwise pick up bleed-through from a splat at the same XZ
    // below it. The composite rejects samples whose worldY differs
    // from this by more than a small tolerance.
    // zw reserved.
    vec4 timeData;
    // Blood-field world\u2192UV transform. xy = world XZ origin
    // (min corner of floor AABB), zw = inverse extent. When all
    // zero the field is inactive and the composite is skipped.
    vec4 bloodFieldXform;
    // Reserved \u2014 was a per-room AABB gate for the porthole.
    // The gate jittered at room boundaries (alcoves /
    // corridors aren't covered by a single BSP rect) and the
    // `tFrag < distPlayer` check already prevents seeing past
    // the player into the next room, so the gate was dropped.
    // Kept in the layout to preserve the UBO size.
    vec4 reservedRoomAabb;
} ubo;

layout(set = 0, binding = 1) uniform sampler2D unusedSampler; // legacy slot, kept for descriptor compatibility
layout(set = 0, binding = 2) uniform sampler2DShadow shadowMap;
layout(set = 0, binding = 3) uniform samplerCubeArray pointShadowAtlas;
layout(set = 0, binding = 4) uniform sampler2D bloodField;

// Per-object PBR material set. Bindings must match the
// `BINDING_*` constants in `crates/rift-engine/src/renderer/material.rs`.
layout(set = 1, binding = 0) uniform sampler2D baseColorMap;
layout(set = 1, binding = 1) uniform sampler2D normalMap;
layout(set = 1, binding = 2) uniform sampler2D mrMap;     // R = metallic, G = roughness
layout(set = 1, binding = 3) uniform sampler2D aoMap;
layout(set = 1, binding = 4) uniform sampler2D heightMap;

layout(location = 0) in vec3 fragWorldPos;
layout(location = 1) in vec3 fragNormal;
layout(location = 2) in vec3 fragColor;
layout(location = 3) in vec2 fragUV;

layout(location = 0) out vec4 outColor;

// Per-object push constant. Layout must mirror the vert
// (`mat4 model` at offset 0, `vec4 tint` at 64,
// `vec4 materialParams` at 80) because Vulkan validates
// push-constant ranges per pipeline. `materialParams`:
//   x = uvScale          (already applied to fragUV in the vert)
//   y = parallaxScale    (tangent-space parallax depth amplitude;
//                         `0` disables parallax)
//   z = flagsFloat       (bit 0 = enable PBR + normal mapping)
//   w = reserved
layout(push_constant) uniform PushConstants {
    mat4 model;
    vec4 tint;
    vec4 materialParams;
} push;

const float PI = 3.14159265359;
