#version 450

// Fragment shader for the omnidirectional point-light shadow pass.
// Writes the world-space distance from the light to this fragment,
// normalized by the light's effective radius, into a R32_SFLOAT color
// attachment. The main fragment shader samples this atlas as a
// `samplerCubeArray` and compares the stored value to its own
// normalized distance to determine occlusion.

layout(location = 0) in vec3 worldPos;
layout(location = 0) out float outNormalizedDistance;

layout(set = 0, binding = 0) uniform UniformData {
    mat4 view;
    mat4 proj;
    vec4 cameraPos;
    vec4 lightDir;
    vec4 lightColor;
    vec4 fogColor;
    vec4 fogParams;
    vec4 fogOrigin;
    vec4 pointLightPos[16];   // xyz = position, w = radius
    vec4 pointLightColor[16];
    vec4 pointLightCount;
    mat4 lightVP;
    mat4 pointShadowFaceVP[48];
    vec4 pointShadowMeta;
} ubo;

layout(push_constant) uniform PushConstants {
    mat4 model;
    uvec4 indices;
} pc;

void main() {
    vec3 lightPos = ubo.pointLightPos[pc.indices.y].xyz;
    float radius  = max(ubo.pointLightPos[pc.indices.y].w, 1e-3);
    outNormalizedDistance = length(worldPos - lightPos) / radius;
}
