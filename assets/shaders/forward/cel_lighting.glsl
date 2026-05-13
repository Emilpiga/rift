vec3 evalCharLight(
    vec3 N, vec3 L, vec3 V,
    vec3 baseColor, vec3 lightCol,
    float skinMask, float clothMask, float leatherMask,
    float specMul, float specPower
) {
    float NdotL = dot(N, L);
    float NdotLp = max(NdotL, 0.0);
    float NdotV = max(dot(N, V), 0.0);
    // `normalize(L + V)` blows up to NaN when L and V are
    // exactly anti-parallel (light coming from the camera
    // direction). Even in dynamic lighting this is rare, but
    // it's exactly the kind of edge case that produces
    // momentary 2x2 black squares as the camera moves through
    // the singularity. Guard with a tiny lower bound on the
    // half-vector length and fall back to N when degenerate.
    vec3  HRaw = L + V;
    float HLen = length(HRaw);
    vec3  H = HLen > 1e-4 ? HRaw / HLen : N;
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
    // Normal can collapse to a zero vector for two reasons:
    //   * the asset itself ships a zero normal at a vertex
    //     (rare but happens on hand-authored meshes), or
    //   * the GPU skin pass produced a zero post-skinned
    //     normal because the bone palette and the rest
    //     normal happen to align with the matrix kernel.
    // `normalize(0)` returns NaN, which then poisons every
    // downstream dot/pow and the fragment outputs black.
    // Fall back to a stable up vector (most characters are
    // upright; this is biased toward looking "lit from above"
    // rather than collapsing to a black square).
    vec3 nRaw = fragNormal;
    float nLen = length(nRaw);
    vec3 N = nLen > 1e-4 ? nRaw / nLen : vec3(0.0, 1.0, 0.0);
    vec3 L = normalize(ubo.lightDir.xyz);
    vec3 vRaw = ubo.cameraPos.xyz - fragWorldPos;
    float vLen = length(vRaw);
    vec3 V = vLen > 1e-4 ? vRaw / vLen : vec3(0.0, 0.0, 1.0);

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
            vec3 Lp = normalize(toLight);
            // Backface skip \u2014 see `shadePbr` rationale. The
            // cel path's `evalCharLight` is significantly
            // heavier than the PBR Cook-Torrance loop (skin
            // SSS approximation, cloth Fresnel rim, leather
            // sheen branches), so skipping it on shaded
            // sides of the model is an even bigger win here.
            if (max(dot(N, Lp), 0.0) > 0.0) {
            float atten = 1.1 - (dist / radius);
            atten = atten * atten;
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
        }

        // Hemispherical floor-bounce. See `shadePbr` for the
        // rationale; gated to shadow-casting lights only — the
        // perceptual point of the bounce is static torches lit
        // dungeon stone, not transient AoE/VFX flashes.
        if (i < int(ubo.pointShadowMeta.x)) {
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
        }  // end shadow-light bounce gate (cel path)
    }

    // Final NaN/Inf sweep. Even with the entry guards above,
    // a single anomalous input (e.g. a NaN sneaking out of
    // `samplePointShadow` on a degenerate cube face) would
    // poison the accumulated `lighting` and the fragment
    // would output black across the entire 2x2 derivative
    // quad — the "random black squares on the skin" symptom.
    // Replace any non-finite component with the unlit base
    // colour so the worst case degrades to flat-shaded
    // instead of opaque black.
    if (any(isnan(lighting)) || any(isinf(lighting))) {
        lighting = baseColor * ambient;
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
