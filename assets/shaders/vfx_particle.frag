#version 450

// VFX particle fragment shader. Evaluates one of six procedural
// sprite shapes selected by `vSprite`:
//
//   0 SoftGlow — dual-radius core + faint outer halo.
//   1 Spark    — anisotropic streak along velocity (or +/× cross
//                when stationary), tight bright core.
//   2 Smoke    — 2-octave value-noise modulated disc with
//                eroded silhouette.
//   3 Shard    — diamond SDF with bright rim highlight.
//   4 Ring     — antialiased annular band.
//   5 Streak   — pure motion line oriented along velocity,
//                length-driven by `vStretchDir`.
//   8 GroundCrack — flat XZ impact fissure / scorch decal.
//
// Output is HDR pre-multiplied alpha. The renderer's two
// pipelines (alpha / additive) drive `SRC = ONE` so a single
// shader feeds both blend modes correctly.

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
    // Padding to reach `timeData` at the same std140 offset as
    // the world shader's UBO (binding 0 is shared across every
    // pipeline that uses descriptor set 0). The particle shader
    // doesn't actually read these shadow fields.
    mat4 lightVP;
    mat4 pointShadowFaceVP[48];
    vec4 pointShadowMeta;
    /// x = seconds since renderer start. Powers flow-map UV
    /// scrolling and temporal noise modulation.
    vec4 timeData;
} ubo;

// Set 1, binding 0: the scene depth buffer captured by the
// opaque scene pass. Sampled here so particles can fade out
// smoothly as they approach world geometry — without this the
// fragment alpha is binary (depth-test pass/fail) and the
// silhouette of a smoke puff intersecting a wall reads as a
// hard, flickering edge. Linear sampler is fine; depth values
// don't average meaningfully but for soft-particle fade we
// only care about the broad relationship to fragment depth.
layout(set = 1, binding = 0) uniform sampler2D sceneDepth;

layout(location = 0) in vec4  vColor;
layout(location = 1) in vec2  vUv;
layout(location = 2) flat in uint vSprite;
layout(location = 3) in float vSeed;
layout(location = 4) in vec2  vStretchDir;   // direction & magnitude (0..2)
layout(location = 5) in float vFogFactor;
layout(location = 6) in float vDistDim;

layout(location = 0) out vec4 outColor;

// Linearise a Vulkan depth-buffer value (z_ndc in [0,1]) into
// a *positive* eye-space distance. For our standard perspective
// projection (looking down -Z, depth 0..1):
//
//     z_ndc = (proj[2][2] * z_eye + proj[3][2]) / -z_eye
//
// Solving for z_eye gives a negative number; we negate so the
// returned value is a positive linear distance from the eye,
// which is what the soft-particle compare expects.
float linearEyeDepth(float z_ndc) {
    return ubo.proj[3][2] / (z_ndc + ubo.proj[2][2]);
}

float hash21(vec2 p) {
    p = fract(p * vec2(127.1, 311.7));
    p += dot(p, p + 19.19);
    return fract(p.x * p.y);
}

float valueNoise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    vec2 u = f * f * (3.0 - 2.0 * f);
    float a = hash21(i);
    float b = hash21(i + vec2(1.0, 0.0));
    float c = hash21(i + vec2(0.0, 1.0));
    float d = hash21(i + vec2(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

float fbm2(vec2 p) {
    float v = 0.0;
    float a = 0.55;
    for (int i = 0; i < 3; i++) {
        v += a * valueNoise(p);
        p = p * 2.07 + vec2(11.3, 17.7);
        a *= 0.5;
    }
    return v;
}

// ---- Flow / temporal helpers ---------------------------------

// Cheap pseudo-curl: take the gradient of `valueNoise` at two
// offsets, then return its perpendicular. Output is a smooth
// 2-channel vector field roughly in [-1, 1] that we use as a
// flow map — sample the noise at `uv + curl(uv) * amp` and
// the silhouette starts to *flow* along the field lines
// rather than just churn in place.
vec2 curl2(vec2 p) {
    const float e = 0.05;
    float n_x1 = valueNoise(p + vec2(e, 0.0));
    float n_x0 = valueNoise(p - vec2(e, 0.0));
    float n_y1 = valueNoise(p + vec2(0.0, e));
    float n_y0 = valueNoise(p - vec2(0.0, e));
    // Gradient -> perpendicular
    return vec2(n_y1 - n_y0, -(n_x1 - n_x0)) * (1.0 / (2.0 * e));
}

// ---- Shapes ---------------------------------------------------

// SoftGlow returns a single value that drives both alpha and
// emissive contributions — it's a glow, the brightness *is*
// the alpha. Dual-radius read so bloom catches the wide halo
// while the eye reads the tight core as the centre.
float softGlow(vec2 uv) {
    vec2 c = uv - 0.5;
    float d2 = dot(c, c) * 4.0;          // 0 at centre, 1 at edge
    float core = exp(-d2 * 11.0);         // tight bright core
    float halo = exp(-d2 * 3.6) * 0.30;   // wider faint halo
    return core + halo;
}

// Spark: oriented motion streak along `vStretchDir` if the
// particle is moving, falling back to a +/× cross when it is
// stationary. The fast / slow distinction is automatic — the
// vertex shader has already stretched the geometry, so even a
// stationary spark gets its corner UVs in the original
// [0,1] frame.
float spark(vec2 uv) {
    vec2 c = (uv - 0.5) * 2.0;       // [-1, 1]
    float d = length(c);
    float core = exp(-d * d * 28.0);

    // Motion-aligned streak: project onto velocity direction.
    // `vStretchDir` length up to 2.0 means the geometry has
    // already been elongated — we narrow the across-axis here
    // to 8× to give the streak a hairline feel.
    float streakAniso = 0.0;
    if (length(vStretchDir) > 0.01) {
        vec2 along  = normalize(vStretchDir);
        vec2 across = vec2(-along.y, along.x);
        float a = dot(c, along);
        float t = dot(c, across);
        streakAniso = exp(-t * t * 80.0) * exp(-abs(a) * 1.6);
    }

    // Static cross (visible when not moving): two perpendicular
    // hairlines along the rotated billboard axes.
    float crossX = exp(-c.y * c.y * 110.0) * exp(-abs(c.x) * 3.2);
    float crossY = exp(-c.x * c.x * 110.0) * exp(-abs(c.y) * 3.2);
    float crossLines = (crossX + crossY) * 0.5;

    // Blend cross into streak as motion increases.
    float motionBlend = smoothstep(0.10, 0.80, length(vStretchDir));
    float streak = mix(crossLines, streakAniso, motionBlend);

    return clamp(core + 0.7 * streak, 0.0, 2.5);
}

// Smoke: billowing puff with a flow-mapped, temporally
// modulated silhouette + emissive hot core.
//
// Returns `.x` = alpha mask (silhouette + density), `.y` =
// emissive fraction in [0,1] indicating how much of the
// particle's RGB should be boosted into bloom range. The
// emissive fraction tracks the *internal density*: dense
// pockets read as glowing embers buried in the smoke, wispy
// outer regions stay dark. This is what gives a fireball
// cloud the deep-orange-glow-inside-grey-smoke read instead
// of either a flat-grey puff or a flat-bright disc.
//
// Flow:
//   1. Per-particle seed rotates the noise sample plane so
//      no two puffs look alike.
//   2. A curl-of-noise field defines a slow, time-scrolled
//      flow map; the silhouette's edge erosion samples noise
//      at `uv + curl * amp + time * dir` so the silhouette
//      *moves* along stream lines (not just rotates with the
//      billboard's `spin`).
//   3. Internal billows roll faster than the silhouette
//      shifts, sold by sampling the same fbm at
//      `noiseUV * 5 + time * faster_dir`.
vec2 smokePuff(vec2 uv, float seed) {
    vec2 c = uv - 0.5;
    float r = length(c);

    // Per-particle rotation of the noise plane.
    float ang = seed * 6.2831853;
    float ca = cos(ang), sa = sin(ang);
    vec2 cr = vec2(ca * c.x - sa * c.y, sa * c.x + ca * c.y);

    float t = ubo.timeData.x;

    // Curl-of-noise flow map. Sampled at a low frequency so
    // the field lines are smooth and large compared to the
    // detail noise on top — the silhouette edge follows broad
    // currents, not random jitter.
    vec2 flow = curl2(cr * 1.3 + vec2(seed * 4.0, seed * 7.0))
              * 0.18;

    // Silhouette sample plane: rotated cr + curl displacement
    // + slow temporal scroll. The scroll direction is rotated
    // by `seed` so neighbouring puffs scroll in different
    // directions — this is the trick that prevents 60 puffs
    // from a burst all sliding the same way.
    vec2 scrollDir = vec2(0.13 * cos(seed * 12.5),
                          0.13 * sin(seed * 12.5));
    vec2 noiseUV = cr + flow + t * scrollDir;

    // Erode the silhouette. Strong amplitude (0.18) so the
    // outer band has long fingers — stacking neighbouring
    // puffs causes their fingers to interweave, which is the
    // SDF-pseudo-fusion trick: each puff's silhouette is
    // already broken up at the boundary, so where two puffs
    // overlap the alpha unions visually instead of reading
    // as a hard intersection.
    float n_edge = fbm2(noiseUV * 3.0 + vec2(seed * 17.0, seed * 5.0));
    float r_eroded = r + (n_edge - 0.5) * 0.18;
    float disc = 1.0 - smoothstep(0.28, 0.50, r_eroded);

    // Internal billows — faster temporal scroll so the inside
    // visibly churns even when the silhouette is still.
    vec2 billowUV = cr * 5.0
                  + t * vec2(0.55, -0.32) * (0.6 + seed * 0.6)
                  - vec2(seed * 9.0, seed * 21.0);
    float n_billow = fbm2(billowUV);

    // Fine grain — highest frequency, fastest scroll. Adds
    // glittering hot pockets when combined with `n_billow`.
    vec2 fineUV = cr * 12.0
                + t * vec2(-0.9, 1.1)
                + vec2(seed * 3.0, seed * 7.0);
    float n_fine = fbm2(fineUV);

    float density = mix(0.45, 1.10, n_billow);
    density *= mix(0.85, 1.05, n_fine);

    float alpha = disc * density;

    // Emissive fraction: where billows + fine grain align,
    // there's a hot pocket (ember). Sharpened with a power so
    // only the brightest ~20% of the particle area picks up
    // the bloom boost — the rest stays cool grey/orange smoke.
    float hot = smoothstep(0.55, 0.95, n_billow * 0.7 + n_fine * 0.3);
    // Multiply by `disc` so emissive can't escape the
    // silhouette — we want hot embers *inside* the smoke,
    // never floating in the (faded) outer band.
    float emissive = hot * disc;

    return vec2(alpha, emissive);
}

// Shard: diamond SDF with a bright rim highlight. The rim is
// a thin band where the SDF crosses 0.34..0.36, brightened
// 1.6×. Reads as a crystal facet rather than a flat polygon.
float shard(vec2 uv, float seed) {
    float ang = seed * 6.2831853;
    vec2 c = uv - 0.5;
    float ca = cos(ang), sa = sin(ang);
    vec2 r = vec2(ca * c.x - sa * c.y, sa * c.x + ca * c.y);
    float d = abs(r.x) + abs(r.y) * 1.6;     // diamond, slightly tall
    float body = 1.0 - smoothstep(0.30, 0.40, d);
    // Rim highlight: a thin bright band right at the silhouette.
    float rim = exp(-pow((d - 0.36) * 60.0, 2.0)) * 0.7;
    return body + rim;
}

// Ring: antialiased annular band. Single Gaussian centred at
// r = 0.40, falloff width 24× — narrow enough that the ring
// reads as a hoop rather than a smear.
float ring(vec2 uv) {
    vec2 c = uv - 0.5;
    float r = length(c);
    return exp(-pow((r - 0.40) * 24.0, 2.0));
}

// Streak: pure motion line. Always anisotropic, even at low
// speed — the caller picked this sprite specifically because
// they want a streak look. The head pinprick has a small
// time-driven shimmer so a continuous trail of streaks reads
// as actively burning rather than a chain of static dots.
float streak(vec2 uv) {
    vec2 c = (uv - 0.5) * 2.0;
    vec2 along  = (length(vStretchDir) > 0.01)
                ? normalize(vStretchDir)
                : vec2(1.0, 0.0);
    vec2 across = vec2(-along.y, along.x);
    float a = dot(c, along);
    float t = dot(c, across);
    // Tight across-axis (line thickness), gentle along-axis
    // taper at the ends.
    float line = exp(-t * t * 90.0) * (1.0 - smoothstep(0.85, 1.05, abs(a)));
    // Bright pinprick at the head, time-modulated so embers
    // in a continuous trail shimmer at different phases.
    float headPhase = ubo.timeData.x * 14.0 + vSeed * 6.28318;
    float headMod   = 0.85 + 0.15 * sin(headPhase);
    float head = exp(-pow(a - 0.85, 2.0) * 60.0) * exp(-t * t * 220.0);
    return clamp(line + head * 1.4 * headMod, 0.0, 2.0);
}

// Wisp: ethereal vertical fog column. The vertex shader has
// already oriented the billboard along world-up in screen
// space (encoded in `vStretchDir`) and stretched it into a
// tall capsule. Here we draw a *very* soft volumetric strand
// — closer to a god-ray or magical smoke plume than a hard
// laser cylinder. The silhouette has a wide gaussian falloff
// (no crisp edge), the body is eroded by scrolling fBm so it
// reads as drifting density rather than a uniform tube, and
// there is no hard inner core: brightness comes purely from
// stacking many of these and letting additive blend build the
// pillar in screen space.
//
// Returns mask in [0, 1] — the soft falloff means most pixels
// contribute very little, so the caller is expected to layer
// several wisps to build readable density.
float wisp(vec2 uv, float seed) {
    vec2 c = (uv - 0.5) * 2.0;
    vec2 along  = (length(vStretchDir) > 0.01)
                ? normalize(vStretchDir)
                : vec2(0.0, 1.0);
    vec2 across = vec2(-along.y, along.x);
    float a = dot(c, along);   // -1..1 along the strand
    float t = dot(c, across);  // -1..1 across thickness

    // ----- Soft fog silhouette -----
    // Wide gaussian across (sigma ~ 0.5 so the body is *fully*
    // soft, no visible edge). Long smoothstep at the ends so
    // adjacent wisps overlap and fuse into one continuous
    // column rather than terminating in visible caps.
    float thickness = exp(-t * t * 4.0);
    float endFade   = (1.0 - smoothstep(0.30, 1.00, abs(a)));

    // ----- Scrolling fBm density -----
    // Sample fBm in (across, along) coords with a vertical
    // scroll. Per-particle seed offsets the pattern so
    // neighbouring wisps don't resonate. We use the noise
    // *only* to break up density — never to make the body
    // disappear — so the bias is heavy toward the high end.
    float scroll = ubo.timeData.x * 0.45 + seed * 17.0;
    vec2 noiseUV = vec2(t * 1.1 + seed * 3.7,
                        a * 1.4 + scroll);
    float n = fbm2(noiseUV * 1.6);
    // Heavy floor: density never drops below 60% of base, so
    // the column always reads as continuous fog rather than
    // dashed strands.
    float density = mix(0.60, 1.00, n);

    // Pure body — no bright core. Density layering happens
    // in screen space via additive blend across many wisps.
    float body = thickness * endFade * density;
    return clamp(body, 0.0, 1.0);
}

// SilkStrand: a single sprite that *is* the loot beam — a
// full-height pillar with a soft ethereal body and several
// sharp sine-wave "silk thread" highlights spiralling around
// it. Designed to be HD-readable up close (the sharp threads
// give a sub-pixel-feeling crisp highlight) while still
// looking ethereal at distance (the wide soft body bleeds
// into surrounding bloom).
//
// Geometry assumptions:
//   * Vertex shader has oriented the billboard along
//     world-up (vStretchDir) and stretched it ~6:1 so the
//     quad is tall and narrow.
//   * Particle anchor sits at the *base* of the beam. We
//     therefore only render content in the upper half of
//     the billboard (h in [0,1] mapped from a in [0,1]).
//     The lower half stays empty so the underground portion
//     of the billboard is invisible and doesn't double-up
//     with the ground halo.
//
// All widths and amplitudes taper toward the top: the beam
// physically narrows to pixel-width at h=1 and then fades to
// zero so the silhouette "melts into air".
float silkStrand(vec2 uv, float seed) {
    // The vertex shader anchors this billboard's *bottom
    // edge* at the particle's world position and orients it
    // as a true cylindrical billboard (vertical = world-up,
    // horizontal = camera-right perpendicular to up). So the
    // full quad is visible content: uv.y = 0 is the base,
    // uv.y = 1 is the top. We use the UV directly rather
    // than the symmetric (uv - 0.5)·2 mapping the other
    // sprites use.
    float h = clamp(uv.y, 0.0, 1.0);  // 0 = base, 1 = top
    // Across-axis position. The billboard is widened 2.5× by
    // the vertex shader so the fog shell can extend past the
    // bright core; `tShell` covers the full quad width
    // (-1..1), while `tCore` is rescaled so the core/threads
    // render at their original width regardless of how wide
    // we make the billboard.
    float tShell = (uv.x - 0.5) * 2.0;          // -1..1 across full quad
    float tCore  = tShell * 2.5;                // -1..1 across original core width

    // Discard outside the visible vertical envelope early —
    // saves the fbm/sin work below for empty pixels.
    if (h < 0.001 || h > 0.999) return 0.0;

    // Use `tCore` for everything that previously used `t`.
    float t = tCore;

    // ----- Vertical envelope -----
    // Quick fade-in at base so the bottom doesn't slam into
    // the floor with a hard cap; long smooth fade in the top
    // 12% so the beam visibly "melts into air" rather than
    // terminating at a fixed height.
    float vFade = smoothstep(0.0, 0.04, h)
                * (1.0 - smoothstep(0.88, 1.00, h));

    // Time-driven phase shift so threads visibly rise.
    float scroll = ubo.timeData.x * 0.55 + seed * 11.0;

    // ----- Cloudy density field -----
    // Slow, low-frequency fBm sampled in (across, along)
    // with a gentle vertical scroll. We use this to *modulate*
    // the body and threads so the beam reads as drifting
    // density rather than hard parallel ribbons. Heavy floor
    // (0.55) keeps the beam continuous; the variation just
    // breaks up the linear streaking. A second slower octave
    // adds soft horizontal swells.
    vec2  cloudUV  = vec2(t * 0.9 + seed * 4.3,
                          h * 1.6 + scroll * 0.45);
    float cloudA   = fbm2(cloudUV);
    float cloudB   = fbm2(cloudUV * vec2(0.55, 0.40)
                          + vec2(scroll * 0.20, 0.0));
    float cloud    = mix(0.55, 1.00, mix(cloudA, cloudB, 0.5));

    // ----- Soft ethereal body -----
    // Wide gaussian central column. Width tapers via
    // `pow(h, 0.55)` so the narrowing happens *early* — the
    // beam transitions from a prominent base to a thin shaft
    // within the first ~30% of the height, then holds thin
    // for the rest. Slightly wider base (0.34) than before
    // for a more visible "root".
    float taperH = pow(h, 0.55);
    float bodyW  = mix(0.34, 0.005, taperH);
    float dB     = t / max(bodyW, 1e-4);
    float body   = exp(-dB * dB * 1.8) * 0.30 * cloud;

    // ----- Soft silk threads -----
    // N sine-displaced threads at evenly-spaced phases.
    // Tuned for a *cloudy* reading rather than crisp
    // filaments: wider widths, softer cores, and the cloud
    // density modulates each strand so they break into
    // cloud-like puffs instead of reading as parallel
    // ribbon lines. Amplitude still shrinks toward the top
    // so strands converge into the central pillar.
    float threads = 0.0;
    const int N = 4;
    for (int i = 0; i < N; i++) {
        float phase   = float(i) * 1.5708 + seed * 6.2831 + scroll;
        float amp     = mix(0.58, 0.0, taperH);
        // Two-frequency sine for a less mechanical wave.
        float wave    = sin(h * 5.0 + phase) * amp
                      + sin(h * 11.0 + phase * 1.7) * amp * 0.18;
        // Wider, softer thread — no more pixel-sharp filament.
        float threadW = mix(0.075, 0.012, taperH);
        float dT      = (t - wave) / max(threadW, 1e-4);
        // Soft body (gaussian σ wider) + a tiny inner accent.
        // Cores are now σ ≈ 0.5 instead of σ ≈ 0.27, which
        // turns each strand into a soft cloud streak rather
        // than a crisp HD line.
        float blob    = exp(-dT * dT * 2.0);
        float accent  = exp(-dT * dT * 5.0) * 0.20;
        // Cloud-modulate per-strand contribution. Sample at
        // an offset of `wave` along the thread so the noise
        // travels with the strand (no double-streaking).
        vec2  strandUV = vec2((t - wave) * 1.4 + float(i) * 7.1,
                              h * 2.2 + scroll * 0.6 + float(i) * 3.3);
        float strandN  = fbm2(strandUV);
        float strandM  = mix(0.45, 1.00, strandN);
        threads += (blob + accent) * strandM;
    }
    threads *= 0.32;

    // ----- Broad fog shell -----
    // Wide low-opacity halo wrapping the bright core. Uses
    // the full quad width (`tShell` ∈ [-1,1]) and is eroded
    // by scrolling fBm so the silhouette reads as drifting
    // mist rather than a hard cylinder. Tapers with the
    // same `taperH` curve so it shares the beam's overall
    // shape — wide root, pinched top.
    //
    // The shell is intentionally subtle (≤0.15 contribution)
    // — its job is to bleed a soft glow into the surrounding
    // air and give the beam atmospheric volume, not to
    // compete with the silk threads for attention.
    float shellW    = mix(0.85, 0.05, taperH);
    float dShell    = tShell / max(shellW, 1e-4);
    float shellBase = exp(-dShell * dShell * 1.4);
    // fBm erosion — sample in (across, along) with vertical
    // scroll so the shell breathes upward. Heavy floor so it
    // never goes fully dark.
    vec2  shellUV   = vec2(tShell * 1.6 + seed * 3.7,
                           h * 2.4 + scroll * 0.7);
    float shellN    = fbm2(shellUV);
    float shell     = shellBase * mix(0.55, 1.00, shellN) * 0.18;

    return clamp((shell + body + threads) * vFade, 0.0, 1.4);
}

// GroundCrack: procedural fracture/scorch decal for heavy floor
// impacts. The vertex shader draws this sprite flat on XZ; here
// we compose long irregular fissures, broken annular chips near
// the gameplay radius, and noisy scorched fill around the centre.
float groundCrack(vec2 uv, float seed) {
    vec2 p = (uv - 0.5) * 2.0;
    float r = length(p);
    if (r > 1.0) return 0.0;

    float theta = atan(p.y, p.x);
    float cracks = 0.0;

    const int SPOKES = 14;
    for (int i = 0; i < SPOKES; i++) {
        float fi = float(i);
        float h1 = hash21(vec2(fi + seed * 13.7, seed * 31.1));
        float h2 = hash21(vec2(fi * 5.3, seed * 19.9 + 2.0));
        float h3 = hash21(vec2(fi * 9.1 + 4.0, seed * 7.7));

        float angle = (fi + h1 * 0.72) * 6.2831853 / float(SPOKES);
        float diff = atan(sin(theta - angle), cos(theta - angle));

        float len = mix(0.42, 0.98, h2);
        float start = mix(0.04, 0.22, h3);
        float spokeMask = smoothstep(start, start + 0.08, r)
                * (1.0 - smoothstep(len, len + 0.10, r));

        float width = mix(0.010, 0.028, h1) * mix(1.35, 0.55, r);
        float line = exp(-pow(diff / max(width, 0.003), 2.0)) * spokeMask;

        float branchAngle = angle + mix(-0.55, 0.55, h3);
        float branchDiff = atan(sin(theta - branchAngle), cos(theta - branchAngle));
        float branchStart = mix(0.26, 0.58, h1);
        float branchActive = smoothstep(branchStart, branchStart + 0.05, r)
                           * (1.0 - smoothstep(len * 0.92, len, r));
        float branch = exp(-pow(branchDiff / max(width * 0.62, 0.003), 2.0))
                     * branchActive;

        cracks = max(cracks, max(line, branch * 0.72));
    }

    float n = fbm2(p * 3.2 + vec2(seed * 8.0, seed * 3.0));

    float ringBand = exp(-pow((r - 0.78) * 18.0, 2.0));
    float angular = valueNoise(vec2(theta * 2.4 + seed * 5.0, seed * 11.0));
    float brokenRing = ringBand * smoothstep(0.42, 0.78, angular + n * 0.28);

    float core = (1.0 - smoothstep(0.05, 0.38, r)) * mix(0.35, 1.0, n);
    float scorch = (1.0 - smoothstep(0.18, 0.92, r)) * smoothstep(0.48, 0.92, n) * 0.42;

    return clamp(max(max(cracks, brokenRing * 0.75), max(core * 0.38, scorch)), 0.0, 1.25);
}

void main() {
    // Most sprites are pure-alpha (mask drives both alpha and
    // brightness). Smoke is special: it returns a separate
    // emissive fraction so we can boost RGB into bloom range
    // only where the smoke is dense + has a hot internal
    // pocket, leaving the wispy outer band cool.
    float mask;
    float emissive = 0.0;
    if      (vSprite == 0u) mask = softGlow(vUv);
    else if (vSprite == 1u) mask = spark(vUv);
    else if (vSprite == 2u) {
        vec2 sm = smokePuff(vUv, vSeed);
        mask = sm.x;
        emissive = sm.y;
    }
    else if (vSprite == 3u) mask = shard(vUv, vSeed);
    else if (vSprite == 4u) mask = ring(vUv);
    else if (vSprite == 5u) mask = streak(vUv);
    else if (vSprite == 6u) mask = wisp(vUv, vSeed);
    else if (vSprite == 7u) mask = silkStrand(vUv, vSeed);
    else                    mask = groundCrack(vUv, vSeed);

    // ----- Hard quad-edge fade -----
    // Every procedural sprite already tries to fade to zero
    // at the quad boundary, but most retain a tiny residual
    // alpha at the cardinal edges. Additive blend + bloom +
    // ACES amplifies that into a visible billboard square
    // outline. Force the mask to zero over the outermost ~12%
    // of the quad in both axes so no sprite ever leaks its
    // bounding box.
    vec2 edgeUV = abs(vUv - 0.5) * 2.0;
    float quadFade = 1.0 - smoothstep(0.86, 1.00,
                                      max(edgeUV.x, edgeUV.y));
    mask *= quadFade;
    emissive *= quadFade;

    // Per-particle distance dim — keeps very-near big puffs
    // from crushing ACES.
    mask *= vDistDim;
    emissive *= vDistDim;

    float a = clamp(vColor.a * mask, 0.0, 1.0);

    // ----- Soft-particle fade -----
    // Compare this fragment's eye-space depth to the scene
    // depth (sampled from the opaque scene pass). When the
    // particle is just in front of geometry, fade alpha so
    // the intersection silhouette goes away smoothly instead
    // of cutting hard against the surface. Eye-space units
    // are world units, so a 0.5 m fade band feels natural for
    // smoke/glow puffs at typical sizes.
    {
        vec2 screenSize = vec2(textureSize(sceneDepth, 0));
        vec2 screenUV = gl_FragCoord.xy / screenSize;
        float scene_z_ndc = texture(sceneDepth, screenUV).r;
        float scene_eye = linearEyeDepth(scene_z_ndc);
        float frag_eye  = linearEyeDepth(gl_FragCoord.z);
        float dz = scene_eye - frag_eye;
        // 0.5 m fade band; clamp so we don't make the back
        // side brighter (dz can be larger than the band).
        float soft = clamp(dz / 0.5, 0.0, 1.0);
        a *= soft;
        emissive *= soft;
    }

    // Apply atmospheric fog. Particles fade into the same fog
    // band as world geometry: at the fog wall they go dark
    // (additive layers fade to zero, alpha layers fade toward
    // the fog colour). Without this fade the particles read
    // as stickers floating in haze.
    vec3 rgb = vColor.rgb;
    rgb = mix(rgb, ubo.fogColor.rgb, vFogFactor);

    // Emissive boost: where the smoke has a hot pocket we
    // multiply the RGB by `1 + emissive * boost` *before*
    // pre-multiplying by alpha. This drives the RGB above
    // 1.0 in those pixels so bright-pass bloom catches them
    // while the outer wispy regions stay cool. The boost is
    // gated by `(1 - vFogFactor)` so embers respect fog along
    // with the rest of the particle.
    if (emissive > 0.0) {
        float boost = 1.0 + emissive * 1.6 * (1.0 - vFogFactor);
        rgb *= boost;
    }

    // Pull alpha down with fog so additive embers don't punch
    // through the fog wall as bright pixels.
    a *= 1.0 - vFogFactor * 0.85;

    // Output is **pre-multiplied alpha**. Both pipelines drive
    // this through `SRC = ONE`:
    //
    //   Alpha pipeline    : ONE × rgb + (1-SRC_ALPHA) × dst
    //   Additive pipeline : ONE × rgb +           ONE × dst
    //
    // …so a single shader feeds both blend modes correctly.
    outColor = vec4(rgb * a, a);
}
