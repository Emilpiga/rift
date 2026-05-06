#version 450

// VFX ribbon: camera-aligned quad expanded from (origin, tip,
// width). Each instance produces one quad; we receive the four
// corner positions in [-0.5..0.5] × [0..1] via inCorner.
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
    vec4 pointLightPos[8];
    vec4 pointLightColor[8];
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

void main() {
    vec3 origin = inOrigin.xyz;
    vec3 tip    = inTip.xyz;
    float width = inOrigin.w;

    vec3 along  = tip - origin;
    float len   = length(along);
    vec3 axis   = (len > 1e-5) ? along / len : vec3(1.0, 0.0, 0.0);

    // Cross direction = axis × (camera→midpoint), so the ribbon
    // always presents its broad face to the camera even as the
    // beam rotates.
    vec3 mid    = 0.5 * (origin + tip);
    vec3 toCam  = ubo.cameraPos.xyz - mid;
    vec3 perp   = normalize(cross(axis, toCam));
    if (any(isnan(perp)) || dot(perp, perp) < 0.5) {
        // Beam pointed straight at the camera — fall back to up.
        perp = vec3(0.0, 1.0, 0.0);
    }

    // Quad expansion.
    vec3 along_offset  = along * inCorner.y;
    vec3 cross_offset  = perp * (width * inCorner.x);
    vec3 worldPos      = origin + along_offset + cross_offset;

    gl_Position = ubo.proj * ubo.view * vec4(worldPos, 1.0);

    vUv     = vec2(inCorner.x + 0.5, inCorner.y);
    vParams = inParams;
    vTime   = inTip.w;

    vCross[0] = inCross0; vCross[1] = inCross1;
    vCross[2] = inCross2; vCross[3] = inCross3;
    vCross[4] = inCross4; vCross[5] = inCross5;
    vCross[6] = inCross6; vCross[7] = inCross7;

    vLength[0] = inLen0; vLength[1] = inLen1;
    vLength[2] = inLen2; vLength[3] = inLen3;
}
