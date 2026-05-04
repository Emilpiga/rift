#version 450

layout(location = 0) in vec4 fragColor;
layout(location = 1) in vec2 fragUV;

layout(location = 0) out vec4 outColor;

void main() {
    // Soft circular particle: fade alpha by distance from center
    vec2 centered = fragUV - 0.5;
    float dist = length(centered) * 2.0; // 0 at center, 1 at edge
    float alpha = 1.0 - smoothstep(0.6, 1.0, dist);

    outColor = vec4(fragColor.rgb, fragColor.a * alpha);
}
