// Spark: oriented motion streak along `vStretchDir` if the
// particle is moving, falling back to a +/× cross when it is
// stationary. The fast / slow distinction is automatic — the
// vertex shader has already stretched the geometry, so even a
// stationary spark gets its corner UVs in the original
// [0,1] frame.
float spark(vec2 uv) {
    vec2 c = (uv - 0.5) * 2.0;       // [-1, 1]
    float d = length(c);
    float core = exp(-d * d * 34.0);

    // Motion-aligned streak: project onto velocity direction.
    // `vStretchDir` length up to 2.0 means the geometry has
    // already been elongated — we narrow the across-axis here
    // to 8× to give the streak a hairline feel.
    float streakAniso = 0.0;
    if (length(vStretchDir) > 0.01) {
        vec2 dir = normalize(vStretchDir);

        // project but immediately destabilize it
        float a = dot(c, dir);
        float t = dot(c, vec2(-dir.y, dir.x));

        float wobble = valueNoise(c * 8.0 + vSeed * 20.0 + a);

        float coreLine = exp(-t * t * 60.0);
        float breakUp  = smoothstep(0.2, 0.8, wobble);

        streakAniso = coreLine * breakUp;
    }

    // Static cross (visible when not moving): two perpendicular
    // hairlines along the rotated billboard axes.
    float crossX = exp(-c.y * c.y * 140.0) * exp(-abs(c.x) * 3.2);
    float crossY = exp(-c.x * c.x * 140.0) * exp(-abs(c.y) * 3.2);
    float crossLines = (crossX + crossY) * 0.5;

    // Blend cross into streak as motion increases.
    float motionBlend = smoothstep(0.10, 0.80, length(vStretchDir));
    float streak = mix(crossLines, streakAniso, motionBlend);

    float glint = exp(-d * d * 120.0) * (0.72 + 0.28 * valueNoise(uv * 22.0 + vSeed));
    return clamp(core + 0.76 * streak + glint * 0.36, 0.0, 2.9);
}
