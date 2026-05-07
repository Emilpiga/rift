#version 450

// Separable Gaussian blur. `direction` selects horizontal or
// vertical; the same shader is used by both blur passes. Nine
// taps total (1 centre + 4 each side) using 5 linear-sample
// fetches via the standard "lerp two texels with one fetch"
// trick — gives 9-tap quality for 5 fetches.

layout(location = 0) in  vec2 v_uv;
layout(location = 0) out vec4 outColor;

layout(set = 0, binding = 0) uniform sampler2D u_input;

layout(push_constant) uniform Push {
    vec2 texel_size;   // 1 / textureSize(u_input)
    vec2 direction;    // (1, 0) horizontal | (0, 1) vertical
} pc;

// Pre-computed 9-tap Gaussian (sigma ~= 2). Offsets in texels.
const float OFFSETS[3] = float[3](0.0, 1.3846153846, 3.2307692308);
const float WEIGHTS[3] = float[3](0.2270270270, 0.3162162162, 0.0702702703);

void main() {
    vec3 sum = texture(u_input, v_uv).rgb * WEIGHTS[0];
    vec2 step = pc.texel_size * pc.direction;
    sum += texture(u_input, v_uv + step * OFFSETS[1]).rgb * WEIGHTS[1];
    sum += texture(u_input, v_uv - step * OFFSETS[1]).rgb * WEIGHTS[1];
    sum += texture(u_input, v_uv + step * OFFSETS[2]).rgb * WEIGHTS[2];
    sum += texture(u_input, v_uv - step * OFFSETS[2]).rgb * WEIGHTS[2];
    outColor = vec4(sum, 1.0);
}
