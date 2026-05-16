// Procedural volumetric smoke puff (sprite 2).

// Curl-style domain warp — breaks grid alignment so density
// doesn't read as a texture stamped on a disc.
vec3 smokeWarp(vec3 p, float t) {
    vec3 q = p;
    q.x += curl2(p.yz + vec2(t * 0.7, 2.1)).x * 0.48;
    q.y += curl2(p.xz + vec2(t * 0.5, 5.3)).y * 0.44;
    q.z += curl2(p.xy + vec2(t * 0.6, 8.7)).x * 0.40;
    return q;
}

// Large-scale world cloud density (macro + meso only — no HF grain).
float smokeVolume(vec3 worldPos, float t) {
    vec3 p = smokeWarp(worldPos + vec3(t * 0.018, t * 0.016, t * 0.014), t);
    float macro = fbm3iso(p * 0.68);
    float meso  = fbm3iso(p * 1.45 + vec3(0.0, t * 0.05, 0.0)) * 0.32;
    return macro + meso;
}

// Aperiodic edge noise — no sin(theta*N) terms (those read as stars).
float smokeEdgeNoise(vec2 c, vec3 worldPos, float t) {
    vec2 wp = worldPos.xz;
    vec3 p = vec3(wp * 2.1, worldPos.y * 1.5 + t * 0.04);
    float eWorld = fbm3iso(smokeWarp(p, t));
    float eQuad  = fbm2(c * 1.9 + wp * 2.3 + vec2(t * 0.06, 0.0));
    float eMix   = fbm2(wp * 5.0 + vec2(c.x * 0.9 + t, c.y * 0.7));
    return eWorld * 0.48 + eQuad * 0.32 + eMix * 0.20;
}
struct SmokeLayer {
    float alpha;
    float heat;
};

SmokeLayer sampleSmokeLayer(
    vec2 uv,
    float seed,
    float lifeT,
    vec2 worldXZ,
    float worldY,
    float layerDepth   // 0 = front, 1 = mid, 2 = back
)
{
    float time = ubo.timeData.x;

    // -----------------------------------------------------
    // DEPTH OFFSET (this is what creates “volume” illusion)
    // -----------------------------------------------------
    float depthBias = layerDepth * 0.35;

    vec2 p = uv - 0.5;

    // stronger blur as we go back
    p *= mix(1.0, 1.6, layerDepth);

    // temporal offset = fake internal parallax
    float rise = time * (0.10 + layerDepth * 0.06);

    // layer-specific curl sampling (CRITICAL)
    vec2 curl = curl2(
        p * (1.2 + layerDepth * 0.8)
        + seed * (7.0 + layerDepth * 11.0)
        + vec2(0.0, -rise)
    );

    p += curl * (0.22 + layerDepth * 0.18);

    // -----------------------------------------------------
    // DOMAIN WARPED VOLUME FIELD
    // -----------------------------------------------------
    vec3 wp = vec3(worldXZ * 0.25, worldY * 0.45);

    vec3 pos = vec3(
        wp.xy + p * (1.1 + layerDepth * 0.4),
        wp.z + p.y * 1.6 - rise
    );

    // de-correlated noise per layer (VERY important)
    vec3 p1 = pos * (0.62 + layerDepth * 0.10);
    vec3 p2 = pos * (1.35 + layerDepth * 0.22);
    vec3 p3 = pos * (2.10 + layerDepth * 0.18);

    float n1 = fbm3iso(p1);
    float n2 = fbm3iso(p2 + vec3(2.7, 5.1, 3.3));
    float n3 = fbm3iso(p3 + vec3(7.9, 1.8, 6.4));

    // soften contrast BEFORE combining
    n1 = n1 * n1;
    n2 = n2 * n2;
    n3 = n3 * n3;

    float density =
        n1 * 0.55 +
        n2 * 0.30 +
        n3 * 0.15;

    // -----------------------------------------------------
    // SHAPE (NO radial SDF dominance)
    // -----------------------------------------------------
    float h = uv.y;

    float vertical =
        1.0 - smoothstep(0.78, 1.0, h - layerDepth * 0.05);

    float body =
        smoothstep(0.32, 0.86, density) * vertical;

    // soften back layers heavily (depth fog illusion)
    body *= mix(1.0, 0.65, layerDepth);

    // internal breakup
    float pockets =
        fbm2(p * 1.6 + seed * 6.5 + layerDepth * 3.0);

    // soften contrast response
    pockets = smoothstep(0.2, 0.85, pockets);

    body *= mix(0.78, 1.0, pockets);

    // life envelope
    float grow = smoothstep(0.0, 0.18, lifeT);
    float die  = 1.0 - smoothstep(0.65, 1.0, lifeT);

    float alpha = body * grow * die;

    // -----------------------------------------------------
    // HEAT (only strongest in front layer)
    // -----------------------------------------------------
    float heat =
        alpha *
        (1.0 - layerDepth) *
        smoothstep(0.5, 0.9, density);

    return SmokeLayer(alpha, heat);
}

vec2 smokePuff(vec2 uv, float seed, float lifeT, vec2 worldXZ, float worldY)
{
    SmokeLayer front = sampleSmokeLayer(uv, seed, lifeT, worldXZ, worldY, 0.0);
    SmokeLayer mid   = sampleSmokeLayer(uv, seed, lifeT, worldXZ, worldY, 1.0);
    SmokeLayer back  = sampleSmokeLayer(uv, seed, lifeT, worldXZ, worldY, 2.0);

    // -----------------------------------------------------
    // VOLUMETRIC COMPOSITION (NOT additive blending)
    // -----------------------------------------------------
    float a_front = front.alpha;
    float a_mid   = mid.alpha;
    float a_back  = back.alpha;

    // front occludes mid/back (fake volume stacking)
    float alpha =
        a_front +
        a_mid * (1.0 - a_front) +
        a_back * (1.0 - max(a_front, a_mid));

    alpha = alpha * alpha * (3.0 - 2.0 * alpha); // smoothstep shaping
    alpha = clamp(alpha, 0.0, 1.0);

    // -----------------------------------------------------
    // SELF SHADOW APPROXIMATION
    // -----------------------------------------------------

    // back layer shadows front density
    float shadow =
        a_back * 0.55 +
        a_mid * 0.30;

    float occlusion =
        1.0 - shadow * (1.0 - a_front);

    alpha *= occlusion;

    // -----------------------------------------------------
    // HEAT COMPOSITION
    // -----------------------------------------------------
    float heat =
        front.heat * 1.0 +
        mid.heat   * 0.6 +
        back.heat  * 0.25;

    alpha = smoothstep(0.02, 0.98, alpha);
    return vec2(alpha, heat);
}
