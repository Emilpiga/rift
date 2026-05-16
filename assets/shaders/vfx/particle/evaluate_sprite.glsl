#include "sprite_ids.glsl"

struct GfxStyle {
    float presetId;
    float energy;
    float sharpness;
    float emissiveBias;
    float turbulence;
    float density;
    float edgeSoft;
};

GfxStyle unpackGfxStyle(vec4 pack, vec4 aux) {
    GfxStyle s;
    s.presetId = pack.x;
    s.energy = (pack.y > 0.0) ? pack.y : 1.0;
    s.sharpness = (pack.z > 0.0) ? pack.z : 0.5;
    s.emissiveBias = (pack.w > 0.0) ? pack.w : 1.0;
    s.turbulence = 1.0;
    s.density = 1.0;
    s.edgeSoft = 0.25;
    if (s.presetId >= 0.5 && s.presetId < 1.5) {
        s.turbulence = 1.28;
        s.edgeSoft = 0.18;
    } else if (s.presetId >= 1.5 && s.presetId < 2.5) {
        s.turbulence = 1.15;
        s.edgeSoft = 0.32;
    } else if (s.presetId >= 2.5 && s.presetId < 3.5) {
        s.turbulence = 1.08;
        s.edgeSoft = 0.22;
    }
    if (aux.x > 0.0) s.turbulence = aux.x;
    if (aux.y > 0.0) s.density = aux.y;
    if (aux.z > 0.0) s.edgeSoft = aux.z;
    return s;
}

vec2 evaluateSpriteBase(
    uint sprite,
    vec2 uv,
    float seed,
    float lifeT,
    vec2 worldXZ,
    float worldY
) {
    float mask;
    float emissive = 0.0;
    if      (sprite == SPRITE_SOFT_GLOW) mask = softGlow(uv);
    else if (sprite == SPRITE_SPARK) mask = spark(uv);
    else if (sprite == SPRITE_SMOKE) {
        vec2 sm = smokePuff(uv, seed, lifeT, worldXZ, worldY);
        mask = sm.x;
        emissive = sm.y;
    }
    else if (sprite == SPRITE_HYBRID) {
        vec2 sm = hybridParticle(uv, seed, lifeT, worldXZ, worldY);
        mask = sm.x;
        emissive = sm.y;
    }
    else if (sprite == SPRITE_SHARD) mask = shard(uv, seed);
    else if (sprite == SPRITE_RING) mask = ring(uv);
    else if (sprite == SPRITE_STREAK) mask = streak(uv);
    else if (sprite == SPRITE_WISP) mask = wisp(uv, seed);
    else if (sprite == SPRITE_SILK_STRAND) mask = silkStrand(uv, seed);
    else if (sprite == SPRITE_GROUND_CRACK) mask = groundCrack(uv, seed);
    else if (sprite == SPRITE_FLAME) mask = flameTongue(uv, seed, lifeT);
    else mask = softGlow(uv);
    return vec2(mask, emissive);
}

vec2 applyVoidFrostStyle(GfxStyle st, uint sprite, vec2 uv, float lifeT, vec2 base) {
    float mask = base.x;
    float emissive = base.y;
    float sharpPow = mix(1.0, 1.55, st.sharpness);

    if (sprite == SPRITE_SOFT_GLOW || sprite == SPRITE_WISP) {
        float col = 1.0 - smoothstep(0.12, 0.88, abs(uv.x - 0.5) * 2.0);
        mask *= mix(0.82, 1.0, col);
        mask = pow(mask, sharpPow);
    } else if (sprite == SPRITE_SPARK || sprite == SPRITE_STREAK) {
        mask = pow(mask, mix(1.0, 1.35, st.sharpness));
        float rim = smoothstep(0.35, 0.08, length(uv - 0.5));
        mask += rim * 0.22 * st.emissiveBias;
    } else if (sprite == SPRITE_SMOKE || sprite == SPRITE_HYBRID) {
        mask = pow(mask, mix(1.0, 1.12, st.sharpness));
        emissive *= st.emissiveBias;
        vec2 w = curl2((uv - 0.5) * 4.0 + vSeed) * 0.08 * st.turbulence;
        mask *= mix(0.88, 1.0, valueNoise((uv + w) * 14.0 + vSeed * 5.0));
    } else if (sprite == SPRITE_SHARD) {
        mask = pow(mask, sharpPow);
        mask += smoothstep(0.42, 0.0, abs((uv.x - uv.y) * 1.4)) * 0.18 * st.emissiveBias;
    } else if (sprite == SPRITE_RING) {
        mask = pow(mask, mix(1.0, 1.4, st.sharpness));
        float ringT = length(uv - 0.5) * 2.0;
        mask *= 1.0 + smoothstep(0.55, 0.35, ringT) * 0.15 * st.emissiveBias;
    } else if (sprite == SPRITE_SILK_STRAND) {
        float veil = valueNoise(vec2(uv.x * 18.0, uv.y * 6.0 + ubo.timeData.x * 0.6));
        mask *= mix(0.86, 1.0, veil);
        mask *= mix(0.9, 1.05, 1.0 - smoothstep(0.2, 0.9, abs(uv.x - 0.5) * 2.0));
    } else if (sprite == SPRITE_GROUND_CRACK) {
        mask = pow(mask, mix(1.0, 1.3, st.sharpness));
        float crack = valueNoise(uv * 40.0 + vSeed * 8.0);
        mask *= mix(0.88, 1.12, crack);
    } else if (sprite == SPRITE_FLAME) {
        mask = pow(mask, mix(1.0, 1.15, st.sharpness));
        emissive *= st.emissiveBias * 0.9;
    }

    mask *= mix(1.0, st.energy, 0.35);
    return vec2(mask, emissive);
}

vec2 applyEmberVoidStyle(GfxStyle st, uint sprite, vec2 uv, float lifeT, vec2 base) {
    float mask = base.x;
    float emissive = base.y;
    float soft = mix(1.0, 0.88, st.sharpness);

    if (sprite == SPRITE_SOFT_GLOW || sprite == SPRITE_FLAME || sprite == SPRITE_WISP) {
        mask = pow(mask, soft);
        mask += exp(-dot(uv - 0.5, uv - 0.5) * 14.0) * 0.35 * st.emissiveBias;
        emissive *= st.emissiveBias;
    } else if (sprite == SPRITE_SPARK || sprite == SPRITE_STREAK || sprite == SPRITE_SHARD) {
        mask = pow(mask, mix(1.0, 0.9, st.sharpness));
        emissive = max(emissive, mask * 0.35 * st.emissiveBias);
    } else if (sprite == SPRITE_SMOKE || sprite == SPRITE_HYBRID) {
        emissive *= st.emissiveBias;
        mask *= mix(0.92, 1.08, valueNoise(uv * 22.0 + vSeed * 9.0 + ubo.timeData.x * 0.5));
    } else if (sprite == SPRITE_RING || sprite == SPRITE_GROUND_CRACK) {
        mask = pow(mask, mix(1.0, 0.85, st.edgeSoft));
    } else if (sprite == SPRITE_SILK_STRAND) {
        float shimmer = valueNoise(vec2(uv.x * 30.0, uv.y * 8.0 + ubo.timeData.x));
        mask *= mix(0.9, 1.1, shimmer);
        emissive *= st.emissiveBias * 0.85;
    }

    mask *= mix(1.0, st.energy, 0.28);
    return vec2(mask, emissive);
}

vec2 applyArcLightningStyle(GfxStyle st, uint sprite, vec2 uv, float lifeT, vec2 base) {
    float mask = base.x;
    float emissive = base.y;
    float sharpPow = mix(1.0, 1.45, st.sharpness);

    if (sprite == SPRITE_SOFT_GLOW || sprite == SPRITE_SPARK || sprite == SPRITE_STREAK) {
        mask = pow(mask, sharpPow);
        float filament = valueNoise(uv * 48.0 + vSeed * 13.0 + ubo.timeData.x * 2.0);
        mask *= mix(0.88, 1.18, filament);
        emissive = max(emissive, filament * 0.15 * st.emissiveBias);
    } else if (sprite == SPRITE_SMOKE || sprite == SPRITE_HYBRID) {
        emissive *= st.emissiveBias;
        vec2 w = curl2((uv - 0.5) * 6.0 + vSeed) * 0.06;
        mask *= mix(0.85, 1.12, valueNoise((uv + w) * 32.0 + ubo.timeData.x * 3.0));
    } else if (sprite == SPRITE_SHARD) {
        mask = pow(mask, sharpPow);
        mask += valueNoise(uv * 64.0 + vSeed * 7.0) * 0.12 * st.emissiveBias;
    } else if (sprite == SPRITE_RING || sprite == SPRITE_GROUND_CRACK) {
        mask = pow(mask, mix(1.0, 1.35, st.sharpness));
        float ringT = length(uv - 0.5) * 2.0;
        mask *= 1.0 + smoothstep(0.5, 0.32, ringT) * 0.2 * st.emissiveBias;
    } else if (sprite == SPRITE_WISP || sprite == SPRITE_SILK_STRAND) {
        float bolt = valueNoise(vec2(uv.x * 40.0 + ubo.timeData.x * 4.0, uv.y * 12.0));
        mask *= mix(0.82, 1.15, bolt);
        emissive = max(emissive, bolt * 0.2 * st.emissiveBias);
    } else if (sprite == SPRITE_FLAME) {
        mask = pow(mask, mix(1.0, 1.2, st.sharpness));
        emissive *= st.emissiveBias;
    }

    mask *= mix(1.0, st.energy, 0.3);
    return vec2(mask, emissive);
}

#include "evaluate_role.glsl"

vec2 evaluateSprite(
    uint sprite,
    vec2 uv,
    float seed,
    float lifeT,
    vec2 worldXZ,
    float worldY,
    vec4 stylePack,
    vec4 styleAux,
    vec4 rolePack
) {
    vec2 base = evaluateSpriteBase(sprite, uv, seed, lifeT, worldXZ, worldY);
    vec2 styled = base;
    if (stylePack.x >= 0.5) {
        GfxStyle st = unpackGfxStyle(stylePack, styleAux);
        if (st.presetId < 1.5) {
            styled = applyVoidFrostStyle(st, sprite, uv, lifeT, base);
        } else if (st.presetId < 2.5) {
            styled = applyEmberVoidStyle(st, sprite, uv, lifeT, base);
        } else if (st.presetId < 3.5) {
            styled = applyArcLightningStyle(st, sprite, uv, lifeT, base);
        }
        styled.x *= mix(1.0, st.density, 0.18);
        styled = applyRoleAccent(rolePack.x, st, sprite, uv, lifeT, styled);
    }
    return styled;
}
