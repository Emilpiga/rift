use glam::Vec3;
use std::collections::BinaryHeap;
use std::cmp::Ordering;

use crate::dungeon::{Floor, Tile};

/// Navigation grid for A* pathfinding.
#[derive(Clone)]
pub struct NavGrid {
    pub width: usize,
    pub depth: usize,
    /// true = walkable, false = blocked
    walkable: Vec<bool>,
    /// Extra movement cost per cell (higher near walls to encourage paths through open space).
    cost: Vec<f32>,
}

impl NavGrid {
    /// Build a nav grid from a dungeon floor with wall-proximity cost inflation.
    pub fn from_floor(floor: &Floor) -> Self {
        let walkable: Vec<bool> = floor.tiles.iter().map(|t| *t == Tile::Floor).collect();
        let w = floor.width;
        let d = floor.depth;
        let size = w * d;

        // Compute proximity cost: tiles adjacent to walls get extra cost
        let mut cost = vec![1.0_f32; size];
        for z in 0..d {
            for x in 0..w {
                if !walkable[z * w + x] {
                    continue;
                }
                // Check if any neighbor (including diagonals) is a wall
                let mut adjacent_walls = 0u8;
                for &(dx, dz) in &NEIGHBORS {
                    let nx = x as isize + dx;
                    let nz = z as isize + dz;
                    if nx < 0 || nz < 0 || nx >= w as isize || nz >= d as isize {
                        adjacent_walls += 1;
                    } else if !walkable[nz as usize * w + nx as usize] {
                        adjacent_walls += 1;
                    }
                }
                // Cells touching walls cost more — discourages wall-hugging paths
                if adjacent_walls > 0 {
                    cost[z * w + x] = 2.5 + adjacent_walls as f32 * 0.5;
                }
            }
        }

        Self {
            width: w,
            depth: d,
            walkable,
            cost,
        }
    }

    /// Get the movement cost for a cell.
    #[inline]
    pub fn cell_cost(&self, x: usize, z: usize) -> f32 {
        if x >= self.width || z >= self.depth {
            return f32::MAX;
        }
        self.cost[z * self.width + x]
    }

    /// Check if a position has walls nearby (for local avoidance).
    /// Returns a repulsion vector pushing away from nearby walls.
    pub fn wall_repulsion(&self, pos: Vec3, radius: f32) -> Vec3 {
        let mut repulsion = Vec3::ZERO;
        let (cx, cz) = self.world_to_grid(pos);
        let check_radius = (radius.ceil() as isize) + 1;

        for dz in -check_radius..=check_radius {
            for dx in -check_radius..=check_radius {
                let nx = cx as isize + dx;
                let nz = cz as isize + dz;
                if nx < 0 || nz < 0 || nx >= self.width as isize || nz >= self.depth as isize {
                    continue;
                }
                let nx = nx as usize;
                let nz = nz as usize;
                if self.is_walkable(nx, nz) {
                    continue;
                }
                // Wall cell — compute repulsion
                let wall_center = Vec3::new(nx as f32, 0.0, nz as f32);
                let diff = Vec3::new(pos.x - wall_center.x, 0.0, pos.z - wall_center.z);
                let dist = diff.length();
                if dist < radius && dist > 0.01 {
                    // Stronger push the closer we are
                    let strength = (radius - dist) / radius;
                    repulsion += diff.normalize_or_zero() * strength;
                }
            }
        }
        repulsion
    }

    #[inline]
    pub fn is_walkable(&self, x: usize, z: usize) -> bool {
        if x >= self.width || z >= self.depth {
            return false;
        }
        self.walkable[z * self.width + x]
    }

    /// Convert world position to grid coordinates.
    #[inline]
    pub fn world_to_grid(&self, pos: Vec3) -> (usize, usize) {
        let x = (pos.x.round() as isize).clamp(0, self.width as isize - 1) as usize;
        let z = (pos.z.round() as isize).clamp(0, self.depth as isize - 1) as usize;
        (x, z)
    }

    /// Convert grid coordinates to world position (center of cell).
    #[inline]
    pub fn grid_to_world(&self, x: usize, z: usize) -> Vec3 {
        Vec3::new(x as f32, 0.0, z as f32)
    }

    /// A* pathfinding from start to goal. Returns path as world positions (excluding start).
    /// Returns None if no path exists. Path is limited to max_steps to avoid long searches.
    pub fn find_path(&self, start: Vec3, goal: Vec3, max_steps: usize) -> Option<Vec<Vec3>> {
        let (sx, sz) = self.world_to_grid(start);
        let (gx, gz) = self.world_to_grid(goal);

        // Same cell — already there
        if sx == gx && sz == gz {
            return Some(vec![goal]);
        }

        // Goal not walkable — find nearest walkable neighbor to goal
        let (gx, gz) = if !self.is_walkable(gx, gz) {
            self.nearest_walkable(gx, gz)?
        } else {
            (gx, gz)
        };

        let w = self.width;
        let d = self.depth;
        let size = w * d;

        // A* open set
        let mut g_score = vec![f32::MAX; size];
        let mut came_from = vec![usize::MAX; size];
        let mut closed = vec![false; size];

        let start_idx = sz * w + sx;
        let goal_idx = gz * w + gx;

        g_score[start_idx] = 0.0;

        let mut open = BinaryHeap::new();
        open.push(AStarNode {
            f_score: heuristic(sx, sz, gx, gz),
            g_score: 0.0,
            index: start_idx,
        });

        let mut steps = 0usize;

        while let Some(current) = open.pop() {
            let ci = current.index;
            if ci == goal_idx {
                // Reconstruct path
                return Some(self.reconstruct_path(came_from, goal_idx, start_idx, goal));
            }

            if closed[ci] {
                continue;
            }
            closed[ci] = true;

            steps += 1;
            if steps > max_steps {
                // Couldn't find full path in budget — return partial path toward closest explored node
                return self.partial_path(g_score, came_from, start_idx, gx, gz);
            }

            let cx = ci % w;
            let cz = ci / w;

            // 8-directional neighbors
            for &(dx, dz) in &NEIGHBORS {
                let nx = cx as isize + dx;
                let nz = cz as isize + dz;

                if nx < 0 || nz < 0 || nx >= w as isize || nz >= d as isize {
                    continue;
                }

                let nx = nx as usize;
                let nz = nz as usize;
                let ni = nz * w + nx;

                if closed[ni] || !self.is_walkable(nx, nz) {
                    continue;
                }

                // For diagonal movement, check that both cardinal neighbors are walkable
                // (prevents cutting corners through walls)
                if dx != 0 && dz != 0 {
                    if !self.is_walkable(cx, nz) || !self.is_walkable(nx, cz) {
                        continue;
                    }
                }

                let base_cost = if dx != 0 && dz != 0 { 1.414 } else { 1.0 };
                let move_cost = base_cost * self.cost[ni];
                let tentative_g = g_score[ci] + move_cost;

                if tentative_g < g_score[ni] {
                    g_score[ni] = tentative_g;
                    came_from[ni] = ci;
                    open.push(AStarNode {
                        f_score: tentative_g + heuristic(nx, nz, gx, gz),
                        g_score: tentative_g,
                        index: ni,
                    });
                }
            }
        }

        None // No path found
    }

    fn reconstruct_path(
        &self,
        came_from: Vec<usize>,
        goal_idx: usize,
        start_idx: usize,
        goal_world: Vec3,
    ) -> Vec<Vec3> {
        let mut path = Vec::new();
        let mut current = goal_idx;

        while current != start_idx && current != usize::MAX {
            let x = current % self.width;
            let z = current / self.width;
            path.push(self.grid_to_world(x, z));
            current = came_from[current];
        }

        path.reverse();

        // Replace last point with actual goal world position (smoother movement)
        if let Some(last) = path.last_mut() {
            last.x = goal_world.x;
            last.z = goal_world.z;
        }

        // Simplify path by removing collinear points
        simplify_path(&mut path);

        path
    }

    fn partial_path(
        &self,
        g_score: Vec<f32>,
        came_from: Vec<usize>,
        start_idx: usize,
        gx: usize,
        gz: usize,
    ) -> Option<Vec<Vec3>> {
        // Find the explored node closest to the goal
        let mut best_idx = start_idx;
        let mut best_h = f32::MAX;

        for (i, &g) in g_score.iter().enumerate() {
            if g < f32::MAX {
                let x = i % self.width;
                let z = i / self.width;
                let h = heuristic(x, z, gx, gz);
                if h < best_h {
                    best_h = h;
                    best_idx = i;
                }
            }
        }

        if best_idx == start_idx {
            return None;
        }

        let goal_world = self.grid_to_world(best_idx % self.width, best_idx / self.width);
        Some(self.reconstruct_path(came_from, best_idx, start_idx, goal_world))
    }

    fn nearest_walkable(&self, x: usize, z: usize) -> Option<(usize, usize)> {
        // BFS outward from (x, z) to find nearest walkable cell
        for radius in 1..5 {
            for dz in -(radius as isize)..=(radius as isize) {
                for dx in -(radius as isize)..=(radius as isize) {
                    if dx.unsigned_abs() as usize != radius && dz.unsigned_abs() as usize != radius {
                        continue;
                    }
                    let nx = x as isize + dx;
                    let nz = z as isize + dz;
                    if nx >= 0 && nz >= 0 {
                        let nx = nx as usize;
                        let nz = nz as usize;
                        if self.is_walkable(nx, nz) {
                            return Some((nx, nz));
                        }
                    }
                }
            }
        }
        None
    }
}

/// 8-directional neighbors (dx, dz).
const NEIGHBORS: [(isize, isize); 8] = [
    (-1, 0), (1, 0), (0, -1), (0, 1),
    (-1, -1), (-1, 1), (1, -1), (1, 1),
];

/// Octile distance heuristic.
fn heuristic(x1: usize, z1: usize, x2: usize, z2: usize) -> f32 {
    let dx = (x1 as f32 - x2 as f32).abs();
    let dz = (z1 as f32 - z2 as f32).abs();
    let (min, max) = if dx < dz { (dx, dz) } else { (dz, dx) };
    max + (1.414 - 1.0) * min
}

/// Remove collinear waypoints for smoother paths.
fn simplify_path(path: &mut Vec<Vec3>) {
    if path.len() <= 2 {
        return;
    }

    let mut simplified = Vec::with_capacity(path.len());
    simplified.push(path[0]);

    for i in 1..path.len() - 1 {
        let prev = simplified.last().unwrap();
        let next = &path[i + 1];
        let curr = &path[i];

        // Keep waypoint if direction changes
        let d1 = (*curr - *prev).normalize_or_zero();
        let d2 = (*next - *curr).normalize_or_zero();
        if d1.dot(d2) < 0.99 {
            simplified.push(*curr);
        }
    }

    simplified.push(*path.last().unwrap());
    *path = simplified;
}

/// A* priority queue node.
#[derive(Clone, PartialEq)]
struct AStarNode {
    f_score: f32,
    g_score: f32,
    index: usize,
}

impl Eq for AStarNode {}

impl Ord for AStarNode {
    fn cmp(&self, other: &Self) -> Ordering {
        // Min-heap by f_score (reverse ordering)
        other.f_score.partial_cmp(&self.f_score).unwrap_or(Ordering::Equal)
    }
}

impl PartialOrd for AStarNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
