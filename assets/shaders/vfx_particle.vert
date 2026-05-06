#version 450

// VFX particle: instanced billboard with procedural sprite shape.
//
// Each instance carries everything the fragment shader needs —
// world position, size, HDR colour (already gradient-sampled
// CPU-side), sprite shape index, and seed for per-particle
// procedural variation. No texture atlas.

layout(binding = 0) uniform UniformData {
    mat4 view;
    mat4 proj;
    vec4 cameraPos;
    vec4 lightDir;
    vec4 lightColor;
    vec4 fogColor;
    vec4 fogParams;
    vec4 fogOrigin;
    vec4 pointLightPos[8];
    vec4 pointLightColor[8];
    vec4 pointLightCount;
} ubo;

// Per-vertex quad corner (binding 0).
layout(location = 0) in vec2 inCorner; // [-0.5, 0.5]^2

// Per-instance (binding 1) — must match VfxParticleInstance.
layout(location = 1) in vec4 inPosSize;     // xyz = position, w = size
layout(location = 2) in vec4 inColor;       // HDR rgba (alpha = opacity)
layout(location = 3) in vec4 inMisc;        // x = seed, y = sprite (uint), z = blend, w = _pad

layout(location = 0) out vec4 vColor;
layout(location = 1) out vec2 vUv;          // [0, 1]^2 within the quad
layout(location = 2) flat out uint vSprite;
layout(location = 3) out float vSeed;

void main() {
    vec3 camRight = vec3(ubo.view[0][0], ubo.view[1][0], ubo.view[2][0]);
    vec3 camUp    = vec3(ubo.view[0][1], ubo.view[1][1], ubo.view[2][1]);

    float size = inPosSize.w;
    vec3 worldPos = inPosSize.xyz
        + camRight * inCorner.x * size
        + camUp    * inCorner.y * size;

    gl_Position = ubo.proj * ubo.view * vec4(worldPos, 1.0);

    vColor  = inColor;
    vUv     = inCorner + 0.5;
    vSprite = floatBitsToUint(inMisc.y);
    vSeed   = inMisc.x;
}
