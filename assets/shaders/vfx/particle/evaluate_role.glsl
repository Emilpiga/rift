// Phase 4 — semantic role accents on top of preset styling.

vec2 applyRoleAccent(
    float roleId,
    GfxStyle st,
    uint sprite,
    vec2 uv,
    float lifeT,
    vec2 styled
) {
    if (roleId < 0.5) {
        return styled;
    }
    float mask = styled.x;
    float emissive = styled.y;

    if (roleId < 1.5) {
        if (sprite == SPRITE_SOFT_GLOW) {
            float core = exp(-dot(uv - 0.5, uv - 0.5) * 12.0);
            mask += core * 0.12 * st.emissiveBias;
        }
    } else if (roleId < 2.5) {
        float edge = smoothstep(0.28, 0.05, length(uv - 0.5));
        mask += edge * 0.15 * st.emissiveBias;
        if (sprite == SPRITE_STREAK) {
            mask = pow(mask, mix(1.0, 1.2, st.sharpness));
        }
    } else if (roleId < 3.5) {
        if (sprite == SPRITE_RING || sprite == SPRITE_GROUND_CRACK) {
            float ringT = length(uv - 0.5) * 2.0;
            mask *= 1.0 + smoothstep(0.5, 0.3, ringT) * 0.18 * st.emissiveBias;
        }
    } else if (roleId < 4.5) {
        if (sprite == SPRITE_SMOKE || sprite == SPRITE_HYBRID) {
            emissive *= mix(1.0, st.emissiveBias, 0.35);
            float puff = valueNoise(uv * 20.0 + vSeed * 4.0);
            mask *= mix(0.9, 1.05, puff);
        }
    } else if (roleId < 5.5) {
        mask = pow(mask, mix(1.0, 1.25, st.sharpness));
        float flash = 1.0 - smoothstep(0.0, 0.35, lifeT);
        mask *= mix(1.0, 1.15, flash);
    } else if (roleId < 6.5) {
        mask *= mix(0.85, 1.0, lifeT);
        if (sprite == SPRITE_SMOKE) {
            emissive *= 0.7;
        }
    }

    return vec2(mask, emissive);
}
