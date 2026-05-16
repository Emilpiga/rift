// Wisp: ethereal strand, not a straight light column. The vertex
// shader provides a world-up cylindrical billboard, but the mask
// below bends the centreline, tapers the ends, and breaks the
// body into drifting density pockets so a field of wisps reads as
// animated vapor rather than repeated vertical smears.
float wisp(vec2 uv, float seed) {
    vec2 c = (uv - 0.5) * 2.0;
    vec2 along  = (length(vStretchDir) > 0.01)
                ? normalize(vStretchDir)
                : vec2(0.0, 1.0);
    vec2 across = vec2(-along.y, along.x);
    float a = dot(c, along);   // -1..1 along the strand
    float t = dot(c, across);  // -1..1 across thickness

    float h = a * 0.5 + 0.5;
    float time = ubo.timeData.x;
    float phase = seed * 6.2831853;

    float endFade = smoothstep(0.00, 0.16, h) * (1.0 - smoothstep(0.78, 1.00, h));
    float taper = mix(0.72, 0.24, smoothstep(0.10, 1.00, h));

    float centre = sin(h * 5.4 + phase + time * 0.62) * 0.18
                 + sin(h * 11.0 + phase * 1.7 - time * 0.38) * 0.07;
    centre *= endFade;
    float localT = t - centre;

    vec2 flowUV = vec2(localT * 1.8 + seed * 3.1,
                       h * 2.4 - time * 0.34 + seed * 8.7);
    float largeNoise = fbm2(flowUV);
    float fineNoise = fbm2(flowUV * 2.6 + vec2(time * 0.18, -time * 0.27));

    float width = mix(0.16, 0.34, largeNoise) * taper;
    float shell = exp(-pow(localT / max(width * 1.8, 0.02), 2.0)) * 0.28;
    float core = exp(-pow(localT / max(width * 0.48, 0.012), 2.0)) * 0.64;

    float strand = 0.0;
    for (int i = 0; i < 3; i++) {
        float fi = float(i);
        float strandPhase = phase + fi * 2.0943951;
        float offset = centre * (0.45 + fi * 0.18)
                     + sin(h * (7.0 + fi * 2.1) + strandPhase - time * (0.45 + fi * 0.12))
                     * (0.09 + fi * 0.025) * endFade;
        float d = localT - offset;
        float strandWidth = mix(0.035, 0.075, fineNoise) * taper;
        float broken = smoothstep(0.22, 0.88, fbm2(vec2(fi * 5.3 + h * 3.0, seed * 4.0 - time * 0.55)));
        strand += exp(-pow(d / max(strandWidth, 0.01), 2.0)) * broken;
    }
    strand *= 0.22;

    float density = mix(0.35, 1.08, largeNoise) * mix(0.72, 1.08, fineNoise);
    float edgeBreak = smoothstep(0.10, 0.78, fineNoise + largeNoise * 0.35);
    float mask = (shell + core + strand) * density * edgeBreak * endFade;
    return clamp(mask, 0.0, 1.15);
}
