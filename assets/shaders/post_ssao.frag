#version 450

// Post graph SSAO node. Reads the resolved depth buffer and writes a
// single-channel ambient-occlusion visibility term for the final composite.

layout(location = 0) in  vec2 v_uv;
layout(location = 0) out vec4 outColor;

layout(set = 0, binding = 0) uniform sampler2D u_depth;

layout(push_constant) uniform Push {
    mat4  inv_proj;
    float strength;
} pc;

const float PI = 3.14159265359;

vec3 view_pos_from_depth(vec2 uv, float depth) {
    vec4 clip = vec4(uv * 2.0 - 1.0, depth, 1.0);
    vec4 view = pc.inv_proj * clip;
    return view.xyz / view.w;
}

float hash12(vec2 p) {
    return fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453);
}

float compute_ssao(vec2 uv, float depth) {
    if (depth >= 0.9999) return 1.0;

    vec3 origin = view_pos_from_depth(uv, depth);
    if (origin.z >= -0.001) return 1.0;

    vec3 nrm = normalize(cross(dFdx(origin), dFdy(origin)));
    if (dot(nrm, vec3(0.0, 0.0, 1.0)) < 0.0) nrm = -nrm;

    const float WORLD_RADIUS = 0.10;
    float radius_uv = WORLD_RADIUS / max(-origin.z, 0.1) * 0.5;

    float rot = hash12(uv * vec2(textureSize(u_depth, 0))) * 2.0 * PI;
    float cr = cos(rot), sr = sin(rot);
    mat2 rotate = mat2(cr, -sr, sr, cr);

    const int N = 4;
    const float GOLDEN = 2.39996323;
    float occlusion = 0.0;

    for (int i = 0; i < N; ++i) {
        float fi = float(i) + 0.5;
        float r = sqrt(fi / float(N));
        float theta = fi * GOLDEN;
        vec2 disk = vec2(cos(theta), sin(theta)) * r;
        vec2 offset = rotate * disk * radius_uv;
        vec2 sample_uv = clamp(uv + offset, vec2(0.001), vec2(0.999));

        float sample_depth = texture(u_depth, sample_uv).r;
        if (sample_depth >= 0.9999) continue;
        vec3 sample_pos = view_pos_from_depth(sample_uv, sample_depth);

        vec3 v = sample_pos - origin;
        float dist = length(v);
        if (abs(sample_pos.z - origin.z) > WORLD_RADIUS * 1.5) continue;

        float range = smoothstep(WORLD_RADIUS * 1.4, WORLD_RADIUS * 0.05, dist);
        float ndotv = max(dot(nrm, v / max(dist, 0.0001)), 0.0);
        const float BIAS = 0.015;
        occlusion += step(BIAS, ndotv) * ndotv * range;
    }

    occlusion /= float(N);
    return clamp(1.0 - mix(occlusion, sqrt(occlusion), 0.7), 0.0, 1.0);
}

void main() {
    float ao = 1.0;
    if (pc.strength > 0.0001) {
        float depth = texture(u_depth, v_uv).r;
        ao = mix(1.0, compute_ssao(v_uv, depth), clamp(pc.strength, 0.0, 1.0));
    }
    outColor = vec4(ao, 0.0, 0.0, 1.0);
}
