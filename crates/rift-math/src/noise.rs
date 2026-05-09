//! Tiny, dependency-free procedural noise primitives.
//!
//! These exist so any crate in the workspace (terrain, sky,
//! VFX, gameplay perturbations, …) can sample reproducible
//! pseudo-random fields without pulling in a 3rd-party noise
//! library and without bloating each call site with its own
//! ad-hoc hash.
//!
//! Determinism guarantees:
//!
//! - Every function below is a pure function of its `seed` and
//!   coordinate inputs. No global state, no thread-local RNGs.
//! - Scalar mixing uses fixed integer constants so the output
//!   is bit-exact stable across machines and Rust versions.
//! - The `f32` arithmetic we *do* use (lerps, smoothsteps) is
//!   IEEE-754 single precision and identical across x86 / ARM
//!   for the operands we feed it.
//!
//! Why not Perlin/Simplex from a crate?
//!
//! - This module is deliberately small (≈100 lines) and easy
//!   to audit; the workspace can pin its terrain look without
//!   waking up to a `noise = "0.9"` semver bump that subtly
//!   shifts every mountain.
//! - Value-noise + fbm is plenty for the scales we use
//!   (distant terrain silhouettes, gradient overlays). Simplex
//!   buys us nothing visually at these distances.

/// 32-bit integer hash used as the seed for a single lattice
/// cell. SplitMix64-style avalanche with a 64-bit mix; folds
/// `(x, y, seed)` into a uniform 32-bit integer.
#[inline]
pub fn hash2(x: i32, y: i32, seed: u64) -> u32 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= (y as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = h.wrapping_add(seed);
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h >> 31;
    h as u32
}

/// `[0.0, 1.0)` sample for a single lattice cell.
#[inline]
fn cell01(x: i32, y: i32, seed: u64) -> f32 {
    // 24-bit mantissa is enough; matching `rand`'s `gen` for
    // f32. Using the high bits of the hash gives better mixing
    // than the low bits.
    (hash2(x, y, seed) >> 8) as f32 / ((1u32 << 24) as f32)
}

/// Cubic Hermite smoothstep applied componentwise. Matches the
/// scalar `crate::smoothstep` but avoids the cross-module hop
/// for the inner noise loop.
#[inline]
fn fade(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// 2D bilinear value noise on an integer lattice, faded with a
/// cubic Hermite. Output range is `[0.0, 1.0]`.
///
/// The lattice spacing is 1 unit — caller scales coordinates
/// to control feature size (e.g. `value_noise2(x*0.05, …)`
/// produces 20-unit-wide features).
pub fn value_noise2(x: f32, y: f32, seed: u64) -> f32 {
    let xi = x.floor() as i32;
    let yi = y.floor() as i32;
    let xf = x - xi as f32;
    let yf = y - yi as f32;
    let u = fade(xf);
    let v = fade(yf);
    let a = cell01(xi, yi, seed);
    let b = cell01(xi + 1, yi, seed);
    let c = cell01(xi, yi + 1, seed);
    let d = cell01(xi + 1, yi + 1, seed);
    let ab = a + (b - a) * u;
    let cd = c + (d - c) * u;
    ab + (cd - ab) * v
}

/// Fractional Brownian motion: octaves of [`value_noise2`] at
/// successively higher frequencies and lower amplitudes,
/// renormalised to `[0.0, 1.0]`.
///
/// `lacunarity = 2.0`, `gain = 0.5` is the classic mountain-y
/// shape. Higher gain (≈0.65) gives rougher, more cumulus-y
/// fields; lower (≈0.35) gives gentle rolling hills.
pub fn fbm2(x: f32, y: f32, seed: u64, octaves: u32, lacunarity: f32, gain: f32) -> f32 {
    let mut amp = 1.0_f32;
    let mut freq = 1.0_f32;
    let mut sum = 0.0_f32;
    let mut norm = 0.0_f32;
    for o in 0..octaves {
        // Decorrelate octaves by perturbing the seed per band
        // — without this the same lattice phase prints through
        // every octave and produces visible axis-aligned
        // streaks at the top-frequency band.
        let s = seed.wrapping_add((o as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        sum += value_noise2(x * freq, y * freq, s) * amp;
        norm += amp;
        amp *= gain;
        freq *= lacunarity;
    }
    sum / norm.max(1e-6)
}

/// Ridged-multifractal variant of [`fbm2`]. Folds each octave
/// around 0.5 so the lattice midline becomes a sharp ridge,
/// giving the classic alpine "spine of rock" look. Output
/// range is `[0.0, 1.0]`.
///
/// Combine with [`fbm2`] (e.g. `0.6 * ridged + 0.4 * fbm`) to
/// keep the silhouette interesting without every peak looking
/// identical.
pub fn ridged_fbm2(
    x: f32,
    y: f32,
    seed: u64,
    octaves: u32,
    lacunarity: f32,
    gain: f32,
) -> f32 {
    let mut amp = 1.0_f32;
    let mut freq = 1.0_f32;
    let mut sum = 0.0_f32;
    let mut norm = 0.0_f32;
    for o in 0..octaves {
        let s = seed.wrapping_add((o as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9));
        let n = value_noise2(x * freq, y * freq, s);
        // Fold and square so the ridge is C¹ at the spine.
        let r = 1.0 - (n * 2.0 - 1.0).abs();
        sum += r * r * amp;
        norm += amp;
        amp *= gain;
        freq *= lacunarity;
    }
    sum / norm.max(1e-6)
}
