// Flame tongue whose *silhouette morphs over the particle's life* —
// not a static teardrop that only translates and shrinks. `lifeT`
// is normalised age from the CPU (0 = spawn, 1 = death).
float flameTongue(vec2 uv, float seed, float lifeT) {
    float h = clamp(uv.y, 0.0, 1.0);
    float t = (uv.x - 0.5) * 2.0;
    // Slower internal animation — torch licks, not a jet scroll.
    float time = ubo.timeData.x * 0.38;
    float phase = seed * 6.2831853;

    float ignite = smoothstep(0.0, 0.18, lifeT);
    float death  = 1.0 - smoothstep(0.75, 1.0, lifeT);
    float lifeEnv = ignite * death;

    float baseFade = smoothstep(0.00, 0.08, h) * lifeEnv;
    float tipFade  = (1.0 - smoothstep(0.48, 0.94, h))
                   * (1.0 - smoothstep(0.58, 1.0, lifeT) * 0.75);

    float lifeWidth = mix(0.45, 0.92, smoothstep(0.0, 0.42, lifeT));
    lifeWidth *= 1.0 - 0.38 * smoothstep(0.70, 1.0, lifeT);
    float hTaper = mix(1.0, 0.18, smoothstep(0.06, 0.82, h));
    float taper = max(hTaper * lifeWidth, 0.06);

    float bendAmt = mix(0.08, 0.20, smoothstep(0.10, 0.60, lifeT));
    float bendSlow = sin(h * 1.6 + time * 1.15 + phase + lifeT * 2.8) * bendAmt * pow(h, 1.35);
    float bendFast = sin(h * 3.2 - time * 1.85 + phase * 1.4 + lifeT * 4.5) * bendAmt * 0.28 * h;

    float flowStrength = sin(lifeT * 3.1415927) * 0.14 + 0.03;
    vec2 flow = curl2(vec2(t * 0.9, h * 1.6 - lifeT * 0.9)
                    + vec2(time * 0.06 + lifeT * 0.8, phase * 0.3)) * flowStrength;

    vec2 warped = vec2(t - bendSlow - bendFast + flow.x,
                       h - time * 0.04 - lifeT * 0.10 + flow.y);
    float localT = warped.x / taper;

    vec2 noiseUV = vec2(localT * 1.9, warped.y * 2.8 - time * 0.65 - lifeT * 1.2)
                 + vec2(seed * 7.0, seed * 13.0);
    float nEdge   = fbm2(noiseUV);
    float nBillow = fbm2(noiseUV * 1.9 + vec2(time * 0.28 + lifeT * 1.6, -time * 0.20));
    float nFine   = fbm2(noiseUV * 3.8 + vec2(-time * 0.40, lifeT * 2.5));

    float tipBreak = smoothstep(0.50, 0.92, h) * smoothstep(0.55, 1.0, lifeT);
    float edgeAmp  = 0.14 + 0.20 * tipBreak
                   + 0.05 * sin(time * 2.8 + phase + h * 4.5 + lifeT * 3.5);
    float r = abs(localT) + (nEdge - 0.5) * edgeAmp;

    float lobePhase = h * 3.2 + time * 1.45 + phase + lifeT * 3.5;
    float lobeA = exp(-pow(localT - sin(lobePhase) * 0.26 * sin(lifeT * 3.14159), 2.0) * 10.0);
    float lobeB = exp(-pow(localT + sin(lobePhase * 1.2) * 0.22 * sin(lifeT * 3.14159), 2.0) * 12.0);
    float lobes = (lobeA + lobeB) * sin(lifeT * 3.1415927) * 0.18 * lifeEnv;

    float baseCohesion = exp(-pow(r / 1.10, 2.0) * 2.0)
                       * (1.0 - smoothstep(0.0, 0.38, h))
                       * (1.0 - smoothstep(0.60, 0.95, lifeT)) * 0.45;

    float shell = exp(-pow(r / 0.88, 2.0) * 2.8) * baseFade * tipFade;
    float core  = exp(-pow(r / 0.34, 2.0) * 9.5)
                * exp(-h * 1.9)
                * mix(0.75, 1.10, nBillow)
                * mix(1.0, 0.40, tipBreak);

    float sheets = smoothstep(0.18, 0.88, nBillow) * shell * 0.24 * (0.65 + 0.35 * nFine);
    float tipDissolve = tipBreak * (0.28 + 0.55 * nFine) * shell;
    float coolVein = smoothstep(0.30, 0.60, nEdge) * shell * 0.14;

    float breathe = 0.91 + 0.05 * sin(time * 2.6 + phase + lifeT * 2.0)
                  + 0.03 * sin(time * 4.0 + h * 3.0 + phase * 1.5);

    return clamp((baseCohesion + lobes + shell * 0.42 + core * 1.02
                + sheets + tipDissolve - coolVein) * breathe, 0.0, 1.42);
}
