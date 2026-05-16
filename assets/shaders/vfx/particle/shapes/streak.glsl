// Streak: pure motion line. Always anisotropic, even at low
// speed — the caller picked this sprite specifically because
// they want a streak look. The head pinprick has a small
// time-driven shimmer so a continuous trail of streaks reads
// as actively burning rather than a chain of static dots.
float streak(vec2 uv) {
    vec2 c = (uv - 0.5) * 2.0;
    vec2 along  = (length(vStretchDir) > 0.01)
                ? normalize(vStretchDir)
                : vec2(1.0, 0.0);
    vec2 across = vec2(-along.y, along.x);
    float a = dot(c, along);
    float t = dot(c, across);
    // Tight across-axis (line thickness), gentle along-axis
    // taper at the ends.
    float line = exp(-t * t * 128.0) * (1.0 - aaStep(0.98, abs(a)));
    float filament = exp(-pow(abs(t) - 0.075, 2.0) * 720.0)
                   * (1.0 - aaStep(0.82, abs(a))) * 0.20;
    // Bright pinprick at the head, time-modulated so embers
    // in a continuous trail shimmer at different phases.
    float headPhase = ubo.timeData.x * 14.0 + vSeed * 6.28318;
    float headMod   = 0.85 + 0.15 * sin(headPhase);
    float head = exp(-pow(a - 0.85, 2.0) * 68.0) * exp(-t * t * 260.0);
    return clamp(line + filament + head * 1.45 * headMod, 0.0, 2.15);
}
