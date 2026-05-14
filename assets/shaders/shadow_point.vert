#version 450

// Depth-and-distance vertex shader for the omnidirectional point-light
// shadow pass. Reuses the main pipeline's Vertex layout (binding 0).
//
// The fragment shader writes a normalized linear distance from the light
// to the color attachment; we just need to forward the per-vertex world
// position so the fragment can compute that distance, and project the
// vertex through the right cube face's view-projection matrix.

layout(location = 0) in vec3 inPosition;
layout(location = 1) in vec3 inNormal;
layout(location = 2) in vec3 inColor;
layout(location = 3) in vec2 inUV;

layout(set = 0, binding = 0) uniform UniformData {
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
    mat4 lightVP;
    // Per-face cube-shadow VPs, packed as
    //   [light0 +X, -X, +Y, -Y, +Z, -Z, light1 +X, -X, ...]
    // Indexed in the vertex shader by `pc.indices.x` (0 .. 23).
    mat4 pointShadowFaceVP[48];
    // x = number of active point lights with a shadow slot,
    // y/z/w unused.
    vec4 pointShadowMeta;
} ubo;

layout(push_constant) uniform PushConstants {
    mat4 model;
    // x = global face slot (light_idx * 6 + face_idx),
    // y = light index (used by frag for light pos + radius),
    // z/w reserved.
    uvec4 indices;
    // x = uvScale, y = parallaxScale, z = flags, w = reserved.
    vec4 materialParams;
} pc;

layout(location = 0) out vec3 worldPos;

void main() {
    vec4 wp = pc.model * vec4(inPosition, 1.0);
    worldPos = wp.xyz;
    gl_Position = ubo.pointShadowFaceVP[pc.indices.x] * wp;
}
