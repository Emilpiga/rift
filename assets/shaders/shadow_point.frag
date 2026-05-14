#version 450

// Fragment shader for the omnidirectional point-light shadow pass.
// Writes the world-space distance from the light to this fragment,
// normalized by the light's effective radius, into a R32_SFLOAT color
// attachment. The main fragment shader samples this atlas as a
// `samplerCubeArray` and compares the stored value to its own
// normalized distance to determine occlusion.

layout(location = 0) in vec3 worldPos;
layout(location = 1) in vec3 worldNormal;
layout(location = 2) in vec2 shadowUV;
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
    vec4 materialParams;
} pc;

// Matches the forward material descriptor set. The shadow pass only
// needs heightMap, but Vulkan pipeline layout compatibility requires
// binding the same full material set.
layout(set = 1, binding = 4) uniform sampler2D heightMap;

void main() {
    gl_FragDepth = gl_FragCoord.z;

    vec3 lightPos = ubo.pointLightPos[pc.indices.y].xyz;
    float radius  = max(ubo.pointLightPos[pc.indices.y].w, 1e-3);
    uint flags = floatBitsToUint(pc.materialParams.z);
    bool usePbrHeight = (flags & 1u) != 0u
        && ubo.pointShadowMeta.z > 0.5
        && pc.materialParams.y > 0.001;

    vec3 shadowWorldPos = worldPos;
    if (usePbrHeight) {
        float h = texture(heightMap, shadowUV).r;
        vec3 N = normalize(worldNormal);
        if (!any(isnan(N))) {
            shadowWorldPos += N * ((h - 0.5) * pc.materialParams.y * 14.0);
        }
    }

    if (usePbrHeight) {
        vec4 clip = ubo.pointShadowFaceVP[pc.indices.x] * vec4(shadowWorldPos, 1.0);
        gl_FragDepth = clamp(clip.z / max(clip.w, 1e-5), 0.0, 1.0);
    }

    outNormalizedDistance = length(shadowWorldPos - lightPos) / radius;
}
