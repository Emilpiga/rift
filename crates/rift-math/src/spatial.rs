//! Cheap 2D uniform spatial hash for proximity queries over a
//! sparse set of XZ-positioned items. Designed for the per-tick
//! "given this point + radius, what items are nearby?" pattern
//! that turns into an O(N²) hot loop the moment N hits a few
//! hundred — enemy separation steering, projectile↔enemy
//! collision, AoE-zone damage sweeps, etc.
//!
//! The grid is index-based on purpose: it does NOT store the
//! caller's payload (which may be borrowed, large, or already
//! living in some other slice). Instead [`SpatialGrid::build`]
//! sees each input via a position extractor closure and records
//! only its `u32` index. Queries return `u32` indices the caller
//! looks up against the original slice. This keeps the grid's
//! memory footprint flat and makes it trivially reusable across
//! `(Entity, Vec3, NetId, f32)` slices, `(NetId, Vec3)` slices,
//! `Vec<MyThing>`, etc., without generics noise.
//!
//! Y is ignored throughout — the dungeon is effectively a
//! horizontal grid and every existing caller queries against an
//! XZ disc. Adding a vertical axis would just bloat empty cells.
//!
//! Coordinates are signed: `(pos / cell_size).floor() as i32`
//! works fine for negative world coordinates. Cells use std
//! `HashMap` keyed on the integer pair — fine for the few
//! hundred occupied cells we ever see; if profiling ever flags
//! the hash itself we can swap in a denser representation.

use std::collections::HashMap;

use glam::Vec3;

/// Uniform XZ spatial hash. Build once per tick from the caller's
/// item slice; query repeatedly within the same tick.
///
/// Pick `cell_size` close to the *typical* query radius. Queries
/// with a larger radius still work — they just scan a wider cell
/// neighbourhood — but very large radii relative to the cell
/// size approach O(N) and lose the speedup.
#[derive(Debug, Clone)]
pub struct SpatialGrid {
    cell_size: f32,
    inv_cell: f32,
    cells: HashMap<(i32, i32), Vec<u32>>,
}

impl SpatialGrid {
    /// Build a fresh grid from `items`, using `pos` to read each
    /// item's XZ position. `cell_size` must be > 0; pick it close
    /// to the typical query radius (separation radius, projectile
    /// collision radius, etc.).
    pub fn build<T>(items: &[T], cell_size: f32, mut pos: impl FnMut(&T) -> Vec3) -> Self {
        assert!(cell_size > 0.0, "SpatialGrid cell_size must be > 0");
        let inv = 1.0 / cell_size;
        // Capacity guess: most cells hold a handful of items, so
        // half the input count is a reasonable starting point.
        // Rehash cost is negligible at AI-tick frequencies.
        let mut cells: HashMap<(i32, i32), Vec<u32>> =
            HashMap::with_capacity(items.len().max(8) / 2);
        for (idx, item) in items.iter().enumerate() {
            let p = pos(item);
            let cx = (p.x * inv).floor() as i32;
            let cz = (p.z * inv).floor() as i32;
            cells.entry((cx, cz)).or_default().push(idx as u32);
        }
        Self {
            cell_size,
            inv_cell: inv,
            cells,
        }
    }

    /// The cell size the grid was built with.
    #[inline]
    pub fn cell_size(&self) -> f32 {
        self.cell_size
    }

    /// Iterate every item index whose owning cell intersects an
    /// XZ disc of radius `radius` centred on `center`. Y is
    /// ignored. The iterator may yield indices whose actual
    /// distance is slightly greater than `radius` (cell-rounding
    /// slop on the edges) — the caller is expected to do the
    /// exact distance test on each candidate.
    ///
    /// Walking is bounded to `ceil(radius / cell_size)` cells in
    /// each direction, so the query stays O(occupied_cells_in_disc)
    /// regardless of the global item count.
    pub fn query_radius<'a>(&'a self, center: Vec3, radius: f32) -> impl Iterator<Item = u32> + 'a {
        let cx = (center.x * self.inv_cell).floor() as i32;
        let cz = (center.z * self.inv_cell).floor() as i32;
        // Round up so cells straddling the disc boundary are
        // included. Minimum 1 covers the corner case where the
        // caller queries with radius < cell_size — we still
        // need to look at the 3×3 neighbourhood.
        let r = ((radius * self.inv_cell).ceil() as i32).max(1);
        (-r..=r)
            .flat_map(move |dz| (-r..=r).filter_map(move |dx| self.cells.get(&(cx + dx, cz + dz))))
            .flat_map(|bucket| bucket.iter().copied())
    }
}

/// Sum repulsion vectors from every neighbour inside `radius` of
/// `pos`, using `grid` to avoid an O(N²) all-pairs scan. The
/// falloff is linear: `(radius - d) / radius`, which is the
/// classic boids-style separation curve — touching neighbours
/// produce a unit-magnitude push, neighbours just inside the
/// boundary produce nearly none.
///
/// `items` is the slice the grid was built against. `pos_of`
/// reads each item's XZ position; `skip` returns `true` for
/// items that must be excluded (typically the querying entity
/// itself, plus anything inert like dying mobs). Y is ignored
/// throughout — the push vector's `y` component is always 0.
///
/// Degenerate exact-overlap pairs (`d² < 1e-6`) are skipped so a
/// pair of stacked entities doesn't divide by zero. The caller
/// scales the returned vector to taste (e.g. by `move_speed *
/// strength_constant`).
pub fn separation_push<T>(
    grid: &SpatialGrid,
    pos: Vec3,
    radius: f32,
    items: &[T],
    mut pos_of: impl FnMut(&T) -> Vec3,
    mut skip: impl FnMut(&T) -> bool,
) -> Vec3 {
    let mut push = Vec3::ZERO;
    let r2 = radius * radius;
    for idx in grid.query_radius(pos, radius) {
        let item = &items[idx as usize];
        if skip(item) {
            continue;
        }
        let npos = pos_of(item);
        let dx = pos.x - npos.x;
        let dz = pos.z - npos.z;
        let d2 = dx * dx + dz * dz;
        if d2 >= r2 || d2 < 1.0e-6 {
            continue;
        }
        let d = d2.sqrt();
        let weight = (radius - d) / radius;
        push += Vec3::new(dx / d, 0.0, dz / d) * weight;
    }
    push
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_grid_has_no_neighbours() {
        let items: Vec<Vec3> = Vec::new();
        let g = SpatialGrid::build(&items, 1.0, |&p| p);
        assert_eq!(g.query_radius(Vec3::ZERO, 5.0).count(), 0);
    }

    #[test]
    fn finds_items_inside_radius() {
        let items = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.5, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 0.0),
        ];
        let g = SpatialGrid::build(&items, 1.0, |&p| p);
        let hits: Vec<u32> = g.query_radius(Vec3::ZERO, 1.0).collect();
        // Indices 0 and 1 must be reachable; index 2 is far
        // enough that even with cell slop it won't be in the
        // 3×3 around the origin cell.
        assert!(hits.contains(&0));
        assert!(hits.contains(&1));
        assert!(!hits.contains(&2));
    }

    #[test]
    fn negative_coordinates_work() {
        let items = [Vec3::new(-5.0, 0.0, -5.0), Vec3::new(-4.7, 0.0, -5.2)];
        let g = SpatialGrid::build(&items, 1.0, |&p| p);
        let hits: Vec<u32> = g.query_radius(Vec3::new(-5.0, 0.0, -5.0), 1.0).collect();
        assert!(hits.contains(&0));
        assert!(hits.contains(&1));
    }

    #[test]
    fn large_radius_scans_more_cells() {
        // 50 items evenly spaced; a radius-10 query around the
        // middle should reach roughly half of them. Exact count
        // isn't fixed (cell slop, edge inclusion) — we just
        // assert the breadth is substantially more than a tight
        // 1.0-radius query and substantially less than the
        // total.
        let items: Vec<Vec3> = (0..50).map(|i| Vec3::new(i as f32, 0.0, 0.0)).collect();
        let g = SpatialGrid::build(&items, 1.0, |&p| p);
        let tight = g.query_radius(Vec3::new(25.0, 0.0, 0.0), 1.0).count();
        let wide = g.query_radius(Vec3::new(25.0, 0.0, 0.0), 10.0).count();
        assert!(tight < wide);
        assert!(wide < items.len());
    }

    #[test]
    fn separation_push_points_away_from_neighbour() {
        // Two items at (0,0,0) and (0.5,0,0). The push on item 0
        // should be along -X (away from item 1) and have
        // magnitude `(R - 0.5) / R` = 0.5 with R=1.
        let items = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.5, 0.0, 0.0)];
        let g = SpatialGrid::build(&items, 1.0, |&p| p);
        let push = separation_push(&g, items[0], 1.0, &items, |&p| p, |&p| p == items[0]);
        assert!(push.x < 0.0);
        assert!(push.z.abs() < 1e-5);
        assert!((push.length() - 0.5).abs() < 1e-5);
    }

    #[test]
    fn separation_push_skips_self_and_out_of_range() {
        let items = [
            Vec3::new(0.0, 0.0, 0.0), // self
            Vec3::new(0.5, 0.0, 0.0), // close — contributes
            Vec3::new(5.0, 0.0, 0.0), // outside R=1, skipped
        ];
        let g = SpatialGrid::build(&items, 1.0, |&p| p);
        // Skip the self row by exact-position match (good
        // enough for the test; real callers use a NetId or
        // entity id).
        let push = separation_push(&g, items[0], 1.0, &items, |&p| p, |&p| p == items[0]);
        // Single contributing neighbour at distance 0.5, so the
        // length should still be 0.5 — same as the previous
        // test. If item 2 had been included the length would
        // exceed 0.5 by the contribution from that neighbour
        // (it'd be skipped by the radius gate, but only after
        // a cell-window scan we'd still measure).
        assert!((push.length() - 0.5).abs() < 1e-5);
    }
}
