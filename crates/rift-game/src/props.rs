//! Static environment props (barrels, benches, bookcases, candles, …)
//!
//! Loaded from `assets/models/fantasy-props/Exports/glTF/`.  Each prop is
//! tagged with one of the four trim sheet textures shipped with the pack;
//! we upload one shared descriptor set per trim sheet (so the descriptor
//! pool budget stays small) and bind it to every prop that uses it.
//!
//! Placement: along the inner perimeter of arena/boss rooms, deterministic
//! per floor seed, avoiding the player spawn and the room centre (where
//! enemy packs spawn).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use glam::{Mat4, Quat, Vec3};
use rift_engine::ash::vk;
use rift_engine::ash::Device;
use rift_engine::ecs::components::{Collider, Static, Transform};
use rift_engine::gpu_allocator::vulkan::Allocator;
use rift_engine::renderer::texture::Texture;
use rift_engine::dungeon::{RoomType, Tile};
use rift_engine::{Floor, Mesh, Renderer};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrimSheet {
    Furniture,
    Metal,
    Cloth,
    Props,
}

impl TrimSheet {
    fn texture_path(self) -> &'static str {
        match self {
            TrimSheet::Furniture => "assets/models/fantasy-props/Textures/T_Trim_Furniture_BaseColor.png",
            TrimSheet::Metal     => "assets/models/fantasy-props/Textures/T_Trim_Metal_BaseColor.png",
            TrimSheet::Cloth     => "assets/models/fantasy-props/Textures/T_Trim_Cloth_BaseColor.png",
            TrimSheet::Props     => "assets/models/fantasy-props/Textures/T_Trim_Props_BaseColor.png",
        }
    }
}

#[derive(Clone, Copy)]
pub struct PropDef {
    pub gltf: &'static str,
    pub trim: TrimSheet,
    pub scale: f32,
    /// `true` = should be placed flush against a wall, `false` = free-standing.
    pub against_wall: bool,
    /// Whether the prop is solid (gets a static AABB collider).
    pub solid: bool,
    /// Selection weight (relative).  Higher = more common.
    pub weight: u32,
}

/// Curated subset of the fantasy-props pack.  Anything not listed here
/// stays unused for now — we can grow this table as needed.
pub const PROP_DEFS: &[PropDef] = &[
    // wall furniture
    PropDef { gltf: "assets/models/fantasy-props/Exports/glTF/Barrel.gltf",          trim: TrimSheet::Furniture, scale: 1.0, against_wall: true,  solid: true,  weight: 6 },
    PropDef { gltf: "assets/models/fantasy-props/Exports/glTF/Barrel_Apples.gltf",   trim: TrimSheet::Furniture, scale: 1.0, against_wall: true,  solid: true,  weight: 2 },
    PropDef { gltf: "assets/models/fantasy-props/Exports/glTF/Barrel_Holder.gltf",   trim: TrimSheet::Furniture, scale: 1.0, against_wall: true,  solid: true,  weight: 2 },
    PropDef { gltf: "assets/models/fantasy-props/Exports/glTF/Bench.gltf",           trim: TrimSheet::Furniture, scale: 1.0, against_wall: true,  solid: true,  weight: 3 },
    PropDef { gltf: "assets/models/fantasy-props/Exports/glTF/Bookcase_2.gltf",      trim: TrimSheet::Furniture, scale: 1.0, against_wall: true,  solid: true,  weight: 2 },
    PropDef { gltf: "assets/models/fantasy-props/Exports/glTF/Cabinet.gltf",         trim: TrimSheet::Furniture, scale: 1.0, against_wall: true,  solid: true,  weight: 2 },
    PropDef { gltf: "assets/models/fantasy-props/Exports/glTF/Bed_Twin1.gltf",       trim: TrimSheet::Furniture, scale: 1.0, against_wall: true,  solid: true,  weight: 1 },
    PropDef { gltf: "assets/models/fantasy-props/Exports/glTF/Bed_Twin2.gltf",       trim: TrimSheet::Furniture, scale: 1.0, against_wall: true,  solid: true,  weight: 1 },
    PropDef { gltf: "assets/models/fantasy-props/Exports/glTF/Anvil_Log.gltf",       trim: TrimSheet::Furniture, scale: 1.0, against_wall: true,  solid: true,  weight: 1 },
    PropDef { gltf: "assets/models/fantasy-props/Exports/glTF/Bucket_Wooden_1.gltf", trim: TrimSheet::Furniture, scale: 1.0, against_wall: true,  solid: true,  weight: 2 },

    // metal
    PropDef { gltf: "assets/models/fantasy-props/Exports/glTF/Anvil.gltf",           trim: TrimSheet::Metal, scale: 1.0, against_wall: false, solid: true,  weight: 1 },
    PropDef { gltf: "assets/models/fantasy-props/Exports/glTF/Cauldron.gltf",        trim: TrimSheet::Metal, scale: 1.0, against_wall: false, solid: true,  weight: 2 },
    PropDef { gltf: "assets/models/fantasy-props/Exports/glTF/Bucket_Metal.gltf",    trim: TrimSheet::Metal, scale: 1.0, against_wall: true,  solid: true,  weight: 2 },
    PropDef { gltf: "assets/models/fantasy-props/Exports/glTF/Cage_Small.gltf",      trim: TrimSheet::Metal, scale: 1.0, against_wall: true,  solid: true,  weight: 1 },
    PropDef { gltf: "assets/models/fantasy-props/Exports/glTF/CandleStick_Triple.gltf", trim: TrimSheet::Metal, scale: 1.0, against_wall: false, solid: false, weight: 2 },
    PropDef { gltf: "assets/models/fantasy-props/Exports/glTF/CandleStick_Stand.gltf",  trim: TrimSheet::Metal, scale: 1.0, against_wall: true,  solid: false, weight: 2 },
    PropDef { gltf: "assets/models/fantasy-props/Exports/glTF/Chain_Coil.gltf",      trim: TrimSheet::Metal, scale: 1.0, against_wall: true,  solid: false, weight: 1 },

    // cloth
    PropDef { gltf: "assets/models/fantasy-props/Exports/glTF/Bag.gltf",             trim: TrimSheet::Cloth, scale: 1.0, against_wall: true,  solid: false, weight: 2 },
    PropDef { gltf: "assets/models/fantasy-props/Exports/glTF/Banner_1.gltf",        trim: TrimSheet::Cloth, scale: 1.0, against_wall: true,  solid: false, weight: 1 },
    PropDef { gltf: "assets/models/fantasy-props/Exports/glTF/Banner_2.gltf",        trim: TrimSheet::Cloth, scale: 1.0, against_wall: true,  solid: false, weight: 1 },

    // small props
    PropDef { gltf: "assets/models/fantasy-props/Exports/glTF/Book_Stack_1.gltf",    trim: TrimSheet::Props, scale: 1.0, against_wall: false, solid: false, weight: 1 },
    PropDef { gltf: "assets/models/fantasy-props/Exports/glTF/Book_Stack_2.gltf",    trim: TrimSheet::Props, scale: 1.0, against_wall: false, solid: false, weight: 1 },
    PropDef { gltf: "assets/models/fantasy-props/Exports/glTF/BookGroup_Medium_1.gltf", trim: TrimSheet::Props, scale: 1.0, against_wall: false, solid: false, weight: 1 },
    PropDef { gltf: "assets/models/fantasy-props/Exports/glTF/BookStand.gltf",       trim: TrimSheet::Furniture, scale: 1.0, against_wall: false, solid: true,  weight: 1 },
];

pub struct PropLibrary {
    /// Cached static meshes keyed by glTF path.
    meshes: HashMap<&'static str, Mesh>,
    /// Local-space (pre-scale) AABB min/max keyed by glTF path.
    bounds: HashMap<&'static str, (Vec3, Vec3)>,
    /// Paths we've already tried and failed, so we don't spam the log.
    failed: HashSet<&'static str>,
    /// One descriptor set per trim sheet.
    trim_sets: HashMap<TrimSheet, vk::DescriptorSet>,
    /// Owned textures backing those descriptor sets.  Freed in [`Self::cleanup_gpu`].
    trim_textures: Vec<Texture>,
}

impl Default for PropLibrary {
    fn default() -> Self {
        Self::new()
    }
}

impl PropLibrary {
    pub fn new() -> Self {
        Self {
            meshes: HashMap::new(),
            bounds: HashMap::new(),
            failed: HashSet::new(),
            trim_sets: HashMap::new(),
            trim_textures: Vec::new(),
        }
    }

    fn ensure_mesh(&mut self, def: &PropDef) -> bool {
        if self.meshes.contains_key(def.gltf) {
            return true;
        }
        if self.failed.contains(def.gltf) {
            return false;
        }
        match Mesh::from_gltf(def.gltf) {
            Ok(m) => {
                let (mn, mx) = aabb_of(&m);
                self.bounds.insert(def.gltf, (mn, mx));
                self.meshes.insert(def.gltf, m);
                true
            }
            Err(e) => {
                log::warn!("prop mesh load failed {}: {}", def.gltf, e);
                self.failed.insert(def.gltf);
                false
            }
        }
    }

    fn ensure_trim(
        &mut self,
        sheet: TrimSheet,
        renderer: &mut Renderer,
    ) -> Option<vk::DescriptorSet> {
        if let Some(ds) = self.trim_sets.get(&sheet).copied() {
            return Some(ds);
        }
        let path = sheet.texture_path();
        let candidates = [
            std::path::PathBuf::from(path),
            std::path::PathBuf::from("..").join(path),
            std::path::PathBuf::from("../..").join(path),
            std::path::PathBuf::from("../../..").join(path),
        ];
        let resolved = candidates.iter().find(|p| p.exists()).cloned()?;
        let bytes = std::fs::read(&resolved)
            .map_err(|e| log::warn!("trim texture read {:?}: {}", resolved, e))
            .ok()?;
        let (tex, ds) = renderer
            .upload_shared_texture_from_bytes(&bytes)
            .map_err(|e| log::warn!("trim texture upload failed: {}", e))
            .ok()?;
        self.trim_textures.push(tex);
        self.trim_sets.insert(sheet, ds);
        Some(ds)
    }

    /// Scatter props through every Arena/BossRoom on this floor.
    pub fn decorate(
        &mut self,
        world: &mut hecs::World,
        renderer: &mut Renderer,
        floor: &Floor,
        seed: u64,
    ) {
        let mut rng = SmallRng::new(seed.wrapping_add(0xC1A0_5EED));

        // Pre-warm trim sheets so the first prop in a room doesn't pay the cost.
        let _ = self.ensure_trim(TrimSheet::Furniture, renderer);
        let _ = self.ensure_trim(TrimSheet::Metal, renderer);
        let _ = self.ensure_trim(TrimSheet::Cloth, renderer);
        let _ = self.ensure_trim(TrimSheet::Props, renderer);

        let total_w: u32 = PROP_DEFS.iter().map(|d| d.weight).sum();

        for room in &floor.rooms {
            if room.room_type == RoomType::Corridor {
                continue;
            }
            let area = room.area();
            let count = match room.room_type {
                RoomType::BossRoom => (area / 18).clamp(4, 10),
                RoomType::Arena    => (area / 22).clamp(2, 6),
                _ => 0,
            };
            if count == 0 {
                continue;
            }

            // Build candidate (position, yaw, wall_dir) tuples along the inner room edge.
            // wall_dir = (ox, oz) where the wall is at tile + (ox, oz); used
            // to offset wall-aligned props by their actual depth.
            let mut candidates: Vec<(Vec3, f32, (i32, i32))> = Vec::new();
            let dirs = [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)];
            for dx in 0..room.width as i32 {
                for dz in 0..room.depth as i32 {
                    let tx = room.x as i32 + dx;
                    let tz = room.z as i32 + dz;
                    if tx < 0 || tz < 0 {
                        continue;
                    }
                    if floor.get(tx as usize, tz as usize) != Tile::Floor {
                        continue;
                    }
                    let mut wall_dir: Option<(i32, i32)> = None;
                    for &(ox, oz) in &dirs {
                        let nx = tx + ox;
                        let nz = tz + oz;
                        if nx < 0 || nz < 0 {
                            continue;
                        }
                        if floor.get(nx as usize, nz as usize) == Tile::Wall {
                            wall_dir = Some((ox, oz));
                            break;
                        }
                    }
                    if let Some((ox, oz)) = wall_dir {
                        // Place wall props at tile centre; we'll offset by
                        // the prop's actual depth toward the wall in
                        // `spawn_prop`.  Free-standing props also use tile
                        // centre (no per-side bias).
                        let pos = Vec3::new(tx as f32, 0.0, tz as f32);
                        let face = Vec3::new(-ox as f32, 0.0, -oz as f32);
                        let yaw = face.x.atan2(face.z);
                        candidates.push((pos, yaw, (ox, oz)));
                    }
                }
            }

            // Discard candidates near the player spawn or near the centre
            // (where mob packs cluster).
            let center = room.center_world();
            candidates.retain(|(p, _, _)| {
                p.distance(center) > 2.5 && p.distance(floor.spawn_pos) > 4.5
            });

            // Track placed positions to enforce a minimum spacing between props.
            let mut placed: Vec<Vec3> = Vec::new();
            let min_spacing_sq = 1.6 * 1.6;

            for _ in 0..count {
                if candidates.is_empty() {
                    break;
                }
                let i = (rng.next() as usize) % candidates.len();
                let (pos, wall_yaw, wall_dir) = candidates.swap_remove(i);
                if placed.iter().any(|p| p.distance_squared(pos) < min_spacing_sq) {
                    continue;
                }

                // Weighted pick. Prefer wall-friendly props for these slots.
                let mut pick = rng.range(0, total_w);
                let def = PROP_DEFS
                    .iter()
                    .find(|d| {
                        if pick < d.weight {
                            true
                        } else {
                            pick -= d.weight;
                            false
                        }
                    })
                    .copied()
                    .unwrap_or(PROP_DEFS[0]);

                // Free-standing props get a slight random rotation; wall props
                // align with the wall yaw plus a tiny jitter for variety.
                let yaw = if def.against_wall {
                    wall_yaw + (rng.range(0, 21) as f32 - 10.0).to_radians()
                } else {
                    (rng.range(0, 360) as f32).to_radians()
                };

                self.spawn_prop(world, renderer, &def, pos, yaw, wall_dir);
                placed.push(pos);
            }
        }
    }

    fn spawn_prop(
        &mut self,
        world: &mut hecs::World,
        renderer: &mut Renderer,
        def: &PropDef,
        tile_pos: Vec3,
        yaw: f32,
        wall_dir: (i32, i32),
    ) {
        if !self.ensure_mesh(def) {
            return;
        }
        let trim_set = self.ensure_trim(def.trim, renderer);
        let (mn, mx) = match self.bounds.get(def.gltf).copied() {
            Some(b) => b,
            None => return,
        };

        // Local AABB in the model's own frame, after applying scale.
        let s = def.scale;
        let half_x = ((mx.x - mn.x) * 0.5 * s).max(0.05);
        let half_y = ((mx.y - mn.y) * 0.5 * s).max(0.05);
        let half_z = ((mx.z - mn.z) * 0.5 * s).max(0.05);
        // Centre offset (model space): the AABB is rarely centred on the
        // origin — we translate so the prop's footprint sits at `tile_pos`.
        let local_center = ((mn + mx) * 0.5) * s;

        // After yaw rotation, compute the world-space half-extents along
        // X and Z so we can push the prop away from / toward the wall.
        let (sin_y, cos_y) = yaw.sin_cos();
        let world_half_x = (cos_y.abs() * half_x) + (sin_y.abs() * half_z);
        let world_half_z = (sin_y.abs() * half_x) + (cos_y.abs() * half_z);

        // Tile centre is 0.5 from the inner wall face.  For wall-aligned
        // props, push toward the wall so the back face touches it (less a
        // 4 cm gap to avoid z-fighting).
        let mut pos = tile_pos;
        if def.against_wall && wall_dir != (0, 0) {
            let (ox, oz) = wall_dir;
            let inner_wall_dist = 0.5; // wall face is 0.5 from tile centre
            let half_along = if ox != 0 { world_half_x } else { world_half_z };
            let push = (inner_wall_dist - half_along - 0.04).max(0.0);
            pos.x += ox as f32 * push;
            pos.z += oz as f32 * push;
        }
        // Lift so the prop's footprint sits on the floor (y = 0).
        pos.y = -mn.y * s;

        // The model's authored origin may be offset from its bbox centre,
        // which would skew the placement.  Compensate after rotating that
        // offset into world space.
        let centre_offset = Vec3::new(
            cos_y * local_center.x + sin_y * local_center.z,
            0.0,
            -sin_y * local_center.x + cos_y * local_center.z,
        );
        let placement = pos - Vec3::new(centre_offset.x, 0.0, centre_offset.z);

        let model = Mat4::from_scale_rotation_translation(
            Vec3::splat(def.scale),
            Quat::from_rotation_y(yaw),
            placement,
        );
        let mesh = match self.meshes.get(def.gltf) {
            Some(m) => m,
            None => return,
        };
        if renderer.add_mesh(mesh, model).is_ok() {
            let idx = renderer.objects.len() - 1;
            if let Some(ds) = trim_set {
                renderer.set_object_shared_material(idx, ds);
            }
            if def.solid {
                // Static AABB collider, slightly shrunk so the player can
                // squeeze past without snagging on corners.
                let collider_half = Vec3::new(
                    (world_half_x * 0.85).max(0.10),
                    half_y.max(0.20),
                    (world_half_z * 0.85).max(0.10),
                );
                let collider_pos = pos + Vec3::new(0.0, half_y, 0.0);
                world.spawn((
                    Transform::from_position(collider_pos),
                    Collider::new(collider_half.x, collider_half.y, collider_half.z),
                    Static,
                ));
            }
        }
    }

    /// Free the trim-sheet textures owned by this library.  Must be called
    /// before the renderer's allocator is dropped (typically from
    /// `App::shutdown`).
    pub fn cleanup_gpu(&mut self, device: &Device, allocator: &Arc<Mutex<Allocator>>) {
        for mut tex in self.trim_textures.drain(..) {
            tex.cleanup(device, allocator);
        }
        self.trim_sets.clear();
    }
}

/// Compute the local-space AABB of a static mesh.
fn aabb_of(mesh: &Mesh) -> (Vec3, Vec3) {
    let mut mn = Vec3::splat(f32::INFINITY);
    let mut mx = Vec3::splat(f32::NEG_INFINITY);
    for v in &mesh.vertices {
        mn = mn.min(v.position);
        mx = mx.max(v.position);
    }
    if !mn.is_finite() {
        (Vec3::ZERO, Vec3::ZERO)
    } else {
        (mn, mx)
    }
}

/// Tiny xorshift64 RNG, identical in spirit to the engine's `dungeon::SimpleRng`
/// (which is `pub(crate)` and not reachable from here).
struct SmallRng {
    state: u64,
}

impl SmallRng {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed },
        }
    }
    fn next(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }
    fn range(&mut self, lo: u32, hi: u32) -> u32 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next() % (hi - lo) as u64) as u32
    }
}
