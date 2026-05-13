#version 450

layout(binding = 0) uniform sampler2D fontAtlas;

layout(location = 0) in vec4 fragColor;
layout(location = 1) in vec2 fragUV;

layout(location = 0) out vec4 outColor;

// 2D hash → [0,1). Stable across frames because gl_FragCoord
// snaps to integer pixel centres in our pipeline.
float hash21(vec2 p) {
    p = fract(p * vec2(123.34, 456.21));
    p += dot(p, p + 45.32);
    return fract(p.x * p.y);
}

// Smooth value noise: bilinearly interpolate hashes at the
// four lattice corners, with smoothstep on the local
// fraction. Cheap and shimmer-free at integer pixel coords.
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

// Three-octave fractal brownian motion. Lower base
// frequency + more octaves = soft cloudy bands rather than
// per-pixel grain. Each octave is rotated by ~73° to break
// up axis-aligned artefacts from the lattice noise.
float fbm3(vec2 p) {
    const mat2 R = mat2(0.292, -0.956, 0.956, 0.292); // rot ~73°
    float v = 0.0;
    float amp = 0.55;
    v += amp * vnoise(p);
    p = R * p * 2.03 + vec2(7.7, 3.1);
    amp *= 0.55;
    v += amp * vnoise(p);
    p = R * p * 2.07 + vec2(1.9, 9.4);
    amp *= 0.55;
    v += amp * vnoise(p);
    return v;
}

void main() {
    // RGBA atlas: solid-colour rects sample the white pixel at
    // UV(0,0) -> (1,1,1,1); font glyphs are stored as
    // (1,1,1, mask); icons store full RGBA. Multiplying by the
    // vertex colour lets callers tint glyphs/icons or fill rects
    // with arbitrary colours through the same path.
    //
    // Sentinel UV.x < 0 marks "noisy" geometry (textured stone
    // panels, forge-iron buttons): skip the atlas sample and
    // modulate the per-vertex colour with a soft cloud noise
    // field driven by gl_FragCoord. Keeping the noise in
    // pixel space means it doesn't crawl when the surface
    // resizes — each pixel always evaluates to the same grain.

    // ── Glow-disc sentinel (UV in [-2.0, -1.1]) ────────────
    //
    // Stoney bevelled disc with a dark-centre → tint-rim
    // radial gradient and a shader-rasterised glow halo
    // outside the solid disc.
    //
    // The disc emits UVs in [-2.0, -1.1] so the gating check
    // can clear the -1.0 sentinel boundary by a wide margin
    // — under rasteriser interpolation a vertex at exactly
    // -1.001 produces pixel UVs that overshoot to -1.0 along
    // the corner diagonal, leaking into the atlas-sampling
    // path and rendering as solid white strips next to the
    // node. The 0.1-wide guard band fixes that.
    //
    // `fragColor.rgb` is the node tint. `fragColor.a` is the
    // master *brightness* multiplier (gating dimming). The
    // disc is rendered fully opaque so edges drawn behind
    // it don't bleed through.
    if (fragUV.x >= -2.0 && fragUV.x <= -1.1) {
        // Map UV [-2.0, -1.1] linearly to local [0, 1].
        vec2 lp = (fragUV - vec2(-2.0)) / 0.9;
        vec2 c = (lp - 0.5) * 2.0;
        float r = length(c);
        // Solid disc occupies r in [0, CORE_R]; glow halo
        // covers r in (CORE_R, 1.0].
        const float CORE_R = 0.65;

        if (r > 1.0) {
            discard;
        }

        vec3 tint = fragColor.rgb;
        float bright = fragColor.a;          // gating dim factor

        if (r > CORE_R) {
            // ── Outer glow halo ──
            float t = (r - CORE_R) / (1.0 - CORE_R);
            float halo = exp(-t * 3.4);
            float aa = 1.0 - smoothstep(0.96, 1.0, r);
            outColor = vec4(tint * halo * bright, halo * bright * aa * 0.85);
            return;
        }

        // ── Solid disc: tint centre → dark rim ──
        // Bright node-tint core blooms outward and fades to a
        // near-black ring at the silhouette. `pow(rn, 2.2)`
        // keeps the centre saturated across the inner ~70%
        // of the disc; only the outer third darkens, which
        // reads as an emissive gem set into a stone bezel.
        float rn = r / CORE_R;                          // [0, 1]
        float fade = pow(rn, 2.2);
        vec3 deep = vec3(0.015, 0.015, 0.02);
        // Brighten the centre tint slightly so it clearly
        // wins against the dark rim.
        vec3 core_tint = tint * 1.15;
        vec3 col = mix(core_tint, deep, fade);

        // Thin stone highlight at the very edge — a 1-2 px
        // bright lip that gives the chip its 3D stamped feel
        // without overwhelming the dark rim band.
        vec2 nrm = (r > 1e-3) ? c / r : vec2(0.0, -1.0);
        float light = dot(nrm, normalize(vec2(-0.55, -0.83)));
        float lip = smoothstep(0.88, 0.94, rn) * (1.0 - smoothstep(0.96, 0.99, rn));
        float hi = max(0.0, light);
        vec3 stone_hi = vec3(0.55, 0.52, 0.48);
        col = mix(col, stone_hi, lip * hi * 0.55);

        // Subtle stone grain across the whole core.
        float n = fbm3(gl_FragCoord.xy * (1.0 / 14.0));
        col *= 1.0 + (n - 0.5) * 0.10;

        // Thin near-black outline at the very rim.
        float outline = smoothstep(0.96, 1.0, rn);
        col = mix(col, vec3(0.01, 0.01, 0.015), outline * 0.92);

        // Gating dim: multiply the resolved colour by the
        // brightness factor. Alpha stays opaque (except AA
        // edge) so the disc reliably hides edges drawn
        // behind it.
        col *= bright;
        float disc_aa = 1.0 - smoothstep(0.985, 1.0, rn);
        outColor = vec4(clamp(col, 0.0, 1.0), disc_aa);
        return;
    }

    // ── Glow-line sentinel (UV in [-3.0, -2.1]) ────────────
    if (fragUV.x >= -3.0 && fragUV.x <= -2.1) {
        // Map UV [-3.0, -2.1] → across-axis [0, 1].
        float across = (fragUV.x - (-3.0)) / 0.9;
        float d = abs(across - 0.5) * 2.0;
        vec3 tint = fragColor.rgb;
        float bright = fragColor.a;
        float halo = exp(-d * 3.0);
        float core_light = smoothstep(0.35, 0.0, d);
        vec3 col = mix(tint, vec3(1.0), core_light * 0.75);
        outColor = vec4(col * halo * bright, halo * bright);
        return;
    }

    if (fragUV.x < 0.0) {
        // Domain-warped fbm: sample the noise twice and offset
        // the second sample by the first's vector field. The
        // warp shears the lattice in soft swirls so the tile
        // structure of plain fbm disappears — visually reads
        // as wet-smudged stone rather than a repeating bumpmap.
        // Base period stays ~52 px so the smudges are broad
        // ribbons, not pixel grain.
        vec2 p = gl_FragCoord.xy * (1.0 / 52.0);
        // Warp field: two fbm samples offset to act as a 2D
        // gradient. Multiply by 1.4 to give the smudge real
        // travel — small values look like noise still tiles,
        // bigger values fully break the lattice.
        vec2 q = vec2(fbm3(p), fbm3(p + vec2(5.2, 1.3)));
        float n = fbm3(p + 1.4 * q);
        // Centre on 0 and squeeze. ±18 % brightness modulation
        // keeps the gradient readable while giving the surface
        // a real cloudy / smudged quality.
        float m = (n - 0.5) * 0.36;
        vec3 rgb = clamp(fragColor.rgb * (1.0 + m), 0.0, 1.0);
        outColor = vec4(rgb, fragColor.a);
        return;
    }
    vec4 tex = texture(fontAtlas, fragUV);
    outColor = fragColor * tex;
}
