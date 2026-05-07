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
    float ghost_mix;       // 0 = normal, 1 = full ghost view
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
    vec3 mapped = aces(col);

    // Ghost view post effect. Mixed in by `ghost_mix` so the
    // local client can hold ramp control (instant-on for now,
    // could be eased later). Three components stacked on the
    // tonemapped LDR colour:
    //   1. Desaturate to luma (Rec.709 weights).
    //   2. Cool cyan-blue tint added on top of the luma.
    //   3. Radial vignette darkens the edges, keeps the centre
    //      readable.
    if (pc.ghost_mix > 0.0001) {
        float luma = dot(mapped, vec3(0.2126, 0.7152, 0.0722));
        vec3 desat = vec3(luma);
        vec3 cool = desat * vec3(0.78, 0.92, 1.10);
        // Radial vignette: 0 at centre, ~0.55 at corners. We
        // bias the centre a bit toward the upper third so the
        // player avatar (drawn lower-centre) stays cleanly
        // visible.
        vec2 c = v_uv - vec2(0.5, 0.45);
        c.x *= 1.6; // squash horizontally so vignette isn't elliptical
        float vig = clamp(dot(c, c) * 1.4, 0.0, 0.85);
        vec3 ghost = mix(cool, vec3(0.02, 0.04, 0.07), vig);
        mapped = mix(mapped, ghost, pc.ghost_mix);
    }

    outColor = vec4(mapped, 1.0);
}
