bool shouldDiscardWallXray(uint flags) {
    if ((flags & 8u) == 0u || ubo.fogOrigin.w <= 0.001) {
        return false;
    }

    vec3 camToFrag   = fragWorldPos     - ubo.cameraPos.xyz;
    vec3 camToPlayer = ubo.fogOrigin.xyz - ubo.cameraPos.xyz;
    float distPlayer = length(camToPlayer);
    if (distPlayer <= 0.001) {
        return false;
    }

    vec3 dirPlayer = camToPlayer / distPlayer;
    float tFrag = dot(camToFrag, dirPlayer);
    if (tFrag <= 0.2 || tFrag >= distPlayer) {
        return false;
    }

    vec3 closest = ubo.cameraPos.xyz + dirPlayer * tFrag;
    vec3 perp    = fragWorldPos - closest;
    float perpY  = perp.y;
    float perpH  = length(perp - vec3(0.0, perpY, 0.0));

    vec2 shaped = vec2(perpH / 2.4, perpY * 1.1);
    float r = length(shaped);

    float R_inner = 0.12 * tFrag + 1.0;
    float R_outer = 0.17 * tFrag + 1.4;
    if (r >= R_outer) {
        return false;
    }

    float xrayStrength = clamp(ubo.fogOrigin.w, 0.0, 1.0);
    float mask = (1.0 - smoothstep(R_inner, R_outer, r)) * xrayStrength;
    float hash = fract(sin(dot(gl_FragCoord.xy, vec2(12.9898, 78.233))) * 43758.5453);
    float scan = 0.5 + 0.5 * sin(gl_FragCoord.y * 0.45 + ubo.timeData.x * 1.8);
    float stipple = mix(hash, scan, 0.35);
    return stipple < mask - 0.05;
}

void main() {
    // Bit-test the flags float to pick a shading path. Using
    // floatBitsToUint so we can pack other booleans into the
    // same float later (bit 1, bit 2, ...) without touching
    // the Rust side.
    uint flags = floatBitsToUint(push.materialParams.z);
    bool usePbr  = (flags & 1u) != 0u;
    bool useRift = (flags & 2u) != 0u;
    bool selected = (flags & 16u) != 0u;
    bool portrait = (flags & 32u) != 0u;
    bool hovered = (flags & 64u) != 0u;
    bool outlinePass = (flags & 128u) != 0u;

    if (outlinePass) {
        outColor = vec4(1.0, 1.0, 1.0, push.tint.a);
        return;
    }

    if (portrait) {
        if (gl_FragCoord.x < push.tint.x || gl_FragCoord.x > push.tint.z ||
            gl_FragCoord.y < push.tint.y || gl_FragCoord.y > push.tint.w) {
            discard;
        }
    }

    if (shouldDiscardWallXray(flags)) {
        discard;
    }

    vec3 lighting;
    if (useRift)      lighting = shadeRift();
    else if (usePbr)  lighting = shadePbr();
    else              lighting = shadeCel();

    // Distance fog (player-anchored). The rift is a hole
    // through reality — fog still applies (so you can't see
    // it from across an entire dungeon) but is dampened so
    // the rift retains presence in the haze rather than
    // dissolving into the fog colour.
    float dist = length(ubo.fogOrigin.xyz - fragWorldPos);
    float fogRaw = clamp((dist - ubo.fogParams.x) / (ubo.fogParams.y - ubo.fogParams.x), 0.0, 1.0);
    float fogFactor = fogRaw;
    fogFactor = fogFactor * fogFactor;
    if (useRift) fogFactor *= 0.35;
    if (portrait) fogFactor = 0.0;

    vec3 finalColor = mix(lighting, ubo.fogColor.rgb, fogFactor);

    if (hovered && !selected) {
        vec3 N = normalize(fragNormal);
        vec3 V = normalize(ubo.cameraPos.xyz - fragWorldPos);
        float fresnel = 1.0 - clamp(abs(dot(N, V)), 0.0, 1.0);
        float rim = smoothstep(0.42, 0.88, fresnel);
        float pulse = 0.78 + 0.22 * sin(ubo.timeData.x * 5.5);
        vec3 outline = vec3(1.0, 0.68, 0.18);
        float strength = 0.72;
        finalColor += outline * rim * pulse * strength;
    }

    if (portrait) {
        outColor = vec4(finalColor, 1.0);
    } else {
        outColor = vec4(finalColor * push.tint.rgb, push.tint.a);
    }
}
