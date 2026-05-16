// SoftGlow returns a single value that drives both alpha and
// emissive contributions — it's a glow, the brightness *is*
// the alpha. Dual-radius read so bloom catches the wide halo
// while the eye reads the tight core as the centre.
float softGlow(vec2 uv) {
    vec2 p = (uv - 0.5);

    // unstable warp (kills clean radial symmetry)
    vec2 w = curl2(p * 3.0 + vSeed) * 0.35;
    p += w;

    float r = length(p);

    // break radial coherence with jittered radius
    float jitter = valueNoise(p * 12.0 + vSeed * 10.0);
    r += (jitter - 0.5) * 0.25;

    float core = exp(-r * r * 9.0);
    float halo = exp(-r * r * 2.2) * 0.45;

    // NO angular symmetry rays anymore
    float breakup = valueNoise(p * 18.0 + vSeed * 3.0);

    return (core + halo) * (0.6 + breakup * 0.6);
}
