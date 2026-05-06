//! Floor lifecycle: generate the dungeon for a `(seed, index)` pair
//! and reset combat state on transitions.
//!
//! Floor 0 is the safe hub; everything else is rift content. The
//! seed mixing here mirrors the SP code so identical `(seed, index)`
//! pairs produce identical layouts on either side.

use rift_dungeon::{Floor, FloorConfig};

/// Build the `Floor` geometry for the given seed/index pair.
pub fn generate(seed: u64, index: u32) -> Floor {
    if index == 0 {
        Floor::hub()
    } else {
        let mixed = seed + index as u64 * 7;
        Floor::generate(FloorConfig::for_floor(index), mixed)
    }
}
