// SilkStrand: a single sprite that *is* the loot beam — a
// full-height pillar with a soft ethereal body and several
// sharp sine-wave "silk thread" highlights spiralling around
// it. Designed to be HD-readable up close (the sharp threads
// give a sub-pixel-feeling crisp highlight) while still
// looking ethereal at distance (the wide soft body bleeds
// into surrounding bloom).
//
// Geometry assumptions:
//   * Vertex shader has oriented the billboard along
//     world-up (vStretchDir) and stretched it ~6:1 so the
//     quad is tall and narrow.
//   * Particle anchor sits at the *base* of the beam. We
//     therefore only render content in the upper half of
//     the billboard (h in [0,1] mapped from a in [0,1]).
//     The lower half stays empty so the underground portion
//     of the billboard is invisible and doesn't double-up
//     with the ground halo.
//
// All widths and amplitudes taper toward the top: the beam
// physically narrows to pixel-width at h=1 and then fades to
// zero so the silhouette "melts into air".
float silkStrand(vec2 uv, float seed) {
    // The vertex shader anchors this billboard's *bottom
    // edge* at the particle's world position and orients it
    // as a true cylindrical billboard (vertical = world-up,
    // horizontal = camera-right perpendicular to up). So the
    // full quad is visible content: uv.y = 0 is the base,
    // uv.y = 1 is the top. We use the UV directly rather
    // than the symmetric (uv - 0.5)·2 mapping the other
    // sprites use.
    float h = clamp(uv.y, 0.0, 1.0);  // 0 = base, 1 = top
    // Across-axis position. The billboard is widened 2.5× by
    // the vertex shader so the fog shell can extend past the
    // bright core; `tShell` covers the full quad width
    // (-1..1), while `tCore` is rescaled so the core/threads
    // render at their original width regardless of how wide
    // we make the billboard.
    float tShell = (uv.x - 0.5) * 2.0;          // -1..1 across full quad
    float tCore  = tShell * 2.5;                // -1..1 across original core width

    // Discard outside the visible vertical envelope early —
    // saves the fbm/sin work below for empty pixels.
    if (h < 0.001 || h > 0.999) return 0.0;

    // Use `tCore` for everything that previously used `t`.
    float t = tCore;

    // ----- Vertical envelope -----
    // Quick fade-in at base so the bottom doesn't slam into
    // the floor with a hard cap; long smooth fade in the top
    // 12% so the beam visibly "melts into air" rather than
    // terminating at a fixed height.
    float vFade = smoothstep(0.0, 0.04, h)
                * (1.0 - smoothstep(0.88, 1.00, h));

    // Time-driven phase shift so threads visibly rise.
    float scroll = ubo.timeData.x * 0.55 + seed * 11.0;

    // ----- Soft vertical coherence -----
    // Keep the loot pillar as a smooth, stable column. Noise should
    // shimmer the intensity, not reshape the width into a cone.
    vec2  cloudUV  = vec2(t * 0.9 + seed * 4.3,
                          h * 1.6 + scroll * 0.45);
    float cloudA   = fbm2(cloudUV);
    float cloudB   = fbm2(cloudUV * vec2(0.55, 0.40)
                          + vec2(scroll * 0.20, 0.0));
    float cloud    = mix(0.82, 1.00, mix(cloudA, cloudB, 0.5));

    // ----- Soft ethereal body -----
    // Stable central column. It stays mostly constant vertically
    // and only narrows in the last bit before fading away.
    float topNarrow = smoothstep(0.78, 1.00, h);
    float bodyW  = mix(0.070, 0.030, topNarrow);
    float dB     = t / max(bodyW, 1e-4);
    float body   = exp(-dB * dB * 1.9) * 0.42 * cloud;

    // ----- Soft silk threads -----
    // N sine-displaced threads at evenly-spaced phases.
    // Tuned for a *cloudy* reading rather than crisp
    // filaments: wider widths, softer cores, and the cloud
    // density modulates each strand subtly. Amplitude is restrained so
    // the pillar doesn't bulge or snake into a broad vertical glow.
    float threads = 0.0;
    const int N = 4;
    for (int i = 0; i < N; i++) {
        float phase   = float(i) * 1.5708 + seed * 6.2831 + scroll;
        float amp     = mix(0.18, 0.10, topNarrow);
        // Two-frequency sine for a less mechanical wave.
        float wave    = sin(h * 25.0 + phase) * amp
                      + sin(h * 21.0 + phase * 1.7) * amp * 0.18;
        float threadW = mix(0.130, 0.016, topNarrow);
        float dT      = (t - wave) / max(threadW, 1e-4);
        float blob    = exp(-dT * dT * 12.8);
        float accent  = exp(-dT * dT * 17.0) * 0.12;
        // Cloud-modulate per-strand contribution. Sample at
        // an offset of `wave` along the thread so the noise
        // travels with the strand (no double-streaking).
        vec2  strandUV = vec2((t - wave) * 11.4 + float(i) * 7.1,
                              h * 2.2 + scroll * 0.6 + float(i) * 3.3);
        float strandN  = fbm2(strandUV);
        float strandM  = mix(2.70, 1.00, strandN);
        threads += (blob + accent) * strandM;
    }
    threads *= 0.24;

    // ----- Broad fog shell -----
    // Constant-width low-opacity halo. This avoids the old broad-base /
    // slim-top glow while preserving a smooth pillar silhouette.
    float shellW    = mix(0.22, 0.10, topNarrow);
    float dShell    = tShell / max(shellW, 1e-4);
    float shellBase = exp(-dShell * dShell * 1.8);
    vec2  shellUV   = vec2(tShell * 1.6 + seed * 3.7,
                           h * 2.4 + scroll * 0.7);
    float shellN    = fbm2(shellUV);
    float shell     = shellBase * mix(0.82, 1.00, shellN) * 0.10;

    return clamp((shell + body + threads) * vFade, 0.0, 1.15);
}
