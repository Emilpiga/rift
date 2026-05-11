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
