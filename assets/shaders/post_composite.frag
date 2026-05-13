#version 450

// Final composite + tonemap. Reads heat-distorted HDR, blurred bloom,
// graph-produced AO and volumetrics, then tonemaps to the swapchain (sRGB).

layout(location = 0) in  vec2 v_uv;
layout(location = 0) out vec4 outColor;

layout(set = 0, binding = 0) uniform sampler2D u_hdr;
layout(set = 0, binding = 1) uniform sampler2D u_bloom;
layout(set = 0, binding = 2) uniform sampler2D u_ao;
layout(set = 0, binding = 3) uniform sampler2D u_volumetrics;

layout(push_constant) uniform Push {
    float bloom_intensity; // multiplier on bloom contribution
    float exposure;        // scene exposure scalar (1.0 default)
    float ghost_mix;       // 0 = normal, 1 = full ghost view
    float _pad0;
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

// ---------------------------------------------------------------
// Stylised grade.
//
// Applied AFTER the filmic tonemap, so it operates on a known
// `[0, 1]` LDR signal and is straightforward to dial. The intent
// is to push the raw ACES output toward a deliberate look:
//
//   * deepen mood — slight global contrast lift via a soft S-curve
//     centred at 0.5, separating darks from mids without crushing.
//   * unify materials — gentle highlight desaturation pulls the
//     hottest pixels toward white-warm, hiding source-material
//     colour differences in lit zones.
//   * tame reds — super-saturated red dominance (action-bar
//     glow, blood pools, danger indicators) gets desaturated so
//     they stop reading as candy-coloured plastic.
//   * torch warmth — a midtone warm bias adds amber to the
//     scene's diffuse fill where torches dominate the lighting.
//   * separate darks — a tiny cool lift in the deep shadows
//     plus a contrast bump at the foot of the curve makes black
//     feel tactile rather than a flat solid colour.
//
// This is the cheap inline equivalent of a custom LUT. When/if a
// proper LUT is authored in an external tool, swap this function
// for a `texture(u_lut3D, ...)` lookup and remove the inline
// math; the surrounding pipeline does not need to change.
// ---------------------------------------------------------------
vec3 grade(vec3 c) {
    float l = dot(c, vec3(0.2126, 0.7152, 0.0722));

    // 1) Cool lift in the deep shadows. Faint blue-violet so
    //    pure black gets a hint of depth instead of reading as
    //    a hole in the screen. Faded as luminance rises.
    float shadowMask = 1.0 - smoothstep(0.0, 0.18, l);
    c += vec3(0.004, 0.006, 0.014) * shadowMask;

    // 2) Soft S-curve. Re-centred around 0.5; the curvature is
    //    proportional to (1 - 4t²) so it tapers at both ends and
    //    leaves the extremes alone (no clipping below 0 / above 1).
    {
        vec3 t = c - 0.5;
        c = 0.5 + t * (1.0 + 0.16 * (1.0 - 4.0 * t * t));
        c = clamp(c, 0.0, 1.0);
    }

    // 3) Highlight desaturation. Pull pixels above ~0.7 luma
    //    toward their luma to unify hot zones into a coherent
    //    warm-white instead of saturated primaries.
    {
        float hi = smoothstep(0.65, 1.0, l);
        c = mix(c, vec3(l), hi * 0.22);
    }

    // 4) Tame super-saturated reds. Triggers when red strongly
    //    dominates green & blue; otherwise no-op (keeps healthy
    //    skin / wood / fire tones).
    {
        float redDom = smoothstep(0.05, 0.35, c.r - max(c.g, c.b));
        c.r = mix(c.r, c.r * 0.88, redDom);
        c.g = mix(c.g, c.g * 1.03, redDom * 0.4); // a touch of orange
    }

    // 5) Mid-tone torch warmth. Peaks around luma=0.45; falls off
    //    to nothing in shadows and highlights so it feels like
    //    diffuse fill rather than an overall colour cast.
    {
        float midMask = clamp(1.0 - abs(l - 0.45) * 2.4, 0.0, 1.0);
        c += vec3(0.022, 0.011, -0.012) * midMask;
    }

    return clamp(c, 0.0, 1.0);
}

void main() {
    vec3 hdr         = texture(u_hdr,         v_uv).rgb;
    vec3 bloom       = texture(u_bloom,       v_uv).rgb;
    vec3 volumetrics = texture(u_volumetrics, v_uv).rgb;

    float ao = texture(u_ao, v_uv).r;

    vec3 col = (hdr * ao + volumetrics + bloom * pc.bloom_intensity) * pc.exposure;
    vec3 mapped = aces(col);
    mapped = grade(mapped);

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
