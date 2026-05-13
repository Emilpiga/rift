#version 450

// Procedural three-band sky dome (zenith / horizon / ground) plus a
// soft sun disc. Designed to be cheap and stylized — no atmospheric
// scattering, just a vertical gradient with a directional highlight.
//
// All parameters arrive as push constants so the renderer can swap
// presets per-biome (sunny meadow for the hub, near-black for the
// dungeons) without touching descriptor sets or rebuilding pipelines.

layout(location = 0) in vec2 v_ndc;
layout(location = 0) out vec4 outColor;

layout(push_constant) uniform Push {
    // Camera inverse view-projection with translation removed —
    // mapping NDC (xy, z=1) to a world-space *direction* from the
    // camera. The CPU pre-strips the translation so the sky stays
    // at infinity regardless of camera position.
    mat4  inv_view_proj_dir;
    // rgb = zenith colour (top of dome).
    // a   = horizon falloff exponent (1 = linear, 4 = tight horizon).
    vec4  zenith_falloff;
    // rgb = horizon colour (band around y=0).
    // a   = sun cosine threshold (cos of angular radius). 0.999 ≈
    //       small disc; 0.95 ≈ huge sun.
    vec4  horizon_sun_size;
    // rgb = ground colour (used below the horizon).
    // a   = sun strength multiplier. 0 disables the sun.
    vec4  ground_sun_str;
    // xyz = direction toward the sun (normalised).
    // w   = unused.
    vec4  sun_dir;
    // x   = global time in seconds (drives cloud advection).
    // y   = cloud strength (0 = clear sky, 1 = dense storm).
    // z   = lightning flash intensity (0..3+, lifts cloud
    //       brightness toward the bolt colour for a frame).
    // w   = unused.
    vec4  cloud_params;
    // rgb = lightning bolt colour (cool blue-white or hellfire
    //       amber). Only meaningful when cloud_params.z > 0.
    // a   = unused.
    vec4  cloud_flash_color;
} pc;

// ────────────────────────────────────────────────────────────────
// Hash + value-noise helpers — cheap, deterministic, no textures.
// ────────────────────────────────────────────────────────────────
float hash21(vec2 p) {
    p = fract(p * vec2(123.34, 456.21));
    p += dot(p, p + 45.32);
    return fract(p.x * p.y);
}

float vnoise2(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    // Smooth step (Hermite) — keeps the noise C1 continuous so
    // the cloud silhouettes don't show grid bands.
    vec2 u = f * f * (3.0 - 2.0 * f);
    float a = hash21(i);
    float b = hash21(i + vec2(1.0, 0.0));
    float c = hash21(i + vec2(0.0, 1.0));
    float d = hash21(i + vec2(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// 4-octave fbm. Cheap enough for a fullscreen pass and produces
// the lumpy cumulonimbus profile we want for storm clouds.
float fbm2(vec2 p) {
    float v = 0.0;
    float a = 0.5;
    for (int i = 0; i < 4; ++i) {
        v += a * vnoise2(p);
        p *= 2.07;   // non-power-of-two avoids axis-aligned grain
        a *= 0.5;
    }
    return v;
}

void main() {
    // NDC + z=1 -> world direction.
    vec4 clip  = vec4(v_ndc, 1.0, 1.0);
    vec4 world = pc.inv_view_proj_dir * clip;
    vec3 dir   = normalize(world.xyz);

    // Vertical blend. h is +1 looking straight up, -1 straight down.
    float h = dir.y;

    vec3 sky;
    if (h >= 0.0) {
        // Above horizon: horizon -> zenith with a tunable falloff.
        // Higher exponent = sharper transition right above the
        // skyline (think "low haze ring with deep blue overhead").
        float t = pow(clamp(h, 0.0, 1.0), 1.0 / max(pc.zenith_falloff.a, 0.01));
        sky = mix(pc.horizon_sun_size.rgb, pc.zenith_falloff.rgb, t);
    } else {
        // Below horizon: horizon -> ground (only visible if the
        // floor mesh ever opens out, e.g. the hub apron). Square-
        // root keeps the band tight to the horizon line.
        float t = pow(clamp(-h, 0.0, 1.0), 0.5);
        sky = mix(pc.horizon_sun_size.rgb, pc.ground_sun_str.rgb, t);
    }

    // Sun disc — just a soft circle of warm white scaled by
    // `sun_strength`. The threshold lives in horizon_sun_size.a
    // (0..1, cos of angular radius); a tiny smoothstep softens
    // the edge so it doesn't pixelate.
    float cos_sun  = dot(dir, normalize(pc.sun_dir.xyz));
    float thr      = pc.horizon_sun_size.a;
    float core     = smoothstep(thr,                thr + 0.0010, cos_sun);
    // A wider, dimmer corona around the disc adds weight without
    // washing the rest of the sky. Only kicks in when the strength
    // is non-trivial — gameplay can disable the sun by setting a
    // strength of zero.
    float corona   = smoothstep(thr - 0.040, thr,                cos_sun);
    float strength = pc.ground_sun_str.a;
    sky += core   * strength * vec3(1.00, 0.95, 0.80);
    sky += corona * strength * 0.25 * vec3(1.00, 0.85, 0.65);

    // Low-angle storm glow. The gameplay camera mostly exposes
    // the horizon / below-horizon dome rather than the overhead
    // cloud sheet, so storm presets get a broad animated ember
    // band that sits where platform edges and distant silhouettes
    // actually meet the sky.
    float storm = clamp(pc.cloud_params.y, 0.0, 1.0);
    if (storm > 0.001) {
        float lowBand = smoothstep(-0.80, -0.12, h)
                      * (1.0 - smoothstep(0.18, 0.55, h));
        if (lowBand > 0.001) {
            vec2 xz = dir.xz / max(length(dir.xz), 1e-4);
            float theta = atan(xz.y, xz.x);
            float t = pc.cloud_params.x;
            vec2 smokeUV = vec2(theta * 1.8 + t * 0.035,
                                h * 4.5 - t * 0.025);
            float smoke = fbm2(smokeUV);
            float ember = pow(max(0.0, smoke - 0.52), 2.0) * 3.0;

            vec3 haze = pc.horizon_sun_size.rgb * (0.14 + smoke * 0.18);
            vec3 hot = pc.horizon_sun_size.rgb * 1.70
                     + pc.zenith_falloff.rgb * 0.25;
            sky += (haze + hot * ember) * lowBand * storm;
        }
    }

    // ─── Procedural storm clouds ────────────────────────────────
    // Only meaningful when cloud_strength > 0. Two horizontally-
    // advected fbm layers stacked over the upper hemisphere give
    // the dome a roiling cumulonimbus feel without any texture.
    // Costs four fbm calls per fragment — sky is a fullscreen
    // triangle without depth so this is cheap.
    float cloud_strength = pc.cloud_params.y;
    if (cloud_strength > 0.001 && h > -0.05) {
        // Project the view direction onto a planar "cloud sheet"
        // sitting at altitude 1. Adding a small bias to `h` keeps
        // samples well-defined as we approach the horizon (the
        // 1/h projection diverges otherwise).
        float ph = max(h + 0.08, 0.08);
        vec2 plane = dir.xz / ph;

        float t = pc.cloud_params.x;
        // Frequency multipliers control puff size on the cloud
        // sheet. The previous 0.55 / 1.10 produced a single
        // huge fbm cell across the visible dome — the sky read
        // as one slow blob, not "clouds". Bumping these to
        // 4.0 / 9.0 puts ~tens of recognisable puffs across
        // the field of view at typical camera angles. Slow
        // lower deck — the brooding mass.
        vec2 q1 = plane * 4.0 + vec2( 0.012, 0.005) * t;
        float n1 = fbm2(q1);
        // Faster upper wisps — a second layer that drags
        // perpendicularly across the lower deck so the
        // silhouette looks like it's churning rather than
        // scrolling.
        vec2 q2 = plane * 9.0 + vec2(-0.025, 0.018) * t;
        float n2 = fbm2(q2);
        float cloud_n = n1 * 0.65 + n2 * 0.35;

        // Stretch toward the horizon — clouds sit higher overhead
        // and densify near the horizon line so the band feels
        // like an oncoming storm wall. `pow(h, …)` softly fades
        // the layer out at the zenith.
        float horizon_density = mix(1.0, 0.55, smoothstep(0.0, 0.95, h));
        // Lower the bias so 50%-ish noise actually reads as cloud
        // rather than barely-visible static. The previous −0.30
        // bias combined with the dark cloud body made the layer
        // visually disappear against the already-dark abyss
        // horizon.
        float density = clamp(cloud_n * horizon_density - 0.18, 0.0, 1.0);
        // Soft remap so values past ~0.45 fully saturate to opaque
        // cloud, and values below ~0.05 read as clear sky.
        float coverage = smoothstep(0.04, 0.45, density) * cloud_strength;

        // ── God rays through cloud gaps ────────────────────
        // Where the line of sight passes near the sun
        // direction *and* cloud coverage is thin at that
        // point, lift the sky brightness toward the sun
        // colour so the dome reads as having a hot light
        // source punching through gaps in the cover. Cheap
        // approximation — we don't ray-march; we just use
        // the same fbm sample (which is constant along the
        // sun direction, but the coverage falloff via the
        // dot product against `sun_dir` does most of the
        // visual work).
        float sunAlign = max(0.0, cos_sun);
        // Tight cone around the sun — readable bloom only
        // within ~25° of sun direction.
        float rayCone = pow(sunAlign, 28.0);
        // Gate by inverse coverage so the bloom only fires
        // in cloud gaps. A heavily-covered sun reads as
        // overcast; a thinly-covered sun reads as a god-ray
        // burst.
        float gapMask = 1.0 - smoothstep(0.20, 0.80, coverage);
        // Warm bloom colour — biased toward the horizon /
        // dust palette so it ties into the dome without
        // looking like a separate spotlight overlay.
        vec3 bloom = pc.horizon_sun_size.rgb * 1.6
                   + vec3(0.40, 0.20, 0.05);
        sky += bloom * rayCone * gapMask * 0.85;

        // Cloud body colour: a dark/bright pair derived from
        // the biome's `horizon` and `zenith` colours so the
        // cloud layer always tints to the dome it sits in.
        // For a crimson storm sky this gives slate-with-warm-
        // rim cumulonimbus; for a sandstorm dome it gives the
        // tan dust streaks the rest of the palette demands —
        // no per-biome shader branch needed.
        //
        // Dark side: a dim shadowed mix of the two band
        // colours, biased toward the zenith so the underside
        // of the cloud reads like sky behind it.
        // Lit side: a saturated lift of the horizon colour
        // so the puff tops catch the dominant biome warmth.
        vec3 cloud_dark = mix(pc.zenith_falloff.rgb,
                              pc.horizon_sun_size.rgb, 0.30) * 0.55;
        vec3 cloud_lit  = pc.horizon_sun_size.rgb * 1.25
                        + pc.zenith_falloff.rgb * 0.10;
        // Sun-side rim on the cloud puff tops — clouds in
        // the sun's angular vicinity get a brighter lit side
        // so the cover reads as front-lit by the sun rather
        // than uniformly self-illuminated.
        vec3 cloud_color = mix(cloud_dark, cloud_lit,
                               smoothstep(0.30, 0.85, n1));
        cloud_color += pc.horizon_sun_size.rgb * rayCone * 0.40;

        // Lightning flash: lift cloud brightness sharply toward
        // the bolt colour while a strike is firing. Per-fragment
        // hash modulates the flash intensity so the flash reads
        // as a fork sweeping through the cloud rather than a
        // uniform fade.
        float flash = pc.cloud_params.z;
        if (flash > 0.0) {
            float fork = mix(0.4, 1.0, hash21(plane * 1.7));
            cloud_color += pc.cloud_flash_color.rgb * (flash * 0.6 * fork);
        }

        sky = mix(sky, cloud_color, coverage);
    }

    outColor = vec4(sky, 1.0);
}
