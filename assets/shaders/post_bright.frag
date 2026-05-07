#version 450

// Bright-pass extract: read HDR scene, output only the energy
// above `threshold` with a soft-knee falloff so we don't get
// hard-edged bloom rings.

layout(location = 0) in  vec2 v_uv;
layout(location = 0) out vec4 outColor;

layout(set = 0, binding = 0) uniform sampler2D u_hdr;

layout(push_constant) uniform Push {
    float threshold;   // brightness cutoff in linear HDR
    float soft_knee;   // softness of the cutoff (0..1)
    float _pad0;
    float _pad1;
} pc;

// Karis-style soft-knee threshold (a.k.a. "soft bloom curve")
// — same shape used by Unreal/Unity. `b` controls the curve
// width either side of `t`.
vec3 soft_threshold(vec3 c, float t, float k) {
    float br = max(c.r, max(c.g, c.b));
    float knee = t * k + 1.0e-5;
    float s = clamp(br - t + knee, 0.0, 2.0 * knee);
    s = s * s / (4.0 * knee + 1.0e-5);
    float contribution = max(s, br - t) / max(br, 1.0e-5);
    return c * contribution;
}

void main() {
    vec3 hdr = texture(u_hdr, v_uv).rgb;
    vec3 bright = soft_threshold(hdr, pc.threshold, pc.soft_knee);
    outColor = vec4(bright, 1.0);
}
