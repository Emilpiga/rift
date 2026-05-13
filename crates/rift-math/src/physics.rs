use glam::Vec3;

/// A ray with origin and direction.
#[derive(Clone, Copy, Debug)]
pub struct Ray {
    pub origin: Vec3,
    pub direction: Vec3,
}

impl Ray {
    pub fn new(origin: Vec3, direction: Vec3) -> Self {
        Self {
            origin,
            direction: direction.normalize(),
        }
    }

    /// Create a ray from point A toward point B.
    pub fn between(from: Vec3, to: Vec3) -> (Self, f32) {
        let diff = to - from;
        let len = diff.length();
        let dir = if len > 1e-6 { diff / len } else { Vec3::Y };
        (
            Self {
                origin: from,
                direction: dir,
            },
            len,
        )
    }

    /// Get point along ray at distance t.
    pub fn at(&self, t: f32) -> Vec3 {
        self.origin + self.direction * t
    }
}

/// Axis-aligned bounding box for physics queries.
#[derive(Clone, Copy, Debug)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }

    /// Create from center + half extents.
    pub fn from_center(center: Vec3, half_extents: Vec3) -> Self {
        Self {
            min: center - half_extents,
            max: center + half_extents,
        }
    }

    /// Ray-AABB intersection test. Returns distance to entry point, or None if no hit.
    pub fn ray_intersect(&self, ray: &Ray) -> Option<f32> {
        let inv_dir = Vec3::new(
            if ray.direction.x.abs() > 1e-8 {
                1.0 / ray.direction.x
            } else {
                f32::MAX.copysign(ray.direction.x)
            },
            if ray.direction.y.abs() > 1e-8 {
                1.0 / ray.direction.y
            } else {
                f32::MAX.copysign(ray.direction.y)
            },
            if ray.direction.z.abs() > 1e-8 {
                1.0 / ray.direction.z
            } else {
                f32::MAX.copysign(ray.direction.z)
            },
        );

        let t1 = (self.min.x - ray.origin.x) * inv_dir.x;
        let t2 = (self.max.x - ray.origin.x) * inv_dir.x;
        let t3 = (self.min.y - ray.origin.y) * inv_dir.y;
        let t4 = (self.max.y - ray.origin.y) * inv_dir.y;
        let t5 = (self.min.z - ray.origin.z) * inv_dir.z;
        let t6 = (self.max.z - ray.origin.z) * inv_dir.z;

        let tmin = t1.min(t2).max(t3.min(t4)).max(t5.min(t6));
        let tmax = t1.max(t2).min(t3.max(t4)).min(t5.max(t6));

        if tmax < 0.0 || tmin > tmax {
            None
        } else if tmin < 0.0 {
            // Origin is inside the box, return 0
            Some(0.0)
        } else {
            Some(tmin)
        }
    }
}

/// Result of a raycast hit.
#[derive(Clone, Copy, Debug)]
pub struct RayHit {
    pub distance: f32,
    pub point: Vec3,
}

/// Cast a ray against a list of AABBs. Returns true if any AABB is hit (early exit).
/// Much faster than full raycast when you only need occlusion/LOS information.
pub fn raycast_any(ray: &Ray, max_distance: f32, aabbs: &[Aabb]) -> bool {
    for aabb in aabbs {
        if let Some(t) = aabb.ray_intersect(ray) {
            if t >= 0.0 && t <= max_distance {
                return true;
            }
        }
    }
    false
}

/// Cast a ray against a list of AABBs. Returns the closest hit.
pub fn raycast(ray: &Ray, max_distance: f32, aabbs: &[Aabb]) -> Option<RayHit> {
    let mut closest: Option<RayHit> = None;

    for aabb in aabbs {
        if let Some(t) = aabb.ray_intersect(ray) {
            if t >= 0.0 && t <= max_distance {
                if closest.is_none() || t < closest.unwrap().distance {
                    closest = Some(RayHit {
                        distance: t,
                        point: ray.at(t),
                    });
                }
            }
        }
    }

    closest
}

/// Generic horizontal line-of-sight test against a 1 m tile
/// grid. Walks the XZ segment from `a` to `b` in 0.5 m steps
/// (half a tile — fine enough to catch diagonal corner peeks
/// without false-positive "see through" results across
/// single-tile gaps) and returns `false` as soon as
/// `is_blocked` reports a sample tile is solid.
///
/// `is_blocked` receives integer tile coordinates using the
/// engine's standard world→grid convention: tile `(i, j)`'s
/// centre is at world `(i, j)` and covers
/// `[i-0.5, i+0.5] × [j-0.5, j+0.5]`. Negative coordinates are
/// passed through as-is so callers can short-circuit
/// out-of-bounds samples however they prefer (typically by
/// returning `true`).
///
/// Y is ignored. Endpoints are *not* tested — they're usually
/// at entity origins which can be wall-adjacent (e.g. a melee
/// enemy shoved into a wall by separation steering); sampling
/// the literal coordinate would produce a false negative.
///
/// Pure function. Lives here so every LOS user (server AI,
/// targeted abilities, future client-side telegraph culling)
/// shares one deterministic implementation.
pub fn line_of_sight_grid<F: FnMut(i32, i32) -> bool>(a: Vec3, b: Vec3, mut is_blocked: F) -> bool {
    let dx = b.x - a.x;
    let dz = b.z - a.z;
    let dist = (dx * dx + dz * dz).sqrt();
    if dist < 0.001 {
        return true;
    }
    // 0.5 m sampling: smallest step that still catches diagonal
    // sneak-throughs against the 1 m tile grid.
    let steps = (dist * 2.0).ceil() as i32;
    for i in 1..steps {
        let t = i as f32 / steps as f32;
        let px = a.x + dx * t;
        let pz = a.z + dz * t;
        let gx = (px + 0.5).floor() as i32;
        let gz = (pz + 0.5).floor() as i32;
        if is_blocked(gx, gz) {
            return false;
        }
    }
    true
}

/// Generic 4-connected A* on an integer tile grid. Returns the
/// path from `from` (excluded — caller is already there) up to
/// and including `goal`, or `None` if no path exists or the
/// search exceeds `max_expanded` nodes (a budget cap so a
/// runaway search on a fragmented grid can't burn the AI tick).
///
/// `is_walkable` answers "may a search node land on this tile?"
/// using the same `(i32, i32)` convention as
/// [`line_of_sight_grid`]. Negative / out-of-bounds tiles
/// should return `false`. The `goal` tile itself is always
/// allowed to be expanded into even if `is_walkable(goal)` is
/// `false` — callers don't have to special-case targets that
/// have briefly clipped into an unwalkable cell.
///
/// Heuristic is Manhattan distance, which is admissible for
/// 4-connected uniform-cost grids and produces optimal paths.
///
/// Pure function. Lives here so every grid path consumer
/// (server enemy nav, future client-side autopath, debug
/// tools) shares one deterministic implementation.
pub fn astar_grid<F>(
    from: (i32, i32),
    goal: (i32, i32),
    max_expanded: usize,
    is_walkable: F,
) -> Option<Vec<(i32, i32)>>
where
    F: FnMut(i32, i32) -> bool,
{
    astar_grid_weighted(from, goal, max_expanded, is_walkable, |_, _| 0)
}

/// Like [`astar_grid`] but with a per-tile entry cost added on
/// top of the base step cost of 1. Use for "prefer interior /
/// avoid wall-hugging" pathfinding — return e.g. 2 for tiles
/// adjacent to walls so the search picks a slightly longer
/// route through the room centre over a shorter route that
/// scrapes along the wall.
///
/// Heuristic remains Manhattan distance (admissible because
/// `extra_cost` is non-negative).
pub fn astar_grid_weighted<F, C>(
    from: (i32, i32),
    goal: (i32, i32),
    max_expanded: usize,
    mut is_walkable: F,
    mut extra_cost: C,
) -> Option<Vec<(i32, i32)>>
where
    F: FnMut(i32, i32) -> bool,
    C: FnMut(i32, i32) -> i32,
{
    use std::cmp::Reverse;
    use std::collections::{BinaryHeap, HashMap};

    if from == goal {
        return Some(Vec::new());
    }
    // Start cell must be walkable — searching from a wall would
    // explore nothing useful and return `None` after burning
    // the budget. Be defensive: callers usually hand us an
    // entity position which is wall-clamped by the integrator,
    // but we can't prove it from here.
    if !is_walkable(from.0, from.1) {
        return None;
    }

    let h = |x: i32, z: i32| -> i32 { (x - goal.0).abs() + (z - goal.1).abs() };
    let mut open: BinaryHeap<Reverse<(i32, (i32, i32))>> = BinaryHeap::new();
    let mut g_score: HashMap<(i32, i32), i32> = HashMap::new();
    let mut came_from: HashMap<(i32, i32), (i32, i32)> = HashMap::new();
    open.push(Reverse((h(from.0, from.1), from)));
    g_score.insert(from, 0);

    let mut expanded: usize = 0;
    while let Some(Reverse((_, current))) = open.pop() {
        if current == goal {
            // Reconstruct path back to (but excluding) `from`.
            let mut path = vec![current];
            let mut cur = current;
            while let Some(&prev) = came_from.get(&cur) {
                if prev == from {
                    break;
                }
                path.push(prev);
                cur = prev;
            }
            path.reverse();
            return Some(path);
        }
        expanded += 1;
        if expanded > max_expanded {
            return None;
        }
        let g = *g_score.get(&current).unwrap_or(&i32::MAX);
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let nx = current.0 + dx;
            let nz = current.1 + dz;
            let n = (nx, nz);
            // Allow expansion into the goal even if it's
            // unwalkable — the target may briefly clip a wall.
            if n != goal && !is_walkable(nx, nz) {
                continue;
            }
            let tentative = g + 1 + extra_cost(nx, nz);
            if tentative < *g_score.get(&n).unwrap_or(&i32::MAX) {
                came_from.insert(n, current);
                g_score.insert(n, tentative);
                open.push(Reverse((tentative + h(nx, nz), n)));
            }
        }
    }
    None
}
