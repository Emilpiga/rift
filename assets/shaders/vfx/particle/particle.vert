#version 450

// VFX particle: instanced billboard with procedural sprite shape.
//
// Each instance carries everything the fragment shader needs —
// world position, size, HDR colour (already gradient-sampled
// CPU-side), sprite shape index, seed, world velocity, and a
// rotation phase. The vertex shader does five things:
//
//   1. Build the camera-aligned billboard basis and rotate it
//      by the particle's `spin` so smoke / shards / rings
//      tumble independently.
//   2. Project the particle's world velocity into screen-space
//      and stretch the quad along that direction proportional
//      to speed — sparks become real motion streaks instead of
//      dots.
//   3. Compute a fog factor matching the world shader's
//      player-anchored quadratic fog so particles fade into
//      the fog band like geometry.
//   4. Compute a near-camera dim factor so very-close particles
//      don't blow out the ACES tonemap.
//   5. Emit a `vStretchDir` whose direction matches the
//      screen-space velocity and length encodes stretch
//      amount; the fragment shader uses this for direction-
//      dependent SDFs (Spark cross / Streak line).
//
// Sprite id 8 is a special ground decal path: it draws the quad
// flat on the XZ plane, using `spin` as a stable yaw, so impact
// cracks sit in the world instead of billboarding toward camera.

layout(binding = 0) uniform UniformData {
    mat4 view;
    mat4 proj;
    vec4 cameraPos;
    vec4 lightDir;
    vec4 lightColor;
    vec4 fogColor;
    vec4 fogParams;        // x = fog start, y = fog end
    vec4 fogOrigin;
    vec4 pointLightPos[16];
    vec4 pointLightColor[16];
    vec4 pointLightCount;
    // The world shader's UBO continues with shadow matrices and
    // metadata before reaching `timeData`. The particle shader
    // doesn't sample shadow maps, but the UBO block is bound by
    // descriptor index — we need to mirror the std140 layout up
    // to and including `timeData` so byte offsets line up.
    mat4 lightVP;
    mat4 pointShadowFaceVP[48];
    vec4 pointShadowMeta;
    /// x = seconds since renderer start. Drives flow-map UV
    /// scrolling and temporal noise modulation in the fragment
    /// shader so smoke / streaks read as actively flowing
    /// instead of static patterns.
    vec4 timeData;
} ubo;

// Per-vertex quad corner (binding 0).
layout(location = 0) in vec2 inCorner; // [-0.5, 0.5]^2

// Per-instance (binding 1) — must match VfxParticleInstance.
layout(location = 1) in vec4 inPosSize;     // xyz = position, w = size
layout(location = 2) in vec4 inColor;       // HDR rgba (alpha = opacity)
layout(location = 3) in vec4 inMisc;        // x = seed, y = sprite (uint), z = blend, w = life_t
layout(location = 4) in vec4 inOriginVel;   // x = spawn origin Y, yzw = velocity
layout(location = 5) in float inSpin;       // rotation phase (radians)
layout(location = 6) in vec4 inHybridMeta;  // texture slot, profile kind, p0, p1
layout(location = 7) in vec4 inHybridParams;
layout(location = 8) in vec4 inStylePack;
layout(location = 9) in vec4 inStyleAux;
layout(location = 10) in vec4 inRolePack;

// Outputs to fragment shader.
layout(location = 0) out vec4  vColor;
layout(location = 1) out vec2  vUv;          // [0, 1]^2 within the quad
layout(location = 2) flat out uint vSprite;
layout(location = 3) out float vSeed;
// Anisotropy direction in screen-space UV (length encodes
// stretch amount). The fragment shader uses this to orient
// direction-dependent SDFs without recomputing view-space
// basis per pixel.
layout(location = 4) out vec2  vStretchDir;
layout(location = 5) out float vFogFactor;
// Per-pixel brightness multiplier — cuts very-near particles
// slightly so they don't blow ACES into pure white.
layout(location = 6) out float vDistDim;
layout(location = 7) out float vLifeT;
layout(location = 8) out float vWorldY;
layout(location = 9) out float vOriginY;
layout(location = 10) out vec2 vWorldXZ;
layout(location = 11) flat out vec4 vHybridMeta;
layout(location = 12) flat out vec4 vHybridParams;
// Particle centre XZ — flat so billow UV is not sheared across the quad.
layout(location = 13) flat out vec2 vWorldXZAnchor;
layout(location = 14) flat out vec4 vStylePack;
layout(location = 15) flat out vec4 vStyleAux;
layout(location = 16) flat out vec4 vRolePack;

void main() {
    vStylePack    = inStylePack;
    vStyleAux     = inStyleAux;
    vRolePack     = inRolePack;
    vHybridMeta   = inHybridMeta;
    vHybridParams = inHybridParams;
    vLifeT    = inMisc.w;
    vOriginY  = inOriginVel.x;
    vec3 camRight = vec3(ubo.view[0][0], ubo.view[1][0], ubo.view[2][0]);
    vec3 camUp    = vec3(ubo.view[0][1], ubo.view[1][1], ubo.view[2][1]);

    float size = inPosSize.w;
    vec3  worldCentre = inPosSize.xyz;
    vWorldXZAnchor = worldCentre.xz;

    // ---- Per-particle rotation ----
    // Rotate the (camRight, camUp) basis by `spin` so each
    // billboard tumbles independently. One sin/cos per vertex.
    float spin = inSpin;
    float cs = cos(spin);
    float sn = sin(spin);
    vec3 axisR = camRight * cs + camUp * sn;
    vec3 axisU = camUp    * cs - camRight * sn;

    uint spriteId = floatBitsToUint(inMisc.y);

    // ---- Motion stretch ----
    // Project the particle's world velocity into the camera's
    // tangent plane. Component along `camFwd` is discarded —
    // that's depth motion, no screen-space stretch. Speed
    // becomes a fraction of the sprite size capped so very
    // fast particles don't draw a kilometre-long line.
    // Faded in over 0.3..1.2 m/s so settled smoke / idling
    // embers don't smear in jitter directions.
    vec3 vel = inOriginVel.yzw;
    float velR = dot(vel, camRight);
    float velU = dot(vel, camUp);
    vec2  velScreen = vec2(velR, velU);
    float speed = length(velScreen);
    const float STRETCH_TIME = 0.05; // 50 ms of motion smear
    float stretchAmount = clamp(speed * STRETCH_TIME / max(size, 1e-3),
                                0.0, 2.0);
    stretchAmount *= smoothstep(0.30, 1.20, speed);
    vec2 stretchDir = (speed > 1e-4) ? velScreen / speed : vec2(0.0);

    // Subtle world-up elongation for fire plumes (not smoke — stretch
    // reads as streaks; smoke stays isotropic round billboards).
    if (spriteId == 0u || spriteId == 9u) {
        const vec3 worldUp = vec3(0.0, 1.0, 0.0);
        vec2 upScreen = vec2(dot(worldUp, camRight), dot(worldUp, camUp));
        float upLen = length(upScreen);
        if (upLen > 1e-4) {
            upScreen /= upLen;
            float upVel = max(dot(vel, worldUp), 0.0);
            float flameStretch = 0.11 + 0.05 * smoothstep(0.0, 1.8, upVel);
            if (speed < 0.35) {
                stretchDir = upScreen;
                stretchAmount = max(stretchAmount, flameStretch);
            } else {
                stretchDir = normalize(stretchDir + upScreen * 0.45);
                stretchAmount = min(stretchAmount + flameStretch * 0.55, 1.35);
            }
        }
    }

    // ---- Wisp / SilkStrand: cylindrical billboard ----
    // The `Wisp` sprite (id 6) and `SilkStrand` sprite (id 7)
    // are vertical-fixed pillars. Earlier we projected
    // world-up into the camera's tangent plane and used the
    // 2D screen-space stretch path, but that collapses the
    // beam when the camera looks straight down (world-up
    // projects to near-zero screen length and the pillar
    // "lays down").
    //
    // Instead, build a true cylindrical billboard in 3D:
    //   * vertical axis  = world-up (always)
    //   * horizontal axis = camera-right projected onto the
    //     plane perpendicular to world-up, then re-normalised
    // The quad rotates around the vertical axis to face the
    // camera horizontally, but its "up" never tilts. This
    // matches god-ray / loot-beam billboards in standard
    // engines and survives any camera elevation.
    //
    // We assemble the 3D world position here directly and
    // skip the generic 2D corner-stretch path below by
    // returning early at the end.
    // Flame (9): world-up tongue, bottom-anchored. Whole billboard
    // sways and leans with time + particle velocity so tongues
    // bend as a unit instead of only shimmering inside UV space.
    if (spriteId == 9u) {
        const vec3 worldUp = vec3(0.0, 1.0, 0.0);
        vec3 hAxis = camRight - worldUp * dot(camRight, worldUp);
        float hLen = length(hAxis);
        hAxis = (hLen > 1e-4) ? hAxis / hLen : vec3(1.0, 0.0, 0.0);

        // Shorter reach than before — tall quads read as a flamethrower jet.
        float stretch = 1.52;
        float vSize = size * (1.0 + stretch);
        float hSize = size * 0.46;

        float lifeT = inMisc.w;
        float h01 = inCorner.y + 0.5;
        float phase = inMisc.x * 6.2831853;
        float t = ubo.timeData.x * 0.42;

        float lifeBulge = sin(lifeT * 3.1415927) * 0.28;
        float widthScale = (1.0 + lifeBulge * smoothstep(0.10, 0.50, h01))
                         * (1.0 - 0.22 * smoothstep(0.70, 1.0, lifeT));
        float heightScale = mix(0.90, 1.05, smoothstep(0.0, 0.40, lifeT))
                          * (1.0 + 0.08 * smoothstep(0.60, 1.0, lifeT) * h01);
        hSize = hSize * widthScale;
        vSize = vSize * heightScale;

        float sway = (sin(t * 1.35 + phase + lifeT * 2.5) * 0.11
                    + sin(t * 2.2 + phase * 1.4 - lifeT * 3.5) * 0.04) * h01 * h01;
        float velSide = dot(inOriginVel.yzw, hAxis);
        float velBend = clamp(velSide * 0.018, -0.06, 0.06) * h01 * h01;
        float breathe = 1.0 + 0.04 * sin(t * 3.2 + phase * 0.9 + lifeT * 2.5)
                            * smoothstep(0.0, 0.80, h01);

        vec3 worldPos = worldCentre
            + hAxis    * (inCorner.x * hSize + (sway + velBend) * vSize)
            + worldUp  * (inCorner.y + 0.5) * vSize * breathe;

        gl_Position = ubo.proj * ubo.view * vec4(worldPos, 1.0);
        vWorldY = worldPos.y;

        float fogStart = ubo.fogParams.x;
        float fogEnd   = ubo.fogParams.y;
        float fogDist  = length(ubo.fogOrigin.xyz - worldCentre);
        float fogRaw   = clamp((fogDist - fogStart) / max(fogEnd - fogStart, 1e-3),
                               0.0, 1.0);
        vFogFactor = fogRaw * fogRaw;

        float camDist = length(ubo.cameraPos.xyz - worldCentre);
        vDistDim = mix(0.55, 1.0, smoothstep(0.4, 1.5, camDist));

        vColor      = inColor;
        vUv         = inCorner + 0.5;
        vSprite     = spriteId;
        vSeed       = inMisc.x;
        vStretchDir = vec2(0.0, 1.0) * stretch;
        return;
    }

    if (spriteId == 6u || spriteId == 7u) {
        const vec3 worldUp = vec3(0.0, 1.0, 0.0);

        // Horizontal billboard axis: camera-right with the
        // vertical component projected out. Falls back to
        // world-X if the camera is exactly aligned with up
        // (degenerate but possible at gimbal extremes).
        vec3 hAxis = camRight - worldUp * dot(camRight, worldUp);
        float hLen = length(hAxis);
        hAxis = (hLen > 1e-4) ? hAxis / hLen : vec3(1.0, 0.0, 0.0);

        // Stretch differs by sprite — SilkStrand carries the
        // entire beam in one billboard, Wisp is layered.
        float stretch = (spriteId == 7u) ? 14.0 : 3.0;
        float vSize = size * (1.0 + stretch);
        // SilkStrand widens its billboard 2.5× so the
        // fragment shader has room to draw the broad
        // low-alpha fog shell that wraps the bright core.
        // The fragment shader rescales its `t` coordinate to
        // keep the core/threads at their original width.
        float hSize = (spriteId == 7u) ? size * 2.5 : size;

        // Anchor the beam at the particle's world position
        // and grow upward only — the visible content lives in
        // the upper half of the SilkStrand sprite anyway, so
        // building the billboard symmetrically would waste
        // half the quad height. Shifting the centre up by
        // 0.5 * vSize puts the bottom edge at the anchor.
        vec3 worldPos = worldCentre
            + hAxis    * inCorner.x * hSize
            + worldUp  * (inCorner.y + 0.5) * vSize;

        gl_Position = ubo.proj * ubo.view * vec4(worldPos, 1.0);
        vWorldY = worldPos.y;

        // Per-pixel passthrough — same as the generic path.
        float fogStart = ubo.fogParams.x;
        float fogEnd   = ubo.fogParams.y;
        float fogDist  = length(ubo.fogOrigin.xyz - worldCentre);
        float fogRaw   = clamp((fogDist - fogStart) / max(fogEnd - fogStart, 1e-3),
                               0.0, 1.0);
        vFogFactor = fogRaw * fogRaw;

        float camDist = length(ubo.cameraPos.xyz - worldCentre);
        vDistDim = mix(0.55, 1.0, smoothstep(0.4, 1.5, camDist));

        vColor      = inColor;
        vUv         = inCorner + 0.5;
        vSprite     = spriteId;
        vSeed       = inMisc.x;
        // Encode stretch direction in UV-space (y-up) for
        // the fragment shader. Constant per-vertex; the
        // fragment shader expects this in the same UV basis
        // it samples vUv in. Length encodes the stretch the
        // fragment uses to decide along/across geometry.
        vStretchDir = vec2(0.0, 1.0) * stretch;
        return;
    }

    // ---- GroundCrack: flat XZ decal ----
    if (spriteId == 8u) {
        float yaw = inSpin;
        float cy = cos(yaw);
        float sy = sin(yaw);
        vec2 rotated = vec2(
            inCorner.x * cy - inCorner.y * sy,
            inCorner.x * sy + inCorner.y * cy
        );

        vec3 worldPos = worldCentre
            + vec3(rotated.x * size, 0.0, rotated.y * size);

        gl_Position = ubo.proj * ubo.view * vec4(worldPos, 1.0);
        vWorldY = worldPos.y;

        float fogStart = ubo.fogParams.x;
        float fogEnd   = ubo.fogParams.y;
        float fogDist  = length(ubo.fogOrigin.xyz - worldCentre);
        float fogRaw   = clamp((fogDist - fogStart) / max(fogEnd - fogStart, 1e-3),
                               0.0, 1.0);
        vFogFactor = fogRaw * fogRaw;

        float camDist = length(ubo.cameraPos.xyz - worldCentre);
        vDistDim = mix(0.55, 1.0, smoothstep(0.4, 1.5, camDist));

        vColor      = inColor;
        vUv         = inCorner + 0.5;
        vSprite     = spriteId;
        vSeed       = inMisc.x;
        vStretchDir = vec2(0.0);
        vWorldXZ    = worldCentre.xz;
        return;
    }

    // Decompose the corner into "along stretch" + "across".
    // The along-stretch component scales by (1 + stretch); the
    // across-stretch component stays at 1×, so the billboard
    // becomes an oriented ellipse. When stretchAmount is zero
    // this collapses back to the original square. Wisps go
    // through this path even at zero velocity because the
    // override above sets `stretchAmount` independently.
    // Smoke / hybrid billow: no motion stretch — velocity smear reads as streaks.
    if (spriteId == 2u || spriteId == 10u) {
        stretchAmount = 0.0;
        stretchDir = vec2(0.0);
    }

    vec2 corner = inCorner;
    // Oversized quad so fade hits zero before the hard triangle edge.
    if (spriteId == 2u || spriteId == 10u) {
        corner *= 1.45;
    }
    if (stretchAmount > 1e-4) {
        vec2 along  = stretchDir;
        vec2 across = vec2(-stretchDir.y, stretchDir.x);
        float a = dot(corner, along);
        float c = dot(corner, across);
        corner = along * (a * (1.0 + stretchAmount)) + across * c;
    }

    vec3 worldPos = worldCentre
        + axisR * corner.x * size
        + axisU * corner.y * size;

    gl_Position = ubo.proj * ubo.view * vec4(worldPos, 1.0);
    vWorldY  = worldPos.y;
    vWorldXZ = worldPos.xz;

    // ---- Fog factor ----
    // Match `forward/main.glsl`'s player-anchored quadratic fog so
    // particles fade into the same band as world geometry.
    float fogStart = ubo.fogParams.x;
    float fogEnd   = ubo.fogParams.y;
    float fogDist  = length(ubo.fogOrigin.xyz - worldCentre);
    float fogRaw   = clamp((fogDist - fogStart) / max(fogEnd - fogStart, 1e-3),
                           0.0, 1.0);
    vFogFactor = fogRaw * fogRaw;

    // ---- Distance dim ----
    // Particles within 1.5 m of the camera fall to ~0.55×
    // brightness so a smoke puff engulfing the player doesn't
    // dump a wall of HDR into the tonemapper.
    float camDist = length(ubo.cameraPos.xyz - worldCentre);
    vDistDim = mix(0.55, 1.0, smoothstep(0.4, 1.5, camDist));

    vColor      = inColor;
    vUv         = inCorner + 0.5;
    vSprite     = floatBitsToUint(inMisc.y);
    vSeed       = inMisc.x;
    vStretchDir = stretchAmount > 0.0 ? stretchDir * stretchAmount : vec2(0.0);
}
