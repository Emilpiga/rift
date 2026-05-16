// Authored-texture hybrid particles (sprite 10).

// Hybrid tiling billow — authored texture owns primary density.
// Shader: UV flow, dissolve/erosion, lighting, motion envelopes only.
// FBM is never used to build the puff shape.
// Returns x = alpha, y = emissive fraction.
vec2 hybridTilingBillow(
    vec2 uv,
    float seed,
    float lifeT,
    vec2 worldAnchor,
    float worldY,
    uint texIdx,
    float tile,
    float flowStrength,
    float puffFootprint
) {
    float time = ubo.timeData.x;

    // ---------------------------------------------------------
    // LIFE ENVELOPE
    // ---------------------------------------------------------

    float grow =
        smoothstep(0.0, 0.10, lifeT);

    float die =
        1.0 - smoothstep(0.72, 1.0, lifeT);

    float life =
        grow * die;

    // ---------------------------------------------------------
    // CENTERED CARD UV
    // ---------------------------------------------------------
    //
    // IMPORTANT:
    // The billboard itself IS the smoke puff.
    //
    // We do NOT:
    // - tile
    // - fract()
    // - world-anchor scroll
    // - crop into subregions
    //
    // The authored texture owns the silhouette.
    //

    vec2 centeredUV = uv;

    // ---------------------------------------------------------
    // SUBTLE INTERNAL FLOW
    // ---------------------------------------------------------
    //
    // Distort INTERNAL density slightly without
    // destroying the silhouette.
    //

    vec2 flowField =
        curl2(
            uv * 2.2
            + vec2(
                time * 0.05,
                seed * 11.7
            )
        );

    vec2 flow =
        flowField
        * flowStrength
        * 0.018;

    centeredUV += flow;

    // Keep inside safe sample region.
    centeredUV =
        clamp(centeredUV, 0.001, 0.999);

    // ---------------------------------------------------------
    // AUTHORED DENSITY
    // ---------------------------------------------------------
    //
    // One sample. The PNG owns the silhouette, holes, and soft-vs-dense
    // alpha. Do not layer a second scaled copy or remap with smoothstep:
    // both destroy the texture's authored contrast.

    float density = clamp(sampleVfxDensity(texIdx, centeredUV), 0.0, 1.0);

    // Preserve low-alpha detail while keeping high-density smoke crisp.
    // A mild power curve cuts haze from overdraw without collapsing the
    // texture into an on/off mask.
    float alpha = pow(density, 1.18) * life;

    return vec2(clamp(alpha, 0.0, 1.0), 0.0);
}

vec2 hybridFlipbook(vec2 uv, float seed, float lifeT, uint texIdx,
                    float cols, float rows, vec4 params) {
    float fps = params.x;
    float frameStart = params.y;
    float frameCount = max(params.z, 1.0);
    bool looped = params.w > 0.5;

    float t = ubo.timeData.x + seed * 0.17;
    float frameF = frameStart + (looped
        ? mod(floor(t * fps), frameCount)
        : min(floor(t * fps), frameCount - 1.0));

    vec2 cell = vec2(1.0 / cols, 1.0 / rows);
    uint frame = uint(frameF);
    uint col = frame % uint(cols);
    uint row = (frame / uint(cols)) % uint(rows);
    vec2 atlasUV = fract(uv) * cell + vec2(float(col), float(row)) * cell;

    float density = sampleVfxDensity(texIdx, atlasUV);

    float grow = smoothstep(0.0, 0.12, lifeT);
    float die = 1.0 - smoothstep(0.80, 1.0, lifeT);
    float alpha = density * grow * die;

    return vec2(clamp(alpha, 0.0, 1.0), 0.0);
}

vec2 hybridParticle(vec2 uv, float seed, float lifeT, vec2 worldXZ, float worldY) {
    uint texIdx = uint(vHybridMeta.x + 0.5);
    uint kind = uint(vHybridMeta.y + 0.5);
    texIdx = min(texIdx, 7u);
    if (kind == 0u) {
        return hybridTilingBillow(uv, seed, lifeT, vWorldXZAnchor, worldY, texIdx,
                                  vHybridMeta.z, vHybridMeta.w, vHybridParams.x);
    }
    if (kind == 1u) {
        return hybridFlipbook(uv, seed, lifeT, texIdx, vHybridMeta.z, vHybridMeta.w, vHybridParams);
    }
    return vec2(0.0);
}
