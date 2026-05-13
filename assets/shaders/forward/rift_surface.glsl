float riftHash(vec2 p) {
    return fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453);
}

float riftValueNoise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    float a = riftHash(i);
    float b = riftHash(i + vec2(1.0, 0.0));
    float c = riftHash(i + vec2(0.0, 1.0));
    float d = riftHash(i + vec2(1.0, 1.0));
    vec2 u = f * f * (3.0 - 2.0 * f);
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

float riftFbm(vec2 p) {
    float v = 0.0;
    float a = 0.5;
    for (int i = 0; i < 4; i++) {
        v += a * riftValueNoise(p);
        p *= 2.13;
        a *= 0.5;
    }
    return v;
}

vec3 shadeRift() {
    float r = clamp(fragUV.x, 0.0, 1.0);
    float theta = fragUV.y * 6.28318530718;
    float t = ubo.timeData.x;

    // ---- Intermittent destabilisation pulse ----
    // Every ~7 s the rift "spasms": a low-freq pulse that runs
    // 0..1..0 over ~1.5 s, gating extra wobble amplitude and
    // tendril strength. The rest of the time it sits at near-
    // zero and the rift looks calm. We synthesise the schedule
    // from a fract+smoothstep instead of an explicit timer so
    // there's no CPU-side state to manage.
    float spasmPhase = fract(t * 0.14);                  // 0..1 every 7.14 s
    float spasm = smoothstep(0.00, 0.08, spasmPhase)
                * (1.0 - smoothstep(0.18, 0.28, spasmPhase));
    // Second, faster, weaker tremor so the rim never feels
    // perfectly still even between major spasms.
    float tremorPhase = fract(t * 0.37 + 0.21);
    float tremor = smoothstep(0.00, 0.05, tremorPhase)
                 * (1.0 - smoothstep(0.10, 0.16, tremorPhase));

    // ---- Animated silhouette wobble ----
    // Stacked low-freq sines at 3/5/11 lobes, drifting at
    // different rates so the rim never repeats. Combined with
    // the *static* mesh-side wobble (in `portal_with_palette`)
    // the contour reads as continuously deforming, never a
    // perfect circle. During a spasm the high-freq amplitude
    // doubles — chunks of the silhouette appear to peel and
    // re-form.
    float spasmAmp = 1.0 + spasm * 1.4 + tremor * 0.35;
    float edgeWobble = 0.030 * sin(theta * 3.0 + t * 0.40)
                     + 0.045 * sin(theta * 5.0 - t * 0.70) * spasmAmp
                     + 0.020 * sin(theta * 11.0 + t * 1.30) * spasmAmp;
    // Slow breathing — silhouette gently inflates / deflates
    // ±2.5% on a 6 s cycle. Subtle on its own but the eye reads
    // the rift as alive instead of static.
    float breath = 0.025 * sin(t * 1.05);
    float silhouette =
        1.0 - smoothstep(0.86 + edgeWobble + breath,
                         1.00 + edgeWobble + breath, r);

    // ---- Multi-layer counter-rotating swirl fields ----
    // Five layers at different angular velocities, radii, and
    // frequencies fake true volumetric parallax: the eye picks
    // the patterns apart as they slide over each other and
    // perceives depth, even though we're sampling on a 2D
    // plane.
    //
    //   Layer A — outer slow swirl  (CCW, low freq)
    //   Layer B — mid counter-spin  (CW,  mid freq)
    //   Layer C — inner fast vortex (CCW, high freq, tight)
    //   Layer D — drifting "stars"  (radially inward, sparse)
    //   Layer E — micro-grain       (no rotation, fine detail)
    vec2 swA = vec2(cos(theta + t * 0.10),
                    sin(theta + t * 0.10)) * (r + 0.10);
    vec2 swB = vec2(cos(theta - t * 0.35),
                    sin(theta - t * 0.35)) * (r * 1.30 + 0.05);
    vec2 swC = vec2(cos(theta * 2.0 + t * 0.85),
                    sin(theta * 2.0 + t * 0.85)) * (r * 0.55);
    float nA = riftFbm(swA * 3.5);
    float nB = riftFbm(swB * 7.0 + 17.3);
    float nC = riftFbm(swC * 11.0 - 8.4);
    float nE = riftFbm(vec2(theta * 6.0, r * 12.0 - t * 0.05));

    // Radially-inward starfield: hash on (theta * 17, t * 0.08)
    // gates a bright pinprick that drifts toward the centre as
    // r decreases (faked by sampling at a time-shifted radial
    // offset). Reads as motes falling forever inward.
    float starR = fract(r * 1.6 - t * 0.08);
    float starHash = riftHash(vec2(floor(theta * 32.0),
                                   floor(starR * 24.0)));
    float starMask = pow(max(0.0, starHash - 0.985), 2.0) * 1500.0;
    starMask *= smoothstep(0.05, 0.30, r) * (1.0 - smoothstep(0.55, 0.85, r));

    // ---- Radial bands ----
    // Core: pitch black, very slight charcoal glow (so it's
    // not pure 0,0,0 — bloom needs *something* to read).
    // Mid: crimson swirl driven by n1, with a dark-blue
    // impossible-color hint underneath driven by inverted n2.
    // Rim: bright ember vein band driven by n3.
    float coreFade = smoothstep(0.0, 0.35, r);
    float midBand =
        smoothstep(0.20, 0.55, r) * (1.0 - smoothstep(0.55, 0.85, r));
    float rimBand = smoothstep(0.65, 1.0, r);

    vec3 col = vec3(0.0);
    col += vec3(0.04, 0.01, 0.025) * (1.0 - coreFade);          // dim core
    col += vec3(2.20, 0.18, 0.06) * midBand * pow(nA, 1.6);     // outer crimson swirl
    col += vec3(1.40, 0.10, 0.05) * midBand * pow(nB, 2.0) * 0.55; // mid counter-swirl
    col += vec3(0.10, 0.04, 0.20) * midBand
                                    * pow(1.0 - nB, 3.0) * 0.55; // blue-shift hint
    col += vec3(3.00, 1.20, 0.50) * pow(nC, 3.0)
                                    * smoothstep(0.05, 0.40, r)
                                    * (1.0 - smoothstep(0.55, 0.80, r))
                                    * 0.45;                      // hot inner vortex
    col += vec3(2.60, 1.80, 1.40) * starMask;                    // drifting motes
    col += vec3(0.06, 0.02, 0.04) * nE * (1.0 - rimBand);        // grain
    float emberVeins = pow(max(0.0, nC - 0.55), 2.0) * 4.0;
    col += vec3(2.80, 0.95, 0.30) * emberVeins * rimBand;        // ember band

    // ---- Dark-red veins crawling under the surface ----
    // Sharp thresholded fbm in (theta, r)-space, slowly drifting.
    // Reads as cracks / tension lines rather than a smooth
    // gradient.
    float vein = riftFbm(vec2(theta * 1.5 + t * 0.10,
                              r * 4.0 - t * 0.20));
    float veinMask = pow(max(0.0, vein - 0.60), 2.0) * 5.0;
    col += vec3(1.40, 0.06, 0.10) * veinMask;

    // ---- Edge tendrils ----
    // Sharp angular spikes that flicker outward past the
    // silhouette. `tendrilMask` is concentrated in the outer
    // 25% of the disc; combined with the mesh's wobbly outer
    // ring the silhouette gains "tearing" fingers. Spasm
    // pulses double their reach for a moment — chunks of edge
    // appear to peel and stretch.
    float tendril = pow(max(0.0,
                           sin(theta * 8.0 + nB * 6.0 + t * 0.30)),
                       12.0);
    float tendrilMask = smoothstep(0.74, 0.99, r) * tendril;
    col += vec3(2.60, 0.55, 0.18) * tendrilMask
                                    * (1.0 + spasm * 1.5);

    // ---- Chromatic split at boundary ----
    // R channel pushes outward, B channel pulls inward as r
    // approaches 1. Reads as light bending at the rift's edge
    // — the visual cue the eye associates with refraction
    // without needing a screen-space pass.
    float ca = smoothstep(0.65, 1.0, r);
    col.r *= 1.0 + ca * 0.65;
    col.b *= 1.0 - ca * 0.45;

    // ---- Slow breathing pulse ----
    float pulse = 0.88 + 0.12 * sin(t * 0.85) + spasm * 0.15;
    col *= pulse;

    // ---- Apply silhouette mask ----
    // The mask is intentionally NOT applied to the tendrils
    // (they extend past the rim by being added before the
    // multiply *only on the disc*), so we re-add a small
    // outside-rim component to keep them visible.
    vec3 outsideRim = vec3(2.60, 0.55, 0.18) * tendrilMask
                                              * (1.0 - silhouette);
    col *= silhouette;
    col += outsideRim * 0.6;

    // ---- Pseudo-refraction lensing halo ----
    // True screen-space refraction would require sampling the
    // scene HDR target inside this pass, which Vulkan disallows
    // (we'd be reading the same attachment we're writing to).
    // Instead we synthesise the *visual cue* the eye uses to
    // recognise space-bending: a thin chromatic-split halo just
    // past the silhouette where R bleeds outward and B inward,
    // modulated by a low-freq angular wobble so the band looks
    // like reality is rippling against the rim. Subtle by
    // design — the eye reads "lensing" without seeing distinct
    // colour fringes. Energised during destabilisation pulses.
    float halo = (1.0 - silhouette)
                * smoothstep(1.05, 1.00, r)
                * smoothstep(0.92, 1.00, r);
    float haloWobble = 0.5 + 0.5 * sin(theta * 7.0
                                        + nA * 4.0
                                        + t * 0.65);
    float haloAmp = halo * (0.30 + 0.55 * haloWobble)
                         * (1.0 + spasm * 1.2 + tremor * 0.6);
    col.r += haloAmp * 0.45;
    col.b += haloAmp * 0.18;
    // Tiny dark-vacuum band immediately outside the rim sells
    // the lensing illusion further: real gravitational lenses
    // create a thin dark Einstein ring just outside their
    // bright halo. We darken the local output by a small
    // factor right at r ≈ 1.005..1.04.
    float vacuum = (1.0 - silhouette)
                 * smoothstep(1.04, 1.00, r)
                 * smoothstep(0.99, 1.02, r);
    col *= 1.0 - vacuum * 0.55;

    // ---- Extract-portal recolour ----
    // When bit 2 of the flags float is set, this is the
    // *extract* portal — the boss-room exit that ferries the
    // player back to the hub, sibling to the descend portal.
    // The two portals stand side-by-side in the corridor, so
    // they need to be unmistakable at a glance: descend stays
    // crimson (the rift's own colour), extract becomes a
    // cool cyan/teal so the player reads "way home" vs "way
    // deeper" instantly.
    //
    // The recolour is applied as a per-channel *swap* of the
    // existing crimson palette into a cyan one, scaled to
    // preserve the same luminance envelope so the silhouette,
    // halo, and pulse all still read as the same animated
    // shape. The cheap form `rgb -> bgr * tint` happens to map
    // the rift's hot-red highlights into hot-cyan/blue
    // highlights, which is exactly what we want.
    uint flags2 = floatBitsToUint(push.materialParams.z);
    if ((flags2 & 4u) != 0u) {
        // Channel swap (R ↔ B) plus a slight green lift —
        // produces a glacial cyan-teal that's the chromatic
        // complement of the rift crimson. The dim outer
        // glow stays cool (so the surrounding floor halo
        // pushed by `portal_system::push_lights` matches),
        // while the rim highlights still bloom hard.
        col = vec3(col.b * 0.65, col.g * 1.15 + col.r * 0.20, col.r * 1.45);
    }

    return col;
}
