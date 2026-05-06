//! Stable widget identity.
//!
//! Every interactive widget needs an `Id` so the [`UiState`] can
//! remember which one is focused, hovered, or being dragged across
//! frames. `Id` is a 64-bit hash derived by mixing parent ids with
//! a per-call seed, so identical hierarchies in different parts of
//! the screen don't collide and loops can salt with an index.
//!
//! ```ignore
//! let panel = Id::root("inventory");
//! let slot  = panel.child(("bag", row, col));
//! ```
//!
//! Pure decorative draws (rects, labels) don't need an `Id`.

use std::hash::{Hash, Hasher};

/// Stable identifier for an interactive widget across frames.
///
/// Cheap to create (one `FxHasher::write_u64` call per `child`),
/// `Copy`, and `Eq`/`Hash` so it can live in [`UiState`] sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Id(pub u64);

impl Id {
    /// Sentinel "no widget" id. Used by [`UiState`] as the default
    /// for `focus` / `hovered_last_frame` so the equivalence check
    /// `state.focus == Some(my_id)` stays uniform.
    pub const NONE: Id = Id(0);

    /// Root id for a top-level surface (HUD, inventory panel, …).
    /// `seed` is typically a string literal naming the surface.
    pub fn root<H: Hash>(seed: H) -> Self {
        let mut h = FxHasher::default();
        seed.hash(&mut h);
        // Bias away from 0 so a hash collision with `NONE` is
        // statistically impossible without breaking valid hashes
        // that happen to land on 1.
        Self((h.finish() ^ 0x9E37_79B9_7F4A_7C15).wrapping_add(1))
    }

    /// Derive a child id by mixing this id's hash with a `seed`.
    /// Use loop indices as the seed inside `for` bodies to keep
    /// per-iteration ids distinct.
    pub fn child<H: Hash>(self, seed: H) -> Self {
        let mut h = FxHasher::default();
        h.write_u64(self.0);
        seed.hash(&mut h);
        Self(h.finish().wrapping_add(1))
    }
}

/// Tiny FxHash-style hasher. Avoids the `std` SipHash overhead on
/// the per-frame id-mixing path. Not cryptographic — that's fine,
/// the hash never escapes the UI layer.
#[derive(Default)]
struct FxHasher(u64);

impl Hasher for FxHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u8(b);
        }
    }
    fn write_u8(&mut self, b: u8) {
        self.0 = (self.0.rotate_left(5) ^ b as u64).wrapping_mul(0x517C_C1B7_2722_0A95);
    }
    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x517C_C1B7_2722_0A95);
    }
}
