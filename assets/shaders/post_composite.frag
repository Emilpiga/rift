#version 450

// Final composite + tonemap. Reads the HDR scene plus the
// blurred bloom and writes to the swapchain (sRGB). We tonemap
// the *combined* HDR + bloom signal so the bloom doesn't read
// as a separate flat layer.

layout(location = 0) in  vec2 v_uv;
layout(location = 0) out vec4 outColor;

layout(set = 0, binding = 0) uniform sampler2D u_hdr;
layout(set = 0, binding = 1) uniform sampler2D u_bloom;

layout(push_constant) uniform Push {
    float bloom_intensity; // multiplier on bloom contribution
    float exposure;        // scene exposure scalar (1.0 default)
    float _pad0;
    float _pad1;
} pc;

// Narkowicz ACES filmic tonemap — cheap, hits LDR cleanly,
// holds saturation in highlights. Output is in linear space;
// the swapchain is sRGB so the GPU does the gamma encode.
vec3 aces(vec3 x) {
    const float a = 2.51;
    const float b = 0.03;
    const float c = 2.43;
    const float d = 0.59;
    const float e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), 0.0, 1.0);
}

void main() {
    vec3 hdr   = texture(u_hdr,   v_uv).rgb;
    vec3 bloom = texture(u_bloom, v_uv).rgb;
    vec3 col   = (hdr + bloom * pc.bloom_intensity) * pc.exposure;
    outColor = vec4(aces(col), 1.0);
}
