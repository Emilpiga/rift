//! Walkability bitmap derived from a [`Floor`]. Used by the
//! client minimap and any future system that needs a flat
//! "is this tile walkable" lookup without re-scanning
//! `Floor::tiles`.

use crate::{Floor, Tile};

#[derive(Clone)]
pub struct NavGrid {
    pub width: usize,
    pub depth: usize,
    walkable: Vec<bool>,
}

impl NavGrid {
    pub fn from_floor(floor: &Floor) -> Self {
        let walkable: Vec<bool> = floor.tiles.iter().map(|t| *t == Tile::Floor).collect();
        Self {
            width: floor.width,
            depth: floor.depth,
            walkable,
        }
    }

    #[inline]
    pub fn is_walkable(&self, x: usize, z: usize) -> bool {
        if x >= self.width || z >= self.depth {
            return false;
        }
        self.walkable[z * self.width + x]
    }
}
