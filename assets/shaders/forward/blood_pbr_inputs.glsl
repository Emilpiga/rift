// Cook-Torrance BRDF building blocks
// ---------------------------------------------------------------------------
float distributionGGX(vec3 N, vec3 H, float roughness) {
    float a = roughness * roughness;
    float a2 = a * a;
    float NdotH = max(dot(N, H), 0.0);
    float denom = (NdotH * NdotH) * (a2 - 1.0) + 1.0;
    return a2 / (PI * denom * denom);
}

float geometrySchlickGGX(float NdotV, float roughness) {
    float r = roughness + 1.0;
    float k = (r * r) / 8.0;
    return NdotV / (NdotV * (1.0 - k) + k);
}

float geometrySmith(vec3 N, vec3 V, vec3 L, float roughness) {
    float NdotV = max(dot(N, V), 0.0);
    float NdotL = max(dot(N, L), 0.0);
    return geometrySchlickGGX(NdotV, roughness) * geometrySchlickGGX(NdotL, roughness);
}

vec3 fresnelSchlick(float cosTheta, vec3 F0) {
    return F0 + (1.0 - F0) * pow(clamp(1.0 - cosTheta, 0.0, 1.0), 5.0);
}

// ---------------------------------------------------------------------------
// Blood-field composite.
// ---------------------------------------------------------------------------
// Samples the per-floor blood accumulation texture (R = wet intensity,
// G = spawn time in seconds) at the fragment's world XZ position and
// mutates the incoming PBR inputs in place.
//
// Gating:
//   - `bloodFieldXform == 0` → no active field (hub / boot). Skip.
//   - `Ngeo.y < 0.55` → fragment isn't a near-horizontal floor surface.
//     Walls and ceilings don't accumulate ground blood; we'll add a
//     separate vertical field in a follow-up pass.
//   - UV outside [0, 1] (the floor's padded extent) → skip.
//
// Wet/dry curve:
//   - `wet`  = coverage * (1 - smoothstep(0, 25 s, age))  → low-roughness
//             dark-red sheen for the first ~25 s.
//   - `dry`  = smoothstep(20 s, 75 s, age)               → albedo and
//             roughness drift toward iron-rust matte.
// Beyond ~75 s the splat continues to read as a brownish stain (it never
// fully disappears — pools dry, they don't evaporate). The next splat
// at the same texel resets `G` via the `MAX` blend, restoring the wet
// look.
//
// Normal bevel: when coverage is non-trivial, four offset samples build
// a coarse gradient that perturbs the surface normal slightly so pools
// catch torchlight along their rim. Skipped when the texel is dry —
// dried blood has no thickness.
void applyBloodField(
    inout vec3 albedo,
    inout float roughness,
    inout float metallic,
    inout vec3 N,
    vec3 Ngeo
) {
    if (ubo.bloodFieldXform.z == 0.0 && ubo.bloodFieldXform.w == 0.0) return;

    vec2 worldXZ = vec2(fragWorldPos.x, fragWorldPos.z);
    vec2 uv = (worldXZ - ubo.bloodFieldXform.xy) * ubo.bloodFieldXform.zw;
    if (any(lessThan(uv, vec2(0.0))) || any(greaterThan(uv, vec2(1.0)))) return;

    float floorY = ubo.timeData.y;
    float floorYMax = ubo.timeData.z;
    // For a flat (single-elevation) floor `floorYMax` equals
    // `floorY`; for a rift floor with raised daises / lowered
    // pits the two bracket the playable surface band. `yAbove`
    // is kept relative to the lowest plane (the historical
    // anchor) so the wall splatter pattern hashes stay
    // continuous; the surface-classification gates below use
    // both bounds.
    float yAbove = fragWorldPos.y - floorY;

    // ----- Surface classification -----
    // Three cases:
    //   floor   : Ngeo.y > 0.55 AND frag Y near any platform
    //             elevation in `[floorY, floorYMax]` (\u00b1 a few
    //             cm tolerance for shader / rasteriser drift)
    //   wall    : Ngeo.y < 0.45 AND frag Y in [floorY - 0.1,
    //             floorYMax + 2.5] above the lowest platform
    //             \u2014 i.e. anywhere up to wall-cap-ish height
    //             above the highest walkable plane
    //   reject  : everything else (wall caps, ceilings)
    bool isFloor = Ngeo.y > 0.55
                && fragWorldPos.y >= floorY    - 0.25
                && fragWorldPos.y <= floorYMax + 0.25;
    bool isWall  = Ngeo.y < 0.45
                && fragWorldPos.y > floorY    - 0.10
                && fragWorldPos.y < floorYMax + 2.50;
    if (!isFloor && !isWall) return;

    // ----- Time-evolving advection (floor only) -----
    // Sample the field once at the un-warped UV to read the splat's
    // age, then re-sample at a small upstream offset so the body of
    // each pool drifts along its impact direction over the first few
    // seconds. Subtle (capped at ~3 cm in world space) and tapers
    // off as the splat ages — fresh blood pulls forward; old blood
    // is locked in place. Walls don't get this because gravity drag
    // is already baked into the wall composite.
    vec2 sampleUV = uv;
    if (isFloor) {
        // Read centre to get spawn time, derive age, then build a
        // small forward-axis warp from a low-frequency hash of the
        // splat's spawn time so each kill drifts its own way.
        float t0 = texture(bloodField, uv).g;
        float age0 = max(0.0, ubo.timeData.x - t0);
        // Direction is a hash of spawn time → stable per-splat.
        float hashDir = fract(sin(t0 * 12.713) * 4321.7);
        float dirAng = hashDir * 6.2831853;
        vec2 flowDir = vec2(cos(dirAng), sin(dirAng));
        // Drift magnitude in UV space. World 0..3cm (cap) × inv extent.
        // Ramp in over the first ~0.6 s, hold at full, fade out by 8 s.
        float flowAmt = smoothstep(0.0, 0.6, age0)
                      * (1.0 - smoothstep(4.0, 8.0, age0))
                      * 0.030; // metres
        // Convert metres → UV using inv extent.
        vec2 invExtent = ubo.bloodFieldXform.zw;
        sampleUV = uv - flowDir * flowAmt * invExtent;
    }

    vec2 bloodSample = texture(bloodField, sampleUV).rg;
    float coverage = bloodSample.r;
    if (coverage < 0.01) return;

    // ----- Wall composite -----
    // Walls share the same 2D field as the floor. Naively
    // extruding the field signal upward gives painted stripes;
    // scattering pure cells gives "polka-dot balls". The right
    // structure is *splatter blobs with drip trails*: a few
    // organically-shaped masses with FBM-warped outlines, each
    // shedding thin vertical streaks below it. Existence of each
    // blob is gated by `coverage` so blobs only appear in
    // columns where the field actually has blood, but their
    // *shape* is generated procedurally so the wall reads as
    // splatter rather than a stripe of the field signal.
    //
    // Two splat scales (big body splats + smaller satellite
    // splats) plus per-splat drip trails. No vertical falloff
    // multiplier, no cell grid — the silhouette comes from the
    // blobs themselves.
    float heightMask = 0.0;
    if (isWall) {
        // 1D coord along the wall surface. For an axis-aligned
        // wall the tangent is whichever of X/Z is *not* the
        // dominant component of the geometric normal.
        float u = abs(Ngeo.x) > abs(Ngeo.z) ? fragWorldPos.z : fragWorldPos.x;
        float yA = yAbove;

        #define H11(n) fract(sin((n) * 12.9898) * 43758.5453)
        #define H21(p) fract(sin(dot((p), vec2(127.1, 311.7))) * 43758.5453)

        // Multi-octave value noise — used to warp blob outlines
        // (irregular silhouettes, not circles) and to break up
        // the body of each splat with internal texture.
        vec2 nP = vec2(u, yA);
        float n1 = H21(nP * 4.5);
        float n2 = H21(nP * 11.0 + 17.3);
        float n3 = H21(nP * 26.0 + 5.7);
        float fbm = n1 * 0.55 + n2 * 0.30 + n3 * 0.15;

        // ---- Splat blobs ----
        // Two passes at different cell pitches. Big splats sit
        // at ~30 cm pitch (one major impact per stride along
        // the wall), small satellites at ~12 cm. Each pass
        // examines its three nearest cells along the u axis,
        // so blobs straddle cell boundaries naturally.
        float blobAcc = 0.0;
        float dripAcc = 0.0;

        // Pass 0: big splats. radius 0.10–0.22 m.
        // Pass 1: small splats. radius 0.04–0.09 m.
        for (int pass = 0; pass < 2; pass++) {
            float cellSize = pass == 0 ? 0.30 : 0.12;
            float rMin     = pass == 0 ? 0.10 : 0.04;
            float rVar     = pass == 0 ? 0.12 : 0.05;
            float covGate  = pass == 0 ? 1.40 : 1.60;
            // Drip strength scales by pass; big blobs drip
            // hard, small blobs barely.
            float dripStrength = pass == 0 ? 1.0 : 0.45;
            float seed     = pass == 0 ? 13.0 : 91.0;

            float baseCellId = floor(u / cellSize);
            for (int i = -1; i <= 1; i++) {
                float cellId = baseCellId + float(i);
                vec2 cc = vec2(cellId, seed);

                // Existence gate: coverage at this fragment must
                // clear a hashed threshold for the cell to host
                // a blob. This ties blob density to the field
                // signal strength — a heavy splatter column
                // hosts many blobs, a light one hosts few.
                float pres = step(H21(cc * 5.7), coverage * covGate);
                if (pres < 0.5) continue;

                // Blob centre. u offset within the cell + Y
                // hashed around chest height ±35 cm (so blobs
                // don't all line up at the same height).
                float cu = (cellId + 0.18 + H21(cc * 1.7) * 0.64) * cellSize;
                float cy = 0.95 + (H21(cc * 2.3) - 0.5) * 0.70;

                // Radius hashed.
                float r = rMin + H21(cc * 3.1) * rVar;
                // Aspect — slight vertical stretch (gravity pull
                // before drying) to taste.
                float aspectY = 0.85 + H21(cc * 4.7) * 0.40;

                // Distance to centre with FBM-driven warp so the
                // outline is irregular, not a perfect ellipse.
                vec2 d = vec2(u - cu, (yA - cy) / aspectY);
                float warp = (fbm - 0.5) * r * 0.55;
                float dist = length(d) + warp;
                float body = 1.0 - smoothstep(r * 0.65, r * 1.10, dist);
                // Internal texture — slightly thinner inside the
                // blob so it doesn't read as a flat fill.
                body *= 0.75 + 0.25 * fbm;
                blobAcc = max(blobAcc, body);

                // ---- Drip trails from this blob ----
                // Drips emerge from the *bottom* of the blob.
                // belowDist measures how far the fragment sits
                // below blob centre.
                float belowDist = cy - yA;
                if (belowDist > 0.0 && belowDist < 0.80) {
                    // Each blob spawns up to ~3 narrow drip
                    // streaks within ±r of its centre. Streaks
                    // live in fine 8 mm columns.
                    float colId = floor((u - cu) / 0.008);
                    float colCenter = (colId + 0.5) * 0.008 + cu;
                    // Per-streak hashed length and presence.
                    vec2 sc = vec2(colId, seed * 3.7);
                    float dripLen = 0.18 + H21(sc * 7.3) * 0.50;
                    // Presence: streak only fires if it sits
                    // within the blob's lateral footprint AND
                    // clears a coverage-modulated threshold AND
                    // its column hash is above a sparsity gate
                    // (so we don't get a continuous curtain).
                    float lateralFromCenter = abs(colCenter - cu);
                    float lateralOK = step(lateralFromCenter, r * 0.85);
                    float streakPres = step(H21(sc * 11.1),
                                            coverage * 0.55 * dripStrength);
                    if (lateralOK > 0.5
                        && streakPres > 0.5
                        && belowDist < dripLen) {
                        // Width: tapers from ~half a column at
                        // the top to ~third at the bottom.
                        float dripT = belowDist / max(dripLen, 1e-3);
                        float taper = mix(1.0, 0.45, dripT);
                        float streakW = 0.0035 * taper;
                        // Wobble: gentle horizontal drift.
                        float wobble = sin((colId * 0.71)
                                           + dripT * 11.0) * 0.0012;
                        float streakDist = abs(u - colCenter - wobble);
                        float streakBody = 1.0 - smoothstep(
                            streakW, streakW * 1.6, streakDist);
                        // Bead at the leading edge.
                        float bead = 1.0 - smoothstep(
                            0.0, 0.06, abs(dripT - 0.92));
                        bead *= 1.0 - smoothstep(0.005, 0.010,
                                                 streakDist);
                        // Fade in just below blob, fade out at
                        // tail.
                        float aliveY = smoothstep(0.0, 0.020,
                                                  belowDist)
                                     * (1.0 - smoothstep(
                                         dripLen - 0.020,
                                         dripLen, belowDist));
                        float drip = (streakBody + bead * 0.6)
                                    * aliveY * dripStrength;
                        dripAcc = max(dripAcc, drip);
                    }
                }
            }
        }

        heightMask = max(blobAcc, dripAcc);

        // ----- Capillary contact pooling -----
        // Where the wall meets a bloodied floor, real fluids
        // climb the wall via surface tension and gather along
        // the join. We add a thin, very wet, slightly darkened
        // strip at the bottom of the wall whose intensity is
        // gated by the floor's coverage in this column. Reads
        // as a wet line tracing the corner where wall touches
        // bloody floor — exactly the contact cue real fluids
        // produce.
        float contactPool = (1.0 - smoothstep(0.0, 0.06, yA))
                          * smoothstep(0.0, 0.005, yA);
        // Modulate by base coverage so dry columns stay clean.
        contactPool *= clamp(coverage * 0.9, 0.0, 1.0);
        heightMask = max(heightMask, contactPool);

        // Hard upper cap: nothing above 2.0 m.
        heightMask *= 1.0 - smoothstep(1.85, 2.05, yA);
        // Soft lower edge: blend into floor pool seamlessly.
        heightMask *= smoothstep(-0.04, 0.04, yA);

        // Wall coverage usually arrives a bit weaker than floor
        // coverage from the same kill (rays scatter), so push
        // it up a touch so the wall splatter reads at parity
        // with the pool below it.
        coverage = clamp(coverage * 1.4, 0.0, 1.0);

        #undef H11
        #undef H21
    } else {
        heightMask = 1.0;
    }

    if (heightMask < 0.02) return;
    coverage *= heightMask;
    if (coverage < 0.01) return;

    float age = max(0.0, ubo.timeData.x - bloodSample.g);
    // Stay vivid — wet phase out to 45s, then a long dried tail. The
    // overlap means there's a window where blood is partly tacky
    // (still red, no longer mirror-glossy) which sells the
    // "recently bled" read at typical play pacing.
    float wet = coverage * (1.0 - smoothstep(0.0, 45.0, age));
    float dry = smoothstep(35.0, 120.0, age);

    // ----- Crease-aware accumulation (floor only) -----
    // Real spilled blood pools wherever the surface dips —
    // grout lines, cracks, mortar gaps, divots in worn stone.
    // Rather than hardcoding an axis-aligned tile grid (which
    // doesn't match the diagonal layout of the desert-rocks
    // tiles we ship and looks square on any other floor pack),
    // we derive a "crease mask" from the normal map itself:
    // wherever the perturbed normal `N` deviates from the
    // geometric `Ngeo`, the fragment is sitting on a slope —
    // a crevice or tile edge. Blood pooled in those creases
    // reads thicker, darker, and stays wet longer, while
    // raised tile faces (where N ≈ Ngeo) dry first.
    //
    // This works for any normal-mapped floor surface and
    // automatically follows the texture's actual layout
    // direction. Falls back to no modulation if the floor's
    // normal map is flat (e.g. a procedural floor that hasn't
    // installed a real PBR pack yet).
    float groutBoost = 1.0;
    float centreFade = 1.0;
    if (isFloor) {
        // Normal deviation from geometric: 0 on flat tile
        // faces, > 0 on slopes / crevices. Cube the value so
        // only sharper slope angles register — flat faces
        // don't accidentally pick up tiny normal-map noise.
        float ndev = 1.0 - clamp(dot(N, Ngeo), 0.0, 1.0);
        float creaseMask = smoothstep(0.04, 0.30, ndev);
        // Pooling: creases add up to +75 % wet weight.
        groutBoost = 1.0 + 0.75 * creaseMask;
        // Tile-face fade: flat areas dry by up to ~25 %.
        centreFade = 1.0 - 0.25 * (1.0 - creaseMask);

        // ----- Contact accumulation against vertical
        //       geometry -----
        // Where a wall, pillar or prop meets the floor, fluids
        // gather along the contact ring (a few millimetres of
        // capillary creep). We detect "I am a floor pixel
        // adjacent to a non-floor pixel" by looking at the
        // screen-space derivative of the geometric normal —
        // wherever |dNgeo|/|dpos| spikes, the floor surface is
        // ending against a vertical face within one pixel.
        // The detector is cheap (two ddx/ddy taps already done
        // implicitly by the GPU) and surface-agnostic, so it
        // catches barrel feet, pillar bases, character feet,
        // and prop touch-downs without per-object setup.
        vec3 dNx = dFdx(Ngeo);
        vec3 dNy = dFdy(Ngeo);
        float nGrad = length(dNx) + length(dNy);
        float contactRing = smoothstep(0.20, 1.20, nGrad);
        // Pooling boost: contact rings are 1.5× the inner crease
        // multiplier so a wet floor visibly thickens against
        // every vertical it meets.
        groutBoost *= mix(1.0, 1.8, contactRing);
        // And drying slows in the ring, since the contact line
        // is shaded and shielded from airflow.
        centreFade *= mix(1.0, 1.15, contactRing);
    }
    wet *= groutBoost * centreFade;
    // Drying advances faster on flat tile faces (centreFade < 1
    // ↔ higher dry).
    dry = clamp(dry * (2.0 - centreFade), 0.0, 1.0);

    // Fresh blood: vivid arterial red. Dried blood: warm iron-rust
    // brown. Both are sRGB-decoded values; the forward target is
    // linear so we don't need a manual pow(2.2). The fresh tone is
    // intentionally bright — the post-pipeline ACES tonemap pulls
    // saturated reds toward orange, so we overshoot here to land on
    // a deep, readable blood-red on screen.
    vec3 fresh = vec3(0.62, 0.04, 0.03);
    vec3 dried = vec3(0.20, 0.07, 0.05);
    vec3 bloodAlbedo = mix(fresh, dried, dry);

    // Coverage controls how much of the underlying floor albedo is
    // overwritten. A small floor-show-through stays even at full
    // coverage so the silhouette doesn't read as a flat sticker.
    //
    // ----- Edge-hardness modulation -----
    // Real spilled blood has thin feathered outskirts, but also
    // sharp coagulated ridges, tiny islands, and torn-paper
    // breakup where surface tension pulls the surface apart.
    // The raw `coverage` field gives uniform Gaussian-style edges,
    // which read as soft and rubbery. We modulate the edge
    // sharpness with high-frequency hash noise so different
    // sections of the same perimeter have different falloffs:
    //
    //   * `edgeBand` is 1 in the rim transition (where coverage
    //     is sliding from 0 → 1) and 0 inside the body and far
    //     outside.
    //   * `edgeNoise` is a tile-aware hash on world XZ so the
    //     pattern is stable across frames and isn't visibly
    //     animated.
    //   * `coagWidth` shifts the edge from a wide soft falloff
    //     (where the noise is low — feathered, blood seeped
    //     into porous stone) to a tight hard cliff (where it's
    //     high — dried surface tension ridge / coagulated rim).
    //
    // The result reads as a varied perimeter with both crusty
    // ridges and feathered outskirts, with ragged broken islands
    // where the dither hash tips the threshold past the body.
    float covRaw = coverage;
    float edgeBand = smoothstep(0.04, 0.50, covRaw)
                   * (1.0 - smoothstep(0.50, 0.95, covRaw));
    if (isFloor && edgeBand > 0.001) {
        // Two-octave hash-noise on world XZ for high-frequency
        // breakup. World-space so the pattern doesn't crawl
        // across the floor as the field UV shifts.
        vec2 nP = vec2(fragWorldPos.x, fragWorldPos.z);
        #define H21(p) fract(sin(dot((p), vec2(127.1, 311.7))) * 43758.5453)
        float hN = H21(nP * 22.0);
        float hN2 = H21(nP * 7.0 + 13.7);
        float edgeNoise = hN * 0.7 + hN2 * 0.3;
        // Map noise to a remap centre and width. Width swings
        // from 0.18 (soft feathered) to 0.04 (sharp coag rim).
        float coagWidth = mix(0.18, 0.04, edgeNoise);
        float coagCenter = mix(0.32, 0.55, edgeNoise);
        float remapped = smoothstep(
            coagCenter - coagWidth,
            coagCenter + coagWidth,
            covRaw);
        // Stochastic islands: a small number of isolated
        // fragments pop *out* of the body just inside the rim,
        // and a few isolated splashes pop *into* coverage just
        // outside it. Simulates surface tension breakup.
        float islandHash = H21(nP * 95.0 + 4.1);
        // Pull-in: where the high-freq hash is very low along
        // the inner rim, force coverage to zero (a tear).
        float tearMask = smoothstep(0.10, 0.45, covRaw)
                       * (1.0 - smoothstep(0.45, 0.70, covRaw));
        if (tearMask > 0.001 && islandHash < 0.05) {
            remapped *= 1.0 - tearMask * 0.8;
        }
        // Push-out: where the hash is high just outside the
        // rim, let a stray drop fragment through.
        if (covRaw > 0.005 && covRaw < 0.10 && islandHash > 0.985) {
            remapped = max(remapped, 0.55);
        }
        coverage = mix(covRaw, remapped, edgeBand);
        #undef H21
    }
    float cov = clamp(coverage * 1.3, 0.0, 1.0);
    albedo = mix(albedo, bloodAlbedo, cov * 0.92);
    // Grout pools darker — multiply albedo down where groutBoost
    // exceeds 1.0 so blood that settled into cracks reads as a
    // slightly thicker, deeper-coloured streak.
    albedo *= mix(1.0, 0.78, clamp((groutBoost - 1.0) * 1.6, 0.0, 1.0));

    // Roughness: wet pools are glassy (~0.12), dried blood is slightly
    // rougher than the floor it sits on but not chalky. The lerp below
    // sweeps the wet end through to the dried end as the age advances.
    float bloodRoughness = mix(0.12, 0.55, dry);

    // Coagulated rim: the perimeter of a fresh pool dries first
    // (more surface area exposed to air per unit volume) and
    // forms a slightly tackier, matter ring. We bump the
    // roughness toward the dried value within the edge band so
    // the centre of every pool stays glossy while the rim reads
    // as crusted. Also nudge the albedo slightly darker at the
    // rim — coagulated blood is a deeper purple-black than the
    // wet body.
    float rimMask = isFloor
        ? smoothstep(0.55, 0.95, covRaw) * (1.0 - smoothstep(0.95, 1.0, covRaw))
        : 0.0;
    rimMask = 1.0 - rimMask; // 1 at the rim band, 0 in body
    rimMask *= smoothstep(0.10, 0.45, covRaw);
    bloodRoughness = mix(bloodRoughness, mix(0.55, 0.80, dry), rimMask * 0.7);
    bloodAlbedo = mix(bloodAlbedo, bloodAlbedo * 0.55, rimMask * 0.5);

    // ----- Localised high-gloss streaks -----
    // Wet liquid surfaces don't show a uniform gloss response —
    // surface tension + thin-film thickness variation produces
    // razor-thin specular streaks and broken reflective patches
    // ("wet veins" in the puddle) that the eye subconsciously
    // expects. We modulate the roughness with stretched
    // world-space hash noise: anisotropic frequencies (low along
    // X, high along Z) make the noise read as elongated streaks
    // rather than isotropic speckle, mimicking the look of an
    // anisotropic BRDF without the cost. Magnitude is gated by
    // wetness so dry blood stays uniformly matte.
    if (isFloor) {
        #define H21S(p) fract(sin(dot((p), vec2(127.1, 311.7))) * 43758.5453)
        vec2 wp = vec2(fragWorldPos.x, fragWorldPos.z);
        // Two stretched octaves. The first creates broad
        // streaks, the second adds smaller broken patches on
        // top so the gloss response has both long veins and
        // tiny hot reflections.
        float streak1 = H21S(vec2(wp.x * 6.0, wp.y * 32.0) + 1.7);
        float streak2 = H21S(vec2(wp.x * 14.0, wp.y * 70.0) + 9.3);
        float streakNoise = streak1 * 0.65 + streak2 * 0.35;
        // Multiply roughness by a 0.55 .. 1.45 sweep to push
        // wet streaks down to mirror gloss and dry stretches
        // up to a tackier sheen — same average, far more
        // varied per-fragment.
        float streakMul = mix(0.55, 1.45, streakNoise);
        // Only modulate the wet end of the response (1 - dry)
        // so an old stain doesn't sparkle; ramp by `cov` so
        // the streaks live inside the splat, not on the
        // surrounding floor.
        float streakAmp = (1.0 - dry) * cov;
        bloodRoughness *= mix(1.0, streakMul, streakAmp);
        #undef H21S
    }

    roughness = clamp(mix(roughness, bloodRoughness, cov), 0.04, 1.0);

    // Blood is dielectric — kill metallic contribution wherever the
    // floor was metallic (rare, but cleans up edge cases).
    metallic *= (1.0 - cov * 0.95);

    // Normal bevel from the gradient of wet coverage. Only worth doing
    // while the pool is still wet AND we're on the floor — the
    // gradient is in world XZ, which is meaningless for vertical
    // surfaces.
    if (isFloor && wet > 0.05) {
        // Sample 4 neighbours at ~1 texel offset for a finite-difference
        // gradient. The texture is 1024² so 1/1024 in UV is one texel.
        vec2 ts = vec2(1.0 / 1024.0);
        float wL = texture(bloodField, sampleUV + vec2(-ts.x, 0)).r;
        float wR = texture(bloodField, sampleUV + vec2( ts.x, 0)).r;
        float wD = texture(bloodField, sampleUV + vec2(0, -ts.y)).r;
        float wU = texture(bloodField, sampleUV + vec2(0,  ts.y)).r;
        // Gradient in world XZ. The puddle is high in the middle and
        // low at its edges, so the bevel normal tilts outward.
        vec2 grad = vec2(wR - wL, wU - wD);
        // Bevel strength scales with wetness so dried blood stays flat.
        float bevel = 0.55 * wet;
        vec3 nWorld = normalize(N);
        // Push normal away from the puddle centre by translating
        // along world X / Z. We project this delta onto the tangent
        // plane of the geometric normal so we never invert the
        // facing.
        vec3 tilt = vec3(grad.x, 0.0, grad.y) * bevel;
        tilt -= Ngeo * dot(tilt, Ngeo);

        // Micro undulations: low-frequency noise on world XZ
        // adds tiny meniscus bulges + uneven pooling so the
        // surface reads as "fluid sitting on the floor" rather
        // than "wet floor". Driven by central-difference of a
        // procedural height field so adjacent fragments share a
        // consistent gradient (no per-fragment hash sparkle).
        // Amplitudes intentionally tiny — visible as glints
        // moving with the camera, not as a bumpy mess.
        if (cov > 0.05) {
            vec2 wp = vec2(fragWorldPos.x, fragWorldPos.z);
            // Two scales of noise — large lazy undulation at
            // ~12 cm and a finer ripple at ~4 cm. Both use
            // smooth value-noise (cubic Hermite interp on a
            // hashed lattice) so the gradient is C1.
            #define HASH(p) fract(sin(dot((p), vec2(127.1, 311.7))) * 43758.5453)
            vec2 wp1 = wp * 8.0;   // ~12.5 cm period
            vec2 wp2 = wp * 24.0;  // ~4 cm period
            // Coarse hash gradient (forward differences).
            float h1c = HASH(floor(wp1));
            float h1x = HASH(floor(wp1 + vec2(1.0, 0.0)));
            float h1y = HASH(floor(wp1 + vec2(0.0, 1.0)));
            vec2 g1 = vec2(h1x - h1c, h1y - h1c);
            float h2c = HASH(floor(wp2) + 17.3);
            float h2x = HASH(floor(wp2 + vec2(1.0, 0.0)) + 17.3);
            float h2y = HASH(floor(wp2 + vec2(0.0, 1.0)) + 17.3);
            vec2 g2 = vec2(h2x - h2c, h2y - h2c);
            // Combined micro-tilt. Amplitude attenuates as the
            // pool dries (wet × cov gate) so old blood reads
            // flat the way it should.
            vec2 microGrad = (g1 * 0.6 + g2 * 0.4);
            float microAmp = 0.04 * wet;
            vec3 microTilt = vec3(microGrad.x, 0.0, microGrad.y) * microAmp;
            microTilt -= Ngeo * dot(microTilt, Ngeo);
            tilt += microTilt;
            #undef HASH
        }

        N = normalize(nWorld + tilt);
    }
}
