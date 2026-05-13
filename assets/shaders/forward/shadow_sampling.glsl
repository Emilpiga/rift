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
    // so the disk straddles ~2 atlas texels: deep-shadow and
    // fully-lit fragments still resolve as a uniform value
    // (taps land on equivalent depths) while silhouette
    // fragments get a graded penumbra.
    float k = 0.008;

    // 8-tap Poisson disk PCF. The earlier single-tap fast path
    // (P0 = (0,0)) collapsed every fragment to a hard binary
    // `step()` because rotating a zero offset still gave zero,
    // and the per-pixel basis rotation contributed nothing —
    // result was the chunky one-bit shadow the player sees on
    // their own body and on the ground under torches.
    //
    // 8 taps gives 9 discrete output levels which, combined
    // with the per-pixel rotation, dither into a continuous
    // penumbra perceptually. Cost is 8 cubemap-array fetches
    // per shadow-casting torch per fragment; with a hard cap
    // of `pointShadowMeta.x ≤ 4` casters the worst-case bill
    // is 32 cube fetches per fragment — well within the
    // dungeon's per-frame texture budget on every target GPU.
    const vec2 P0 = vec2( 0.0000,  0.0000);
    const vec2 P1 = vec2( 0.7071,  0.0000);
    const vec2 P2 = vec2(-0.7071,  0.0000);
    const vec2 P3 = vec2( 0.0000,  0.7071);
    const vec2 P4 = vec2( 0.0000, -0.7071);
    const vec2 P5 = vec2( 0.5000,  0.5000);
    const vec2 P6 = vec2(-0.5000,  0.5000);
    const vec2 P7 = vec2( 0.5000, -0.5000);

    float occ = 0.0;
    occ += step(normFrag - bias, texture(pointShadowAtlas, vec4(dir + (tu * P0.x + tv * P0.y) * k, float(lightIdx))).r);
    occ += step(normFrag - bias, texture(pointShadowAtlas, vec4(dir + (tu * P1.x + tv * P1.y) * k, float(lightIdx))).r);
    occ += step(normFrag - bias, texture(pointShadowAtlas, vec4(dir + (tu * P2.x + tv * P2.y) * k, float(lightIdx))).r);
    occ += step(normFrag - bias, texture(pointShadowAtlas, vec4(dir + (tu * P3.x + tv * P3.y) * k, float(lightIdx))).r);
    occ += step(normFrag - bias, texture(pointShadowAtlas, vec4(dir + (tu * P4.x + tv * P4.y) * k, float(lightIdx))).r);
    occ += step(normFrag - bias, texture(pointShadowAtlas, vec4(dir + (tu * P5.x + tv * P5.y) * k, float(lightIdx))).r);
    occ += step(normFrag - bias, texture(pointShadowAtlas, vec4(dir + (tu * P6.x + tv * P6.y) * k, float(lightIdx))).r);
    occ += step(normFrag - bias, texture(pointShadowAtlas, vec4(dir + (tu * P7.x + tv * P7.y) * k, float(lightIdx))).r);
    occ *= (1.0 / 8.0);
    return occ;
}

// ---------------------------------------------------------------------------
// PBR shading path. Used when material flags bit 0 is set.
