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
} ubo;

// Per-vertex quad corner (instanced)
layout(location = 0) in vec2 inCorner; // [-0.5, 0.5] quad corner

// Per-instance particle data
layout(location = 1) in vec3 inPosition;   // world position
layout(location = 2) in vec4 inColor;      // RGBA
layout(location = 3) in vec2 inSizeLife;   // x = size, y = life (0..1, 0 = dead)

layout(location = 0) out vec4 fragColor;
layout(location = 1) out vec2 fragUV;

void main() {
    // Billboard: extract camera right and up from view matrix
    vec3 camRight = vec3(ubo.view[0][0], ubo.view[1][0], ubo.view[2][0]);
    vec3 camUp    = vec3(ubo.view[0][1], ubo.view[1][1], ubo.view[2][1]);

    float size = inSizeLife.x;
    vec3 worldPos = inPosition
        + camRight * inCorner.x * size
        + camUp * inCorner.y * size;

    gl_Position = ubo.proj * ubo.view * vec4(worldPos, 1.0);

    fragColor = inColor;
    fragUV = inCorner + 0.5; // [0,1] range for texture sampling
}
