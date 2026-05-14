// ---------------------------------------------------------------------------
// Tangent basis reconstruction
// ---------------------------------------------------------------------------
// We don't ship per-vertex tangents in the global Vertex layout, so we
// rebuild a TBN frame on the fly using screen-space derivatives of world
// position and uv. Standard "no-precomputed-tangent" trick: costs four
// dFdx/dFdy + a couple of cross products, runs at fragment frequency,
// and lines up with whatever uvScale the vert baked in (we sample with
// the post-scaled fragUV).
mat3 cotangentFrame(vec3 N, vec3 p, vec2 uv) {
    vec3 dp1 = dFdx(p);
    vec3 dp2 = dFdy(p);
    vec2 duv1 = dFdx(uv);
    vec2 duv2 = dFdy(uv);

    vec3 dp2perp = cross(dp2, N);
    vec3 dp1perp = cross(N, dp1);
    vec3 T = dp2perp * duv1.x + dp1perp * duv2.x;
    vec3 B = dp2perp * duv1.y + dp1perp * duv2.y;

    // Guard against degenerate quads where the UV derivatives
    // collapse (zero-area triangles in screen space, e.g. on
    // thin character features like eyelids or fingertips
    // viewed edge-on). Without this floor `inversesqrt(0)`
    // returns `+inf`, T/B explode to inf, the normal-map
    // rotation produces NaN, and the *entire* 2x2 derivative
    // quad is shaded black — that's the random "black
    // squares on the skin" symptom.
    float maxLen2 = max(max(dot(T, T), dot(B, B)), 1e-12);
    float invmax = inversesqrt(maxLen2);
    return mat3(T * invmax, B * invmax, N);
}

// ---------------------------------------------------------------------------
// Parallax-occlusion mapping (lightweight)
// ---------------------------------------------------------------------------
// Steps along the view ray in tangent space and stops at the first
// step that crosses the height surface, then refines linearly between
// the last two samples. `scale` is in tangent-space units; values of
// 0.02 - 0.05 tend to look good for stone bricks at our floor scale.
vec2 parallaxOffset(vec2 uv, vec3 viewTS, float scale) {
    if (scale <= 0.0) return uv;
    // Grazing-angle bail-out. When the view ray is almost
    // tangent to the surface (`viewTS.z` small), POM has to
    // walk a long distance per step (`P = viewTS.xy /
    // viewTS.z * scale`) and the result is a smeared,
    // stretched mess that reads as artefacts rather than
    // depth. The frame cost in that regime is also worst-case
    // — every fragment of a near-edge-on wall pays the full
    // 8-tap march, which is what produces the angle-dependent
    // FPS dive when a wall fills the viewport at an oblique
    // angle. Bail out below `0.30`: anything more grazing
    // than ~73° off the surface normal renders without POM.
    // Visually identical because at those angles the
    // perturbation is dominated by stretching artefacts
    // anyway; cost-wise this is the single biggest win for
    // wall-heavy scenes.
    if (viewTS.z < 0.30) return uv;
    // Cheap parallax: 3-6 steps is plenty for the small bumps
    // we use on dungeon walls. The previous 4-8 envelope was
    // measurably the dominant fragment cost when walls
    // dominated the viewport — every screen pixel hit by a
    // wall fragment did up to 8 heightmap taps + a refinement
    // sample. Tightening the envelope cuts worst-case taps
    // by 25 % across the board with no visible quality loss
    // on the shipped 2k brick / ground packs (the relief
    // amplitude `scale` is small enough that 3 steps already
    // resolve the ridge crests).
    const float minLayers = 3.0;
    const float maxLayers = 6.0;
    float numLayers = mix(maxLayers, minLayers, abs(viewTS.z));
    float layerDepth = 1.0 / numLayers;
    float currentDepth = 0.0;

    vec2 P = viewTS.xy / max(abs(viewTS.z), 1e-3) * scale;
    vec2 deltaUV = P / numLayers;

    vec2 currentUV = uv;
    float currentSampled = 1.0 - texture(heightMap, currentUV).r;

    // Match `maxLayers` above. The compiler unrolls this so
    // the bound has to be a literal; keep the two in sync.
    for (int i = 0; i < 6; i++) {
        if (currentDepth >= currentSampled) break;
        currentUV -= deltaUV;
        currentSampled = 1.0 - texture(heightMap, currentUV).r;
        currentDepth += layerDepth;
    }

    vec2 prevUV = currentUV + deltaUV;
    float afterDepth = currentSampled - currentDepth;
    float beforeDepth = (1.0 - texture(heightMap, prevUV).r) - currentDepth + layerDepth;
    float denom = afterDepth - beforeDepth;
    float weight = abs(denom) > 1e-5 ? clamp(afterDepth / denom, 0.0, 1.0) : 0.0;
    return mix(currentUV, prevUV, weight);
}

void applyHeightMaterialDetail(
    vec2 uv,
    float scale,
    inout vec3 albedo,
    inout float roughness,
    inout float ao
) {
    float relief = smoothstep(0.004, 0.025, max(scale, 0.0));
    float h = texture(heightMap, uv).r;

    float cavity = (1.0 - smoothstep(0.22, 0.50, h)) * relief;
    float ridge = smoothstep(0.56, 0.88, h) * relief;
    float edge = smoothstep(0.015, 0.070, length(vec2(dFdx(h), dFdy(h)))) * relief;
    float crevice = clamp(cavity + edge * 0.35, 0.0, 1.0);

    albedo *= mix(1.0, 0.86, crevice);
    ao *= mix(1.0, 0.78, crevice);
    roughness = clamp(roughness + crevice * 0.10 - ridge * 0.025, 0.045, 1.0);
}

bool heightShadowsEnabled(float scale) {
    return ubo.pointShadowMeta.z > 0.5 && scale > 0.001;
}

vec3 heightShadowWorldPos(vec3 worldPos, vec3 normal, vec2 uv, float scale) {
    if (!heightShadowsEnabled(scale)) return worldPos;
    float h = texture(heightMap, uv).r;
    float displacement = (h - 0.5) * scale * 2.20;
    return worldPos + normal * displacement;
}

vec3 heightPointShadowWorldPos(
    vec3 worldPos,
    vec3 normal,
    mat3 TBN,
    vec2 uv,
    vec3 lightTS,
    float scale
) {
    if (!heightShadowsEnabled(scale)) return worldPos;

    float h = texture(heightMap, uv).r;
    float centeredHeight = h - 0.5;
    float normalDisplacement = centeredHeight * scale * 2.35;

    float grazing = 1.0 - smoothstep(0.18, 0.78, lightTS.z);
    vec2 projectedRay = -lightTS.xy / max(lightTS.z, 0.18);
    vec3 tangentOffset = TBN * vec3(projectedRay * centeredHeight * scale * 0.85 * grazing, 0.0);

    return worldPos + normal * normalDisplacement + tangentOffset;
}

float heightTextureSelfShadow(vec2 uv, vec3 lightTS, float scale, float strength) {
    if (!heightShadowsEnabled(scale) || lightTS.z <= 0.12) return 1.0;

    float relief = smoothstep(0.004, 0.020, scale);
    float baseHeight = texture(heightMap, uv).r;
    vec2 ray = lightTS.xy / max(lightTS.z, 0.12) * scale * 2.20;
    float occlusion = 0.0;

    for (int i = 1; i <= 4; i++) {
        float t = float(i) / 4.0;
        float sampleHeight = texture(heightMap, uv + ray * t).r;
        float blocker = sampleHeight - baseHeight - t * 0.035;
        occlusion = max(occlusion, smoothstep(0.010, 0.070, blocker));
    }

    return mix(1.0, 1.0 - strength, occlusion * relief);
}

float heightDirectionalSelfShadow(vec2 uv, vec3 lightTS, float scale) {
    return heightTextureSelfShadow(uv, lightTS, scale, 0.36);
}

float heightPointSelfShadow(vec2 uv, vec3 lightTS, float scale) {
    return heightTextureSelfShadow(uv, lightTS, scale, 0.28);
}
