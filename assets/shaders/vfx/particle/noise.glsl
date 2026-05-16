// Procedural noise, FBM, curl flow, antialiasing helpers.

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
    float a = 0.55;
    for (int i = 0; i < 3; i++) {
        v += a * valueNoise(p);
        p = p * 2.07 + vec2(11.3, 17.7);
        a *= 0.5;
    }
    return v;
}

float fbm4(vec2 p) {
    float v = 0.0;
    float a = 0.52;
    for (int i = 0; i < 4; i++) {
        v += a * valueNoise(p);
        p = p * 2.04 + vec2(9.7, 14.3);
        a *= 0.48;
    }
    return v;
}

// Isotropic 3D noise from three uncorrelated 2D slices (avoids
// axis-aligned streaks from a single 2D fbm lattice).
float valueNoise3(vec3 p) {
    return (valueNoise(p.xy + p.z * 5.17)
          + valueNoise(p.yz + p.x * 3.71)
          + valueNoise(p.xz + p.y * 4.29)) / 3.0;
}

float fbm3iso(vec3 p) {
    float v = 0.0;
    float a = 0.55;
    for (int i = 0; i < 3; i++) {
        v += a * valueNoise3(p);
        p = p * 2.02 + vec3(11.3, 17.7, 9.1);
        a *= 0.5;
    }
    return v;
}

float fbm4iso(vec3 p) {
    float v = 0.0;
    float a = 0.52;
    for (int i = 0; i < 4; i++) {
        v += a * valueNoise3(p);
        p = p * 2.03 + vec3(7.1, 13.3, 5.9);
        a *= 0.48;
    }
    return v;
}

float aaStep(float edge, float x) {
    float w = max(fwidth(x), 0.0015);
    return smoothstep(edge - w, edge + w, x);
}

float aaBand(float x, float centre, float width) {
    float w = max(fwidth(x), 0.0015);
    return 1.0 - smoothstep(width - w, width + w, abs(x - centre));
}

// ---- Flow / temporal helpers ---------------------------------

// Cheap pseudo-curl: take the gradient of `valueNoise` at two
// offsets, then return its perpendicular. Output is a smooth
// 2-channel vector field roughly in [-1, 1] that we use as a
// flow map — sample the noise at `uv + curl(uv) * amp` and
// the silhouette starts to *flow* along the field lines
// rather than just churn in place.
vec2 curl2(vec2 p) {
    const float e = 0.05;
    float n_x1 = valueNoise(p + vec2(e, 0.0));
    float n_x0 = valueNoise(p - vec2(e, 0.0));
    float n_y1 = valueNoise(p + vec2(0.0, e));
    float n_y0 = valueNoise(p - vec2(0.0, e));
    // Gradient -> perpendicular
    return vec2(n_y1 - n_y0, -(n_x1 - n_x0)) * (1.0 / (2.0 * e));
}
