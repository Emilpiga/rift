#version 450

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
    mat4 lightVP;
} ubo;

layout(push_constant) uniform PushConstants {
    mat4 model;
    vec4 tint;            // rgb multiplies frag colour, a is output alpha (frag-only)
    vec4 materialParams;  // x = uvScale, y = parallaxScale, z = flags, w = reserved
} push;

layout(location = 0) in vec3 inPosition;
layout(location = 1) in vec3 inNormal;
layout(location = 2) in vec3 inColor;
layout(location = 3) in vec2 inUV;

layout(location = 0) out vec3 fragWorldPos;
layout(location = 1) out vec3 fragNormal;
layout(location = 2) out vec3 fragColor;
layout(location = 3) out vec2 fragUV;

void main() {
    vec4 worldPos = push.model * vec4(inPosition, 1.0);
    gl_Position = ubo.proj * ubo.view * worldPos;
    fragWorldPos = worldPos.xyz;
    fragNormal = mat3(push.model) * inNormal;
    fragColor = inColor;
    // Per-object UV scale lets the same texture cover larger
    // floor / wall meshes without re-authoring per-tile UVs.
    // Defaults to 1.0 (raw mesh UVs) for legacy objects.
    float uvScale = max(push.materialParams.x, 1e-3);
    fragUV = inUV * uvScale;
}
