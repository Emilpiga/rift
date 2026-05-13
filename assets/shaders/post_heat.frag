#version 450

// Post graph heat-distortion node. Reads HDR and writes a full-resolution
// warped HDR image for the final composite. Bloom and volumetrics continue
// to read the unwarped HDR scene, matching the old composite behavior.

layout(location = 0) in  vec2 v_uv;
layout(location = 0) out vec4 outColor;

layout(set = 0, binding = 0) uniform sampler2D u_hdr;

layout(push_constant) uniform Push {
    vec4 heat_source;
} pc;

float hash12(vec2 p) {
    return fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453);
}

void main() {
    vec2 hdr_uv = v_uv;

    if (pc.heat_source.w > 0.001) {
        vec2 d = (v_uv - pc.heat_source.xy);
        d.x *= 1.6;
        float r = length(d);
        float radius = max(pc.heat_source.z, 0.01);
        float falloff = exp(-(r * r) / (radius * radius));

        vec2 nUV = v_uv * 28.0;
        float t = pc.heat_source.w * 6.0;
        float n0 = hash12(nUV + vec2(t, 0.0));
        float n1 = hash12(nUV + vec2(0.0, t * 1.3));
        vec2 warp = vec2(n0 - 0.5, n1 - 0.5) * 2.0;
        warp += vec2(hash12(nUV + vec2(t + 7.0, 3.0)) - 0.5,
                     hash12(nUV + vec2(11.0, t * 0.9 + 5.0)) - 0.5) * 2.0;
        warp *= 0.5;

        float amp = 0.012 * falloff * pc.heat_source.w;
        hdr_uv += warp * amp;
    }

    outColor = vec4(texture(u_hdr, hdr_uv).rgb, 1.0);
}
