#version 450

// VFX ribbon fragment shader. Evaluates:
//
//   color = sample_cross(u) * sample_length(v) * brightness
//         * (1 - noise_strength + noise_strength * scroll_noise(v, t))
//   alpha = cross.a * length.a * gauss(u)
//
// All gradients are 8-stop (cross) and 4-stop (length) arrays
// pre-baked CPU-side and passed through as flat varyings (one
// upload per ribbon, never per-fragment).
//
// Output is HDR — the tonemap pass downstream is responsible for
// final exposure. Additive blend handles the glow stacking.

layout(location = 0) in vec2 vUv;            // u = cross, v = length
layout(location = 1) in vec4 vParams;        // brightness, noise_strength, noise_scroll, noise_tile
layout(location = 2) in float vTime;
layout(location = 3) flat in vec4 vCross[8];
layout(location = 11) flat in vec4 vLength[4];
layout(location = 15) flat in vec4 vFlags;

layout(location = 0) out vec4 outColor;

vec4 sampleCross(float u) {
    // Linear interp between 8 evenly spaced stops at t = i/7.
    float t = clamp(u, 0.0, 1.0) * 7.0;
    int i = int(floor(t));
    int j = min(i + 1, 7);
    float f = t - float(i);
    return mix(vCross[i], vCross[j], f);
}

vec4 sampleLength(float v) {
    float t = clamp(v, 0.0, 1.0) * 3.0;
    int i = int(floor(t));
    int j = min(i + 1, 3);
    float f = t - float(i);
    return mix(vLength[i], vLength[j], f);
}

// Cheap hash noise used for the flow shimmer. Mirrors the
// CPU-side noise3 — keeps look consistent if we ever want to
// drive ribbons + curl-noise particles with the same field.
float hash21(vec2 p) {
    p = fract(p * vec2(127.1, 311.7));
    p += dot(p, p + 19.19);
    return fract(p.x * p.y);
}

float valueNoise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    vec2 u = f * f * (3.0 - 2.0 * f);
    float a = hash21(i);
    float b = hash21(i + vec2(1.0, 0.0));
    float c = hash21(i + vec2(0.0, 1.0));
    float d = hash21(i + vec2(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

float fbm(vec2 p, int octaves) {
    float sum = 0.0;
    float amp = 0.5;
    float freq = 1.0;
    for (int i = 0; i < 4; i++) {
        if (i >= octaves) break;
        sum += amp * valueNoise(p * freq);
        amp *= 0.5;
        freq *= 2.0;
    }
    return sum;
}

void main() {
    float u = vUv.x;
    float v = vUv.y;

    // Cross profile: gradient is the colour, but we also apply
    // a Gaussian falloff in alpha so the edges always die smoothly
    // even if the gradient stops are fully opaque.
    vec4 crossCol = sampleCross(u);
    float gauss = exp(-pow((u - 0.5) * 2.6, 2.0));

    vec4 lengthVal = sampleLength(v);

    // Scrolling fbm along the beam. Tile is the world-units per
    // noise cell along the length; cross axis uses 1/3 tile so
    // the noise is anisotropic (streaks along the beam, not blobs).
    float brightness = vParams.x;
    float strength = vParams.y;
    float scroll = vParams.z;
    float tile = max(vParams.w, 0.001);

    float n = 1.0;
    if (strength > 0.0) {
        // We don't have world-space length here — use uv length as
        // a proxy. Ribbons of length L produce uv.y in [0, 1] but
        // the consumer can compensate via spec.tile.
        vec2 np = vec2(u * 2.5, v / tile - vTime * scroll);
        int oct = clamp(int(floor(vFlags.x + 0.5)), 1, 4);
        float fbm_val = fbm(np, oct);
        n = mix(1.0, fbm_val * 1.6, strength);
    }

    vec3 rgb = crossCol.rgb * lengthVal.rgb * brightness * n;
    float a  = crossCol.a * lengthVal.a * gauss;

    outColor = vec4(rgb * a, a);
}
