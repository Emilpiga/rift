//! Tiny seeded xorshift64 RNG for loot rolls.
//!
//! Local utility — `rift-game` already avoids the `rand` dep, and
//! loot rolling is the only place inside the crate that needs
//! randomness, so we ship a focused 30-line PRNG instead of pulling
//! in another crate.

pub struct LootRng {
    state: u64,
}

impl LootRng {
    pub fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0x9E37_79B9_7F4A_7C15
            } else {
                seed
            },
        }
    }

    /// Raw 64-bit xorshift output.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// Uniform `f32` in `[0.0, 1.0)`.
    pub fn next_f32(&mut self) -> f32 {
        // Top 24 bits → unit float — avoids the bias of modulo.
        ((self.next_u64() >> 40) as u32) as f32 / (1u32 << 24) as f32
    }

    /// Uniform `f32` in `[lo, hi)`.
    pub fn frange(&mut self, lo: f32, hi: f32) -> f32 {
        if hi <= lo {
            return lo;
        }
        lo + (hi - lo) * self.next_f32()
    }

    /// Uniform integer in `[lo, hi)`.
    pub fn range(&mut self, lo: u32, hi: u32) -> u32 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next_u64() % (hi - lo) as u64) as u32
    }

    /// Pick an index from `weights` proportional to its weight.
    /// Returns `None` if `weights` is empty or all zero.
    pub fn weighted_index(&mut self, weights: &[u32]) -> Option<usize> {
        let total: u32 = weights.iter().sum();
        if total == 0 {
            return None;
        }
        let mut pick = self.range(0, total);
        for (i, &w) in weights.iter().enumerate() {
            if pick < w {
                return Some(i);
            }
            pick -= w;
        }
        None
    }
}
