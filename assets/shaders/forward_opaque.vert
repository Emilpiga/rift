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
    vec4 pointLightPos[16];
    vec4 pointLightColor[16];
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
layout(location = 4) out vec3 fragLocalPos;

void main() {
    vec4 worldPos = push.model * vec4(inPosition, 1.0);
    uint flags = floatBitsToUint(push.materialParams.z);
    vec3 worldNormal = transpose(inverse(mat3(push.model))) * inNormal;
    vec4 clipPos = ubo.proj * ubo.view * worldPos;
    if ((flags & 128u) != 0u) {
        vec4 centerWorld = push.model * vec4(0.0, 0.9, 0.0, 1.0);
        vec4 centerClip = ubo.proj * ubo.view * centerWorld;
        vec2 posNdc = clipPos.xy / max(clipPos.w, 1e-4);
        vec2 centerNdc = centerClip.xy / max(centerClip.w, 1e-4);
        vec2 shellDir = posNdc - centerNdc;
        float shellLen = length(shellDir);
        if (shellLen > 1e-4) {
            float outlineWidth = min(0.0055, shellLen * 0.055);
            clipPos.xy += (shellDir / shellLen) * outlineWidth * clipPos.w;
        }
    }
    gl_Position = clipPos;
    if ((flags & 32u) != 0u) {
        gl_Position.z = gl_Position.w * 0.01;
    }
    fragWorldPos = worldPos.xyz;
    fragNormal = worldNormal;
    fragColor = inColor;
    fragLocalPos = inPosition;
    // Per-object UV scale lets the same texture cover larger
    // floor / wall meshes without re-authoring per-tile UVs.
    // Defaults to 1.0 (raw mesh UVs) for legacy objects.
    float uvScale = max(push.materialParams.x, 1e-3);
    fragUV = inUV * uvScale;
}
