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
    float r = max(fragUV.x, 0.0);
    float theta = fragUV.y * 6.28318530718;
    float t = ubo.timeData.x;

    // ---- Intermittent destabilisation pulse ----
    // Every ~7 s the rift "spasms": a low-freq pulse that runs
    // 0..1..0 over ~1.5 s, gating extra wobble amplitude and
    // lensing intensity. The rest of the time it sits at near-
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

    // ---- Animated torn aperture ----
    // Stacked low-freq sines at 3/5/11 lobes, drifting at
    // different rates so the rim never repeats. A second, sharper
    // angular field adds small static-looking bites to the edge:
    // not particles, just the material mask itself becoming an
    // uneven cut through space. During a spasm the high-freq
    // amplitude increases so chunks of the silhouette appear to
    // peel and re-form.
    float spasmAmp = 1.0 + spasm * 1.4 + tremor * 0.35;
    float edgeWobble = 0.030 * sin(theta * 3.0 + t * 0.40)
                     + 0.045 * sin(theta * 5.0 - t * 0.70) * spasmAmp
                     + 0.020 * sin(theta * 11.0 + t * 1.30) * spasmAmp;
    float edgeTear = (riftFbm(vec2(theta * 22.0 + t * 0.035,
                                   4.0 + sin(theta * 3.0))) - 0.5)
                   * (0.040 + spasm * 0.045 + tremor * 0.018);
    float toothCell = floor(theta * 30.0);
    float toothHash = riftHash(vec2(toothCell, floor(t * 1.7)));
    float toothLocal = fract(theta * 30.0);
    float toothShape = smoothstep(0.08, 0.32, toothLocal)
                     * (1.0 - smoothstep(0.46, 0.88, toothLocal));
    float toothBite = toothShape * smoothstep(0.54, 0.90, toothHash)
                    * (0.030 + spasm * 0.045);
    // Slow breathing — silhouette gently inflates / deflates
    // ±2.5% on a 6 s cycle. Subtle on its own but the eye reads
    // the rift as alive instead of static.
    float breath = 0.025 * sin(t * 1.05);
    float edgeOffset = edgeWobble + breath + edgeTear - toothBite;
    float silhouette =
        1.0 - smoothstep(0.84 + edgeOffset,
                         1.00 + edgeOffset, r);

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
    // Rim: cool fracture/lensing only; no warm rim band.
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
    col += vec3(1.45, 0.42, 2.30) * pow(nC, 3.0)
                                    * smoothstep(0.05, 0.40, r)
                                    * (1.0 - smoothstep(0.55, 0.80, r))
                                    * 0.45;                      // cold inner vortex
    col += vec3(1.70, 1.90, 2.80) * starMask;                    // drifting motes
    col += vec3(0.06, 0.02, 0.04) * nE * (1.0 - rimBand);        // grain
    float fractureVeins = pow(max(0.0, nC - 0.60), 2.0) * 2.2;
    col += vec3(0.55, 0.35, 1.35) * fractureVeins * rimBand;     // cool rim fracture

    // ---- Event-horizon shadow collar ----
    // A tear in space should feel like light is being swallowed
    // at the boundary, not emitted evenly from it. Darken a
    // noisy band just inside the silhouette so the bright inner
    // vortex appears to sit behind a thin black aperture.
    float collarBand = smoothstep(0.70, 0.86, r)
                     * (1.0 - smoothstep(0.94, 1.00, r));
    float collarNoise = riftFbm(vec2(theta * 10.0 - t * 0.06,
                                     r * 7.0 + nB * 2.0));
    float collarCrack = pow(max(0.0, collarNoise - 0.48), 1.6) * 2.4;
    float collarShadow = clamp(collarBand * (0.48 + collarCrack * 0.32)
                                           * (1.0 + spasm * 0.35),
                               0.0,
                               0.78);
    col *= 1.0 - collarShadow;

    // ---- Torn aperture seam ----
    // Make the boundary read like a ripped cut through space:
    // dark angular notches bite inward, while a thin cold seam
    // catches bloom along the actual torn edge. This stays inside
    // the silhouette so it reads as fracture, not an outward plume.
    float tearNoise = riftFbm(vec2(theta * 14.0 + t * 0.10,
                                   r * 5.0 - nA * 1.7));
    float notchMask = smoothstep(0.78, 0.98, r)
                    * pow(max(0.0, tearNoise - 0.54), 1.35)
                    * 1.8;
    col *= 1.0 - clamp(notchMask * (0.22 + spasm * 0.18), 0.0, 0.58);

    float seamR = 0.905 + edgeOffset * 0.52 + (tearNoise - 0.5) * 0.035;
    float seamWidth = 0.010 + 0.012 * (0.5 + 0.5 * sin(theta * 9.0 - t * 0.45));
    float seam = 1.0 - smoothstep(seamWidth, seamWidth + 0.020, abs(r - seamR));
    seam *= smoothstep(0.72, 0.90, r);
    float seamPulse = 0.45 + 0.55 * riftHash(vec2(floor(theta * 34.0), floor(t * 7.0)));
    col += vec3(0.36, 0.52, 2.60) * seam * (0.26 + seamPulse * 0.24 + spasm * 0.20);

    // ---- Negative-space edge bites ----
    // These are the important read: a portal is not glowing trim,
    // it is a hole with a violently irregular boundary. Short
    // angular cells carve black cuts inward from the silhouette,
    // then a cold highlight catches one side of the cut so the
    // edge reads like torn glass / folded space rather than smoke.
    float biteDepth = toothBite * (1.0 + 0.5 * tearNoise);
    float biteBand = smoothstep(0.925, 0.975, r)
                   * (1.0 - smoothstep(0.992 - biteDepth * 1.15, 1.01, r))
                   * toothShape
                   * smoothstep(0.58, 0.92, toothHash);
    col *= 1.0 - clamp(biteBand * (0.48 + spasm * 0.25), 0.0, 0.86);

    float cutSide = smoothstep(0.04, 0.14, toothLocal)
                  * (1.0 - smoothstep(0.16, 0.30, toothLocal));
    float cutLine = cutSide
                  * smoothstep(0.62, 0.95, toothHash)
                  * (1.0 - smoothstep(0.006, 0.026, abs(r - (0.982 - biteDepth * 0.55))));
    col += vec3(0.52, 0.82, 3.80) * cutLine * (0.30 + spasm * 0.30 + tremor * 0.18);

    // Actual aperture lip: a razor-thin black line with a cold
    // chromatic glint sitting on the final mesh edge. This is the
    // part that should sell the effect from gameplay distance: the
    // edge is not a ring drawn around the portal, it is the visible
    // thickness of a cut through reality.
    float edgeR = 0.988 + edgeOffset * 0.30;
    float apertureLip = (1.0 - smoothstep(0.004, 0.022, abs(r - edgeR)))
                      * smoothstep(0.94, 0.985, r);
    float lipNoise = 0.45 + 0.55 * riftHash(vec2(floor(theta * 72.0), floor(t * 9.0)));
    col *= 1.0 - apertureLip * (0.55 + lipNoise * 0.25);
    col += vec3(0.20, 0.42, 2.80) * apertureLip * (0.18 + lipNoise * 0.18 + spasm * 0.26);

    // Discontinuous outer glints: tiny cold segments locked to the
    // same edge lip, not free-floating particles. They give the
    // silhouette a brittle, spatial-tear sparkle without turning
    // into a swarm of lines around the portal.
    float glintCell = floor(theta * 48.0 - t * 0.8);
    float glintHash = riftHash(vec2(glintCell, floor(t * 5.0)));
    float glintLocal = fract(theta * 48.0 - t * 0.8);
    float glintDash = smoothstep(0.06, 0.16, glintLocal)
                    * (1.0 - smoothstep(0.22, 0.44, glintLocal));
    float rimDistance = abs(r - edgeR);
    float glint = glintDash
                * smoothstep(0.82, 0.97, glintHash)
                * (1.0 - smoothstep(0.003, 0.015, rimDistance))
                * (0.45 + spasm * 0.85 + tremor * 0.45);
    col += vec3(0.85, 1.10, 4.60) * glint;

    // ---- Dark-red veins crawling under the surface ----
    // Sharp thresholded fbm in (theta, r)-space, slowly drifting.
    // Reads as cracks / tension lines rather than a smooth
    // gradient.
    float vein = riftFbm(vec2(theta * 1.5 + t * 0.10,
                              r * 4.0 - t * 0.20));
    float veinMask = pow(max(0.0, vein - 0.60), 2.0) * 5.0;
    col += vec3(1.40, 0.06, 0.10) * veinMask;

    // ---- Chromatic split at boundary ----
    // R channel pushes outward, B channel pulls inward as r
    // approaches 1. Reads as light bending at the rift's edge
    // — the visual cue the eye associates with refraction
    // without needing a screen-space pass.
    float ca = smoothstep(0.65, 1.0, r);
    col.r *= 1.0 + ca * 0.18;
    col.b *= 1.0 + ca * 0.22;

    // ---- Slow breathing pulse ----
    float pulse = 0.88 + 0.12 * sin(t * 0.85) + spasm * 0.15;
    col *= pulse;

    // ---- Apply silhouette mask ----
    col *= silhouette;

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
                * (1.0 - smoothstep(1.00, 1.05, r))
                * smoothstep(0.92, 1.00, r);
    float haloWobble = 0.5 + 0.5 * sin(theta * 7.0
                                        + nA * 4.0
                                        + t * 0.65);
    float haloAmp = halo * (0.30 + 0.55 * haloWobble)
                         * (1.0 + spasm * 1.2 + tremor * 0.6);
    col.r += haloAmp * 0.07;
    col.g += haloAmp * 0.13;
    col.b += haloAmp * 0.42;

    // Tiny dark-vacuum band immediately outside the rim sells
    // the lensing illusion further: real gravitational lenses
    // create a thin dark Einstein ring just outside their
    // bright halo. We darken the local output by a small
    // factor right at r ≈ 1.005..1.04.
    float vacuum = (1.0 - silhouette)
                 * (1.0 - smoothstep(1.04, 1.10, r))
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
