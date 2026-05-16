#version 450

layout(binding = 0) uniform sampler2D fontAtlas;

layout(location = 0) in vec4 fragColor;
layout(location = 1) in vec2 fragUV;

layout(location = 0) out vec4 outColor;

float hash21(vec2 p) {
    p = fract(p * vec2(123.34, 456.21));
    p += dot(p, p + 45.32);
    return fract(p.x * p.y);
}

float vnoise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    vec2 u = f * f * (3.0 - 2.0 * f);
    float a = hash21(i + vec2(0.0, 0.0));
    float b = hash21(i + vec2(1.0, 0.0));
    float c = hash21(i + vec2(0.0, 1.0));
    float d = hash21(i + vec2(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

void main() {
    // RGBA atlas: solid-colour rects sample the white pixel at
    // UV(0,0) -> (1,1,1,1); font glyphs are stored as
    // (1,1,1, mask); icons store full RGBA. Multiplying by the
    // vertex colour lets callers tint glyphs/icons or fill rects
    // with arbitrary colours through the same path.
    //
    // Sentinel UV.x < 0 marks void-glass panels: skip the atlas.
    // Vertex carries the radial tint; fragment adds panel-locked frost,
    // rim absorb, thin chroma fringe, and corner/top glass reads.

    // ── Glow-disc sentinel (UV in [-2.0, -1.1]) ────────────
    //
    // Void disc: dark-centre → tint-rim radial gradient plus
    // shader halo outside the core.
    //
    // UV band clears the -1.0 noisy sentinel so rasteriser
    // interpolation never leaks into the atlas path.
    //
    // `fragColor.rgb` node tint; `fragColor.a` brightness gate.
    if (fragUV.x >= -2.0 && fragUV.x <= -1.1) {
        vec2 lp = (fragUV - vec2(-2.0)) / 0.9;
        vec2 c = (lp - 0.5) * 2.0;
        float r = length(c);
        const float CORE_R = 0.65;

        if (r > 1.0) {
            discard;
        }

        vec3 tint = fragColor.rgb;
        float bright = fragColor.a;

        if (r > CORE_R) {
            float t = (r - CORE_R) / (1.0 - CORE_R);
            float halo = exp(-t * 3.4);
            float aa = 1.0 - smoothstep(0.96, 1.0, r);
            outColor = vec4(tint * halo * bright, halo * bright * aa * 0.85);
            return;
        }

        float rn = r / CORE_R;
        float fade = pow(rn, 2.2);
        vec3 deep = vec3(0.012, 0.01, 0.038);
        vec3 core_tint = tint * 1.15;
        vec3 col = mix(core_tint, deep, fade);

        vec2 nrm = (r > 1e-3) ? c / r : vec2(0.0, -1.0);
        float light = dot(nrm, normalize(vec2(-0.55, -0.83)));
        float lip = smoothstep(0.88, 0.94, rn) * (1.0 - smoothstep(0.96, 0.99, rn));
        float hi = max(0.0, light);
        vec3 lip_hi = vec3(0.58, 0.52, 0.95);
        col = mix(col, lip_hi, lip * hi * 0.52);

        float outline = smoothstep(0.96, 1.0, rn);
        col = mix(col, vec3(0.01, 0.01, 0.015), outline * 0.92);

        float nv = vnoise(gl_FragCoord.xy * (1.0 / 248.0));
        col *= 1.0 + (nv - 0.5) * 0.024;

        col *= bright;
        float disc_aa = 1.0 - smoothstep(0.985, 1.0, rn);
        outColor = vec4(clamp(col, 0.0, 1.0), disc_aa);
        return;
    }

    // ── Glow-line sentinel (UV in [-3.0, -2.1]) ────────────
    if (fragUV.x >= -3.0 && fragUV.x <= -2.1) {
        float across = (fragUV.x - (-3.0)) / 0.9;
        float d = abs(across - 0.5) * 2.0;
        vec3 tint = fragColor.rgb;
        float bright = fragColor.a;
        float halo = exp(-d * 3.0);
        float core_light = smoothstep(0.35, 0.0, d);
        vec3 core_hi = vec3(0.94, 0.93, 1.0);
        vec3 col = mix(tint, core_hi, core_light * 0.75);
        outColor = vec4(col * halo * bright, halo * bright);
        return;
    }

    // Packed void-glass: [GU0,GU1]² — overlay.rs `rounded_rect_px_radial_noisy`.
    // guv ties modulation to the panel (vertex radial cannot do frost / fringe).
    if (fragUV.x < 0.0) {
        const float GU0 = -0.985;
        const float GU1 = -0.015;
        float g_span = GU1 - GU0;
        vec2 guv = clamp((fragUV - GU0) / g_span, vec2(0.0), vec2(1.0));

        vec2 fc = gl_FragCoord.xy;
        float n_scr = vnoise(fc * (1.0 / 256.0));

        vec3 rgb = fragColor.rgb * (0.988 + n_scr * 0.022);

        vec2 d_edge = min(guv, 1.0 - guv);
        float edge_dist = min(d_edge.x, d_edge.y);

        // Etched / frosted grain locked to panel UV (stable when UI moves).
        float g1 = vnoise(guv * vec2(108.0, 94.0) + vec2(0.07, 0.29));
        float g2 = vnoise(guv * vec2(203.0, 181.0) + vec2(0.51, 0.04));
        float g3 = vnoise(guv * vec2(47.0, 52.0) + g1);
        float fro = g1 * 0.52 + g2 * 0.30 + g3 * 0.18;
        rgb *= 0.962 + (fro - 0.5) * 0.12;

        // Second mass: cooler, denser glass within ~15% of perimeter (not same as vertex s).
        float plate = smoothstep(0.36, 0.0, edge_dist);
        rgb *= mix(vec3(1.0), vec3(0.80, 0.76, 0.93), plate * 0.52);

        // Thin perimeter chroma (slab edge dispersion), very localised.
        float fringe = smoothstep(0.095, 0.0, edge_dist);
        rgb *= vec3(1.0 + 0.052 * fringe, 1.0, 1.0 - 0.048 * fringe);

        // Hairline spec: grazing edge + corners + narrow top band (low gain).
        float graz = pow(clamp(1.0 - edge_dist / 0.082, 0.0, 1.0), 5.0);
        vec2 cr = min(guv, 1.0 - guv);
        float corner = smoothstep(0.11, 0.0, length(cr));
        float top = smoothstep(0.93, 0.997, 1.0 - guv.y) * smoothstep(0.16, 0.84, guv.x);
        float sp = graz * 0.24 + corner * 0.16 + top * 0.38;
        rgb += vec3(0.40, 0.38, 0.55) * sp * 0.048 * fragColor.a;

        float lum = dot(rgb, vec3(0.299, 0.587, 0.114));
        float lum_w = mix(0.50, 1.0, smoothstep(0.04, 0.42, lum));

        float depth = smoothstep(0.42, 0.0, lum);
        rgb += vec3(0.045, 0.035, 0.11) * depth * 0.046 * fragColor.a;

        float w = sin(fc.x * 0.00275 + fc.y * 0.00205 + n_scr * 0.18) * 0.5 + 0.5;
        float bloom = smoothstep(0.928, 1.0, w) * lum_w;
        rgb += vec3(0.48, 0.44, 0.94) * bloom * 0.026;

        float w2 = sin(fc.x * (-0.00195) + fc.y * 0.00285 + 2.7) * 0.5 + 0.5;
        float bloom2 = smoothstep(0.942, 1.0, w2) * lum_w * 0.62;
        rgb += vec3(0.42, 0.52, 1.0) * bloom2 * 0.014;

        outColor = vec4(clamp(rgb, 0.0, 1.0), fragColor.a);
        return;
    }

    // Icon silhouette: vertices bias atlas U by +ICON_SILHOUETTE_U_BIAS so real UVs
    // stay in [0,1] while fragUV.x lands > 2.5. RGB comes only from fragColor;
    // luminance × alpha from the PNG drives opacity (no hue from tex.rgb).
    const float ICON_SILHOUETTE_U_BIAS = 3.0;
    if (fragUV.x > 2.5) {
        vec2 atlas_uv = fragUV - vec2(ICON_SILHOUETTE_U_BIAS, 0.0);
        vec4 tex = texture(fontAtlas, atlas_uv);
        float lum = dot(tex.rgb, vec3(0.299, 0.587, 0.114));
        float mask = clamp(lum * tex.a, 0.0, 1.0);
        outColor = vec4(fragColor.rgb, fragColor.a * mask);
        return;
    }

    vec4 tex = texture(fontAtlas, fragUV);
    outColor = fragColor * tex;
}
