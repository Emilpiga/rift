// GroundCrack: procedural fracture/scorch decal for heavy floor
// impacts. The vertex shader draws this sprite flat on XZ; here
// we compose long irregular fissures, broken annular chips near
// the gameplay radius, and noisy scorched fill around the centre.
float groundCrack(vec2 uv, float seed) {
    vec2 p = (uv - 0.5) * 2.0;
    float r = length(p);
    if (r > 1.0) return 0.0;

    float theta = atan(p.y, p.x);
    float cracks = 0.0;

    const int SPOKES = 14;
    for (int i = 0; i < SPOKES; i++) {
        float fi = float(i);
        float h1 = hash21(vec2(fi + seed * 13.7, seed * 31.1));
        float h2 = hash21(vec2(fi * 5.3, seed * 19.9 + 2.0));
        float h3 = hash21(vec2(fi * 9.1 + 4.0, seed * 7.7));

        float angle = (fi + h1 * 0.72) * 6.2831853 / float(SPOKES);
        float diff = atan(sin(theta - angle), cos(theta - angle));

        float len = mix(0.42, 0.98, h2);
        float start = mix(0.04, 0.22, h3);
        float spokeMask = smoothstep(start, start + 0.08, r)
                * (1.0 - smoothstep(len, len + 0.10, r));

        float width = mix(0.010, 0.028, h1) * mix(1.35, 0.55, r);
        float line = exp(-pow(diff / max(width, 0.003), 2.0)) * spokeMask;

        float branchAngle = angle + mix(-0.55, 0.55, h3);
        float branchDiff = atan(sin(theta - branchAngle), cos(theta - branchAngle));
        float branchStart = mix(0.26, 0.58, h1);
        float branchActive = smoothstep(branchStart, branchStart + 0.05, r)
                           * (1.0 - smoothstep(len * 0.92, len, r));
        float branch = exp(-pow(branchDiff / max(width * 0.62, 0.003), 2.0))
                     * branchActive;

        cracks = max(cracks, max(line, branch * 0.72));
    }

    float n = fbm2(p * 3.2 + vec2(seed * 8.0, seed * 3.0));

    float ringBand = exp(-pow((r - 0.78) * 18.0, 2.0));
    float angular = valueNoise(vec2(theta * 2.4 + seed * 5.0, seed * 11.0));
    float brokenRing = ringBand * smoothstep(0.42, 0.78, angular + n * 0.28);

    float core = (1.0 - smoothstep(0.05, 0.38, r)) * mix(0.35, 1.0, n);
    float scorch = (1.0 - smoothstep(0.18, 0.92, r)) * smoothstep(0.48, 0.92, n) * 0.42;

    return clamp(max(max(cracks, brokenRing * 0.75), max(core * 0.38, scorch)), 0.0, 1.25);
}
