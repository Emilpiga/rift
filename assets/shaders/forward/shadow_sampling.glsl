// ---------------------------------------------------------------------------
// Shadow sampling shared by PBR and cel paths.
// ---------------------------------------------------------------------------
const vec2 POISSON_DISK[12] = vec2[](
    vec2(-0.326,-0.406), vec2(-0.840,-0.074), vec2(-0.696, 0.457),
    vec2(-0.203, 0.621), vec2( 0.962,-0.195), vec2( 0.473,-0.480),
    vec2( 0.519, 0.767), vec2( 0.185,-0.893), vec2( 0.507, 0.064),
    vec2( 0.896, 0.412), vec2(-0.322,-0.933), vec2(-0.792,-0.598)
);

float sampleShadowAt(vec3 worldPos, vec3 N, vec3 L) {
    vec4 lightClip = ubo.lightVP * vec4(worldPos, 1.0);
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

    // ----- Smooth Gaussian penumbra -----
    // Earlier revisions used a 12-tap rotated-Poisson kernel
    // for the near-range disc. With per-pixel rotation jitter
    // and only 12 samples the *average* across neighbouring
    // fragments was correct, but each individual fragment
    // sampled a different sub-disc of the kernel — so on
    // curved receivers (character torsos, limbs, helmets)
    // the cast shadow read as stippled noise instead of a
    // smooth gradient. The eye picks that up immediately
    // as "pixelated shadow", even with a wide kernel.
    //
    // Solution: replace the rotated-Poisson disc with a
    // fixed 5×5 separable Gaussian. Every fragment samples
    // exactly the same 25 positions in the same orientation,
    // so the result is a *deterministic* smooth blur of the
    // hardware-PCF result instead of a stochastic estimate
    // of one. Cost is higher (25 taps vs 12) but each tap
    // is a hardware-PCF compare, and we drop the second
    // 12-tap "soft" loop entirely — a single kernel whose
    // radius scales with distance/grazing covers everything.
    float nearTexels = 4.0 * (1.0 + grazing * 1.2);
    // Widen further with camera distance so distant shadows
    // soften into the fog band rather than aliasing.
    float kernelTexels = mix(nearTexels, 8.0, distFactor);
    vec2  kernelStep   = texelSize * (kernelTexels / 2.0); // half-radius per ring

    // 5×5 Gaussian weights (sigma ≈ 1.0). Hand-rolled so the
    // compiler unrolls cleanly on every driver.
    const float W0 = 0.2270270270;          // centre
    const float W1 = 0.1945945946;          // axial ±1
    const float W2 = 0.1216216216;          // axial ±2
    // Off-axis weights are products. We don't store them;
    // the compiler folds the constants.
    float s = 0.0;
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2(-2.0, -2.0), shadowUV.z)) * (W2 * W2);
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2(-1.0, -2.0), shadowUV.z)) * (W1 * W2);
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2( 0.0, -2.0), shadowUV.z)) * (W0 * W2);
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2( 1.0, -2.0), shadowUV.z)) * (W1 * W2);
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2( 2.0, -2.0), shadowUV.z)) * (W2 * W2);

    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2(-2.0, -1.0), shadowUV.z)) * (W2 * W1);
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2(-1.0, -1.0), shadowUV.z)) * (W1 * W1);
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2( 0.0, -1.0), shadowUV.z)) * (W0 * W1);
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2( 1.0, -1.0), shadowUV.z)) * (W1 * W1);
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2( 2.0, -1.0), shadowUV.z)) * (W2 * W1);

    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2(-2.0,  0.0), shadowUV.z)) * (W2 * W0);
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2(-1.0,  0.0), shadowUV.z)) * (W1 * W0);
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2( 0.0,  0.0), shadowUV.z)) * (W0 * W0);
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2( 1.0,  0.0), shadowUV.z)) * (W1 * W0);
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2( 2.0,  0.0), shadowUV.z)) * (W2 * W0);

    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2(-2.0,  1.0), shadowUV.z)) * (W2 * W1);
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2(-1.0,  1.0), shadowUV.z)) * (W1 * W1);
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2( 0.0,  1.0), shadowUV.z)) * (W0 * W1);
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2( 1.0,  1.0), shadowUV.z)) * (W1 * W1);
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2( 2.0,  1.0), shadowUV.z)) * (W2 * W1);

    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2(-2.0,  2.0), shadowUV.z)) * (W2 * W2);
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2(-1.0,  2.0), shadowUV.z)) * (W1 * W2);
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2( 0.0,  2.0), shadowUV.z)) * (W0 * W2);
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2( 1.0,  2.0), shadowUV.z)) * (W1 * W2);
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2( 2.0,  2.0), shadowUV.z)) * (W2 * W2);
    // Weights sum to 1.0 by construction (the W{0,1,2}
    // values above are the canonical 5-tap Gaussian
    // coefficients), so no normalisation is required.

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

// Convenience wrapper: sample at the fragment's geometric
// world position. Used by the cel-shading path which has no
// height map to perturb against.
float sampleShadow(vec3 N, vec3 L) {
    return sampleShadowAt(fragWorldPos, N, L);
}

float pointShadowCompare(int lightIdx, vec3 sampleDir, float receiver, float bias, float softness) {
    float stored = texture(pointShadowAtlas, vec4(sampleDir, float(lightIdx))).r;
    return smoothstep(receiver - bias - softness, receiver - bias + softness, stored);
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
    // A 12-tap disk plus a tiny smooth compare gives enough
    // levels that torch shadows read as a continuous penumbra
    // instead of stacked rings. The rotation is world-stable
    // below, so any residual grain sticks to the receiver
    // surface instead of crawling over the screen when the
    // camera pans.
    vec3 up = abs(dir.y) > 0.95 ? vec3(0.0, 0.0, 1.0) : vec3(0.0, 1.0, 0.0);
    vec3 tuRaw = normalize(cross(up, dir));
    vec3 tvRaw = cross(dir, tuRaw);
    // World-stable rotation angle. Screen-space rotation made
    // the PCF grain crawl as the camera panned; anchoring it
    // to receiver position keeps the dither attached to the
    // lit surface instead.
    float rotHash = fract(sin(dot(fragWorld.xz + vec2(fragWorld.y, float(lightIdx) * 7.13),
                                  vec2(12.9898, 78.233))) * 43758.5453);
    float rotAng = rotHash * 6.2831853;
    float rcs = cos(rotAng);
    float rsn = sin(rotAng);
    vec3 tu = tuRaw * rcs + tvRaw * rsn;
    vec3 tv = -tuRaw * rsn + tvRaw * rcs;

    float distFade = smoothstep(0.24, 0.90, normFrag);
    float grazing = 1.0 - clamp(NdotL, 0.0, 1.0);
    // Keep nearby contacts tight, then broaden the angular
    // kernel as the receiver approaches the light's range.
    float k = mix(0.0025, 0.0065, distFade) * (1.0 + grazing * 0.20);
    // Manual compare softness turns the binary R32 distance
    // test into a narrow transition. It removes the 9-level
    // banding without making fully-shadowed interiors glow.
    float softness = mix(0.00025, 0.00140, distFade) * (1.0 + grazing * 0.25);

    float occ = pointShadowCompare(lightIdx, dir, normFrag, bias, softness) * 2.0;
    for (int i = 0; i < 12; i++) {
        vec2 p = POISSON_DISK[i];
        occ += pointShadowCompare(lightIdx, dir + (tu * p.x + tv * p.y) * k,
                                  normFrag, bias, softness);
    }
    occ *= (1.0 / 14.0);
    return occ;
}

// ---------------------------------------------------------------------------
// PBR shading path. Used when material flags bit 0 is set.
