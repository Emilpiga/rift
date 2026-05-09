#version 450

// Depth-only vertex shader for the directional-light shadow pass.
//
// Reuses the main pipeline's Vertex layout (binding 0). Only the position is
// needed; normal/color/uv are skipped via location-but-no-output (the
// fragment stage doesn't exist).

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
    mat4 pointShadowFaceVP[48];
    vec4 pointShadowMeta;
} ubo;

layout(push_constant) uniform PushConstants {
    mat4 model;
} pc;

void main() {
    gl_Position = ubo.lightVP * pc.model * vec4(inPosition, 1.0);
}
