void main() {
    // Bit-test the flags float to pick a shading path. Using
    // floatBitsToUint so we can pack other booleans into the
    // same float later (bit 1, bit 2, ...) without touching
    // the Rust side.
    uint flags = floatBitsToUint(push.materialParams.z);
    bool usePbr  = (flags & 1u) != 0u;
    bool useRift = (flags & 2u) != 0u;

    vec3 lighting;
    if (useRift)      lighting = shadeRift();
    else if (usePbr)  lighting = shadePbr();
    else              lighting = shadeCel();

    // ---- See-through wall x-ray ----
    // Walls tagged with `material_params.z` bit 3 (set in
    // `floor.rs::rebuild_dungeon` for every batched wall
    // object) carve a stippled porthole around the camera→
    // player segment. The effect replaces the older
    // "snap camera forward when occluded" trick in
    // `camera_follow_system`: instead of moving the camera,
    // we leave the framing alone and open a window in the
    // wall so the player stays visible.
    //
    // Anatomy of the cutout:
    //   1. We project `fragWorldPos` onto the camera→player
    //      ray to get a parameter `t` (distance from camera
    //      along the ray). Fragments past the player or
    //      behind the camera are ignored — only walls that
    //      are *actually* between the camera and the player
    //      are eligible.
    //   2. The perpendicular distance `d` from the fragment
    //      to that ray drives the porthole strength: at the
    //      centre of the porthole `d ≈ 0` and the wall
    //      vanishes; at the rim `d ≈ R` and the wall is
    //      fully opaque.
    //   3. A blue-noise hash + scrolling scanline pattern
    //      decides which fragments to `discard`. Discarded
    //      fragments don't write depth, so characters and
    //      props rendered later show through cleanly.
    //   4. A thin cyan glow at the rim (`smoothstep` band)
    //      sells the "this is an x-ray scan, not a hole" read
    //      and gives the eye an edge to lock onto.
    //
    // Because discard is deferred, this runs after fog so the
    // remaining (non-discarded) wall fragments still tonemap
    // and dim with distance like normal walls.
    if ((flags & 8u) != 0u && ubo.fogOrigin.w > 0.001) {
        vec3 camToFrag   = fragWorldPos     - ubo.cameraPos.xyz;
        vec3 camToPlayer = ubo.fogOrigin.xyz - ubo.cameraPos.xyz;
        float distPlayer = length(camToPlayer);
        // CPU-side eased strength: 0 = off, 1 = fully open.
        // We use it as a multiplier on the porthole mask so
        // the cutout fades in over ~240 ms when the camera
        // newly crosses behind a wall, and fades back out
        // when it clears, instead of popping on the frame
        // the raycast result flips.
        float xrayStrength = clamp(ubo.fogOrigin.w, 0.0, 1.0);

        if (distPlayer > 0.001) {
            vec3 dirPlayer = camToPlayer / distPlayer;

            float tFrag = dot(camToFrag, dirPlayer);
            // Only walls strictly between the camera and the
            // player carve the porthole. Extending the test
            // past the player would let walls of adjacent
            // rooms qualify and reveal them through the
            // porthole even when nothing was occluding the
            // player — exactly the leak we want to avoid.
            if (tFrag > 0.2 && tFrag < distPlayer) {
                // Decompose the perpendicular offset from the
                // camera→player ray into horizontal (ground-
                // plane) and vertical (world-Y) components, so
                // we can shape the porthole as a flattened
                // ellipse instead of a perfect circle.
                // Horizontal occluders span the most screen
                // area in a top-down ARPG (corridor walls), so
                // a wide-flat porthole reveals more of the
                // gameplay-relevant area than a circle of the
                // same vertical reach.
                vec3 closest = ubo.cameraPos.xyz + dirPlayer * tFrag;
                vec3 perp    = fragWorldPos - closest;
                // Vertical world-axis component, then take the
                // remainder as horizontal magnitude.
                float perpY  = perp.y;
                float perpH  = length(perp - vec3(0.0, perpY, 0.0));

                // Squash the horizontal axis (divide by 2.4)
                // and stretch the vertical axis a touch (×1.1)
                // so the same threshold yields ~2.4× wider
                // than tall. The threshold itself is then a
                // simple radius test in this stretched space.
                vec2  shaped     = vec2(perpH / 2.4, perpY * 1.1);
                float r          = length(shaped);

                // Distance-scaled radius so the porthole's
                // *world* footprint scales with depth — at the
                // far end of a long corridor it stays the same
                // screen-space size as the near end. The
                // additive base term (`+ 1.6`) keeps the
                // porthole from shrinking to nothing when the
                // camera is zoomed in close to the player; at
                // very low `tFrag` it bottoms out at a
                // generous fixed minimum that reads as a clear
                // viewport rather than a pinhole.
                float R_inner = 0.12 * tFrag + 1.0;
                float R_outer = 0.17 * tFrag + 1.4;

                if (r < R_outer) {
                    float mask = 1.0 - smoothstep(R_inner, R_outer, r);
                    mask *= xrayStrength;

                    float hash = fract(sin(dot(gl_FragCoord.xy,
                                               vec2(12.9898, 78.233)))
                                       * 43758.5453);
                    float scan = 0.5 + 0.5 * sin(gl_FragCoord.y * 0.45
                                                 + ubo.timeData.x * 1.8);
                    float stipple = mix(hash, scan, 0.35);

                    if (stipple < mask - 0.05) {
                        discard;
                    }
                }
            }
        }
    }

    // Distance fog (player-anchored). The rift is a hole
    // through reality — fog still applies (so you can't see
    // it from across an entire dungeon) but is dampened so
    // the rift retains presence in the haze rather than
    // dissolving into the fog colour.
    float dist = length(ubo.fogOrigin.xyz - fragWorldPos);
    float fogFactor = clamp((dist - ubo.fogParams.x) / (ubo.fogParams.y - ubo.fogParams.x), 0.0, 1.0);
    fogFactor = fogFactor * fogFactor;
    if (useRift) fogFactor *= 0.35;
    vec3 finalColor = mix(lighting, ubo.fogColor.rgb, fogFactor);

    outColor = vec4(finalColor * push.tint.rgb, push.tint.a);
}
