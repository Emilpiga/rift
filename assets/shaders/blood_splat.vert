#version 450

// Per-instance splat data, mirrored from `SplatInstance` in
// `crates/rift-engine/src/renderer/blood.rs`. Each instance draws a
// single rotated quad into the floor's blood field.
//
//   center_time_intensity:
//     xy = uv center in the blood field [0..1]
//     z  = spawn time (seconds since renderer start)
//     w  = wet intensity at spawn (0..1)
//
//   size_rot_slice:
//     x = uv half-size along major axis
//     y = aspect ratio (minor / major); 1 = round, <1 = elongated
//     z = rotation in radians (XZ-plane orientation)
//     w = atlas slice index (cast to float; floor() at sample time)

layout(location = 0) in vec4 in_center_time_intensity;
layout(location = 1) in vec4 in_size_rot_slice;

// Built-in `gl_VertexIndex` drives a static unit quad ([-1,1]^2 with
// origin-centred UVs in [0,1]) so we don't need a vertex buffer for
// the quad geometry; only the per-instance attributes are streamed.
layout(location = 0) out vec2 v_atlas_uv;
layout(location = 1) out float v_intensity;
layout(location = 2) out float v_time;
layout(location = 3) out float v_atlas_slice;

void main() {
    // Quad corners in [-1, 1]^2, expanded into UVs in [0, 1] for the
    // atlas sample.
    vec2 quad[4] = vec2[4](
        vec2(-1.0, -1.0),
        vec2( 1.0, -1.0),
        vec2(-1.0,  1.0),
        vec2( 1.0,  1.0)
    );
    vec2 corner = quad[gl_VertexIndex];

    // Build the local-space offset (in field UV units), accounting for
    // aspect along the minor axis.
    vec2 local = corner * vec2(1.0, in_size_rot_slice.y);

    // Rotate by `rotation` so motion-aligned splats orient along the
    // impact direction.
    float c = cos(in_size_rot_slice.z);
    float s = sin(in_size_rot_slice.z);
    vec2 rotated = vec2(c * local.x - s * local.y,
                        s * local.x + c * local.y);

    // Scale by half-size and translate to the splat centre.
    vec2 uv = in_center_time_intensity.xy + rotated * in_size_rot_slice.x;

    // The render pass clips to [0,1] in UV space, which maps to
    // [-1,1] in NDC. We don't flip Y because the field has no fixed
    // up direction; both the splat pass and the forward sampler
    // agree on the same UV → world XZ orientation.
    gl_Position = vec4(uv * 2.0 - 1.0, 0.0, 1.0);

    // Forward atlas sample location (atlas UV = the corner before
    // world rotation, since the mask should rotate with the splat).
    v_atlas_uv = corner * 0.5 + 0.5;
    v_intensity = in_center_time_intensity.w;
    v_time = in_center_time_intensity.z;
    v_atlas_slice = in_size_rot_slice.w;
}
