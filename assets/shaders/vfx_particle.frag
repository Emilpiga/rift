#version 450

// VFX particle fragment shader. Evaluates one of six procedural
// sprite shapes selected by `vSprite`:
//
//   0 SoftGlow — dual-radius core + faint outer halo.
//   1 Spark    — anisotropic streak along velocity (or +/× cross
//                when stationary), tight bright core.
//   2 Smoke    — 2-octave value-noise modulated disc with
//                eroded silhouette.
//   3 Shard    — diamond SDF with bright rim highlight.
//   4 Ring     — antialiased annular band.
//   5 Streak   — pure motion line oriented along velocity,
//                length-driven by `vStretchDir`.
//
// Output is HDR pre-multiplied alpha. The renderer's two
// pipelines (alpha / additive) drive `SRC = ONE` so a single
// shader feeds both blend modes correctly.

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

layout(location = 0) in vec4  vColor;
layout(location = 1) in vec2  vUv;
layout(location = 2) flat in uint vSprite;
layout(location = 3) in float vSeed;
layout(location = 4) in vec2  vStretchDir;   // direction & magnitude (0..2)
layout(location = 5) in float vFogFactor;
layout(location = 6) in float vDistDim;

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

float fbm2(vec2 p) {
    float v = 0.0;
    float a = 0.6;
    for (int i = 0; i < 2; i++) {
        v += a * valueNoise(p);
        p *= 2.07;
        a *= 0.5;
    }
    return v;
}

// ---- Shapes ---------------------------------------------------

// SoftGlow: dual-radius read. A tight bright core + a wider,
// dimmer halo. The halo is what bloom catches; the core gives
// the particle a recognisable centre. The previous single-
// gaussian version produced a flat disc of light; this one
// reads as a luminous body.
float softGlow(vec2 uv) {
    vec2 c = uv - 0.5;
    float d2 = dot(c, c) * 4.0;          // 0 at centre, 1 at edge
    float core = exp(-d2 * 9.0);          // tight bright core
    float halo = exp(-d2 * 2.4) * 0.35;   // wider faint halo
    return core + halo;
}

// Spark: oriented motion streak along `vStretchDir` if the
// particle is moving, falling back to a +/× cross when it is
// stationary. The fast / slow distinction is automatic — the
// vertex shader has already stretched the geometry, so even a
// stationary spark gets its corner UVs in the original
// [0,1] frame.
float spark(vec2 uv) {
    vec2 c = (uv - 0.5) * 2.0;       // [-1, 1]
    float d = length(c);
    float core = exp(-d * d * 28.0);

    // Motion-aligned streak: project onto velocity direction.
    // `vStretchDir` length up to 2.0 means the geometry has
    // already been elongated — we narrow the across-axis here
    // to 8× to give the streak a hairline feel.
    float streakAniso = 0.0;
    if (length(vStretchDir) > 0.01) {
        vec2 along  = normalize(vStretchDir);
        vec2 across = vec2(-along.y, along.x);
        float a = dot(c, along);
        float t = dot(c, across);
        streakAniso = exp(-t * t * 80.0) * exp(-abs(a) * 1.6);
    }

    // Static cross (visible when not moving): two perpendicular
    // hairlines along the rotated billboard axes.
    float crossX = exp(-c.y * c.y * 110.0) * exp(-abs(c.x) * 3.2);
    float crossY = exp(-c.x * c.x * 110.0) * exp(-abs(c.y) * 3.2);
    float crossLines = (crossX + crossY) * 0.5;

    // Blend cross into streak as motion increases.
    float motionBlend = smoothstep(0.10, 0.80, length(vStretchDir));
    float streak = mix(crossLines, streakAniso, motionBlend);

    return clamp(core + 0.7 * streak, 0.0, 2.5);
}

// Smoke: 2-octave fbm modulating a soft disc, with the alpha
// eroded by the noise so the silhouette never reads as a clean
// circle. Tumbling rotation comes from the vertex shader's
// `spin`, so we only need internal noise here.
float smokePuff(vec2 uv, float seed) {
    vec2 c = uv - 0.5;
    float r = length(c);

    // Erode the disc edge with noise — defines the silhouette.
    float n_edge = fbm2(uv * 4.0 + vec2(seed * 17.0, seed * 5.0));
    float r_eroded = r + (n_edge - 0.5) * 0.10;
    float disc = 1.0 - smoothstep(0.34, 0.50, r_eroded);

    // Internal density variation.
    float n_in = fbm2(uv * 7.0 - vec2(seed * 9.0, seed * 21.0));
    return disc * mix(0.45, 1.05, n_in);
}

// Shard: diamond SDF with a bright rim highlight. The rim is
// a thin band where the SDF crosses 0.34..0.36, brightened
// 1.6×. Reads as a crystal facet rather than a flat polygon.
float shard(vec2 uv, float seed) {
    float ang = seed * 6.2831853;
    vec2 c = uv - 0.5;
    float ca = cos(ang), sa = sin(ang);
    vec2 r = vec2(ca * c.x - sa * c.y, sa * c.x + ca * c.y);
    float d = abs(r.x) + abs(r.y) * 1.6;     // diamond, slightly tall
    float body = 1.0 - smoothstep(0.30, 0.40, d);
    // Rim highlight: a thin bright band right at the silhouette.
    float rim = exp(-pow((d - 0.36) * 60.0, 2.0)) * 0.7;
    return body + rim;
}

// Ring: antialiased annular band. Single Gaussian centred at
// r = 0.40, falloff width 24× — narrow enough that the ring
// reads as a hoop rather than a smear.
float ring(vec2 uv) {
    vec2 c = uv - 0.5;
    float r = length(c);
    return exp(-pow((r - 0.40) * 24.0, 2.0));
}

// Streak: pure motion line. Always anisotropic, even at low
// speed — the caller picked this sprite specifically because
// they want a streak look. If `vStretchDir` is zero (stationary),
// fall back to a single hairline along the rotated horizontal
// axis so a static streak still reads as a line, not a dot.
float streak(vec2 uv) {
    vec2 c = (uv - 0.5) * 2.0;
    vec2 along  = (length(vStretchDir) > 0.01)
                ? normalize(vStretchDir)
                : vec2(1.0, 0.0);
    vec2 across = vec2(-along.y, along.x);
    float a = dot(c, along);
    float t = dot(c, across);
    // Tight across-axis (line thickness), gentle along-axis
    // taper at the ends.
    float line = exp(-t * t * 90.0) * (1.0 - smoothstep(0.85, 1.05, abs(a)));
    // Bright pinprick at the head of the streak (a > 0 side)
    // sells the spark-on-tail look.
    float head = exp(-pow(a - 0.85, 2.0) * 60.0) * exp(-t * t * 220.0);
    return clamp(line + head * 1.4, 0.0, 2.0);
}

void main() {
    float mask;
    if      (vSprite == 0u) mask = softGlow(vUv);
    else if (vSprite == 1u) mask = spark(vUv);
    else if (vSprite == 2u) mask = smokePuff(vUv, vSeed);
    else if (vSprite == 3u) mask = shard(vUv, vSeed);
    else if (vSprite == 4u) mask = ring(vUv);
    else                    mask = streak(vUv);

    // ----- Hard quad-edge fade -----
    // Every procedural sprite already tries to fade to zero
    // at the quad boundary, but most of them retain a tiny
    // residual alpha at the cardinal edges (Ring's outer
    // gaussian is ~0.003, SoftGlow's halo ~0.03). Additive
    // blend + bloom + ACES amplifies that residual into a
    // visible billboard *square* outline against bright
    // backgrounds — exactly the artefact that gives away the
    // "these are flat sprites" trick. Force the mask to zero
    // over the outermost ~12% of the quad in both axes,
    // smoothstepped so the transition itself is invisible.
    // Cheap (3 ops) and applies to every sprite uniformly.
    vec2 edgeUV = abs(vUv - 0.5) * 2.0;                  // 0 centre, 1 edge
    float quadFade = 1.0 - smoothstep(0.86, 1.00,
                                      max(edgeUV.x, edgeUV.y));
    mask *= quadFade;

    // Per-particle distance dim — keeps very-near big puffs
    // from crushing ACES.
    mask *= vDistDim;

    float a = clamp(vColor.a * mask, 0.0, 1.0);

    // Apply atmospheric fog. Particles fade into the same fog
    // band as world geometry: at the fog wall they go dark
    // (additive layers fade to zero, alpha layers fade toward
    // the fog colour). Without this fade the particles read as
    // stickers floating in haze.
    vec3 rgb = vColor.rgb;
    rgb = mix(rgb, ubo.fogColor.rgb, vFogFactor);
    // For the *additive* path we also pull the alpha down with
    // fog so additive embers don't punch through the fog wall
    // as bright pixels. The blend pipeline reads `a` as the
    // SRC = ONE multiplier on `rgb*a`, so reducing `a` reduces
    // the contribution proportionally for both alpha and
    // additive blend modes.
    a *= 1.0 - vFogFactor * 0.85;

    // Output is **pre-multiplied alpha**. Both pipelines drive
    // this through `SRC = ONE`:
    //
    //   Alpha pipeline    : ONE × rgb + (1-SRC_ALPHA) × dst
    //   Additive pipeline : ONE × rgb +           ONE × dst
    //
    // …so a single shader feeds both blend modes correctly.
    outColor = vec4(rgb * a, a);
}
