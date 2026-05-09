#version 450

layout(binding = 0) uniform UniformData {
    mat4 view;
    mat4 proj;
    vec4 cameraPos;
    vec4 lightDir;
    vec4 lightColor;
    vec4 fogColor;
    vec4 fogParams; // x = start, y = end
    vec4 fogOrigin; // xyz = world-space anchor (player) for fog distance
    vec4 pointLightPos[8];   // xyz = position, w = radius
    vec4 pointLightColor[8]; // xyz = color, w = intensity
    vec4 pointLightCount;    // x = count
    mat4 lightVP;            // directional light view-projection (for shadow map)
    // Per-face VPs for the cube shadow atlas. Unused by the main pass
    // (only the shadow_point pipeline reads them) but must be present
    // here so the UBO layout matches across all pipelines that bind
    // descriptor set 0.
    mat4 pointShadowFaceVP[24];
    // x = active point-shadow caster count (0..=4). The point-light
    // loop below uses this to decide which point lights have a cube
    // atlas slot (and so should be sampled for occlusion) vs. which
    // are shadowless additive fill.
    vec4 pointShadowMeta;
} ubo;

layout(set = 0, binding = 1) uniform sampler2D unusedSampler; // legacy slot, kept for descriptor compatibility
layout(set = 0, binding = 2) uniform sampler2DShadow shadowMap;
layout(set = 0, binding = 3) uniform samplerCubeArray pointShadowAtlas;

// Per-object PBR material set. Bindings must match the
// `BINDING_*` constants in `crates/rift-engine/src/renderer/material.rs`.
layout(set = 1, binding = 0) uniform sampler2D baseColorMap;
layout(set = 1, binding = 1) uniform sampler2D normalMap;
layout(set = 1, binding = 2) uniform sampler2D mrMap;     // R = metallic, G = roughness
layout(set = 1, binding = 3) uniform sampler2D aoMap;
layout(set = 1, binding = 4) uniform sampler2D heightMap;

layout(location = 0) in vec3 fragWorldPos;
layout(location = 1) in vec3 fragNormal;
layout(location = 2) in vec3 fragColor;
layout(location = 3) in vec2 fragUV;

layout(location = 0) out vec4 outColor;

// Per-object push constant. Layout must mirror the vert
// (`mat4 model` at offset 0, `vec4 tint` at 64,
// `vec4 materialParams` at 80) because Vulkan validates
// push-constant ranges per pipeline. `materialParams`:
//   x = uvScale          (already applied to fragUV in the vert)
//   y = parallaxScale    (tangent-space parallax depth amplitude;
//                         `0` disables parallax)
//   z = flagsFloat       (bit 0 = enable PBR + normal mapping)
//   w = reserved
layout(push_constant) uniform PushConstants {
    mat4 model;
    vec4 tint;
    vec4 materialParams;
} push;

const float PI = 3.14159265359;

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

    float invmax = inversesqrt(max(dot(T, T), dot(B, B)));
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
    // Cheap parallax: 4-8 steps is plenty for the small bumps
    // we use on dungeon walls. Anything heavier shows up in
    // the frame budget immediately, especially at 2k texture
    // resolution where the height-map sampler thrashes the
    // cache.
    const float minLayers = 4.0;
    const float maxLayers = 8.0;
    float numLayers = mix(maxLayers, minLayers, abs(viewTS.z));
    float layerDepth = 1.0 / numLayers;
    float currentDepth = 0.0;

    vec2 P = viewTS.xy / max(abs(viewTS.z), 1e-3) * scale;
    vec2 deltaUV = P / numLayers;

    vec2 currentUV = uv;
    float currentSampled = 1.0 - texture(heightMap, currentUV).r;

    for (int i = 0; i < 8; i++) {
        if (currentDepth >= currentSampled) break;
        currentUV -= deltaUV;
        currentSampled = 1.0 - texture(heightMap, currentUV).r;
        currentDepth += layerDepth;
    }

    vec2 prevUV = currentUV + deltaUV;
    float afterDepth = currentSampled - currentDepth;
    float beforeDepth = (1.0 - texture(heightMap, prevUV).r) - currentDepth + layerDepth;
    float weight = afterDepth / (afterDepth - beforeDepth);
    return mix(currentUV, prevUV, weight);
}

// ---------------------------------------------------------------------------
// Cook-Torrance BRDF building blocks
// ---------------------------------------------------------------------------
float distributionGGX(vec3 N, vec3 H, float roughness) {
    float a = roughness * roughness;
    float a2 = a * a;
    float NdotH = max(dot(N, H), 0.0);
    float denom = (NdotH * NdotH) * (a2 - 1.0) + 1.0;
    return a2 / (PI * denom * denom);
}

float geometrySchlickGGX(float NdotV, float roughness) {
    float r = roughness + 1.0;
    float k = (r * r) / 8.0;
    return NdotV / (NdotV * (1.0 - k) + k);
}

float geometrySmith(vec3 N, vec3 V, vec3 L, float roughness) {
    float NdotV = max(dot(N, V), 0.0);
    float NdotL = max(dot(N, L), 0.0);
    return geometrySchlickGGX(NdotV, roughness) * geometrySchlickGGX(NdotL, roughness);
}

vec3 fresnelSchlick(float cosTheta, vec3 F0) {
    return F0 + (1.0 - F0) * pow(clamp(1.0 - cosTheta, 0.0, 1.0), 5.0);
}

// ---------------------------------------------------------------------------
// Sample the directional shadow map. Shared by both shading paths.
// ---------------------------------------------------------------------------
//
// Uses a 12-tap Poisson-disk PCF kernel, scaled to ~3 shadow-map texels in
// world space, which gives a smooth but defined penumbra at the project's
// 2 k × 28 m shadow projection (~73 texels/m). The fixed disk avoids the
// boxy banding of a 2×2 / 3×3 grid and is cheap enough to evaluate per
// fragment under a `sampler2DShadow` (each tap is one hardware-PCF
// comparison + bilinear).
const vec2 POISSON_DISK[12] = vec2[](
    vec2(-0.326,-0.406), vec2(-0.840,-0.074), vec2(-0.696, 0.457),
    vec2(-0.203, 0.621), vec2( 0.962,-0.195), vec2( 0.473,-0.480),
    vec2( 0.519, 0.767), vec2( 0.185,-0.893), vec2( 0.507, 0.064),
    vec2( 0.896, 0.412), vec2(-0.322,-0.933), vec2(-0.792,-0.598)
);

float sampleShadow(vec3 N, vec3 L) {
    vec4 lightClip = ubo.lightVP * vec4(fragWorldPos, 1.0);
    vec3 lightNDC = lightClip.xyz / max(lightClip.w, 1e-5);
    vec3 shadowUV = vec3(lightNDC.xy * 0.5 + 0.5, lightNDC.z);

    // Slope-scaled depth bias. Surfaces near-perpendicular to the
    // light need almost no bias; grazing surfaces need a lot to
    // avoid shadow acne. The constant tail is the absolute floor
    // — small enough that contact shadows on the ground stay
    // tight against their casters.
    float NdotL = max(dot(N, L), 0.0);
    float bias = max(0.0010 * (1.0 - NdotL), 0.00012);
    shadowUV.z -= bias;

    if (shadowUV.x < 0.0 || shadowUV.x > 1.0 ||
        shadowUV.y < 0.0 || shadowUV.y > 1.0 ||
        shadowUV.z < 0.0 || shadowUV.z > 1.0) {
        return 1.0;
    }

    vec2 texelSize = 1.0 / vec2(textureSize(shadowMap, 0));
    // ~3-texel disk radius. At 73 texels/m this is a ~4 cm penumbra
    // half-width on a flat receiver, which lines up with the visual
    // softness of an overcast sky-fill but still reads as a hard
    // contact shadow at object-scale.
    vec2 kernel = texelSize * 3.0;

    float s = 0.0;
    for (int i = 0; i < 12; i++) {
        vec2 offset = POISSON_DISK[i] * kernel;
        s += texture(shadowMap, vec3(shadowUV.xy + offset, shadowUV.z));
    }
    s *= (1.0 / 12.0);

    // Drop the shadow floor to near-zero so cast shadows in the
    // rift read as a strong, deliberate silhouette instead of a
    // muddy grey patch. The directional term is still added on
    // top of full ambient via `ambient + directional * shadow`,
    // so a fully-shadowed surface keeps the ambient lift —
    // setting this to 0.0 produces "the directional light is
    // gone" rather than "the surface is black".
    return s;
}

// ---------------------------------------------------------------------------
// Sample the omnidirectional point-light shadow atlas for the given light.
// `lightIdx` selects which 6-face cube in the atlas (matching the layout
// the renderer used when rendering the shadow pass). Returns 1.0 = lit,
// 0.0 = fully occluded. Returns 1.0 when `lightIdx` is past the active
// shadow-caster count so non-shadowed point lights remain pure additive.
// ---------------------------------------------------------------------------
float samplePointShadow(int lightIdx, vec3 fragWorld, vec3 lightPos, float radius, vec3 N) {
    if (lightIdx >= int(ubo.pointShadowMeta.x)) {
        return 1.0;
    }
    vec3 toFrag = fragWorld - lightPos;
    float fragDist = length(toFrag);
    if (fragDist >= radius) {
        // Fragment is past the light's effective range — the
        // attenuation factor in the caller already zeroes the
        // contribution, so save the texture taps.
        return 1.0;
    }
    float normFrag = fragDist / radius;
    vec3 dir = toFrag / max(fragDist, 1e-4);

    // Slope-scaled bias: cosine-grazing surfaces need a larger
    // bias to avoid acne. The constant 0.0025 is in normalized
    // distance units (i.e. ~0.0125 m on a 5 m torch radius), the
    // typical scale of cube-atlas texel projection at our 512²
    // per face.
    float NdotL = max(dot(N, -dir), 0.0);
    float bias = max(0.0040 * (1.0 - NdotL), 0.0010);

    // 5-tap PCF: center + four offsets along an orthonormal
    // basis built from `dir`. Offsets scaled to ~1.5 atlas
    // texels in normalized-distance space, which softens the
    // contact penumbra without smearing it across whole walls.
    vec3 up = abs(dir.y) > 0.95 ? vec3(0.0, 0.0, 1.0) : vec3(0.0, 1.0, 0.0);
    vec3 tu = normalize(cross(up, dir));
    vec3 tv = cross(dir, tu);
    float k = 0.020;

    float occ = 0.0;
    float c0 = texture(pointShadowAtlas, vec4(dir, float(lightIdx))).r;
    occ += step(normFrag - bias, c0);
    float c1 = texture(pointShadowAtlas, vec4(dir + tu * k, float(lightIdx))).r;
    occ += step(normFrag - bias, c1);
    float c2 = texture(pointShadowAtlas, vec4(dir - tu * k, float(lightIdx))).r;
    occ += step(normFrag - bias, c2);
    float c3 = texture(pointShadowAtlas, vec4(dir + tv * k, float(lightIdx))).r;
    occ += step(normFrag - bias, c3);
    float c4 = texture(pointShadowAtlas, vec4(dir - tv * k, float(lightIdx))).r;
    occ += step(normFrag - bias, c4);
    return occ * 0.2;
}

// ---------------------------------------------------------------------------
// PBR shading path. Used when material flags bit 0 is set.
// ---------------------------------------------------------------------------
vec3 shadePbr() {
    vec3 Ngeo = normalize(fragNormal);
    vec3 V = normalize(ubo.cameraPos.xyz - fragWorldPos);

    mat3 TBN = cotangentFrame(Ngeo, fragWorldPos, fragUV);
    vec3 viewTS = transpose(TBN) * V;

    vec2 uv = parallaxOffset(fragUV, viewTS, push.materialParams.y);

    vec3 albedo = texture(baseColorMap, uv).rgb * fragColor;
    vec3 nTex = texture(normalMap, uv).xyz * 2.0 - 1.0;
    vec3 N = normalize(TBN * nTex);

    vec2 mr = texture(mrMap, uv).rg;
    float metallic  = mr.r;
    float roughness = clamp(mr.g, 0.045, 1.0);
    float ao        = texture(aoMap, uv).r;

    vec3 F0 = mix(vec3(0.04), albedo, metallic);

    // ---- Directional key light ----
    vec3 L = normalize(ubo.lightDir.xyz);
    vec3 H = normalize(L + V);
    float shadow = sampleShadow(N, L);

    float NDF = distributionGGX(N, H, roughness);
    float G   = geometrySmith(N, V, L, roughness);
    vec3  F   = fresnelSchlick(max(dot(H, V), 0.0), F0);

    vec3 numerator = NDF * G * F;
    float denom = 4.0 * max(dot(N, V), 0.0) * max(dot(N, L), 0.0) + 1e-4;
    vec3 specular = numerator / denom;

    vec3 kS = F;
    vec3 kD = (1.0 - kS) * (1.0 - metallic);
    float NdotL = max(dot(N, L), 0.0);
    vec3 directional = (kD * albedo / PI + specular) *
                       ubo.lightColor.rgb * NdotL * shadow;

    vec3 ambient = albedo * ubo.lightColor.w * ao;

    vec3 lighting = ambient + directional;

    // ---- Point lights (no shadow, with quadratic falloff) ----
    int numLights = int(ubo.pointLightCount.x);
    for (int i = 0; i < numLights && i < 8; i++) {
        vec3 lightPos = ubo.pointLightPos[i].xyz;
        float radius = ubo.pointLightPos[i].w;
        vec3 lightCol = ubo.pointLightColor[i].xyz;
        float intensity = ubo.pointLightColor[i].w;

        vec3 toLight = lightPos - fragWorldPos;
        float dist = length(toLight);
        if (dist >= radius) continue;
        float atten = 1.1 - (dist / radius);
        atten = atten * atten;

        vec3 Lp = normalize(toLight);
        vec3 Hp = normalize(Lp + V);
        float NdotLp = max(dot(N, Lp), 0.0);

        float NDFp = distributionGGX(N, Hp, roughness);
        float Gp   = geometrySmith(N, V, Lp, roughness);
        vec3  Fp   = fresnelSchlick(max(dot(Hp, V), 0.0), F0);

        vec3 specP = (NDFp * Gp * Fp) /
                     (4.0 * max(dot(N, V), 0.0) * NdotLp + 1e-4);
        vec3 kSp = Fp;
        vec3 kDp = (1.0 - kSp) * (1.0 - metallic);
        float pshadow = samplePointShadow(i, fragWorldPos, lightPos, radius, N);
        lighting += (kDp * albedo / PI + specP) * lightCol * intensity * NdotLp * atten * pshadow;
    }

    return lighting;
}

// ---------------------------------------------------------------------------
// Legacy cel-shading path. Preserved verbatim for monsters / players
// / props so the project's existing painted look stays intact.
// ---------------------------------------------------------------------------
vec3 shadeCel() {
    vec3 N = normalize(fragNormal);
    vec3 L = normalize(ubo.lightDir.xyz);
    vec3 V = normalize(ubo.cameraPos.xyz - fragWorldPos);
    vec3 H = normalize(L + V);

    float ambient = ubo.lightColor.w;

    float diffRaw = max(dot(N, L), 0.0);
    float diff;
    if (diffRaw < 0.30) {
        diff = mix(0.30, 0.62, smoothstep(0.25, 0.30, diffRaw));
    } else if (diffRaw < 0.65) {
        diff = mix(0.62, 0.45, smoothstep(0.60, 0.65, diffRaw));
    } else {
        diff = 0.45;
    }

    float specRaw = pow(max(dot(N, H), 0.0), 96.0);
    float floorMask = smoothstep(0.20, 2.37, N.y);
    float spec = specRaw * (1.0 - floorMask);

    float fres = pow(1.0 - max(dot(N, V), 0.0), 5.0);
    vec3 rim = ubo.lightColor.rgb * fres * 0.08 * (2.0 - floorMask);

    vec3 texColor = texture(baseColorMap, fragUV).rgb;
    vec3 baseColor = fragColor * texColor;

    float shadow = sampleShadow(N, L);

    vec3 lighting = baseColor * ambient
                  + baseColor * diff * ubo.lightColor.rgb * shadow
                  + ubo.lightColor.rgb * spec * 0.10 * shadow
                  + rim;

    int numLights = int(ubo.pointLightCount.x);
    for (int i = 0; i < numLights && i < 8; i++) {
        vec3 lightPos = ubo.pointLightPos[i].xyz;
        float radius = ubo.pointLightPos[i].w;
        vec3 lightCol = ubo.pointLightColor[i].xyz;
        float intensity = ubo.pointLightColor[i].w;

        vec3 toLight = lightPos - fragWorldPos;
        float dist = length(toLight);
        if (dist < radius) {
            float atten = 1.1 - (dist / radius);
            atten = atten * atten;
            vec3 Lp = normalize(toLight);
            float diffP = max(dot(N, Lp), 0.1);
            float pshadow = samplePointShadow(i, fragWorldPos, lightPos, radius, N);
            lighting += baseColor * diffP * lightCol * intensity * atten * pshadow;
        }
    }

    return lighting;
}

void main() {
    // Bit-test the flags float to pick a shading path. Using
    // floatBitsToUint so we can pack other booleans into the
    // same float later (bit 1, bit 2, ...) without touching
    // the Rust side.
    uint flags = floatBitsToUint(push.materialParams.z);
    bool usePbr = (flags & 1u) != 0u;

    vec3 lighting = usePbr ? shadePbr() : shadeCel();

    // Distance fog (player-anchored).
    float dist = length(ubo.fogOrigin.xyz - fragWorldPos);
    float fogFactor = clamp((dist - ubo.fogParams.x) / (ubo.fogParams.y - ubo.fogParams.x), 0.0, 1.0);
    fogFactor = fogFactor * fogFactor;
    vec3 finalColor = mix(lighting, ubo.fogColor.rgb, fogFactor);

    outColor = vec4(finalColor * push.tint.rgb, push.tint.a);
}
