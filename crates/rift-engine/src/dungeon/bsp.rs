use super::config::FloorConfig;
use super::rooms::{Room, RoomType};
use super::SimpleRng;

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
        let split_pos = rng.range(config.min_leaf_size as u32, (max_size - config.min_leaf_size) as u32) as usize;

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
                room_type: RoomType::Arena, // Will be assigned later
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
pub fn generate_bsp(config: &FloorConfig, seed: u64) -> (Vec<Room>, Vec<(usize, usize, usize, usize)>) {
    let mut rng = SimpleRng::new(seed);
    let mut root = Leaf::new(0, 0, config.width, config.depth);

    // Recursively split
    let mut leaves_to_split = vec![&mut root as *mut Leaf];
    for _ in 0..20 {
        let mut next = Vec::new();
        for leaf_ptr in leaves_to_split {
            let leaf = unsafe { &mut *leaf_ptr };
            if leaf.width > config.max_leaf_size || leaf.depth > config.max_leaf_size || rng.next() % 4 != 0 {
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
    if let Some(idx) = rooms.iter().enumerate().max_by_key(|(_, r)| r.area()).map(|(i, _)| i) {
        rooms[idx].room_type = RoomType::BossRoom;
    }

    (rooms, corridors)
}
