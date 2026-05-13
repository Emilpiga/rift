#version 450

// Mask atlas (R8_UNORM, 2x2 layout of 4 organic silhouettes).
layout(set = 0, binding = 0) uniform sampler2D maskAtlas;

layout(location = 0) in vec2 v_atlas_uv;
layout(location = 1) in float v_intensity;
layout(location = 2) in float v_time;
layout(location = 3) in float v_atlas_slice;

// R = wet intensity, G = spawn time. Blend op MAX on both channels
// (set in the pipeline) keeps the freshest / wettest splat per texel.
layout(location = 0) out vec2 outBlood;

void main() {
    // The atlas is a fixed 2x2 grid of mask slices. Slice index 0..3
    // selects which sub-rect of the atlas to sample.
    int slice = int(floor(v_atlas_slice + 0.5)) & 3;
    vec2 base_uv = fract(v_atlas_uv);

    // Slice 0 is the boot-footprint silhouette. It has hand-tuned
    // anatomy (heel/arch/sole/toes) which the splatter-flow warp
    // below would smear into an unrecognisable blob. Branch out to
    // a minimal path that just samples the mask with a small edge-
    // noise warp + dither, preserving the foot shape while still
    // breaking the silhouette boundary so prints don't read as
    // stamped clones.
    if (slice == 0) {
        vec2 cell_uv = base_uv * 0.5;
        // Light noise-warp confined to the edge, so the interior
        // shape (toes, sole, heel) stays clean.
        float r = length(base_uv - 0.5) * 2.0;
        float edge = smoothstep(0.55, 1.05, r);
        float seedFp = fract(v_time * 13.71);
        vec2 nfp = vec2(
            fract(sin(dot(base_uv * 22.0 + seedFp,
                          vec2(12.9898, 78.233))) * 43758.5453),
            fract(sin(dot(base_uv * 22.0 + seedFp * 1.7,
                          vec2(63.7264, 10.873))) * 24634.6345)
        ) - 0.5;
        cell_uv += nfp * 0.020 * edge;
        cell_uv = clamp(cell_uv, vec2(0.001), vec2(0.499));
        float maskFp = texture(maskAtlas, cell_uv).r;
        float ditherFp = fract(sin(dot(base_uv * 256.0 + seedFp * 11.0,
                                       vec2(38.917, 73.211))) * 47891.231);
        float threshFp = 0.06 + (ditherFp - 0.5) * 0.08 * edge;
        if (maskFp < threshFp) discard;
        outBlood = vec2(v_intensity * pow(maskFp, 0.70), v_time);
        return;
    }

    // ----- Per-splat shape parameters -----
    // The single biggest "procedural" giveaway is repetition in
    // *flow behaviour*: every splat sharing the same teardrop
    // coefficient, the same drag magnitude, the same spike count,
    // the same warp frequencies. Even with different seed-driven
    // noise patterns the eye picks up the shared signature.
    // Instead, derive every coefficient from the seed so each kill
    // reads as a *different impact* — some violent and elongated,
    // some splattering wide, some throwing tendrils, some
    // pancaking.
    float seed = fract(v_time * 13.71);
    // Helper: cheap stable hash → [0, 1]
    #define H(s) fract(sin((s) * 91.271) * 47213.7)
    float h1 = H(seed * 1.0 + 1.7);   // teardrop strength
    float h2 = H(seed * 2.3 + 5.1);   // drag amplitude
    float h3 = H(seed * 3.7 + 9.4);   // spike count (4..12)
    float h4 = H(seed * 5.1 + 13.9);  // spike amplitude
    float h5 = H(seed * 7.3 + 17.2);  // edge warp 1 magnitude
    float h6 = H(seed * 11.7 + 21.5); // edge warp 2 magnitude
    float h7 = H(seed * 13.1 + 27.7); // anisotropy ratio
    float h8 = H(seed * 17.2 + 31.3); // dither amplitude
    float h9 = H(seed * 19.7 + 39.7); // wisp irregularity
    // Map to ranges. The centres roughly match the previous
    // hard-coded values; the spreads create the per-splat variety.
    float pTeardrop = mix(0.05, 0.32, h1);     // was 0.18
    float pDrag     = mix(0.04, 0.18, h2);     // was 0.10
    float pSpikeCnt = mix(4.0, 12.0, h3);      // was 7
    float pSpikeAmp = mix(0.05, 0.30, h4);     // was 0.18
    float pWarpA    = mix(0.020, 0.065, h5);   // was 0.040
    float pWarpB    = mix(0.040, 0.110, h6);   // was 0.075
    float pAnisoX   = mix(0.4, 1.6, h7);       // X freq scale
    float pAnisoY   = mix(0.7, 1.4, 1.0 - h7); // anti-correlated for variety
    float pDither   = mix(0.04, 0.18, h8);     // was 0.10
    float pWispAmp  = mix(0.02, 0.10, h9);     // was 0.06
    #undef H

    // ----- Directional flow / advection -----
    // The atlas-local +X axis is the splat's forward direction (set
    // by the per-instance rotation in the vertex shader). Real spilled
    // blood under impact: a) elongates forward of the centre, b)
    // throws thin tendrils ahead of the main body, c) drags trailing
    // skirts behind. We bake all three into the mask sample by
    // pre-warping `base_uv` before atlas lookup. Because every splat
    // is stamped exactly once into the accumulation field, the
    // resulting silhouette gets locked in at spawn time and inherits
    // the wet/dry curve naturally.
    vec2 local = (base_uv - 0.5) * 2.0;        // [-1, 1]^2 splat-local
    float forward = local.x;                    // +X is impact dir
    float lateral = local.y;
    float radial = length(local);

    // a) Forward stretch: shear the U coord backward as we move away
    //    from the centerline laterally. This pulls the silhouette into
    //    a teardrop/comma shape with the wide end at the back of the
    //    impact (-X) and a narrowed nose pointing along impact (+X).
    float teardrop = -pTeardrop * (lateral * lateral);

    // b) Forward tendrils: bias near the leading edge (forward > 0)
    //    using a low-frequency angular noise so the front rim grows a
    //    handful of thin spike-like protrusions pointing along +X.
    //    Spike *count* varies per splat — some have a few fat fingers,
    //    some have many thin ones.
    float angle = atan(lateral, forward);
    float spikePhase = angle * pSpikeCnt + seed * 6.283;
    float spikeNoise = fract(sin(spikePhase * 12.733) * 91271.331);
    float spikeMask = smoothstep(0.0, 1.0, forward) * smoothstep(0.55, 0.95, radial);
    float spikeOffset = (spikeNoise - 0.5) * pSpikeAmp * spikeMask;

    // c) Trailing drag: the back rim (forward < 0) gets stretched
    //    further backward with a slight downward droop, like the
    //    splat skirts up behind the impact line.
    float dragU = -smoothstep(0.0, 0.7, -forward) * pDrag;
    // Lateral lift on trailing edge: thin lateral wisps.
    float dragV = (fract(sin(forward * 31.7 + seed * 9.3) * 41273.7) - 0.5)
                  * pWispAmp * smoothstep(0.0, 0.7, -forward);

    // Apply the directional flow. Combined into a single warp so we
    // do exactly one atlas sample.
    vec2 flowWarp = vec2(teardrop + spikeOffset + dragU,
                         dragV);
    base_uv = clamp(base_uv + flowWarp, vec2(0.001), vec2(0.999));

    // ----- Edge-break domain warp -----
    // The atlas silhouettes are smooth metaballs / radial drips, so
    // their alpha cliff lands on a smooth boundary that — at the
    // splat quad's resolution — reads as visibly rounded but slightly
    // axis-aligned. We perturb the sample point with two octaves of
    // hash-based noise to break that contour up into ragged
    // capillaries and torn-paper edges. The warp is strongest near
    // the silhouette boundary (tested via a cheap mask-radius proxy)
    // and tapers in the centre so the bulk of the splat stays solid.
    vec2 c = base_uv - 0.5;
    float r = length(c) * 2.0;            // 0 at centre, ~1 at edge
    float edgeWeight = smoothstep(0.55, 1.05, r);

    // Two-octave value-noise warp. Anisotropy varies per splat so
    // some prints read as long sliding streaks and others as
    // chunky speckle.
    vec2 nf1 = vec2(14.0 * pAnisoX, 38.0 * pAnisoY);
    vec2 nf2 = vec2(5.5 * pAnisoX,  13.0 * pAnisoY);
    vec2 n1 = vec2(
        fract(sin(dot(base_uv * nf1 + seed,        vec2(12.9898, 78.233))) * 43758.5453),
        fract(sin(dot(base_uv * nf1 + seed * 1.7,  vec2(63.7264, 10.873))) * 24634.6345)
    ) - 0.5;
    vec2 n2 = vec2(
        fract(sin(dot(base_uv * nf2 + seed * 0.3, vec2(91.4181, 24.512))) * 18763.123),
        fract(sin(dot(base_uv * nf2 + seed * 2.1, vec2(45.6612, 70.901))) * 31479.823)
    ) - 0.5;
    vec2 warp = (n1 * pWarpA + n2 * pWarpB) * edgeWeight;

    vec2 cell_uv = (base_uv + warp) * 0.5;
    cell_uv += vec2(float(slice & 1), float((slice >> 1) & 1)) * 0.5;
    // Clamp into the slice's sub-rect so the warp never pulls
    // samples into a neighbouring atlas cell.
    vec2 sliceMin = vec2(float(slice & 1), float((slice >> 1) & 1)) * 0.5;
    cell_uv = clamp(cell_uv, sliceMin + 0.001, sliceMin + 0.499);

    float mask = texture(maskAtlas, cell_uv).r;

    // ----- Stochastic edge dither -----
    // Push the discard threshold up/down by a small per-fragment hash
    // so the alpha cliff isn't a clean iso-line. Combined with the
    // warp this gives edges that read as torn / sprayed rather than
    // a smooth oval. The dither amplitude tapers toward the centre
    // so the body of the splat stays opaque.
    float dither = fract(sin(dot(base_uv * 256.0 + seed * 11.0,
                                 vec2(38.917, 73.211))) * 47891.231);
    float threshold = 0.05 + (dither - 0.5) * pDither * edgeWeight;
    if (mask < threshold) discard;

    // Intensity carries the splat's contribution; modulate by mask
    // so the silhouette edges feather. Multiply by a small exponent
    // to push the alpha cliff back toward the centre while keeping a
    // soft rim.
    float wet = v_intensity * pow(mask, 0.75);

    outBlood = vec2(wet, v_time);
}
