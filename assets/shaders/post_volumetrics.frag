#version 450

// Post graph volumetrics node. Reads HDR + depth and writes the
// screen-space god-ray contribution as HDR light for final composite.

layout(location = 0) in  vec2 v_uv;
layout(location = 0) out vec4 outColor;

layout(set = 0, binding = 0) uniform sampler2D u_hdr;
layout(set = 0, binding = 1) uniform sampler2D u_depth;

layout(push_constant) uniform Push {
    vec4 sun_screen;
    vec4 sun_color;
} pc;

float hash12(vec2 p) {
    return fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453);
}

vec3 god_rays(vec2 uv) {
    if (pc.sun_screen.z < 0.001) return vec3(0.0);
    if (pc.sun_screen.w < 0.5)   return vec3(0.0);

    vec2 sun_uv = pc.sun_screen.xy;
    vec2 to_sun = sun_uv - uv;
    float dist  = length(to_sun);
    const float MAX_DIST = 0.85;
    float marchDist = min(dist, MAX_DIST);
    if (marchDist < 1e-4) return vec3(0.0);
    vec2 dir = to_sun / max(dist, 1e-4);

    const int STEPS = 12;
    vec3 accum = vec3(0.0);
    float weightSum = 0.0;
    float jitter = hash12(uv * vec2(1023.0, 769.0));
    for (int i = 0; i < STEPS; i++) {
        float t = (float(i) + jitter) / float(STEPS);
        vec2 sample_uv = uv + dir * (marchDist * t);
        if (sample_uv.x < 0.0 || sample_uv.x > 1.0
         || sample_uv.y < 0.0 || sample_uv.y > 1.0) continue;

        float d = texture(u_depth, sample_uv).r;
        if (d < 0.9995) continue;
        float w = (1.0 - t);
        accum += texture(u_hdr, sample_uv).rgb * w;
        weightSum += w;
    }
    if (weightSum < 1e-4) return vec3(0.0);
    accum /= weightSum;

    float falloff = exp(-dist * 0.2);
    return accum * pc.sun_color.rgb * pc.sun_screen.z * falloff * 0.8;
}

void main() {
    outColor = vec4(god_rays(v_uv), 1.0);
}
