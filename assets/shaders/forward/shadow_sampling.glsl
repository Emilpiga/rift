// ---------------------------------------------------------------------------
// Shadow sampling shared by PBR and cel paths.
// ---------------------------------------------------------------------------
float sampleShadowAt(vec3 worldPos, vec3 N, vec3 L) {
    if (ubo.pointShadowMeta.y < 0.5) {
        return 1.0;
    }

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

    // Fixed 3x3 Gaussian. The old 5x5 kernel was smooth, but it
    // spent 25 hardware-PCF compares on every lit fragment even
    // when texture-height shadows were disabled. This keeps the
    // same stable, non-crawling shape at roughly one third the
    // sampling cost.
    float nearTexels = 2.1 * (1.0 + grazing * 0.75);
    float kernelTexels = mix(nearTexels, 4.6, distFactor);
    vec2 kernelStep = texelSize * kernelTexels;

    float s = 0.0;
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2(-1.0, -1.0), shadowUV.z)) * 0.0625;
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2( 0.0, -1.0), shadowUV.z)) * 0.1250;
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2( 1.0, -1.0), shadowUV.z)) * 0.0625;
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2(-1.0,  0.0), shadowUV.z)) * 0.1250;
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2( 0.0,  0.0), shadowUV.z)) * 0.2500;
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2( 1.0,  0.0), shadowUV.z)) * 0.1250;
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2(-1.0,  1.0), shadowUV.z)) * 0.0625;
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2( 0.0,  1.0), shadowUV.z)) * 0.1250;
    s += texture(shadowMap, vec3(shadowUV.xy + kernelStep * vec2( 1.0,  1.0), shadowUV.z)) * 0.0625;

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
float samplePointShadowBiased(
    int lightIdx,
    vec3 fragWorld,
    vec3 lightPos,
    float radius,
    vec3 N,
    float receiverBias
) {
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
    float normFrag = clamp(fragDist / radius + receiverBias, 0.0, 1.0);
    vec3 dir = toFrag / max(fragDist, 1e-4);

    // Slope-scaled bias: cosine-grazing surfaces need a larger
    // bias to avoid acne. Slightly tighter than before now that cube
    // faces are 1024² (smaller normalized texel footprint).
    float NdotL = max(dot(N, -dir), 0.0);
    float bias = max(0.0029 * (1.0 - NdotL), 0.00065);

    // Cheap path for low-value samples. Near a torch's falloff edge
    // or at a grazing receiver, the light contribution is already
    // small and a broad rotated PCF kernel is not worth five cube
    // lookups plus sin/cos/hash setup. A single softened compare keeps
    // the occlusion stable while preserving the restored light count.
    float distFade = smoothstep(0.24, 0.90, normFrag);
    float grazing = 1.0 - clamp(NdotL, 0.0, 1.0);
    if (normFrag > 0.78 || NdotL < 0.18) {
        float fastSoftness = mix(0.00022, 0.00105, distFade) * (1.0 + grazing * 0.18);
        return pointShadowCompare(lightIdx, dir, normFrag, bias, fastSoftness);
    }

    // Small cross PCF over an orthonormal basis built from `dir`.
    // The earlier 12-tap disk looked a little softer, but the cost
    // was paid once per affecting shadowed point light, per fragment.
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

    // Tighter angular kernel + compare softness — sharper silhouettes on
    // 1024² cube faces (still 5 taps + world-stable rotation).
    float k = mix(0.00185, 0.0048, distFade) * (1.0 + grazing * 0.14);
    float softness = mix(0.00014, 0.00088, distFade) * (1.0 + grazing * 0.16);

    float occ = pointShadowCompare(lightIdx, dir, normFrag, bias, softness) * 0.44;
    occ += pointShadowCompare(lightIdx, dir + tu * k, normFrag, bias, softness) * 0.14;
    occ += pointShadowCompare(lightIdx, dir - tu * k, normFrag, bias, softness) * 0.14;
    occ += pointShadowCompare(lightIdx, dir + tv * k, normFrag, bias, softness) * 0.14;
    occ += pointShadowCompare(lightIdx, dir - tv * k, normFrag, bias, softness) * 0.14;
    return occ;
}

float samplePointShadowAt(int lightIdx, vec3 fragWorld, vec3 lightPos, float radius, vec3 N) {
    return samplePointShadowBiased(lightIdx, fragWorld, lightPos, radius, N, 0.0);
}

float samplePointShadow(int lightIdx, vec3 fragWorld, vec3 lightPos, float radius, vec3 N) {
    return samplePointShadowAt(lightIdx, fragWorld, lightPos, radius, N);
}

float sampleHeightPointShadow(
    int lightIdx,
    vec3 fragWorld,
    vec3 lightPos,
    float radius,
    vec3 N,
    vec3 Ngeo,
    mat3 TBN,
    vec2 uv,
    vec3 lightTS,
    float scale
) {
    if (lightIdx >= int(ubo.pointShadowMeta.x)) {
        return 1.0;
    }
    if (!heightShadowsEnabled(scale) || lightTS.z <= 0.08) {
        return samplePointShadowAt(lightIdx, fragWorld, lightPos, radius, N);
    }

    vec3 toLight = normalize(lightPos - fragWorld);
    float heightTowardLight = max(dot(Ngeo, toLight), 0.0);
    float baseHeight = texture(heightMap, uv).r;
    float shadowRelief = scale * 24.0;
    float relief = smoothstep(0.004, 0.020, scale);
    float invLightZ = 1.0 / max(lightTS.z, 0.08);
    vec2 lightRayUv = lightTS.xy * invLightZ * shadowRelief * 1.35;
    vec2 receiverProjection = -lightTS.xy * invLightZ;
    float grazing = 1.0 - smoothstep(0.14, 0.82, lightTS.z);

    float centeredBaseHeight = baseHeight - 0.5;
    vec3 baseTangentOffset = TBN * vec3(
        receiverProjection * centeredBaseHeight * shadowRelief * 0.85 * grazing,
        0.0
    );
    vec3 baseReceiver = fragWorld + Ngeo * (centeredBaseHeight * shadowRelief)
        + baseTangentOffset;
    float baseBias = -centeredBaseHeight * shadowRelief * heightTowardLight / max(radius, 1e-3);
    float shadow = samplePointShadowBiased(lightIdx, baseReceiver, lightPos, radius, N, baseBias);

    float blockerOcc = 0.0;

    for (int i = 1; i <= 6; i++) {
        float t = float(i) / 6.0;
        vec2 sampleUv = uv + lightRayUv * t;
        float h = texture(heightMap, sampleUv).r;
        float centeredHeight = h - 0.5;
        vec3 tangentOffset = TBN * vec3(
            receiverProjection * centeredHeight * shadowRelief * 0.95 * grazing,
            0.0
        );
        vec3 receiver = fragWorld + Ngeo * (centeredHeight * shadowRelief) + tangentOffset;
        float sampleReceiverBias = -centeredHeight * shadowRelief * heightTowardLight / max(radius, 1e-3);
        shadow = min(
            shadow,
            samplePointShadowBiased(lightIdx, receiver, lightPos, radius, N, sampleReceiverBias)
        );

        float blocker = h - baseHeight - t * 0.018;
        blockerOcc = max(blockerOcc, smoothstep(0.006, 0.045, blocker));
    }

    return shadow * mix(1.0, 0.58, blockerOcc * relief);
}

// ---------------------------------------------------------------------------
// PBR shading path. Used when material flags bit 0 is set.
