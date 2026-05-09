#version 450

// Final composite + tonemap. Reads the HDR scene, blurred bloom
// and depth, computes a small inline screen-space ambient
// occlusion term, multiplies the HDR colour by it, then tonemaps
// to the swapchain (sRGB).
//
// Why SSAO here, not as a separate pass? The renderer has no
// depth pre-pass — depth is produced as a side-effect of the
// forward scene pass, so the earliest moment SSAO *could* be
// computed against a complete depth buffer is after the scene
// pass. By that point the HDR colour already contains shaded
// ambient. Folding AO into the composite at full screen rate
// saves an entire framebuffer + render pass and reads visually
// indistinguishable to the player. The cost is that AO darkens
// direct-lit pixels too — at moderate strength the eye reads
// this as soft contact shading, not a bug.

layout(location = 0) in  vec2 v_uv;
layout(location = 0) out vec4 outColor;

layout(set = 0, binding = 0) uniform sampler2D u_hdr;
layout(set = 0, binding = 1) uniform sampler2D u_bloom;
layout(set = 0, binding = 2) uniform sampler2D u_depth;

layout(push_constant) uniform Push {
    float bloom_intensity; // multiplier on bloom contribution
    float exposure;        // scene exposure scalar (1.0 default)
    float ghost_mix;       // 0 = normal, 1 = full ghost view
    float ssao_strength;   // 0 disables AO, 1 = full effect
    mat4  inv_proj;        // for view-space reconstruction
} pc;

const float PI = 3.14159265359;

// ---------- View-space reconstruction ----------
// Sampled depth is in NDC [0, 1]. Convert UV + depth to a clip
// vector, then inv_proj into view space.
vec3 view_pos_from_depth(vec2 uv, float depth) {
    // GLSL UV (0,0)=top-left. Vulkan NDC y is also top-down for
    // the sampled depth here because the renderer flips the
    // projection matrix's Y. So we map (uv*2-1) directly.
    vec4 clip = vec4(uv * 2.0 - 1.0, depth, 1.0);
    vec4 view = pc.inv_proj * clip;
    return view.xyz / view.w;
}

// Cheap hash → [0, 1).
float hash12(vec2 p) {
    return fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453);
}

// ---------- Inline SSAO ----------
// Approach: 8 samples on a rotated Vogel disk in screen space.
// For each sample we pick a radius proportional to the central
// pixel's view-space depth (so the kernel covers a fixed
// world-space neighbourhood, not a fixed pixel count) and
// compare the sample's view-space position to a hemisphere
// oriented along the reconstructed view-space normal. The
// occlusion is a soft falloff on the depth difference.
//
// Normal reconstruction uses screen-space derivatives of the
// view-space position. ddx/ddy of a per-pixel quantity in a
// fragment shader gives the partial derivatives over a 2x2
// quad, which is exactly the surface tangent basis at this
// pixel. The cross product is the surface normal, sign-flipped
// to match the camera's +Z = into-screen convention.
float compute_ssao(vec2 uv, float depth) {
    // Bail on sky / cleared depth. Sampling the kernel against a
    // depth=1.0 pixel produces unstable normals and we do not
    // want to darken the sky.
    if (depth >= 0.9999) return 1.0;

    vec3 origin = view_pos_from_depth(uv, depth);
    // Bail on degenerate view positions.
    if (origin.z >= -0.001) return 1.0;

    vec3 nrm = normalize(cross(dFdx(origin), dFdy(origin)));
    // Same convention as forward shader: normals point toward
    // the camera (negative view-space Z). The cross-product sign
    // depends on screen-space derivative direction; force the
    // facing.
    if (dot(nrm, vec3(0.0, 0.0, 1.0)) < 0.0) nrm = -nrm;

    // World-space radius of the AO kernel. ~25 cm reads as
    // contact shading without bleeding across whole rooms.
    const float WORLD_RADIUS = 0.25;
    // Project that radius to a screen-space delta at this depth.
    // Any reasonable focal length will do; we use a constant.
    float radius_uv = WORLD_RADIUS / max(-origin.z, 0.1) * 0.5;

    // Per-pixel rotation breaks the visible Vogel pattern.
    float rot = hash12(uv * vec2(textureSize(u_depth, 0))) * 2.0 * PI;
    float cr = cos(rot), sr = sin(rot);
    mat2 rotate = mat2(cr, -sr, sr, cr);

    const int N = 8;
    const float GOLDEN = 2.39996323;
    float occlusion = 0.0;

    for (int i = 0; i < N; ++i) {
        // Vogel disk: r = sqrt(i/N), theta = i * golden_angle.
        float fi = float(i) + 0.5;
        float r = sqrt(fi / float(N));
        float theta = fi * GOLDEN;
        vec2 disk = vec2(cos(theta), sin(theta)) * r;
        vec2 offset = rotate * disk * radius_uv;
        vec2 sample_uv = clamp(uv + offset, vec2(0.001), vec2(0.999));

        float sample_depth = texture(u_depth, sample_uv).r;
        if (sample_depth >= 0.9999) continue;
        vec3 sample_pos = view_pos_from_depth(sample_uv, sample_depth);

        // Vector from origin to sample.
        vec3 v = sample_pos - origin;
        float dist = length(v);
        // Falloff: ignore far samples (they're another surface
        // entirely, not an occluder of this one) and weight by
        // alignment with the surface normal so flat ground
        // doesn't occlude itself.
        float range = smoothstep(WORLD_RADIUS * 1.4, WORLD_RADIUS * 0.05, dist);
        float ndotv = max(dot(nrm, v / max(dist, 0.0001)), 0.0);
        // Bias prevents self-occlusion on flat surfaces.
        const float BIAS = 0.015;
        occlusion += step(BIAS, ndotv) * ndotv * range;
    }
    occlusion /= float(N);
    // Map [0, 1] occlusion to [1, 0] AO multiplier with a soft
    // curve. pow tightens the response so faint occlusion stays
    // bright and only true crevices darken.
    return clamp(1.0 - pow(occlusion, 0.7), 0.0, 1.0);
}

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
    vec3 hdr   = texture(u_hdr,   v_uv).rgb;
    vec3 bloom = texture(u_bloom, v_uv).rgb;

    // Inline SSAO. Skip entirely when strength == 0 to avoid the
    // depth fetches on lower graphics settings.
    float ao = 1.0;
    if (pc.ssao_strength > 0.0001) {
        float depth = texture(u_depth, v_uv).r;
        float raw = compute_ssao(v_uv, depth);
        ao = mix(1.0, raw, pc.ssao_strength);
    }

    vec3 col   = (hdr * ao + bloom * pc.bloom_intensity) * pc.exposure;
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
