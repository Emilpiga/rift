layout(binding = 0) uniform UniformData {
    mat4 view;
    mat4 proj;
    vec4 cameraPos;
    vec4 lightDir;
    vec4 lightColor;
    vec4 fogColor;
    vec4 fogParams;
    vec4 fogOrigin;
    vec4 pointLightPos[16];
    vec4 pointLightColor[16];
    vec4 pointLightCount;
    // Padding to reach `timeData` at the same std140 offset as
    // the world shader's UBO (binding 0 is shared across every
    // pipeline that uses descriptor set 0). The particle shader
    // doesn't actually read these shadow fields.
    mat4 lightVP;
    mat4 pointShadowFaceVP[48];
    vec4 pointShadowMeta;
    /// x = seconds since renderer start. Powers flow-map UV
    /// scrolling and temporal noise modulation.
    vec4 timeData;
} ubo;

// Set 1, binding 0: the scene depth buffer captured by the
// opaque scene pass. Sampled here so particles can fade out
// smoothly as they approach world geometry — without this the
// fragment alpha is binary (depth-test pass/fail) and the
// silhouette of a smoke puff intersecting a wall reads as a
// hard, flickering edge. Linear sampler is fine; depth values
// don't average meaningfully but for soft-particle fade we
// only care about the broad relationship to fragment depth.
layout(set = 1, binding = 0) uniform sampler2D sceneDepth;
layout(set = 1, binding = 1) uniform sampler2D vfxTextures[8];

// Static index per slot — avoids broken dynamic sampler-array indexing
// on drivers without full descriptor indexing.
float sampleVfxDensity(uint idx, vec2 uv) {
    vec4 tex;
    if (idx == 0u) tex = texture(vfxTextures[0], uv);
    else if (idx == 1u) tex = texture(vfxTextures[1], uv);
    else if (idx == 2u) tex = texture(vfxTextures[2], uv);
    else if (idx == 3u) tex = texture(vfxTextures[3], uv);
    else if (idx == 4u) tex = texture(vfxTextures[4], uv);
    else if (idx == 5u) tex = texture(vfxTextures[5], uv);
    else if (idx == 6u) tex = texture(vfxTextures[6], uv);
    else tex = texture(vfxTextures[7], uv);
    // Authored density: R carries smoke structure, A carries cutout/soft
    // transparency when present. Multiplying preserves both; `max(r,a)`
    // filled holes, while `r` alone ignored transparent PNG pixels.
    return tex.r * tex.a;
}

layout(location = 0) in vec4  vColor;
layout(location = 1) in vec2  vUv;
layout(location = 2) flat in uint vSprite;
layout(location = 3) in float vSeed;
layout(location = 4) in vec2  vStretchDir;   // direction & magnitude (0..2)
layout(location = 5) in float vFogFactor;
layout(location = 6) in float vDistDim;
layout(location = 7) in float vLifeT;
layout(location = 8) in float vWorldY;
layout(location = 9) in float vOriginY;
layout(location = 10) in vec2 vWorldXZ;
layout(location = 11) flat in vec4 vHybridMeta;
layout(location = 12) flat in vec4 vHybridParams;
layout(location = 13) flat in vec2 vWorldXZAnchor;
layout(location = 14) flat in vec4 vStylePack;
layout(location = 15) flat in vec4 vStyleAux;
layout(location = 16) flat in vec4 vRolePack;

layout(location = 0) out vec4 outColor;

// Height above spawn (metres) → subtle colour-temperature shift.
// Hot white-yellow near the wick, deeper orange higher in the plume.
vec3 heightTemperatureTint(float worldY, float originY, uint sprite) {
    float h = clamp(worldY - originY, 0.0, 0.45);
    float t = clamp(h / 0.38, 0.0, 1.0);
    vec3 hot  = vec3(1.07, 1.02, 0.86);
    vec3 cool = vec3(1.00, 0.58, 0.20);
    if (sprite == 2u) {
        hot  = vec3(1.02, 0.92, 0.82);
        cool = vec3(0.72, 0.58, 0.52);
    }
    return mix(hot, cool, t);
}

// Linearise a Vulkan depth-buffer value (z_ndc in [0,1]) into
// a *positive* eye-space distance. For our standard perspective
// projection (looking down -Z, depth 0..1):
//
//     z_ndc = (proj[2][2] * z_eye + proj[3][2]) / -z_eye
//
// Solving for z_eye gives a negative number; we negate so the
// returned value is a positive linear distance from the eye,
// which is what the soft-particle compare expects.
float linearEyeDepth(float z_ndc) {
    return ubo.proj[3][2] / (z_ndc + ubo.proj[2][2]);
}
