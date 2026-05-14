use super::config::FloorConfig;
use super::rooms::{Room, RoomShape, RoomTheme, RoomType};
use super::{FloorMood, SimpleRng};

/// A leaf node in the BSP tree.
#[derive(Debug)]
struct Leaf {
    x: usize,
    z: usize,
    width: usize,
    depth: usize,
    left: Option<Box<Leaf>>,
    right: Option<Box<Leaf>>,
    room: Option<Room>,
}

impl Leaf {
    fn new(x: usize, z: usize, width: usize, depth: usize) -> Self {
        Self {
            x,
            z,
            width,
            depth,
            left: None,
            right: None,
            room: None,
        }
    }

    /// Recursively split this leaf into two children.
    /// Returns true if split occurred.
    fn split(&mut self, rng: &mut SimpleRng, config: &FloorConfig) -> bool {
        // Already split
        if self.left.is_some() || self.right.is_some() {
            return false;
        }

        // Determine split direction
        // If much wider than tall, split vertically. If much taller, split horizontally.
        let split_h = if self.width > self.depth && self.width as f32 / self.depth as f32 >= 1.25 {
            false
        } else if self.depth > self.width && self.depth as f32 / self.width as f32 >= 1.25 {
            true
        } else {
            rng.next() % 2 == 0
        };

        let max_size = if split_h { self.depth } else { self.width };

        // Don't split if too small
        if max_size < config.min_leaf_size * 2 {
            return false;
        }

        // Pick split position
        let split_pos = rng.range(
            config.min_leaf_size as u32,
            (max_size - config.min_leaf_size) as u32,
        ) as usize;

        if split_h {
            self.left = Some(Box::new(Leaf::new(self.x, self.z, self.width, split_pos)));
            self.right = Some(Box::new(Leaf::new(
                self.x,
                self.z + split_pos,
                self.width,
                self.depth - split_pos,
            )));
        } else {
            self.left = Some(Box::new(Leaf::new(self.x, self.z, split_pos, self.depth)));
            self.right = Some(Box::new(Leaf::new(
                self.x + split_pos,
                self.z,
                self.width - split_pos,
                self.depth,
            )));
        }

        true
    }

    /// Recursively create rooms in leaf nodes.
    fn create_rooms(&mut self, rng: &mut SimpleRng, config: &FloorConfig) {
        if let Some(ref mut left) = self.left {
            left.create_rooms(rng, config);
        }
        if let Some(ref mut right) = self.right {
            right.create_rooms(rng, config);
        }

        // Only create room if this is a terminal leaf
        if self.left.is_none() && self.right.is_none() {
            let padding = config.room_padding;
            let max_w = self.width - padding * 2;
            let max_d = self.depth - padding * 2;

            if max_w < 5 || max_d < 5 {
                return;
            }

            // Rooms fill at least 60% of available space for bigger combat arenas
            let min_w = (max_w * 3 / 5).max(5);
            let min_d = (max_d * 3 / 5).max(5);
            let w = rng.range(min_w as u32, max_w as u32 + 1) as usize;
            let d = rng.range(min_d as u32, max_d as u32 + 1) as usize;
            let rx = self.x + padding + rng.range(0, (max_w - w + 1) as u32) as usize;
            let rz = self.z + padding + rng.range(0, (max_d - d + 1) as u32) as usize;

            self.room = Some(Room {
                x: rx,
                z: rz,
                width: w,
                depth: d,
                room_type: RoomType::Arena,    // Will be assigned later
                theme: RoomTheme::Generic,     // assigned by `assign_themes_and_shapes`
                shape: RoomShape::Rectangular, // assigned by `assign_themes_and_shapes`
                surface: None,
            });
        }
    }

    /// Collect all rooms from this subtree.
    fn collect_rooms(&self) -> Vec<Room> {
        let mut rooms = Vec::new();
        if let Some(ref room) = self.room {
            rooms.push(room.clone());
        }
        if let Some(ref left) = self.left {
            rooms.extend(left.collect_rooms());
        }
        if let Some(ref right) = self.right {
            rooms.extend(right.collect_rooms());
        }
        rooms
    }

    /// Get the center of a room in this subtree (for corridor connection).
    fn get_room_center(&self) -> Option<(usize, usize)> {
        if let Some(ref room) = self.room {
            return Some(room.center());
        }
        // Try left first, then right
        if let Some(ref left) = self.left {
            if let Some(c) = left.get_room_center() {
                return Some(c);
            }
        }
        if let Some(ref right) = self.right {
            if let Some(c) = right.get_room_center() {
                return Some(c);
            }
        }
        None
    }

    /// Connect sibling rooms with corridors, collecting corridor segments.
    fn create_corridors(&self, corridors: &mut Vec<(usize, usize, usize, usize)>) {
        if let (Some(ref left), Some(ref right)) = (&self.left, &self.right) {
            left.create_corridors(corridors);
            right.create_corridors(corridors);

            // Connect a room from the left subtree to a room from the right subtree
            if let (Some((lx, lz)), Some((rx, rz))) =
                (left.get_room_center(), right.get_room_center())
            {
                corridors.push((lx, lz, rx, rz));
            }
        }
    }
}

/// Generate rooms and corridors using BSP.
pub fn generate_bsp(
    config: &FloorConfig,
    seed: u64,
) -> (Vec<Room>, Vec<(usize, usize, usize, usize)>, FloorMood) {
    let mut rng = SimpleRng::new(seed);
    let mut root = Leaf::new(0, 0, config.width, config.depth);

    // Recursively split
    let mut leaves_to_split = vec![&mut root as *mut Leaf];
    for _ in 0..20 {
        let mut next = Vec::new();
        for leaf_ptr in leaves_to_split {
            let leaf = unsafe { &mut *leaf_ptr };
            if leaf.width > config.max_leaf_size
                || leaf.depth > config.max_leaf_size
                || rng.next() % 4 != 0
            {
                if leaf.split(&mut rng, config) {
                    if let Some(ref mut l) = leaf.left {
                        next.push(l.as_mut() as *mut Leaf);
                    }
                    if let Some(ref mut r) = leaf.right {
                        next.push(r.as_mut() as *mut Leaf);
                    }
                }
            }
        }
        if next.is_empty() {
            break;
        }
        leaves_to_split = next;
    }

    // Create rooms in leaves
    root.create_rooms(&mut rng, config);

    // Collect corridors
    let mut corridors = Vec::new();
    root.create_corridors(&mut corridors);

    // Collect rooms and assign types
    let mut rooms = root.collect_rooms();

    // Assign boss room to the largest room
    let boss_idx = rooms
        .iter()
        .enumerate()
        .max_by_key(|(_, r)| r.area())
        .map(|(i, _)| i);
    if let Some(idx) = boss_idx {
        rooms[idx].room_type = RoomType::BossRoom;
    }

    // Assign the portal room: the non-boss room whose centre
    // is closest to the boss room. BSP corridors connect every
    // room into a single graph, so the geographic neighbour is
    // almost always BSP-sibling-connected by an L-shaped
    // corridor — exactly the "short walk from the boss to the
    // portals" feel we want. Skip if there's only one room
    // (degenerate small floors), and protect against
    // accidentally re-tagging the boss room.
    if let Some(boss_idx) = boss_idx {
        let (bx, bz) = rooms[boss_idx].center();
        let mut best: Option<(usize, usize)> = None; // (idx, dist²)
        for (i, r) in rooms.iter().enumerate() {
            if i == boss_idx || r.room_type != RoomType::Arena {
                continue;
            }
            let (cx, cz) = r.center();
            let dx = cx as isize - bx as isize;
            let dz = cz as isize - bz as isize;
            let d2 = (dx * dx + dz * dz) as usize;
            if best.map(|(_, b)| d2 < b).unwrap_or(true) {
                best = Some((i, d2));
            }
        }
        if let Some((idx, _)) = best {
            rooms[idx].room_type = RoomType::PortalRoom;
        }
    }

    // ---- Theme + shape assignment ----
    //
    // Done in a second pass so it observes the final
    // `room_type` (boss / portal / arena) for each room. The
    // BSP seed re-derived here gives client and server the
    // same assignment without any extra wire data — both
    // sides simply call `Floor::generate` with the same seed.
    //
    // Roles override theme: the boss room is always a Shrine
    // (it's the climactic chamber); the portal room is always
    // Generic (calm transit space, no thematic clutter that
    // would obscure the portals). Arena rooms get themed by
    // weighted draw, biased by size (large rooms favour
    // ceremonial themes — Library, Shrine — and small rooms
    // favour functional ones — Storage, Prison).
    let mood = FloorMood::for_seed(config.floor, seed);
    let mut theme_rng = SimpleRng::new(seed.wrapping_add(0x7E3E_A11E));
    for room in rooms.iter_mut() {
        let (theme, shape) = match room.room_type {
            RoomType::BossRoom => (
                RoomTheme::Shrine,
                choose_shape_for_size(room.width, room.depth, &mut theme_rng, true),
            ),
            RoomType::PortalRoom => (RoomTheme::Generic, RoomShape::Rectangular),
            RoomType::Corridor => (RoomTheme::Generic, RoomShape::Rectangular),
            RoomType::Arena => {
                let area = room.area();
                // Weighted theme draw. Weights are tuned so
                // any given floor sees a healthy mix without
                // any single theme dominating; size bias
                // pushes "ceremonial" themes onto the big
                // rooms (where their centerpiece props have
                // space to breathe) and functional themes
                // onto the small ones.
                let big = area >= 60;
                let base_weights: &[(RoomTheme, u32)] = if big {
                    &[
                        (RoomTheme::Library, 4),
                        (RoomTheme::Crypt, 3),
                        (RoomTheme::Barracks, 3),
                        (RoomTheme::Shrine, 2),
                        (RoomTheme::Storage, 1),
                        (RoomTheme::Prison, 1),
                    ]
                } else {
                    &[
                        (RoomTheme::Storage, 4),
                        (RoomTheme::Prison, 3),
                        (RoomTheme::Crypt, 2),
                        (RoomTheme::Barracks, 2),
                        (RoomTheme::Library, 1),
                        (RoomTheme::Shrine, 1),
                    ]
                };
                let weights: Vec<(RoomTheme, u32)> = base_weights
                    .iter()
                    .map(|&(theme, weight)| (theme, weight + mood.theme_bonus(theme)))
                    .collect();
                let theme = weighted_pick(&weights, &mut theme_rng);
                let shape = choose_shape_for_size(room.width, room.depth, &mut theme_rng, false);
                (theme, shape)
            }
        };
        room.theme = theme;
        room.shape = shape;
    }

    (rooms, corridors, mood)
}

/// Deterministic weighted pick from a `(value, weight)` slice.
/// Used by theme assignment so adjusting palette balance is
/// a one-line tuning knob per room category.
fn weighted_pick<T: Copy>(items: &[(T, u32)], rng: &mut SimpleRng) -> T {
    let total: u32 = items.iter().map(|(_, w)| *w).sum();
    if total == 0 {
        return items[0].0;
    }
    let mut roll = rng.range(0, total);
    for (v, w) in items {
        if roll < *w {
            return *v;
        }
        roll -= *w;
    }
    items[items.len() - 1].0
}

/// Pick a [`RoomShape`] given the room's footprint.
///
/// Currently always returns `Rectangular`. The non-rectangular
/// shapes (Pillared, Alcoved, Cross, Round) all carve interior
/// or near-perimeter wall tiles into the room — Pillared drops
/// freestanding pillars, the others wall off corner / curve
/// blocks. In playtesting these read as "random walls inside
/// the room" rather than meaningful silhouette: they break
/// sightlines, snag ranged kiting, and obscure props the
/// theme palette pinned to the room centre.
///
/// We keep the [`RoomShape`] enum + carving code intact so a
/// future shape (e.g. octagonal-with-no-interior-walls,
/// chamfered corners on the perimeter only) can re-enable
/// itself without revisiting the pipeline.
fn choose_shape_for_size(
    width: usize,
    depth: usize,
    rng: &mut SimpleRng,
    prefer_grand: bool,
) -> RoomShape {
    let _ = (width, depth, rng, prefer_grand);
    RoomShape::Rectangular
}
