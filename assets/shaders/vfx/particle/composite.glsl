void main() {
    // Most sprites are pure-alpha (mask drives both alpha and
    // brightness). Smoke is special: it returns a separate
    // emissive fraction so we can boost RGB into bloom range
    // only where the smoke is dense + has a hot internal
    // pocket, leaving the wispy outer band cool.
    vec2 spriteEval = evaluateSprite(
        vSprite, vUv, vSeed, vLifeT, vWorldXZ, vWorldY, vStylePack, vStyleAux, vRolePack);
    float mask = spriteEval.x;
    float emissive = spriteEval.y;

    // ----- Hard quad-edge fade -----
    // Every procedural sprite already tries to fade to zero
    // at the quad boundary, but most retain a tiny residual
    // alpha at the cardinal edges. Additive blend + bloom +
    // ACES amplifies that into a visible billboard square
    // outline. Force the mask to zero over the outermost ~12%
    // of the quad in both axes so no sprite ever leaks its
    // bounding box.
    vec2 p = vUv - 0.5;
    float r = length(p) * 2.0;

    // anisotropic softening (prevents “perfect circle stamp”)
    vec2 grad = vec2(dFdx(r), dFdy(r));
    float aa = max(length(grad), 0.0015);

    // base radial fade (primary fix for square look)
    float quadFade = 1.0 - smoothstep(0.92 - aa, 1.0, r);

    // extra edge breakup (keeps it organic, not disk-perfect)
    float edgeNoise = valueNoise(vUv * 35.0 + vSeed * 7.0);
    quadFade *= mix(0.92, 1.0, edgeNoise);

    // slight extra corner suppression WITHOUT box shape
    float corner = pow(max(abs(p.x), abs(p.y)) * 2.0, 2.2);
    quadFade *= 1.0 - smoothstep(0.85, 1.15, corner);

    if (vSprite != 2u && vSprite != 10u) {
        mask *= quadFade;
        emissive *= quadFade;
    }

    mask *= smoothstep(0.0, 0.02, 1.0 - r);

    // Per-particle distance dim — keeps very-near big puffs
    // from crushing ACES.
    mask *= vDistDim;
    emissive *= vDistDim;

    float a = clamp(vColor.a * mask, 0.0, 1.0);

    // ----- Soft-particle fade -----
    // Only broad volumetric sprites need the scene-depth soft
    // intersection. Sparks/shards/rings/streaks are small crisp
    // accents; skipping the depth texture sample on those cuts a
    // lot of rift-combat overdraw cost without visible popping.
    bool softParticle = (vSprite == 0u || vSprite == 2u || vSprite == 6u || vSprite == 7u || vSprite == 8u || vSprite == 9u || vSprite == 10u);
    if (softParticle && vSprite != 10u) {
        vec2 screenSize = vec2(textureSize(sceneDepth, 0));
        vec2 screenUV = gl_FragCoord.xy / screenSize;
        float scene_z_ndc = texture(sceneDepth, screenUV).r;
        float scene_eye = linearEyeDepth(scene_z_ndc);
        float frag_eye  = linearEyeDepth(gl_FragCoord.z);
        float dz = scene_eye - frag_eye;
        float fadeBand = (vSprite == 2u) ? 0.85 : 0.5;
        float soft = clamp(dz / fadeBand, 0.0, 1.0);
        soft = soft * soft * (3.0 - 2.0 * soft);
        a *= soft;
        emissive *= soft;
    }

    if (vSprite == 2u) {
        a = 1.0 - exp(-a * 2.8);
        emissive *= 1.0 - exp(-emissive * 2.0);
    }

    // Hybrid billow: texture R = opacity, particle gradient = colour.
    // Skip smoke tints / fog / emissive — they were washing out soft alpha.
    if (vSprite == 10u) {
        outColor = vec4(vColor.rgb * a, a);
        return;
    }

    vec3 rgb = vColor.rgb;
    if (vSprite == 2u || vSprite == 9u) {
        rgb *= heightTemperatureTint(vWorldY, vOriginY, vSprite);
    }
    rgb = mix(rgb, ubo.fogColor.rgb, vFogFactor);

    if (emissive > 0.0) {
        float e = emissive * (1.0 - vFogFactor);

        // hard knee so bright parts “snap” into bloom range
        float boost = 1.0 + pow(e, 1.35) * 3.8;

        // extra hot core emphasis
        float core = smoothstep(0.2, 1.0, mask);
        boost += core * e * 2.2;

        rgb *= boost;

        // optional: slight color temperature push into yellow/white
        rgb += e * e * vec3(0.25, 0.18, 0.05);
    }

    // Shader-only HD pass: tiny luminance variation and core
    // tightening on HDR particles. Kept deliberately subtle so
    // alpha smoke stays soft while additive glows/sparks gain
    // a crisper bloom-catching centre instead of reading like
    // flat blurry discs.
    float maxRgb = max(max(rgb.r, rgb.g), rgb.b);
    float hdrWeight = smoothstep(0.85, 2.25, maxRgb) * (1.0 - vFogFactor);
    bool crispSprite = (vSprite == 0u || vSprite == 1u || vSprite == 3u || vSprite == 4u || vSprite == 5u || vSprite == 9u);
    vec2 maskGrad = vec2(dFdx(mask), dFdy(mask));
    float edgeCatch = smoothstep(0.018, 0.115, length(maskGrad));
    if (crispSprite && hdrWeight > 0.001) {
        float core = pow(mask, 1.8);

        // strong center hot-spot (this is what makes PoE readable)
        float punch = hdrWeight * core;

        rgb *= 1.0 + punch * 0.85;

        // controlled sparkle breakup (NOT grain wash)
        float sparkle = valueNoise(vUv * 90.0 + vSeed * 31.7 + ubo.timeData.x);
        rgb += punch * sparkle * 0.25;

        // edge “electric rim” feel
        rgb += edgeCatch * punch * vec3(0.35, 0.25, 0.15);
    }

    // exponential fog cut instead of linear fade
    a *= exp(-vFogFactor * 1.8);

    // boost contrast in dense regions (PoE trick)
    a = pow(a, 0.85);

    // Output is **pre-multiplied alpha**. Both pipelines drive
    // this through `SRC = ONE`:
    //
    //   Alpha pipeline    : ONE × rgb + (1-SRC_ALPHA) × dst
    //   Additive pipeline : ONE × rgb +           ONE × dst
    //
    // …so a single shader feeds both blend modes correctly.
    outColor = vec4(rgb * a, a);
}
