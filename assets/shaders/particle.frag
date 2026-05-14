#version 450

layout(location = 0) in vec4 fragColor;
layout(location = 1) in vec2 fragUV;

layout(location = 0) out vec4 outColor;

void main() {
    // Soft circular particle: analytic antialiasing plus a tiny
    // bright core so legacy particles read closer to the newer
    // procedural VFX sprites.
    vec2 centered = fragUV - 0.5;
    float dist = length(centered) * 2.0; // 0 at center, 1 at edge
    float aa = max(fwidth(dist), 0.0015);
    float alpha = 1.0 - smoothstep(0.72 - aa, 1.0 + aa, dist);
    float core = exp(-dist * dist * 9.0);
    float ring = 1.0 - smoothstep(0.050 - aa, 0.050 + aa, abs(dist - 0.48));
    vec3 rgb = fragColor.rgb * (1.0 + core * 0.24 + ring * 0.055);

    outColor = vec4(rgb, fragColor.a * alpha);
}
