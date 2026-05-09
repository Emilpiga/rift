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
    vec4 pointLightPos[16];   // xyz = position, w = radius
    vec4 pointLightColor[16]; // xyz = color, w = intensity
    vec4 pointLightCount;    // x = count
    mat4 lightVP;            // directional light view-projection (for shadow map)
    // Per-face VPs for the cube shadow atlas. Unused by the main pass
    // (only the shadow_point pipeline reads them) but must be present
    // here so the UBO layout matches across all pipelines that bind
    // descriptor set 0.
    mat4 pointShadowFaceVP[48];
    // x = active point-shadow caster count (0..=4). The point-light
    // loop below uses this to decide which point lights have a cube
    // atlas slot (and so should be sampled for occlusion) vs. which
    // are shadowless additive fill.
    vec4 pointShadowMeta;
    // x = seconds since renderer start. Used by the blood-field
    // composite to compute splat age (`u_time - splat_spawn_time`)
    // and drive the wet→dry tween.
    // y = floor-plane world Y. The blood field is a top-down 2D
    // accumulation texture, so any horizontal surface (wall caps,
    // pillar tops, raised platforms) above the floor plane would
    // otherwise pick up bleed-through from a splat at the same XZ
    // below it. The composite rejects samples whose worldY differs
    // from this by more than a small tolerance.
    // zw reserved.
    vec4 timeData;
    // Blood-field world\u2192UV transform. xy = world XZ origin
    // (min corner of floor AABB), zw = inverse extent. When all
    // zero the field is inactive and the composite is skipped.
    vec4 bloodFieldXform;
} ubo;

layout(set = 0, binding = 1) uniform sampler2D unusedSampler; // legacy slot, kept for descriptor compatibility
layout(set = 0, binding = 2) uniform sampler2DShadow shadowMap;
layout(set = 0, binding = 3) uniform samplerCubeArray pointShadowAtlas;
layout(set = 0, binding = 4) uniform sampler2D bloodField;

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
// Blood-field composite.
// ---------------------------------------------------------------------------
// Samples the per-floor blood accumulation texture (R = wet intensity,
// G = spawn time in seconds) at the fragment's world XZ position and
// mutates the incoming PBR inputs in place.
//
// Gating:
//   - `bloodFieldXform == 0` → no active field (hub / boot). Skip.
//   - `Ngeo.y < 0.55` → fragment isn't a near-horizontal floor surface.
//     Walls and ceilings don't accumulate ground blood; we'll add a
//     separate vertical field in a follow-up pass.
//   - UV outside [0, 1] (the floor's padded extent) → skip.
//
// Wet/dry curve:
//   - `wet`  = coverage * (1 - smoothstep(0, 25 s, age))  → low-roughness
//             dark-red sheen for the first ~25 s.
//   - `dry`  = smoothstep(20 s, 75 s, age)               → albedo and
//             roughness drift toward iron-rust matte.
// Beyond ~75 s the splat continues to read as a brownish stain (it never
// fully disappears — pools dry, they don't evaporate). The next splat
// at the same texel resets `G` via the `MAX` blend, restoring the wet
// look.
//
// Normal bevel: when coverage is non-trivial, four offset samples build
// a coarse gradient that perturbs the surface normal slightly so pools
// catch torchlight along their rim. Skipped when the texel is dry —
// dried blood has no thickness.
void applyBloodField(
    inout vec3 albedo,
    inout float roughness,
    inout float metallic,
    inout vec3 N,
    vec3 Ngeo
) {
    if (ubo.bloodFieldXform.z == 0.0 && ubo.bloodFieldXform.w == 0.0) return;

    vec2 worldXZ = vec2(fragWorldPos.x, fragWorldPos.z);
    vec2 uv = (worldXZ - ubo.bloodFieldXform.xy) * ubo.bloodFieldXform.zw;
    if (any(lessThan(uv, vec2(0.0))) || any(greaterThan(uv, vec2(1.0)))) return;

    float floorY = ubo.timeData.y;
    float yAbove = fragWorldPos.y - floorY;

    // ----- Surface classification -----
    // Three cases:
    //   floor   : Ngeo.y > 0.55 AND y near floor — full pool composite
    //   wall    : Ngeo.y < 0.45 AND y in [-0.1, 2.5] above floor —
    //             vertical-streak composite, height-attenuated
    //   reject  : everything else (wall caps, ceilings, raised
    //             platforms, ledges)
    bool isFloor = Ngeo.y > 0.55 && abs(yAbove) < 0.25;
    bool isWall  = Ngeo.y < 0.45 && yAbove > -0.10 && yAbove < 2.50;
    if (!isFloor && !isWall) return;

    // ----- Time-evolving advection (floor only) -----
    // Sample the field once at the un-warped UV to read the splat's
    // age, then re-sample at a small upstream offset so the body of
    // each pool drifts along its impact direction over the first few
    // seconds. Subtle (capped at ~3 cm in world space) and tapers
    // off as the splat ages — fresh blood pulls forward; old blood
    // is locked in place. Walls don't get this because gravity drag
    // is already baked into the wall composite.
    vec2 sampleUV = uv;
    if (isFloor) {
        // Read centre to get spawn time, derive age, then build a
        // small forward-axis warp from a low-frequency hash of the
        // splat's spawn time so each kill drifts its own way.
        float t0 = texture(bloodField, uv).g;
        float age0 = max(0.0, ubo.timeData.x - t0);
        // Direction is a hash of spawn time → stable per-splat.
        float hashDir = fract(sin(t0 * 12.713) * 4321.7);
        float dirAng = hashDir * 6.2831853;
        vec2 flowDir = vec2(cos(dirAng), sin(dirAng));
        // Drift magnitude in UV space. World 0..3cm (cap) × inv extent.
        // Ramp in over the first ~0.6 s, hold at full, fade out by 8 s.
        float flowAmt = smoothstep(0.0, 0.6, age0)
                      * (1.0 - smoothstep(4.0, 8.0, age0))
                      * 0.030; // metres
        // Convert metres → UV using inv extent.
        vec2 invExtent = ubo.bloodFieldXform.zw;
        sampleUV = uv - flowDir * flowAmt * invExtent;
    }

    vec2 bloodSample = texture(bloodField, sampleUV).rg;
    float coverage = bloodSample.r;
    if (coverage < 0.01) return;

    // ----- Wall composite -----
    // Walls share the same 2D field as the floor. Naively
    // extruding the field signal upward gives painted stripes;
    // scattering pure cells gives "polka-dot balls". The right
    // structure is *splatter blobs with drip trails*: a few
    // organically-shaped masses with FBM-warped outlines, each
    // shedding thin vertical streaks below it. Existence of each
    // blob is gated by `coverage` so blobs only appear in
    // columns where the field actually has blood, but their
    // *shape* is generated procedurally so the wall reads as
    // splatter rather than a stripe of the field signal.
    //
    // Two splat scales (big body splats + smaller satellite
    // splats) plus per-splat drip trails. No vertical falloff
    // multiplier, no cell grid — the silhouette comes from the
    // blobs themselves.
    float heightMask = 0.0;
    if (isWall) {
        // 1D coord along the wall surface. For an axis-aligned
        // wall the tangent is whichever of X/Z is *not* the
        // dominant component of the geometric normal.
        float u = abs(Ngeo.x) > abs(Ngeo.z) ? fragWorldPos.z : fragWorldPos.x;
        float yA = yAbove;

        #define H11(n) fract(sin((n) * 12.9898) * 43758.5453)
        #define H21(p) fract(sin(dot((p), vec2(127.1, 311.7))) * 43758.5453)

        // Multi-octave value noise — used to warp blob outlines
        // (irregular silhouettes, not circles) and to break up
        // the body of each splat with internal texture.
        vec2 nP = vec2(u, yA);
        float n1 = H21(nP * 4.5);
        float n2 = H21(nP * 11.0 + 17.3);
        float n3 = H21(nP * 26.0 + 5.7);
        float fbm = n1 * 0.55 + n2 * 0.30 + n3 * 0.15;

        // ---- Splat blobs ----
        // Two passes at different cell pitches. Big splats sit
        // at ~30 cm pitch (one major impact per stride along
        // the wall), small satellites at ~12 cm. Each pass
        // examines its three nearest cells along the u axis,
        // so blobs straddle cell boundaries naturally.
        float blobAcc = 0.0;
        float dripAcc = 0.0;

        // Pass 0: big splats. radius 0.10–0.22 m.
        // Pass 1: small splats. radius 0.04–0.09 m.
        for (int pass = 0; pass < 2; pass++) {
            float cellSize = pass == 0 ? 0.30 : 0.12;
            float rMin     = pass == 0 ? 0.10 : 0.04;
            float rVar     = pass == 0 ? 0.12 : 0.05;
            float covGate  = pass == 0 ? 1.40 : 1.60;
            // Drip strength scales by pass; big blobs drip
            // hard, small blobs barely.
            float dripStrength = pass == 0 ? 1.0 : 0.45;
            float seed     = pass == 0 ? 13.0 : 91.0;

            float baseCellId = floor(u / cellSize);
            for (int i = -1; i <= 1; i++) {
                float cellId = baseCellId + float(i);
                vec2 cc = vec2(cellId, seed);

                // Existence gate: coverage at this fragment must
                // clear a hashed threshold for the cell to host
                // a blob. This ties blob density to the field
                // signal strength — a heavy splatter column
                // hosts many blobs, a light one hosts few.
                float pres = step(H21(cc * 5.7), coverage * covGate);
                if (pres < 0.5) continue;

                // Blob centre. u offset within the cell + Y
                // hashed around chest height ±35 cm (so blobs
                // don't all line up at the same height).
                float cu = (cellId + 0.18 + H21(cc * 1.7) * 0.64) * cellSize;
                float cy = 0.95 + (H21(cc * 2.3) - 0.5) * 0.70;

                // Radius hashed.
                float r = rMin + H21(cc * 3.1) * rVar;
                // Aspect — slight vertical stretch (gravity pull
                // before drying) to taste.
                float aspectY = 0.85 + H21(cc * 4.7) * 0.40;

                // Distance to centre with FBM-driven warp so the
                // outline is irregular, not a perfect ellipse.
                vec2 d = vec2(u - cu, (yA - cy) / aspectY);
                float warp = (fbm - 0.5) * r * 0.55;
                float dist = length(d) + warp;
                float body = 1.0 - smoothstep(r * 0.65, r * 1.10, dist);
                // Internal texture — slightly thinner inside the
                // blob so it doesn't read as a flat fill.
                body *= 0.75 + 0.25 * fbm;
                blobAcc = max(blobAcc, body);

                // ---- Drip trails from this blob ----
                // Drips emerge from the *bottom* of the blob.
                // belowDist measures how far the fragment sits
                // below blob centre.
                float belowDist = cy - yA;
                if (belowDist > 0.0 && belowDist < 0.80) {
                    // Each blob spawns up to ~3 narrow drip
                    // streaks within ±r of its centre. Streaks
                    // live in fine 8 mm columns.
                    float colId = floor((u - cu) / 0.008);
                    float colCenter = (colId + 0.5) * 0.008 + cu;
                    // Per-streak hashed length and presence.
                    vec2 sc = vec2(colId, seed * 3.7);
                    float dripLen = 0.18 + H21(sc * 7.3) * 0.50;
                    // Presence: streak only fires if it sits
                    // within the blob's lateral footprint AND
                    // clears a coverage-modulated threshold AND
                    // its column hash is above a sparsity gate
                    // (so we don't get a continuous curtain).
                    float lateralFromCenter = abs(colCenter - cu);
                    float lateralOK = step(lateralFromCenter, r * 0.85);
                    float streakPres = step(H21(sc * 11.1),
                                            coverage * 0.55 * dripStrength);
                    if (lateralOK > 0.5
                        && streakPres > 0.5
                        && belowDist < dripLen) {
                        // Width: tapers from ~half a column at
                        // the top to ~third at the bottom.
                        float dripT = belowDist / max(dripLen, 1e-3);
                        float taper = mix(1.0, 0.45, dripT);
                        float streakW = 0.0035 * taper;
                        // Wobble: gentle horizontal drift.
                        float wobble = sin((colId * 0.71)
                                           + dripT * 11.0) * 0.0012;
                        float streakDist = abs(u - colCenter - wobble);
                        float streakBody = 1.0 - smoothstep(
                            streakW, streakW * 1.6, streakDist);
                        // Bead at the leading edge.
                        float bead = 1.0 - smoothstep(
                            0.0, 0.06, abs(dripT - 0.92));
                        bead *= 1.0 - smoothstep(0.005, 0.010,
                                                 streakDist);
                        // Fade in just below blob, fade out at
                        // tail.
                        float aliveY = smoothstep(0.0, 0.020,
                                                  belowDist)
                                     * (1.0 - smoothstep(
                                         dripLen - 0.020,
                                         dripLen, belowDist));
                        float drip = (streakBody + bead * 0.6)
                                    * aliveY * dripStrength;
                        dripAcc = max(dripAcc, drip);
                    }
                }
            }
        }

        heightMask = max(blobAcc, dripAcc);

        // ----- Capillary contact pooling -----
        // Where the wall meets a bloodied floor, real fluids
        // climb the wall via surface tension and gather along
        // the join. We add a thin, very wet, slightly darkened
        // strip at the bottom of the wall whose intensity is
        // gated by the floor's coverage in this column. Reads
        // as a wet line tracing the corner where wall touches
        // bloody floor — exactly the contact cue real fluids
        // produce.
        float contactPool = (1.0 - smoothstep(0.0, 0.06, yA))
                          * smoothstep(0.0, 0.005, yA);
        // Modulate by base coverage so dry columns stay clean.
        contactPool *= clamp(coverage * 0.9, 0.0, 1.0);
        heightMask = max(heightMask, contactPool);

        // Hard upper cap: nothing above 2.0 m.
        heightMask *= 1.0 - smoothstep(1.85, 2.05, yA);
        // Soft lower edge: blend into floor pool seamlessly.
        heightMask *= smoothstep(-0.04, 0.04, yA);

        // Wall coverage usually arrives a bit weaker than floor
        // coverage from the same kill (rays scatter), so push
        // it up a touch so the wall splatter reads at parity
        // with the pool below it.
        coverage = clamp(coverage * 1.4, 0.0, 1.0);

        #undef H11
        #undef H21
    } else {
        heightMask = 1.0;
    }

    if (heightMask < 0.02) return;
    coverage *= heightMask;
    if (coverage < 0.01) return;

    float age = max(0.0, ubo.timeData.x - bloodSample.g);
    // Stay vivid — wet phase out to 45s, then a long dried tail. The
    // overlap means there's a window where blood is partly tacky
    // (still red, no longer mirror-glossy) which sells the
    // "recently bled" read at typical play pacing.
    float wet = coverage * (1.0 - smoothstep(0.0, 45.0, age));
    float dry = smoothstep(35.0, 120.0, age);

    // ----- Crease-aware accumulation (floor only) -----
    // Real spilled blood pools wherever the surface dips —
    // grout lines, cracks, mortar gaps, divots in worn stone.
    // Rather than hardcoding an axis-aligned tile grid (which
    // doesn't match the diagonal layout of the desert-rocks
    // tiles we ship and looks square on any other floor pack),
    // we derive a "crease mask" from the normal map itself:
    // wherever the perturbed normal `N` deviates from the
    // geometric `Ngeo`, the fragment is sitting on a slope —
    // a crevice or tile edge. Blood pooled in those creases
    // reads thicker, darker, and stays wet longer, while
    // raised tile faces (where N ≈ Ngeo) dry first.
    //
    // This works for any normal-mapped floor surface and
    // automatically follows the texture's actual layout
    // direction. Falls back to no modulation if the floor's
    // normal map is flat (e.g. a procedural floor that hasn't
    // installed a real PBR pack yet).
    float groutBoost = 1.0;
    float centreFade = 1.0;
    if (isFloor) {
        // Normal deviation from geometric: 0 on flat tile
        // faces, > 0 on slopes / crevices. Cube the value so
        // only sharper slope angles register — flat faces
        // don't accidentally pick up tiny normal-map noise.
        float ndev = 1.0 - clamp(dot(N, Ngeo), 0.0, 1.0);
        float creaseMask = smoothstep(0.04, 0.30, ndev);
        // Pooling: creases add up to +75 % wet weight.
        groutBoost = 1.0 + 0.75 * creaseMask;
        // Tile-face fade: flat areas dry by up to ~25 %.
        centreFade = 1.0 - 0.25 * (1.0 - creaseMask);

        // ----- Contact accumulation against vertical
        //       geometry -----
        // Where a wall, pillar or prop meets the floor, fluids
        // gather along the contact ring (a few millimetres of
        // capillary creep). We detect "I am a floor pixel
        // adjacent to a non-floor pixel" by looking at the
        // screen-space derivative of the geometric normal —
        // wherever |dNgeo|/|dpos| spikes, the floor surface is
        // ending against a vertical face within one pixel.
        // The detector is cheap (two ddx/ddy taps already done
        // implicitly by the GPU) and surface-agnostic, so it
        // catches barrel feet, pillar bases, character feet,
        // and prop touch-downs without per-object setup.
        vec3 dNx = dFdx(Ngeo);
        vec3 dNy = dFdy(Ngeo);
        float nGrad = length(dNx) + length(dNy);
        float contactRing = smoothstep(0.20, 1.20, nGrad);
        // Pooling boost: contact rings are 1.5× the inner crease
        // multiplier so a wet floor visibly thickens against
        // every vertical it meets.
        groutBoost *= mix(1.0, 1.8, contactRing);
        // And drying slows in the ring, since the contact line
        // is shaded and shielded from airflow.
        centreFade *= mix(1.0, 1.15, contactRing);
    }
    wet *= groutBoost * centreFade;
    // Drying advances faster on flat tile faces (centreFade < 1
    // ↔ higher dry).
    dry = clamp(dry * (2.0 - centreFade), 0.0, 1.0);

    // Fresh blood: vivid arterial red. Dried blood: warm iron-rust
    // brown. Both are sRGB-decoded values; the forward target is
    // linear so we don't need a manual pow(2.2). The fresh tone is
    // intentionally bright — the post-pipeline ACES tonemap pulls
    // saturated reds toward orange, so we overshoot here to land on
    // a deep, readable blood-red on screen.
    vec3 fresh = vec3(0.62, 0.04, 0.03);
    vec3 dried = vec3(0.20, 0.07, 0.05);
    vec3 bloodAlbedo = mix(fresh, dried, dry);

    // Coverage controls how much of the underlying floor albedo is
    // overwritten. A small floor-show-through stays even at full
    // coverage so the silhouette doesn't read as a flat sticker.
    //
    // ----- Edge-hardness modulation -----
    // Real spilled blood has thin feathered outskirts, but also
    // sharp coagulated ridges, tiny islands, and torn-paper
    // breakup where surface tension pulls the surface apart.
    // The raw `coverage` field gives uniform Gaussian-style edges,
    // which read as soft and rubbery. We modulate the edge
    // sharpness with high-frequency hash noise so different
    // sections of the same perimeter have different falloffs:
    //
    //   * `edgeBand` is 1 in the rim transition (where coverage
    //     is sliding from 0 → 1) and 0 inside the body and far
    //     outside.
    //   * `edgeNoise` is a tile-aware hash on world XZ so the
    //     pattern is stable across frames and isn't visibly
    //     animated.
    //   * `coagWidth` shifts the edge from a wide soft falloff
    //     (where the noise is low — feathered, blood seeped
    //     into porous stone) to a tight hard cliff (where it's
    //     high — dried surface tension ridge / coagulated rim).
    //
    // The result reads as a varied perimeter with both crusty
    // ridges and feathered outskirts, with ragged broken islands
    // where the dither hash tips the threshold past the body.
    float covRaw = coverage;
    float edgeBand = smoothstep(0.04, 0.50, covRaw)
                   * (1.0 - smoothstep(0.50, 0.95, covRaw));
    if (isFloor && edgeBand > 0.001) {
        // Two-octave hash-noise on world XZ for high-frequency
        // breakup. World-space so the pattern doesn't crawl
        // across the floor as the field UV shifts.
        vec2 nP = vec2(fragWorldPos.x, fragWorldPos.z);
        #define H21(p) fract(sin(dot((p), vec2(127.1, 311.7))) * 43758.5453)
        float hN = H21(nP * 22.0);
        float hN2 = H21(nP * 7.0 + 13.7);
        float edgeNoise = hN * 0.7 + hN2 * 0.3;
        // Map noise to a remap centre and width. Width swings
        // from 0.18 (soft feathered) to 0.04 (sharp coag rim).
        float coagWidth = mix(0.18, 0.04, edgeNoise);
        float coagCenter = mix(0.32, 0.55, edgeNoise);
        float remapped = smoothstep(
            coagCenter - coagWidth,
            coagCenter + coagWidth,
            covRaw);
        // Stochastic islands: a small number of isolated
        // fragments pop *out* of the body just inside the rim,
        // and a few isolated splashes pop *into* coverage just
        // outside it. Simulates surface tension breakup.
        float islandHash = H21(nP * 95.0 + 4.1);
        // Pull-in: where the high-freq hash is very low along
        // the inner rim, force coverage to zero (a tear).
        float tearMask = smoothstep(0.10, 0.45, covRaw)
                       * (1.0 - smoothstep(0.45, 0.70, covRaw));
        if (tearMask > 0.001 && islandHash < 0.05) {
            remapped *= 1.0 - tearMask * 0.8;
        }
        // Push-out: where the hash is high just outside the
        // rim, let a stray drop fragment through.
        if (covRaw > 0.005 && covRaw < 0.10 && islandHash > 0.985) {
            remapped = max(remapped, 0.55);
        }
        coverage = mix(covRaw, remapped, edgeBand);
        #undef H21
    }
    float cov = clamp(coverage * 1.3, 0.0, 1.0);
    albedo = mix(albedo, bloodAlbedo, cov * 0.92);
    // Grout pools darker — multiply albedo down where groutBoost
    // exceeds 1.0 so blood that settled into cracks reads as a
    // slightly thicker, deeper-coloured streak.
    albedo *= mix(1.0, 0.78, clamp((groutBoost - 1.0) * 1.6, 0.0, 1.0));

    // Roughness: wet pools are glassy (~0.12), dried blood is slightly
    // rougher than the floor it sits on but not chalky. The lerp below
    // sweeps the wet end through to the dried end as the age advances.
    float bloodRoughness = mix(0.12, 0.55, dry);

    // Coagulated rim: the perimeter of a fresh pool dries first
    // (more surface area exposed to air per unit volume) and
    // forms a slightly tackier, matter ring. We bump the
    // roughness toward the dried value within the edge band so
    // the centre of every pool stays glossy while the rim reads
    // as crusted. Also nudge the albedo slightly darker at the
    // rim — coagulated blood is a deeper purple-black than the
    // wet body.
    float rimMask = isFloor
        ? smoothstep(0.55, 0.95, covRaw) * (1.0 - smoothstep(0.95, 1.0, covRaw))
        : 0.0;
    rimMask = 1.0 - rimMask; // 1 at the rim band, 0 in body
    rimMask *= smoothstep(0.10, 0.45, covRaw);
    bloodRoughness = mix(bloodRoughness, mix(0.55, 0.80, dry), rimMask * 0.7);
    bloodAlbedo = mix(bloodAlbedo, bloodAlbedo * 0.55, rimMask * 0.5);

    // ----- Localised high-gloss streaks -----
    // Wet liquid surfaces don't show a uniform gloss response —
    // surface tension + thin-film thickness variation produces
    // razor-thin specular streaks and broken reflective patches
    // ("wet veins" in the puddle) that the eye subconsciously
    // expects. We modulate the roughness with stretched
    // world-space hash noise: anisotropic frequencies (low along
    // X, high along Z) make the noise read as elongated streaks
    // rather than isotropic speckle, mimicking the look of an
    // anisotropic BRDF without the cost. Magnitude is gated by
    // wetness so dry blood stays uniformly matte.
    if (isFloor) {
        #define H21S(p) fract(sin(dot((p), vec2(127.1, 311.7))) * 43758.5453)
        vec2 wp = vec2(fragWorldPos.x, fragWorldPos.z);
        // Two stretched octaves. The first creates broad
        // streaks, the second adds smaller broken patches on
        // top so the gloss response has both long veins and
        // tiny hot reflections.
        float streak1 = H21S(vec2(wp.x * 6.0, wp.y * 32.0) + 1.7);
        float streak2 = H21S(vec2(wp.x * 14.0, wp.y * 70.0) + 9.3);
        float streakNoise = streak1 * 0.65 + streak2 * 0.35;
        // Multiply roughness by a 0.55 .. 1.45 sweep to push
        // wet streaks down to mirror gloss and dry stretches
        // up to a tackier sheen — same average, far more
        // varied per-fragment.
        float streakMul = mix(0.55, 1.45, streakNoise);
        // Only modulate the wet end of the response (1 - dry)
        // so an old stain doesn't sparkle; ramp by `cov` so
        // the streaks live inside the splat, not on the
        // surrounding floor.
        float streakAmp = (1.0 - dry) * cov;
        bloodRoughness *= mix(1.0, streakMul, streakAmp);
        #undef H21S
    }

    roughness = clamp(mix(roughness, bloodRoughness, cov), 0.04, 1.0);

    // Blood is dielectric — kill metallic contribution wherever the
    // floor was metallic (rare, but cleans up edge cases).
    metallic *= (1.0 - cov * 0.95);

    // Normal bevel from the gradient of wet coverage. Only worth doing
    // while the pool is still wet AND we're on the floor — the
    // gradient is in world XZ, which is meaningless for vertical
    // surfaces.
    if (isFloor && wet > 0.05) {
        // Sample 4 neighbours at ~1 texel offset for a finite-difference
        // gradient. The texture is 1024² so 1/1024 in UV is one texel.
        vec2 ts = vec2(1.0 / 1024.0);
        float wL = texture(bloodField, sampleUV + vec2(-ts.x, 0)).r;
        float wR = texture(bloodField, sampleUV + vec2( ts.x, 0)).r;
        float wD = texture(bloodField, sampleUV + vec2(0, -ts.y)).r;
        float wU = texture(bloodField, sampleUV + vec2(0,  ts.y)).r;
        // Gradient in world XZ. The puddle is high in the middle and
        // low at its edges, so the bevel normal tilts outward.
        vec2 grad = vec2(wR - wL, wU - wD);
        // Bevel strength scales with wetness so dried blood stays flat.
        float bevel = 0.55 * wet;
        vec3 nWorld = normalize(N);
        // Push normal away from the puddle centre by translating
        // along world X / Z. We project this delta onto the tangent
        // plane of the geometric normal so we never invert the
        // facing.
        vec3 tilt = vec3(grad.x, 0.0, grad.y) * bevel;
        tilt -= Ngeo * dot(tilt, Ngeo);

        // Micro undulations: low-frequency noise on world XZ
        // adds tiny meniscus bulges + uneven pooling so the
        // surface reads as "fluid sitting on the floor" rather
        // than "wet floor". Driven by central-difference of a
        // procedural height field so adjacent fragments share a
        // consistent gradient (no per-fragment hash sparkle).
        // Amplitudes intentionally tiny — visible as glints
        // moving with the camera, not as a bumpy mess.
        if (cov > 0.05) {
            vec2 wp = vec2(fragWorldPos.x, fragWorldPos.z);
            // Two scales of noise — large lazy undulation at
            // ~12 cm and a finer ripple at ~4 cm. Both use
            // smooth value-noise (cubic Hermite interp on a
            // hashed lattice) so the gradient is C1.
            #define HASH(p) fract(sin(dot((p), vec2(127.1, 311.7))) * 43758.5453)
            vec2 wp1 = wp * 8.0;   // ~12.5 cm period
            vec2 wp2 = wp * 24.0;  // ~4 cm period
            // Coarse hash gradient (forward differences).
            float h1c = HASH(floor(wp1));
            float h1x = HASH(floor(wp1 + vec2(1.0, 0.0)));
            float h1y = HASH(floor(wp1 + vec2(0.0, 1.0)));
            vec2 g1 = vec2(h1x - h1c, h1y - h1c);
            float h2c = HASH(floor(wp2) + 17.3);
            float h2x = HASH(floor(wp2 + vec2(1.0, 0.0)) + 17.3);
            float h2y = HASH(floor(wp2 + vec2(0.0, 1.0)) + 17.3);
            vec2 g2 = vec2(h2x - h2c, h2y - h2c);
            // Combined micro-tilt. Amplitude attenuates as the
            // pool dries (wet × cov gate) so old blood reads
            // flat the way it should.
            vec2 microGrad = (g1 * 0.6 + g2 * 0.4);
            float microAmp = 0.04 * wet;
            vec3 microTilt = vec3(microGrad.x, 0.0, microGrad.y) * microAmp;
            microTilt -= Ngeo * dot(microTilt, Ngeo);
            tilt += microTilt;
            #undef HASH
        }

        N = normalize(nWorld + tilt);
    }
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

    // Slope-scaled depth bias. Kept very small so the contact
    // shadow stays glued to its caster — too much bias here
    // peels the umbra away from the silhouette and the wide
    // PCF kernel reads as a detached penumbra halo around
    // the now-lifted contact line. The constant floor is
    // intentionally near-zero; the slope-scaled term plus the
    // sampler2DShadow's hardware PCF give us all the acne
    // protection we need on the actual surfaces.
    float NdotL = max(dot(N, L), 0.0);
    float bias = max(0.00035 * (1.0 - NdotL), 0.00004);
    shadowUV.z -= bias;

    if (shadowUV.x < 0.0 || shadowUV.x > 1.0 ||
        shadowUV.y < 0.0 || shadowUV.y > 1.0 ||
        shadowUV.z < 0.0 || shadowUV.z > 1.0) {
        return 1.0;
    }

    // ----- Frustum-edge feather -----
    // The directional shadow map is a fixed-size ortho box
    // that follows the player. Without this fade the player
    // would see a hard square seam where receivers transition
    // from "in-frustum, self-shadowed" to "out-of-frustum,
    // returns 1.0" — that boundary tracked the camera and
    // read as a moving shadow rectangle. We *also* use this
    // fade to mask **caster popping**: when a tall caster
    // (pillar, statue, enemy) crosses the frustum boundary,
    // its shadow on receivers near that boundary appears
    // suddenly. By widening the fade to the outer ~30% of the
    // frustum (~6 world-metres at the current half-extent),
    // those receivers are already faded toward unshadowed by
    // the time the caster's contribution would have been
    // visible — the pop becomes a smooth ramp coinciding with
    // the fog band where things are dimming anyway.
    vec2 edgeUV = abs(shadowUV.xy - 0.5) * 2.0;       // 0 centre, 1 edge
    float edge  = max(edgeUV.x, edgeUV.y);
    float frustumFade = smoothstep(0.70, 1.00, edge); // 0 inside, 1 at rim

    vec2 texelSize = 1.0 / vec2(textureSize(shadowMap, 0));

    // ----- Layered penumbra -----
    // Three independent factors drive the kernel size, so the
    // penumbra grows the way real shadows do instead of being
    // pinned at a fixed radius:
    //
    //   * distFactor — receivers far from the camera get softer
    //     edges. This both matches how the eye perceives depth
    //     (out-of-focus shadows look softer) and hides shadow-
    //     map sampling artefacts at the fog edge where pixel
    //     density is lowest.
    //
    //   * grazing   — surfaces near-perpendicular to the light
    //     have a much larger geometric penumbra in real life. We
    //     amplify the kernel by (1 - NdotL).
    //
    //   * depthFactor — receivers deep into the light's frustum
    //     are farther from the (notional) blocker, so the
    //     penumbra spreads more. Without raw blocker depth from
    //     the shadow map we can't do PCSS proper, but the receiver
    //     depth alone is a useful proxy.
    float camDist     = length(fragWorldPos - ubo.cameraPos.xyz);
    float distFactor  = smoothstep(1.5, 16.0, camDist);
    float grazing     = 1.0 - clamp(NdotL, 0.0, 1.0);
    float depthFactor = smoothstep(0.05, 0.95, clamp(shadowUV.z, 0.0, 1.0));

    // ----- Sharp-near, soft-far -----
    // The classic "detached halo" around a cast shadow is the
    // PCF kernel itself: with 12 taps spanning ~1 texel, each
    // tap that lands outside the silhouette votes "lit" but
    // the average reads as a soft 1-texel fringe ring. To kill
    // that fringe we use a *single* hardware-PCF tap for the
    // foreground (no kernel = no fringe), and only blend in
    // the multi-tap soft penumbra at the fog edge where the
    // shadow map's sampling density is lowest and a soft
    // edge actually helps hide undersampling artefacts.
    //
    // Even the single-tap result needs help: hardware PCF on
    // a sampler2DShadow runs a 2×2 bilinear comparison, which
    // returns one of {0, 0.25, 0.5, 0.75, 1.0}. Those five
    // discrete levels read as 2–3 concentric rings around
    // the umbra ("gradationally softer outlines from the
    // inner shadow"). We dither the sample position by a
    // half-texel of screen-space hash noise so the boundary
    // becomes stochastic instead of stepped — perceptually
    // a clean continuous edge.
    vec2 hashSeed = gl_FragCoord.xy + vec2(ubo.timeData.x * 60.0, 0.0);
    float jitterX = fract(sin(dot(hashSeed, vec2(12.9898, 78.233))) * 43758.5453);
    float jitterY = fract(sin(dot(hashSeed, vec2(63.7264, 10.873))) * 43758.5453);
    vec2 jitter = (vec2(jitterX, jitterY) - 0.5) * texelSize;
    vec3 sharpUV = vec3(shadowUV.xy + jitter, shadowUV.z);
    float sharp = texture(shadowMap, sharpUV);
    float soft  = 0.0;
    // Skip the 12-tap kernel entirely when distFactor is near
    // zero — saves the work and guarantees a clean single-tap
    // result on every foreground pixel.
    if (distFactor > 0.05) {
        // Kernel only widens once the receiver is past ~6 m
        // from the camera. Worst case ~2 texels.
        float kernelScale = mix(0.0, 2.0, distFactor)
                          * (1.0 + grazing * 0.15);
        vec2 kernel = texelSize * kernelScale;
        for (int i = 0; i < 12; i++) {
            vec2 offset = POISSON_DISK[i] * kernel;
            soft += texture(shadowMap, vec3(shadowUV.xy + offset, shadowUV.z));
        }
        soft *= (1.0 / 12.0);
    } else {
        soft = sharp;
    }
    // Blend sharp→soft as the receiver recedes. distFactor is
    // already smoothstep'd 1.5..16 m so the transition feels
    // natural — close shadows are crisp and welded to their
    // casters; distant shadows soften into the fog.
    float s = mix(sharp, soft, distFactor);

    // Apply the frustum-edge feather: pull s toward 1.0 as the
    // sample approaches the frustum boundary. This kills the
    // visible square that otherwise tracked the player.
    s = mix(s, 1.0, frustumFade);

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

    // PCF over an orthonormal basis built from `dir`. Offsets
    // are scaled in normalised-distance space so the kernel
    // covers ~1–2 atlas texels at our 512² per face.
    //
    // Why 8 taps on a Poisson disk with a per-pixel rotation?
    // The previous implementation used 5 hard `step()` taps on
    // a fixed cross pattern, which can only produce 6 discrete
    // output values {0, 0.2, ..., 1.0}. Every neighbouring
    // pixel sampled the same offsets, so those discrete levels
    // lined up across the screen as 4–5 visible concentric
    // "softer outline" rings around every shadow — the exact
    // artefact we kept chasing in the wrong shaders. Rotating
    // the tap basis by a per-pixel screen-space hash turns
    // each ring into spatial noise that the eye averages into
    // a clean penumbra. Doubling the tap count to 8 brings
    // the level count up to 9 so the residual noise is finer.
    vec3 up = abs(dir.y) > 0.95 ? vec3(0.0, 0.0, 1.0) : vec3(0.0, 1.0, 0.0);
    vec3 tuRaw = normalize(cross(up, dir));
    vec3 tvRaw = cross(dir, tuRaw);
    // Per-pixel rotation angle from gl_FragCoord. The constants
    // are the standard sin-fract noise basis; multiplying by
    // 2π turns the [0,1) hash into a full rotation.
    float rotHash = fract(sin(dot(gl_FragCoord.xy, vec2(12.9898, 78.233))) * 43758.5453);
    float rotAng = rotHash * 6.2831853;
    float rcs = cos(rotAng);
    float rsn = sin(rotAng);
    vec3 tu = tuRaw * rcs + tvRaw * rsn;
    vec3 tv = -tuRaw * rsn + tvRaw * rcs;
    // Kernel half-width in normalised-distance units. Sized
    // so the disk fits inside ~1 atlas texel: pixels deep in
    // shadow or fully lit have all 8 taps land in the same
    // texel and read the same value (no grain), while pixels
    // straddling the silhouette get a few taps on each side
    // — combined with the per-pixel basis rotation that gives
    // a clean stochastic feather only at the boundary.
    float k = 0.0025;

    // 8-tap Poisson-ish disk in 2D (radius 1).
    const vec2 P0 = vec2( 0.000,  0.000);
    const vec2 P1 = vec2( 0.866,  0.500);
    const vec2 P2 = vec2(-0.500,  0.866);
    const vec2 P3 = vec2(-0.866, -0.500);
    const vec2 P4 = vec2( 0.500, -0.866);
    const vec2 P5 = vec2( 0.383,  0.924);
    const vec2 P6 = vec2(-0.924,  0.383);
    const vec2 P7 = vec2(-0.383, -0.924);

    float occ = 0.0;
    occ += step(normFrag - bias, texture(pointShadowAtlas, vec4(dir + (tu * P0.x + tv * P0.y) * k, float(lightIdx))).r);
    occ += step(normFrag - bias, texture(pointShadowAtlas, vec4(dir + (tu * P1.x + tv * P1.y) * k, float(lightIdx))).r);
    occ += step(normFrag - bias, texture(pointShadowAtlas, vec4(dir + (tu * P2.x + tv * P2.y) * k, float(lightIdx))).r);
    occ += step(normFrag - bias, texture(pointShadowAtlas, vec4(dir + (tu * P3.x + tv * P3.y) * k, float(lightIdx))).r);
    occ += step(normFrag - bias, texture(pointShadowAtlas, vec4(dir + (tu * P4.x + tv * P4.y) * k, float(lightIdx))).r);
    occ += step(normFrag - bias, texture(pointShadowAtlas, vec4(dir + (tu * P5.x + tv * P5.y) * k, float(lightIdx))).r);
    occ += step(normFrag - bias, texture(pointShadowAtlas, vec4(dir + (tu * P6.x + tv * P6.y) * k, float(lightIdx))).r);
    occ += step(normFrag - bias, texture(pointShadowAtlas, vec4(dir + (tu * P7.x + tv * P7.y) * k, float(lightIdx))).r);
    return occ * 0.125;
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

    // Blood field composite (per-floor wet/dry pools). Mutates the
    // PBR inputs in place before the BRDF math so the lighting picks
    // up the puddle's specular highlight naturally — wet blood
    // glistens off torches, dry blood stays matte.
    applyBloodField(albedo, roughness, metallic, N, Ngeo);

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
    for (int i = 0; i < numLights && i < 16; i++) {
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

        // ---- Fake hemispherical bounce -----
        // Torches throw the bulk of their light down at the
        // floor; in a real scene, that flux scatters off the
        // floor and warmly tints the lower walls, pillars and
        // prop sides nearby. Without GI we can't compute it
        // properly, but a cheap approximation gets 80 % of the
        // perceptual win:
        //   * pretend a virtual light sits at the floor below
        //     the torch, oriented up
        //   * its colour is the torch colour with extra warm
        //     bias (red bounces stronger off warm dungeon stone)
        //   * its falloff is sharper (energy is divided across
        //     a hemisphere now), it carries no specular, and it
        //     does not need a shadow sample
        // The result: lower walls and pillar bases pick up a
        // warm fill exactly where torches sit nearby, without
        // any extra render passes or descriptor work.
        vec3 bouncePos = vec3(lightPos.x, ubo.timeData.y, lightPos.z);
        vec3 bounceVec = bouncePos - fragWorldPos;
        float bounceDist = length(bounceVec);
        float bounceRadius = radius * 0.85;
        if (bounceDist < bounceRadius) {
            vec3 Lb = bounceVec / max(bounceDist, 1e-3);
            // Cosine-weighted by upward component of the
            // surface-to-bounce vector — strongest on
            // upward-facing fragments and lower-wall fragments
            // that face the bounce origin, near-zero on
            // ceilings.
            float NdotB = max(dot(N, Lb), 0.0);
            float bAtten = 1.0 - (bounceDist / bounceRadius);
            bAtten = bAtten * bAtten;
            // Warm bias — multiply colour temperature toward
            // amber so even a cool point light reads as
            // warm-bounced floor light.
            vec3 bounceCol = lightCol * vec3(1.10, 0.85, 0.55);
            // Lambert only, no specular.
            lighting += albedo / PI * bounceCol * intensity
                      * NdotB * bAtten * 0.18;
        }
    }

    return lighting;
}

// ---------------------------------------------------------------------------
// Legacy cel-shading path. Preserved verbatim for monsters / players
// / props so the project's existing painted look stays intact.
// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Per-class light response for the cel shading path.
//
// The legacy cel path applies one Phong-ish curve to every
// fragment, so skin / cloth / leather all read with the same
// response — and skin in particular ends up looking like
// diffuse-lit clay sitting inside a much richer environment.
// `evalCharLight` evaluates a single light's contribution with
// material-aware diffuse + multi-lobe spec + Fresnel sheen +
// terminator subsurface warmth, then blends back to the legacy
// cel curve weighted by `(1 - matWeight)`. The pore-scale
// micro-breakup uses a stable world-space hash so highlights
// don't crawl across the surface as the camera moves.
// ---------------------------------------------------------------------------
vec3 evalCharLight(
    vec3 N, vec3 L, vec3 V,
    vec3 baseColor, vec3 lightCol,
    float skinMask, float clothMask, float leatherMask,
    float specMul, float specPower
) {
    float NdotL = dot(N, L);
    float NdotLp = max(NdotL, 0.0);
    float NdotV = max(dot(N, V), 0.0);
    vec3  H = normalize(L + V);
    float NdotH = max(dot(N, H), 0.0);

    // ---- Diffuse curve ----
    // We deliberately do NOT use a hard cel staircase here any
    // more. The legacy 3-plateau ramp painted visible colour
    // bands across the character (the eye reads them as the
    // dress / arm being striped) and made every monster look
    // hand-painted. Instead, every material now uses a smooth
    // wrap-Lambert curve; the per-class differences live in the
    // *width* of the wrap (skin = soft, cloth = medium, leather
    // = sharp) and in the spec response below.
    //
    // wrapLambert(N·L, w) = saturate((N·L + w) / (1 + w))
    //   w = 0   → straight Lambert (hard terminator)
    //   w = 0.5 → soft, "skin under softbox" terminator
    // Squaring the result darkens the unlit side a touch
    // without re-introducing a step.
    float skinWrap = clamp((NdotL + 0.45) / 1.45, 0.0, 1.0);
    float skinDiff = skinWrap * skinWrap;

    float clothWrap = clamp((NdotL + 0.20) / 1.20, 0.0, 1.0);
    float clothDiff = clothWrap * clothWrap;

    float leatherWrap = clamp((NdotL + 0.10) / 1.10, 0.0, 1.0);
    float leatherDiff = leatherWrap * leatherWrap;

    // Default fallback: wrap-Lambert with a small wrap so
    // unclassified fragments still get a smooth shading curve
    // instead of the old staircase.
    float defaultWrap = clamp((NdotL + 0.15) / 1.15, 0.0, 1.0);
    float defaultDiff = defaultWrap * defaultWrap;

    float matDiff = skinDiff * skinMask
                  + clothDiff * clothMask
                  + leatherDiff * leatherMask;
    float diff = mix(defaultDiff, matDiff, clamp(skinMask + clothMask + leatherMask, 0.0, 1.0));

    // ---- Specular: layered for skin, single lobe otherwise ----
    // Pore-scale micro-breakup. World-space high-frequency hash
    // modulates the sharp lobe so the highlight isn't a
    // perfectly smooth glossy patch — gives the eye/cheek/arm
    // highlight that broken, alive quality real skin has.
    float poreHash = fract(sin(dot(fragWorldPos.xy * 91.7
                                 + fragWorldPos.z * 47.3,
                                  vec2(127.1, 311.7))) * 43758.5453);
    float poreMod = mix(0.55, 1.05, poreHash);

    // Single Phong lobe — used as-is for cloth / leather and
    // as the *broad* component of the layered skin lobe.
    float broadLobe = pow(NdotH, max(specPower * 0.5, 8.0));
    // Sharp oily highlight — skin only. Power is much higher
    // (sebum is glossy at the micrometre scale) and amplitude
    // is gated by the broad lobe so we don't get a stray spike
    // outside the highlight zone.
    float sharpLobe = pow(NdotH, 256.0) * poreMod;
    // Fresnel grazing kick — light scatters off the oily
    // surface most strongly at glancing angles, reading as the
    // soft sheen on a cheek / forearm that catches a torch
    // sideways. Schlick approximation, gated by NdotL so
    // unshaded fragments don't sheen.
    float fres = pow(1.0 - NdotV, 5.0);
    float skinFres = fres * NdotLp * 0.45;

    // Compose skin spec: ~40 % broad sheen, ~60 % sharp oil,
    // plus the grazing Fresnel kick.
    float skinSpec = (broadLobe * 0.45 + sharpLobe * 0.85 + skinFres * 0.6)
                   * specMul;
    // Default lobe: same broad lobe at the legacy intensity,
    // used as the fallback for unclassified materials.
    float celSpec = pow(NdotH, specPower) * specMul;
    // Cloth never specs in a hard lobe — replace with a
    // pure Fresnel band that reads as microfibre back-scatter.
    float clothSpec = fres * NdotLp * 1.4 * specMul;
    // Leather: medium broad lobe. The streak hash already
    // baked into `specMul` provides the per-fragment variation
    // so we don't need a sharp lobe on top.
    float leatherSpec = pow(NdotH, max(specPower * 0.6, 16.0)) * specMul;
    float matSpec = skinSpec * skinMask
                  + clothSpec * clothMask
                  + leatherSpec * leatherMask;
    float spec = mix(celSpec, matSpec, clamp(skinMask + clothMask + leatherMask, 0.0, 1.0));

    // ---- Subsurface terminator warmth ----
    // Just before the shadow boundary, real skin lets some red
    // light scatter through and read on the surface. We detect
    // "near terminator" with a band centred at NdotL ≈ 0.18 and
    // add a warm-tinted lift to the diffuse contribution there.
    // Skin only.
    float termBand = (1.0 - abs(NdotL - 0.18) * 4.5);
    termBand = clamp(termBand, 0.0, 1.0);
    vec3 sssBoost = vec3(0.18, 0.06, 0.04) * termBand * skinMask;

    // ---- Compose final RGB contribution ----
    vec3 diffuseTerm = baseColor * (diff + sssBoost) * lightCol;
    // Spec colour: dielectrics keep the highlight close to the
    // light's colour, but skin's slight subsurface bleed pulls
    // it a touch warmer.
    vec3 specTint = mix(lightCol, lightCol * vec3(1.06, 0.97, 0.90), skinMask);
    vec3 specularTerm = specTint * spec * 0.12;

    return diffuseTerm + specularTerm;
}

// ---------------------------------------------------------------------------
// Legacy cel-shading path. Preserved verbatim for monsters / players
// / props so the project's existing painted look stays intact.
// ---------------------------------------------------------------------------
vec3 shadeCel() {
    vec3 N = normalize(fragNormal);
    vec3 L = normalize(ubo.lightDir.xyz);
    vec3 V = normalize(ubo.cameraPos.xyz - fragWorldPos);

    float ambient = ubo.lightColor.w;

    float floorMask = smoothstep(0.20, 2.37, N.y);
    float fres = pow(1.0 - max(dot(N, V), 0.0), 5.0);
    vec3 rim = ubo.lightColor.rgb * fres * 0.08 * (2.0 - floorMask);

    // ---- Crimson rim from below/behind ----
    // Subtle stylised back/under rim that separates the
    // character silhouette from dark walls and accents the
    // anatomy curves the camera can see — under-shoulders,
    // hip flares, calves, the underside of hair buns. We do
    // this with a directional Fresnel: standard grazing-edge
    // mask AND a "facing away from camera, with a downward
    // bias" term so the rim only fires on the bottom-rear
    // edges of the silhouette, never the lit top. Faded out
    // on near-flat-up surfaces so the floor doesn't pick the
    // tint up.
    //
    // The rim is intentionally anchored on `fres` (which uses
    // V), so it follows the camera. The downward-bias mask
    // ensures the *under* side gets the lift while the upper
    // chest / forehead / hair top stay clean.
    float rimUnder = clamp(0.45 - N.y, 0.0, 1.0);
    rimUnder = rimUnder * rimUnder;
    // Stronger Fresnel curve than the legacy sun rim — pulls
    // the effect tightly into the silhouette edge.
    float rimFres = pow(1.0 - max(dot(N, V), 0.0), 3.5);
    // Suppress on near-horizontal floor surfaces (avoid
    // tinting the floor red — the blood field already does
    // that job).
    float rimFloorKill = 1.0 - smoothstep(0.30, 0.70, N.y);
    // Crimson — deep arterial, intentionally desaturated a
    // touch so it doesn't clash with the post-grade red taming.
    vec3 rimCrimson = vec3(0.55, 0.06, 0.08);
    rim += rimCrimson * rimFres * rimUnder * rimFloorKill * 0.55;

    vec3 texColor = texture(baseColorMap, fragUV).rgb;
    vec3 baseColor = fragColor * texColor;

    // ----- Material classification (procedural) -----
    // The cel path has no authored MR/AO maps, so every
    // character ends up sharing a single Phong response curve.
    // To restore material distinction without per-mesh map
    // authoring we infer a class from the base albedo and use
    // it to retune the BRDF parameters per-fragment. Three
    // soft, overlapping masks (skin / cloth / leather) sum to
    // 1.0 and are then used to lerp the local lighting
    // response. Each mask is fuzzy and additive, so a brown
    // boot's leather mix doesn't suddenly flip to skin at the
    // shin texel boundary — there's a smooth blend.
    float lum = dot(baseColor, vec3(0.2126, 0.7152, 0.0722));
    float mx  = max(max(baseColor.r, baseColor.g), baseColor.b);
    float mn  = min(min(baseColor.r, baseColor.g), baseColor.b);
    float sat = mx - mn;
    // Skin: warm low-medium luma, R > G > B, modest chroma.
    float skinMask = smoothstep(0.10, 0.30, baseColor.r - baseColor.b)
                   * smoothstep(0.18, 0.55, lum)
                   * (1.0 - smoothstep(0.55, 0.80, lum));
    // Cloth: middle luma, low chroma, neutral hue (any colour
    // works as long as it isn't *strongly* one channel).
    float clothMask = (1.0 - smoothstep(0.05, 0.25, sat))
                    * smoothstep(0.18, 0.40, lum)
                    * (1.0 - smoothstep(0.65, 0.85, lum));
    // Leather: dark, low-to-mid chroma, slightly warm.
    float leatherMask = (1.0 - smoothstep(0.05, 0.30, lum))
                      * smoothstep(0.04, 0.20, baseColor.r - baseColor.b);
    // Normalise masks. Anything outside the three classes ends
    // up reading as the legacy cel surface (no extra modulation).
    float maskSum = skinMask + clothMask + leatherMask + 1e-3;
    skinMask    /= maskSum;
    clothMask   /= maskSum;
    leatherMask /= maskSum;

    // ----- Per-class spec coefficients -----
    // Spec strength: skin reads softer (low spec, broad lobe),
    // cloth is matte (Fresnel-only — handled inside evalCharLight),
    // leather is medium spec but broken up by streak hash so it
    // doesn't read as one uniform sheen.
    float lhash = fract(sin(dot(fragWorldPos.xz * 9.7
                              + fragWorldPos.y * 3.1,
                               vec2(127.1, 311.7))) * 43758.5453);
    float skinSpecAmt    = 0.45;
    float clothSpecAmt   = 0.05;
    float leatherSpecAmt = 0.65 * (0.55 + 0.55 * lhash);
    float specMul = mix(1.0,
                        skinSpecAmt * skinMask
                      + clothSpecAmt * clothMask
                      + leatherSpecAmt * leatherMask,
                        clamp(skinMask + clothMask + leatherMask, 0.0, 1.0));
    // Specular sharpness varies too: skin = wide (softer power
    // — `evalCharLight` adds a sharper second lobe of its own),
    // cloth = narrow (Fresnel-only path, ignored), leather = sharp.
    float specPower = mix(96.0,
                          48.0 * skinMask
                        + 192.0 * clothMask
                        + 160.0 * leatherMask,
                          clamp(skinMask + clothMask + leatherMask, 0.0, 1.0));

    // Ambient lift: skin reads better with a slightly warmer
    // ambient (mimics warm bounce off cloth/skin under torch
    // light) so the unlit side doesn't go grey-clay.
    vec3 ambientTint = mix(vec3(1.0), vec3(1.06, 1.00, 0.94), skinMask);

    float shadow = sampleShadow(N, L);

    // ---- Directional key light ----
    vec3 directional = evalCharLight(
        N, L, V, baseColor, ubo.lightColor.rgb,
        skinMask, clothMask, leatherMask,
        specMul, specPower
    ) * shadow;

    vec3 lighting = baseColor * ambient * ambientTint
                  + directional
                  + rim;

    int numLights = int(ubo.pointLightCount.x);
    for (int i = 0; i < numLights && i < 16; i++) {
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
            float pshadow = samplePointShadow(i, fragWorldPos, lightPos, radius, N);
            // Route every point light through the same per-class
            // response used for the directional key. This is
            // what makes torch-lit skin actually look like skin
            // — the highlight on the cheek when the player
            // walks past a sconce, the soft sheen on the
            // forearm catching warm light sideways, the cloth
            // catching its Fresnel rim from below. Without this
            // the point lights would only contribute flat
            // Lambert and skin reads as clay under torchlight.
            vec3 perLight = evalCharLight(
                N, Lp, V, baseColor, lightCol,
                skinMask, clothMask, leatherMask,
                specMul, specPower
            );
            lighting += perLight * intensity * atten * pshadow;
        }

        // Hemispherical floor-bounce. See `shadePbr` for the
        // rationale; here it's a Lambert-only addition since the
        // cel path doesn't compute a microfacet term.
        vec3 bouncePos = vec3(lightPos.x, ubo.timeData.y, lightPos.z);
        vec3 bounceVec = bouncePos - fragWorldPos;
        float bounceDist = length(bounceVec);
        float bounceRadius = radius * 0.85;
        if (bounceDist < bounceRadius) {
            vec3 Lb = bounceVec / max(bounceDist, 1e-3);
            float NdotB = max(dot(N, Lb), 0.0);
            float bAtten = 1.0 - (bounceDist / bounceRadius);
            bAtten = bAtten * bAtten;
            vec3 bounceCol = lightCol * vec3(1.10, 0.85, 0.55);
            lighting += baseColor * bounceCol * intensity
                     * NdotB * bAtten * 0.16;
        }
    }

    return lighting;
}

// ---------------------------------------------------------------------------
// Dimensional rift portal — emissive-only "wound in reality" shader.
//
// Geometry side (see `Mesh::portal_with_palette`) bakes polar
// coordinates into the UV channel: `fragUV.x` = radial 0..1
// (0 at the rift's core, 1 at the wobbly silhouette),
// `fragUV.y` = angle / TAU. This branch ignores `fragColor`
// entirely and synthesises the entire look from those polar
// coords + `ubo.timeData.x` (elapsed time).
//
// The look is intentionally NOT "fantasy gold portal": the
// previous version read as an MMO loot teleporter. This one
// targets:
//   * unstable, asymmetric silhouette (animated edge wobble +
//     bright tendrils flickering past the rim),
//   * layered counter-rotating depth (three swirl noise fields
//     at different speeds → fakes parallax inward),
//   * pitch-black core fading to crimson glow (eye reads as
//     "fall in forever" rather than "lit plate"),
//   * dark-red veins crawling under the surface (tension /
//     wound feel),
//   * chromatic split at the boundary (R shifts outward, B
//     shifts inward → cheap "physics breaking" cue without an
//     actual screen-space refraction pass),
//   * slow breathing pulse so the whole thing feels alive.
//
// Output is HDR emissive (channels routinely hit 3–4); the
// bloom pass amplifies the bright bands and the chromatic
// edge into the surrounding scene.
// ---------------------------------------------------------------------------

float riftHash(vec2 p) {
    return fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453);
}

float riftValueNoise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    float a = riftHash(i);
    float b = riftHash(i + vec2(1.0, 0.0));
    float c = riftHash(i + vec2(0.0, 1.0));
    float d = riftHash(i + vec2(1.0, 1.0));
    vec2 u = f * f * (3.0 - 2.0 * f);
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

float riftFbm(vec2 p) {
    float v = 0.0;
    float a = 0.5;
    for (int i = 0; i < 4; i++) {
        v += a * riftValueNoise(p);
        p *= 2.13;
        a *= 0.5;
    }
    return v;
}

vec3 shadeRift() {
    float r = clamp(fragUV.x, 0.0, 1.0);
    float theta = fragUV.y * 6.28318530718;
    float t = ubo.timeData.x;

    // ---- Intermittent destabilisation pulse ----
    // Every ~7 s the rift "spasms": a low-freq pulse that runs
    // 0..1..0 over ~1.5 s, gating extra wobble amplitude and
    // tendril strength. The rest of the time it sits at near-
    // zero and the rift looks calm. We synthesise the schedule
    // from a fract+smoothstep instead of an explicit timer so
    // there's no CPU-side state to manage.
    float spasmPhase = fract(t * 0.14);                  // 0..1 every 7.14 s
    float spasm = smoothstep(0.00, 0.08, spasmPhase)
                * (1.0 - smoothstep(0.18, 0.28, spasmPhase));
    // Second, faster, weaker tremor so the rim never feels
    // perfectly still even between major spasms.
    float tremorPhase = fract(t * 0.37 + 0.21);
    float tremor = smoothstep(0.00, 0.05, tremorPhase)
                 * (1.0 - smoothstep(0.10, 0.16, tremorPhase));

    // ---- Animated silhouette wobble ----
    // Stacked low-freq sines at 3/5/11 lobes, drifting at
    // different rates so the rim never repeats. Combined with
    // the *static* mesh-side wobble (in `portal_with_palette`)
    // the contour reads as continuously deforming, never a
    // perfect circle. During a spasm the high-freq amplitude
    // doubles — chunks of the silhouette appear to peel and
    // re-form.
    float spasmAmp = 1.0 + spasm * 1.4 + tremor * 0.35;
    float edgeWobble = 0.030 * sin(theta * 3.0 + t * 0.40)
                     + 0.045 * sin(theta * 5.0 - t * 0.70) * spasmAmp
                     + 0.020 * sin(theta * 11.0 + t * 1.30) * spasmAmp;
    // Slow breathing — silhouette gently inflates / deflates
    // ±2.5% on a 6 s cycle. Subtle on its own but the eye reads
    // the rift as alive instead of static.
    float breath = 0.025 * sin(t * 1.05);
    float silhouette =
        1.0 - smoothstep(0.86 + edgeWobble + breath,
                         1.00 + edgeWobble + breath, r);

    // ---- Multi-layer counter-rotating swirl fields ----
    // Five layers at different angular velocities, radii, and
    // frequencies fake true volumetric parallax: the eye picks
    // the patterns apart as they slide over each other and
    // perceives depth, even though we're sampling on a 2D
    // plane.
    //
    //   Layer A — outer slow swirl  (CCW, low freq)
    //   Layer B — mid counter-spin  (CW,  mid freq)
    //   Layer C — inner fast vortex (CCW, high freq, tight)
    //   Layer D — drifting "stars"  (radially inward, sparse)
    //   Layer E — micro-grain       (no rotation, fine detail)
    vec2 swA = vec2(cos(theta + t * 0.10),
                    sin(theta + t * 0.10)) * (r + 0.10);
    vec2 swB = vec2(cos(theta - t * 0.35),
                    sin(theta - t * 0.35)) * (r * 1.30 + 0.05);
    vec2 swC = vec2(cos(theta * 2.0 + t * 0.85),
                    sin(theta * 2.0 + t * 0.85)) * (r * 0.55);
    float nA = riftFbm(swA * 3.5);
    float nB = riftFbm(swB * 7.0 + 17.3);
    float nC = riftFbm(swC * 11.0 - 8.4);
    float nE = riftFbm(vec2(theta * 6.0, r * 12.0 - t * 0.05));

    // Radially-inward starfield: hash on (theta * 17, t * 0.08)
    // gates a bright pinprick that drifts toward the centre as
    // r decreases (faked by sampling at a time-shifted radial
    // offset). Reads as motes falling forever inward.
    float starR = fract(r * 1.6 - t * 0.08);
    float starHash = riftHash(vec2(floor(theta * 32.0),
                                   floor(starR * 24.0)));
    float starMask = pow(max(0.0, starHash - 0.985), 2.0) * 1500.0;
    starMask *= smoothstep(0.05, 0.30, r) * (1.0 - smoothstep(0.55, 0.85, r));

    // ---- Radial bands ----
    // Core: pitch black, very slight charcoal glow (so it's
    // not pure 0,0,0 — bloom needs *something* to read).
    // Mid: crimson swirl driven by n1, with a dark-blue
    // impossible-color hint underneath driven by inverted n2.
    // Rim: bright ember vein band driven by n3.
    float coreFade = smoothstep(0.0, 0.35, r);
    float midBand =
        smoothstep(0.20, 0.55, r) * (1.0 - smoothstep(0.55, 0.85, r));
    float rimBand = smoothstep(0.65, 1.0, r);

    vec3 col = vec3(0.0);
    col += vec3(0.04, 0.01, 0.025) * (1.0 - coreFade);          // dim core
    col += vec3(2.20, 0.18, 0.06) * midBand * pow(nA, 1.6);     // outer crimson swirl
    col += vec3(1.40, 0.10, 0.05) * midBand * pow(nB, 2.0) * 0.55; // mid counter-swirl
    col += vec3(0.10, 0.04, 0.20) * midBand
                                    * pow(1.0 - nB, 3.0) * 0.55; // blue-shift hint
    col += vec3(3.00, 1.20, 0.50) * pow(nC, 3.0)
                                    * smoothstep(0.05, 0.40, r)
                                    * (1.0 - smoothstep(0.55, 0.80, r))
                                    * 0.45;                      // hot inner vortex
    col += vec3(2.60, 1.80, 1.40) * starMask;                    // drifting motes
    col += vec3(0.06, 0.02, 0.04) * nE * (1.0 - rimBand);        // grain
    float emberVeins = pow(max(0.0, nC - 0.55), 2.0) * 4.0;
    col += vec3(2.80, 0.95, 0.30) * emberVeins * rimBand;        // ember band

    // ---- Dark-red veins crawling under the surface ----
    // Sharp thresholded fbm in (theta, r)-space, slowly drifting.
    // Reads as cracks / tension lines rather than a smooth
    // gradient.
    float vein = riftFbm(vec2(theta * 1.5 + t * 0.10,
                              r * 4.0 - t * 0.20));
    float veinMask = pow(max(0.0, vein - 0.60), 2.0) * 5.0;
    col += vec3(1.40, 0.06, 0.10) * veinMask;

    // ---- Edge tendrils ----
    // Sharp angular spikes that flicker outward past the
    // silhouette. `tendrilMask` is concentrated in the outer
    // 25% of the disc; combined with the mesh's wobbly outer
    // ring the silhouette gains "tearing" fingers. Spasm
    // pulses double their reach for a moment — chunks of edge
    // appear to peel and stretch.
    float tendril = pow(max(0.0,
                           sin(theta * 8.0 + nB * 6.0 + t * 0.30)),
                       12.0);
    float tendrilMask = smoothstep(0.74, 0.99, r) * tendril;
    col += vec3(2.60, 0.55, 0.18) * tendrilMask
                                    * (1.0 + spasm * 1.5);

    // ---- Chromatic split at boundary ----
    // R channel pushes outward, B channel pulls inward as r
    // approaches 1. Reads as light bending at the rift's edge
    // — the visual cue the eye associates with refraction
    // without needing a screen-space pass.
    float ca = smoothstep(0.65, 1.0, r);
    col.r *= 1.0 + ca * 0.65;
    col.b *= 1.0 - ca * 0.45;

    // ---- Slow breathing pulse ----
    float pulse = 0.88 + 0.12 * sin(t * 0.85) + spasm * 0.15;
    col *= pulse;

    // ---- Apply silhouette mask ----
    // The mask is intentionally NOT applied to the tendrils
    // (they extend past the rim by being added before the
    // multiply *only on the disc*), so we re-add a small
    // outside-rim component to keep them visible.
    vec3 outsideRim = vec3(2.60, 0.55, 0.18) * tendrilMask
                                              * (1.0 - silhouette);
    col *= silhouette;
    col += outsideRim * 0.6;

    // ---- Pseudo-refraction lensing halo ----
    // True screen-space refraction would require sampling the
    // scene HDR target inside this pass, which Vulkan disallows
    // (we'd be reading the same attachment we're writing to).
    // Instead we synthesise the *visual cue* the eye uses to
    // recognise space-bending: a thin chromatic-split halo just
    // past the silhouette where R bleeds outward and B inward,
    // modulated by a low-freq angular wobble so the band looks
    // like reality is rippling against the rim. Subtle by
    // design — the eye reads "lensing" without seeing distinct
    // colour fringes. Energised during destabilisation pulses.
    float halo = (1.0 - silhouette)
                * smoothstep(1.05, 1.00, r)
                * smoothstep(0.92, 1.00, r);
    float haloWobble = 0.5 + 0.5 * sin(theta * 7.0
                                        + nA * 4.0
                                        + t * 0.65);
    float haloAmp = halo * (0.30 + 0.55 * haloWobble)
                         * (1.0 + spasm * 1.2 + tremor * 0.6);
    col.r += haloAmp * 0.45;
    col.b += haloAmp * 0.18;
    // Tiny dark-vacuum band immediately outside the rim sells
    // the lensing illusion further: real gravitational lenses
    // create a thin dark Einstein ring just outside their
    // bright halo. We darken the local output by a small
    // factor right at r ≈ 1.005..1.04.
    float vacuum = (1.0 - silhouette)
                 * smoothstep(1.04, 1.00, r)
                 * smoothstep(0.99, 1.02, r);
    col *= 1.0 - vacuum * 0.55;

    return col;
}

void main() {
    // Bit-test the flags float to pick a shading path. Using
    // floatBitsToUint so we can pack other booleans into the
    // same float later (bit 1, bit 2, ...) without touching
    // the Rust side.
    uint flags = floatBitsToUint(push.materialParams.z);
    bool usePbr  = (flags & 1u) != 0u;
    bool useRift = (flags & 2u) != 0u;

    vec3 lighting;
    if (useRift)      lighting = shadeRift();
    else if (usePbr)  lighting = shadePbr();
    else              lighting = shadeCel();

    // Distance fog (player-anchored). The rift is a hole
    // through reality — fog still applies (so you can't see
    // it from across an entire dungeon) but is dampened so
    // the rift retains presence in the haze rather than
    // dissolving into the fog colour.
    float dist = length(ubo.fogOrigin.xyz - fragWorldPos);
    float fogFactor = clamp((dist - ubo.fogParams.x) / (ubo.fogParams.y - ubo.fogParams.x), 0.0, 1.0);
    fogFactor = fogFactor * fogFactor;
    if (useRift) fogFactor *= 0.35;
    vec3 finalColor = mix(lighting, ubo.fogColor.rgb, fogFactor);

    outColor = vec4(finalColor * push.tint.rgb, push.tint.a);
}
