// Ring: broad, soft shock/heat band. Avoid hard inner/outer rim outlines;
// the effect should read as atmospheric energy, not a UI targeting circle.
float ring(vec2 uv) {
    vec2 c = uv - 0.5;
    float r = length(c);
    float theta = atan(c.y, c.x);

    // Wide gaussian body with a softer leading edge. It still has a
    // centreline, but no razor-thin outline.
    float body = exp(-pow((r - 0.40) / 0.105, 2.0));
    float glow = exp(-pow((r - 0.40) / 0.205, 2.0)) * 0.26;

    // Irregular angular breakup so large rings do not look like clean UI.
    float angular = valueNoise(vec2(theta * 1.7 + vSeed * 7.0, vSeed * 3.0));
    float breakup = mix(0.70, 1.0, angular);

    // Very soft asymmetric hot arc for motion/readability, not a crisp segment.
    float arc = pow(0.5 + 0.5 * sin(theta * 3.0 + vSeed * 6.2831853), 4.0)
              * exp(-pow((r - 0.41) / 0.145, 2.0)) * 0.12;

    // Fade near the hard quad boundary.
    float cardFade = 1.0 - smoothstep(0.48, 0.52, r);
    return clamp((body * 0.72 + glow) * breakup + arc, 0.0, 1.05) * cardFade;
}
