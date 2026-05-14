vec3 shadePbr() {
    vec3 Ngeo = normalize(fragNormal);
    vec3 V = normalize(ubo.cameraPos.xyz - fragWorldPos);

    mat3 TBN = cotangentFrame(Ngeo, fragWorldPos, fragUV);
    mat3 invTBN = transpose(TBN);
    vec3 viewTS = invTBN * V;

    vec2 uv = parallaxOffset(fragUV, viewTS, push.materialParams.y);
    bool heightShadows = heightShadowsEnabled(push.materialParams.y);

    vec3 albedo = texture(baseColorMap, uv).rgb * fragColor;
    vec3 nTex = texture(normalMap, uv).xyz * 2.0 - 1.0;
    vec3 N = normalize(TBN * nTex);
    // Final NaN guard. If the TBN frame still degenerated
    // for any reason (e.g. nTex itself was a degenerate
    // sample), fall back to the geometric normal so the
    // fragment shades from a sane direction instead of
    // going black across the derivative quad.
    if (any(isnan(N))) N = Ngeo;

    vec2 mr = texture(mrMap, uv).rg;
    float metallic  = mr.r;
    float roughness = clamp(mr.g, 0.045, 1.0);
    float ao        = texture(aoMap, uv).r;

    applyHeightMaterialDetail(uv, push.materialParams.y, albedo, roughness, ao);

    // Blood field composite (per-floor wet/dry pools). Mutates the
    // PBR inputs in place before the BRDF math so the lighting picks
    // up the puddle's specular highlight naturally — wet blood
    // glistens off torches, dry blood stays matte.
    applyBloodField(albedo, roughness, metallic, N, Ngeo);

    vec3 F0 = mix(vec3(0.04), albedo, metallic);

    // ---- Directional key light ----
    vec3 L = normalize(ubo.lightDir.xyz);
    vec3 H = normalize(L + V);
    vec3 shadowWorldPos = heightShadows
        ? heightShadowWorldPos(fragWorldPos, Ngeo, uv, push.materialParams.y)
        : fragWorldPos;
    vec3 lightTS = heightShadows ? invTBN * L : vec3(0.0, 0.0, 1.0);
    float shadow = sampleShadowAt(shadowWorldPos, N, L)
                 * (heightShadows ? heightDirectionalSelfShadow(uv, lightTS, push.materialParams.y) : 1.0);

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
        vec3 Lp = normalize(toLight);
        float NdotLp = max(dot(N, Lp), 0.0);
        // Backface skip: surfaces facing away from the light
        // contribute zero diffuse + zero specular; the only
        // remaining work would be the bounce term below,
        // which has its own NdotB cosine gate. Skipping the
        // ~20 ALU + 1 cube-sample of the BRDF for those
        // pixels is the single biggest win in dungeons where
        // the camera-facing sides of walls + floor are lit
        // by at most 2-3 of the 8 active shadow-casters.
        if (NdotLp <= 0.0) continue;
        float atten = 1.1 - (dist / radius);
        atten = atten * atten;

        vec3 Hp = normalize(Lp + V);

        float NDFp = distributionGGX(N, Hp, roughness);
        float Gp   = geometrySmith(N, V, Lp, roughness);
        vec3  Fp   = fresnelSchlick(max(dot(Hp, V), 0.0), F0);

        vec3 specP = (NDFp * Gp * Fp) /
                     (4.0 * max(dot(N, V), 0.0) * NdotLp + 1e-4);
        vec3 kSp = Fp;
        vec3 kDp = (1.0 - kSp) * (1.0 - metallic);
        // Texture-height shadows add a compact self-shadow
        // march per affecting point light. This is deliberately
        // hidden behind the experimental setting: in torch-lit
        // rifts it is the part of the feature the player can
        // actually see, while slower GPUs can skip the extra
        // height taps entirely.
        float pshadow = samplePointShadowAt(i, fragWorldPos, lightPos, radius, N);
        if (heightShadows) {
            vec3 lightPointTS = invTBN * Lp;
            pshadow = sampleHeightPointShadow(
                i,
                fragWorldPos,
                lightPos,
                radius,
                N,
                Ngeo,
                TBN,
                uv,
                lightPointTS,
                push.materialParams.y
            );
        }
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
        //
        // Restricted to shadow-casting lights only: this is
        // the static torch grid that the bounce is designed
        // for, and skipping it for AoE flashes / VFX lights /
        // secondary unshadowed fillers cuts the per-light cost
        // of the inner loop roughly in half during heavy combat
        // — the ground-bounce halo is invisible from a transient
        // 200ms light burst anyway.
        if (i < int(ubo.pointShadowMeta.x)) {
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
        }  // end shadow-light bounce gate (PBR path)
    }

    if (any(isnan(lighting)) || any(isinf(lighting))) {
        lighting = albedo * max(ao, 0.05);
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
