#version 450

// VFX ribbon: camera-aligned segmented strip expanded from
// (origin, tip, width). Each instance produces a lengthwise strip;
// inCorner carries cross positions in [-0.5..0.5] and segment
// positions in [0..1].
//
// Coordinate convention:
//   inCorner.x in [-0.5, 0.5]  → cross-axis (perpendicular to beam)
//   inCorner.y in [ 0.0, 1.0]  → along-axis (origin → tip)
//
// We generate the quad on the fly in world space using the camera
// right-vector projected perpendicular to the beam direction so
// the ribbon always faces the camera but never tilts off the
// beam.

layout(binding = 0) uniform UniformData {
    mat4 view;
    mat4 proj;
    vec4 cameraPos;
    vec4 lightDir;
    vec4 lightColor;
    vec4 fogColor;
    vec4 fogParams;
    vec4 fogOrigin;
    vec4 pointLightPos[16];
    vec4 pointLightColor[16];
    vec4 pointLightCount;
} ubo;

// Per-vertex quad corner (binding 0).
layout(location = 0) in vec2 inCorner;

// Per-instance ribbon data (binding 1). Layout must match
// VfxRibbonInstance in runtime.rs exactly.
layout(location = 1) in vec4 inOrigin;       // xyz = origin, w = width
layout(location = 2) in vec4 inTip;          // xyz = tip,    w = effect time
layout(location = 3) in vec4 inParams;       // brightness, noise_strength, noise_scroll, noise_tile
layout(location = 4) in vec4 inFlags;        // octaves, _, _, _

// Cross gradient: 8 stops × vec4. Locations 5..12.
layout(location = 5)  in vec4 inCross0;
layout(location = 6)  in vec4 inCross1;
layout(location = 7)  in vec4 inCross2;
layout(location = 8)  in vec4 inCross3;
layout(location = 9)  in vec4 inCross4;
layout(location = 10) in vec4 inCross5;
layout(location = 11) in vec4 inCross6;
layout(location = 12) in vec4 inCross7;

// Length gradient: 4 stops × vec4. Locations 13..16.
layout(location = 13) in vec4 inLen0;
layout(location = 14) in vec4 inLen1;
layout(location = 15) in vec4 inLen2;
layout(location = 16) in vec4 inLen3;

layout(location = 0) out vec2 vUv;            // u = cross [0..1], v = length [0..1]
layout(location = 1) out vec4 vParams;
layout(location = 2) out float vTime;
// Pass the gradient stops through to the fragment shader. They're
// per-instance and constant across the quad, so flat is fine and
// avoids the vertex shader cost of touching them.
layout(location = 3)  flat out vec4 vCross[8];
layout(location = 11) flat out vec4 vLength[4];
layout(location = 15) flat out vec4 vFlags;

void main() {
    vec3 origin = inOrigin.xyz;
    vec3 tip    = inTip.xyz;
    float width = inOrigin.w;
    float time  = inTip.w;

    vec3 along  = tip - origin;
    float len   = length(along);
    vec3 axis   = (len > 1e-5) ? along / len : vec3(1.0, 0.0, 0.0);

    // Cross direction = axis × (camera→midpoint), so the ribbon
    // always presents its broad face to the camera even as the
    // beam rotates.
    vec3 mid     = 0.5 * (origin + tip);
    vec3 viewDir = normalize(ubo.cameraPos.xyz - mid);
    if (any(isnan(viewDir)) || dot(viewDir, viewDir) < 0.5) {
        viewDir = vec3(0.0, 0.0, 1.0);
    }
    vec3 perp = cross(axis, viewDir);
    if (dot(perp, perp) < 1e-5 || any(isnan(perp))) {
        vec3 fallback = abs(axis.y) < 0.90 ? vec3(0.0, 1.0, 0.0) : vec3(1.0, 0.0, 0.0);
        perp = cross(axis, fallback);
    }
    perp = normalize(perp);

    float v = inCorner.y;
    float strength = clamp(inParams.y, 0.0, 1.0);
    float scroll = max(inParams.z, 0.35);
    float envelope = sin(v * 3.14159265);
    float wave_a = sin(v * 12.5663706 + time * scroll * 2.4);
    float wave_b = sin(v * 31.4159265 - time * scroll * 1.35 + 1.7);
    float centre_wave = (wave_a * 0.070 + wave_b * 0.035) * width * strength * envelope;
    float width_wave = 1.0 + (wave_b * 0.12 + wave_a * 0.07) * strength * envelope;

    // Segmented strip expansion. The centreline wave is kept at
    // zero at both endpoints so gameplay-driven beam anchors stay
    // exact while the mid-body breathes.
    vec3 along_offset  = along * v;
    vec3 cross_offset  = perp * (width * width_wave * inCorner.x + centre_wave);
    vec3 worldPos      = origin + along_offset + cross_offset;

    gl_Position = ubo.proj * ubo.view * vec4(worldPos, 1.0);

    vUv     = vec2(inCorner.x + 0.5, inCorner.y);
    vParams = inParams;
    vTime   = time;

    vCross[0] = inCross0; vCross[1] = inCross1;
    vCross[2] = inCross2; vCross[3] = inCross3;
    vCross[4] = inCross4; vCross[5] = inCross5;
    vCross[6] = inCross6; vCross[7] = inCross7;

    vLength[0] = inLen0; vLength[1] = inLen1;
    vLength[2] = inLen2; vLength[3] = inLen3;
    vFlags = inFlags;
}
