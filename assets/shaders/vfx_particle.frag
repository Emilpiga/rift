#version 450

// VFX particle fragment shader. Evaluates one of five procedural
// sprite shapes selected by `vSprite`:
//
//   0 SoftGlow — gaussian falloff (cloud / glow)
//   1 Spark    — sharp point + cross-streaks
//   2 Smoke    — value-noise modulated disc
//   3 Shard    — diamond / chunk SDF
//   4 Ring     — annular ring with falloff
//
// Output is HDR — colour comes from the gradient-sampled instance
// rgba. Alpha is the SDF mask × the instance's per-particle
// opacity. The renderer's two pipelines (alpha / additive) read
// this same shader; the blend op differs.

layout(location = 0) in vec4 vColor;
layout(location = 1) in vec2 vUv;
layout(location = 2) flat in uint vSprite;
layout(location = 3) in float vSeed;

layout(location = 0) out vec4 outColor;

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

float softGlow(vec2 uv) {
    vec2 c = uv - 0.5;
    float d = dot(c, c) * 4.0;          // 0 at centre, 1 at edge
    // Tighter exponent so the core stays bright a larger
    // fraction of the radius and the rolloff is steeper.
    // The previous 3.5 produced a wide gauzy halo that the
    // bloom pass then smeared further; 7.0 keeps the same
    // visible footprint but reads as a defined disc rather
    // than a soft cloud.
    return exp(-d * 7.0);
}

float spark(vec2 uv) {
    vec2 c = (uv - 0.5) * 2.0;
    float d = length(c);
    // Tighter core (was 14.0) so sparks read as crisp
    // points rather than fuzzy dots. The streaks already
    // taper sharply along the cross axis; we leave them.
    float core = exp(-d * d * 32.0);
    // Cross-streaks: bright lines along x and y
    float streakX = exp(-c.y * c.y * 110.0) * exp(-abs(c.x) * 3.5);
    float streakY = exp(-c.x * c.x * 110.0) * exp(-abs(c.y) * 3.5);
    return clamp(core + 0.6 * (streakX + streakY), 0.0, 2.0);
}

float smokePuff(vec2 uv, float seed) {
    vec2 c = uv - 0.5;
    float r = length(c);
    // Narrower transition band so the smoke disc has a
    // defined silhouette instead of bleeding into the
    // background. Smoke shouldn't be razor-sharp, but the
    // old 0.30-wide ramp made every puff read as a 50 %
    // alpha haze.
    float disc = 1.0 - smoothstep(0.36, 0.50, r);
    // Roiling noise modulation, offset by seed so adjacent
    // particles look different even at the same age.
    float n = valueNoise(uv * 5.5 + vec2(seed * 13.0, seed * 7.0));
    return disc * mix(0.55, 1.0, n);
}

float shard(vec2 uv, float seed) {
    // Random-rotated diamond SDF. Different particles get
    // different orientations via the seed.
    float ang = seed * 6.2831853;
    vec2 c = uv - 0.5;
    float ca = cos(ang), sa = sin(ang);
    vec2 r = vec2(ca * c.x - sa * c.y, sa * c.x + ca * c.y);
    float d = abs(r.x) + abs(r.y) * 1.6;  // diamond, slightly tall
    // Hard-ish edge: the previous 0.18–0.42 ramp put 24 %
    // of the sprite radius in a soft transition that read
    // as blur. 0.30–0.40 keeps a 1-pixel anti-alias band
    // at typical sprite sizes while leaving the body solid.
    return 1.0 - smoothstep(0.30, 0.40, d);
}

float ring(vec2 uv) {
    vec2 c = uv - 0.5;
    float r = length(c);
    // Narrower band (was *14.0). The ring now reads as a
    // defined hoop rather than a fat smear of light.
    float band = exp(-pow((r - 0.40) * 24.0, 2.0));
    return band;
}

void main() {
    float mask;
    if (vSprite == 0u)      mask = softGlow(vUv);
    else if (vSprite == 1u) mask = spark(vUv);
    else if (vSprite == 2u) mask = smokePuff(vUv, vSeed);
    else if (vSprite == 3u) mask = shard(vUv, vSeed);
    else                    mask = ring(vUv);

    float a = clamp(vColor.a * mask, 0.0, 1.0);
    // Output is **pre-multiplied alpha**. Both pipelines drive
    // this through `SRC = ONE`:
    //
    //   Alpha pipeline    : ONE × rgb + (1-SRC_ALPHA) × dst
    //   Additive pipeline : ONE × rgb +           ONE × dst
    //
    // …so a single shader feeds both blend modes correctly.
    outColor = vec4(vColor.rgb * a, a);
}
