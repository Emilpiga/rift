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
    // w   = void depth strength. Used by rift floors to draw a
    //       procedural abyss below the horizon.
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

mat2 rot2(float a) {
    float c = cos(a);
    float s = sin(a);
    return mat2(c, -s, s, c);
}

float soft_band(float x, float centre, float width) {
    float aa = max(fwidth(x), 0.003);
    return 1.0 - smoothstep(width - aa, width + aa, abs(x - centre));
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

    // Procedural rift abyss. This only runs for presets that opt in
    // via cloud_params.w, and only below the horizon where the player
    // sees past the edge of a floating dungeon. Keep it broad and
    // coherent: warped depth bands plus broken highlights, not repeated
    // mathematical stripes.
    float void_strength = clamp(pc.cloud_params.w, 0.0, 1.0);
    if (void_strength > 0.001 && h < 0.08) {
        float t = -pc.cloud_params.x;
        float down = clamp((-h + 0.03) / 1.03, 0.0, 1.0);
        float depth = pow(down, 0.56);
        vec2 xz = dir.xz / max(length(dir.xz), 1e-4);
        float theta = atan(xz.y, xz.x);

        vec2 flow_dir = rot2(t * 0.011) * xz;
        vec2 p = flow_dir * mix(1.65, 4.2, depth) + vec2(7.13, -3.71);
        p = rot2(depth * 1.15 + t * 0.018) * p;
        vec2 warp = vec2(
            fbm2(p * 1.55 + vec2(t * 0.012, -1.7)),
            fbm2(rot2(0.83) * p * 1.55 + vec2(3.1, -t * 0.010))
        ) - 0.5;
        vec2 flow = p + warp * (0.72 + depth * 0.45);
        float body_a = fbm2(flow * 1.55 + vec2(0.0, -t * 0.018));
        float body_b = fbm2(rot2(1.19) * flow * 2.10 + vec2(4.2, t * 0.014));
        float body = mix(body_a, body_b, 0.38);
        float detail_a = fbm2(flow * 4.20 + vec2(5.2, t * 0.027));
        float detail_b = fbm2(rot2(0.51) * flow * 5.40 + vec2(-2.6, -t * 0.019));
        float detail = mix(detail_a, detail_b, 0.34);
        float ridged = 1.0 - abs(detail * 2.0 - 1.0);
        float warped_down = clamp(
            down + (body - 0.5) * 0.105 + (detail - 0.5) * 0.035 + sin(theta * 2.0 + t * 0.04) * 0.018,
            0.0,
            1.0
        );

        vec3 abyss_black = vec3(0.003, 0.0008, 0.006);
        vec3 theme_ocean = mix(pc.horizon_sun_size.rgb, pc.zenith_falloff.rgb, 0.32);
        float theme_peak = max(max(theme_ocean.r, theme_ocean.g), max(theme_ocean.b, 0.018));
        vec3 theme_chroma = clamp(theme_ocean / theme_peak, vec3(0.0), vec3(1.0));
        vec3 abyss_shadow = mix(pc.ground_sun_str.rgb, abyss_black, 0.72);
        vec3 abyss_mid = mix(pc.horizon_sun_size.rgb, pc.zenith_falloff.rgb, 0.28);
        vec3 abyss_hot = mix(abyss_mid, theme_ocean * 2.45 + theme_chroma * 0.10, 0.42);
        vec3 abyss_cold = mix(abyss_mid, mix(theme_ocean, vec3(0.035, 0.025, 0.060), 0.46), 0.38);

        float well = smoothstep(0.02, 0.95, warped_down);
        vec3 abyss_base = mix(abyss_mid, abyss_shadow, well);
        abyss_base *= mix(0.76, 0.13, smoothstep(0.24, 1.0, warped_down));

        float band_phase = depth * 12.0 + body * 2.7 + sin(theta * 2.0 + t * 0.06) * 0.35 - t * 0.10;
        float band_wave = 0.5 + 0.5 * sin(band_phase);
        float bands = soft_band(band_wave, 0.72, 0.22) * (0.35 + body * 0.65);
        bands *= smoothstep(0.06, 0.60, warped_down) * (1.0 - smoothstep(0.82, 1.0, warped_down));

        float undercurrent_wave = 0.5 + 0.5 * sin(depth * 5.4 + body * 2.2 - t * 0.045);
        float undercurrent = soft_band(undercurrent_wave, 0.64, 0.30)
                           * smoothstep(0.22, 0.90, warped_down);

        float filament_wave = 0.5 + 0.5 * sin(theta * 4.0 + depth * 7.5 + body * 4.0 - t * 0.12);
        float filaments = soft_band(filament_wave, 0.78, 0.10)
                        * smoothstep(0.58, 0.86, ridged)
                        * smoothstep(0.14, 0.62, warped_down)
                        * (1.0 - smoothstep(0.70, 1.0, warped_down));

        float core_pulse = smoothstep(0.56, 0.86, body + ridged * 0.22);
        core_pulse *= smoothstep(0.42, 1.0, warped_down) * (0.55 + 0.45 * sin(t * 0.55 + body * 3.0));

        float throat = smoothstep(0.58, 1.0, warped_down);
        float throat_core = smoothstep(0.76, 1.0, warped_down) * (0.82 + body * 0.18);

        float spiral_phase = theta * 3.0 + depth * 18.0 + body * 3.5 - t * 0.26;
        float spiral_wave = 0.5 + 0.5 * sin(spiral_phase);
        float liquid_sheets = soft_band(spiral_wave, 0.78, 0.12)
                            * smoothstep(0.18, 0.94, warped_down)
                            * (1.0 - smoothstep(0.96, 1.0, warped_down))
                            * (0.45 + ridged * 0.55);
        float liquid_gloss = soft_band(0.5 + 0.5 * sin(spiral_phase + detail * 4.0 + 1.4), 0.86, 0.055)
                           * liquid_sheets;

        float ocean_depth = clamp(warped_down + (body - 0.5) * 0.08, 0.0, 1.0);
        float ocean_mask_raw = smoothstep(0.015, 0.26, ocean_depth)
                             * (1.0 - smoothstep(0.34, 0.82, ocean_depth));
        float ocean_mask = ocean_mask_raw * ocean_mask_raw * (3.0 - 2.0 * ocean_mask_raw);
        float ocean_wave = 0.5 + 0.5 * sin(
            flow.x * 3.4 + flow.y * 0.85 + body * 3.0 + dot(flow_dir, vec2(1.7, -0.9)) - t * 0.11
        );
        float ocean_swell = soft_band(ocean_wave, 0.54, 0.34) * (0.55 + body * 0.45);
        float ocean_trough = (1.0 - smoothstep(0.30, 0.74, ocean_wave))
                           * ocean_mask
                           * (0.45 + (1.0 - body) * 0.55);
        float ocean_spec = soft_band(ocean_wave, 0.82, 0.085)
                         * smoothstep(0.58, 0.90, ridged)
                         * ocean_mask
                         * (1.0 - smoothstep(0.24, 0.64, ocean_depth));
        float foam_noise = fbm2(rot2(0.37) * flow * 8.5 + vec2(t * 0.045, -2.8));
        float crest_foam = soft_band(ocean_wave + (foam_noise - 0.5) * 0.18, 0.86, 0.060)
                         * smoothstep(0.54, 0.88, ridged)
                         * ocean_mask
                         * (1.0 - smoothstep(0.30, 0.62, ocean_depth));
        float shear_foam = soft_band(spiral_wave + (foam_noise - 0.5) * 0.16, 0.80, 0.070)
                         * smoothstep(0.20, 0.58, warped_down)
                         * (1.0 - smoothstep(0.66, 0.92, warped_down))
                         * smoothstep(0.50, 0.82, ridged);
        float foam = clamp((crest_foam + shear_foam * 0.65) * smoothstep(0.48, 0.82, foam_noise), 0.0, 1.0);

        vec3 abyss = abyss_base;
        vec3 ocean_theme = mix(abyss_mid, abyss_hot, 0.36);
        vec3 ocean_body = ocean_theme * (0.70 + ocean_swell * 0.20);
        ocean_body = mix(ocean_body, abyss_shadow, ocean_trough * 0.52);
        abyss = mix(abyss, ocean_body, ocean_mask * 0.28);
        float sink = smoothstep(0.56, 1.0, warped_down) * (0.72 + body * 0.28);
        float ink = smoothstep(0.14, 0.90, warped_down) * (1.0 - smoothstep(0.50, 0.96, body));
        ink += smoothstep(0.52, 1.0, warped_down) * smoothstep(0.18, 0.72, ridged) * 0.35;
        ink = clamp(ink, 0.0, 1.0);
        abyss *= mix(1.0, 0.46, sink);
        abyss = mix(abyss, abyss_black, ink * 0.42);
        abyss = mix(abyss, abyss_shadow * 0.34, throat_core * 0.66);

        float warm_tint = clamp(
            undercurrent * 0.32 + filaments * 0.38 + liquid_sheets * 0.24 + foam * 0.18 + body * 0.12,
            0.0,
            1.0
        );
        float cold_tint = clamp(bands * 0.24 + core_pulse * 0.34 + throat * 0.16 + (1.0 - body) * 0.10, 0.0, 1.0);
        vec3 tint_target = mix(abyss, abyss_cold, cold_tint * 0.14);
        tint_target = mix(tint_target, abyss_hot, warm_tint * 0.12);
        abyss = mix(abyss, tint_target, 0.36);

        vec3 lift_color = mix(abyss_mid, pc.zenith_falloff.rgb, 0.22);
        float flow_lift = undercurrent * 0.030
                + bands * 0.066
                + filaments * 0.036
                + liquid_sheets * 0.062
                + liquid_gloss * 0.018
                        + ocean_spec * 0.080
                + foam * 0.090
                + core_pulse * 0.052
                + smoothstep(0.64, 1.0, ridged) * 0.028;
        abyss += lift_color * flow_lift;
        abyss += lift_color * ocean_spec * 0.050;
        abyss += lift_color * foam * 0.070;

        vec3 depth_grade = mix(abyss_mid, abyss_shadow, smoothstep(0.14, 0.94, warped_down));
        abyss = mix(abyss, depth_grade + (abyss - depth_grade) * 0.54, 0.52);
        float abyss_luma = max(dot(abyss, vec3(0.2126, 0.7152, 0.0722)), 0.0001);
        float grade_luma = max(dot(depth_grade, vec3(0.2126, 0.7152, 0.0722)), 0.0001);
        vec3 unified_hue = depth_grade * (abyss_luma / grade_luma);
        float seam_suppress = smoothstep(0.12, 0.82, ocean_mask)
                            + smoothstep(0.16, 0.70, foam)
                            + smoothstep(0.12, 0.70, ocean_spec);
        seam_suppress = clamp(seam_suppress, 0.0, 1.0);
        float hue_unify = 0.34 + smoothstep(0.12, 0.88, warped_down) * 0.20 + seam_suppress * 0.16;
        abyss = mix(abyss, unified_hue, hue_unify);

        float horizon_keep = 1.0 - smoothstep(-0.01, 0.22, h + 0.02 + (body - 0.5) * 0.035);
        float mask = void_strength * horizon_keep * smoothstep(0.0, 0.18, warped_down);
        sky = mix(sky, abyss, mask);
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
