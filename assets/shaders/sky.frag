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
} pc;

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

    outColor = vec4(sky, 1.0);
}
