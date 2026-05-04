use glam::{Mat4, Vec3};
use rift_engine::loot::item::{ItemRarity, ItemSlot};
use rift_engine::loot::Equipment;
use rift_engine::renderer::mesh::{Mesh, Vertex};
use rift_engine::Renderer;

/// Visible equipment slots that get rendered on the player.
const VISIBLE_SLOTS: &[ItemSlot] = &[
    ItemSlot::Helmet,
    ItemSlot::Chest,
    ItemSlot::Boots,
    ItemSlot::Weapon,
];

/// Body-part offsets relative to player center (at feet).
fn slot_offset(slot: ItemSlot) -> Vec3 {
    match slot {
        ItemSlot::Helmet => Vec3::new(0.0, 1.05, 0.0),  // top of head
        ItemSlot::Chest => Vec3::new(0.0, 0.55, 0.0),   // torso center
        ItemSlot::Boots => Vec3::new(0.0, 0.05, 0.0),   // feet
        ItemSlot::Weapon => Vec3::new(0.45, 0.4, 0.0),  // right hand
        _ => Vec3::ZERO,
    }
}

/// Tracks render object indices for each visible equipment slot.
pub struct EquipmentVisuals {
    /// (slot, render_object_index, currently_shown_rarity)
    slots: Vec<(ItemSlot, usize, Option<ItemRarity>)>,
}

impl EquipmentVisuals {
    pub fn new() -> Self {
        Self { slots: Vec::new() }
    }

    /// Create render objects for all visible slots (initially hidden).
    /// Call this after player spawn during floor generation.
    pub fn init(&mut self, renderer: &mut Renderer) {
        self.slots.clear();
        for &slot in VISIBLE_SLOTS {
            let mesh = slot_mesh(slot, [0.8, 0.8, 0.8]); // default gray, will be recolored
            if renderer.add_mesh(&mesh, Mat4::ZERO).is_ok() {
                let obj_idx = renderer.objects.len() - 1;
                self.slots.push((slot, obj_idx, None));
            }
        }
    }

    /// Update equipment visuals based on current equipment and player position.
    /// Call each frame after render_sync_system.
    pub fn sync(&mut self, equipment: &Equipment, player_pos: Vec3, renderer: &mut Renderer) {
        for (slot, obj_idx, shown_rarity) in &mut self.slots {
            let equipped = equipment.get(*slot);

            match equipped {
                Some(item) => {
                    // If rarity changed, rebuild the mesh with new color in-place
                    if *shown_rarity != Some(item.rarity) {
                        *shown_rarity = Some(item.rarity);
                        let color = item.rarity.color();
                        let new_mesh = slot_mesh(*slot, color);
                        renderer.replace_mesh(*obj_idx, &new_mesh).ok();
                    }

                    // Show the piece at the correct offset from player
                    let offset = slot_offset(*slot);
                    let world_pos = player_pos + offset;

                    // Scale based on slot
                    let scale = match *slot {
                        ItemSlot::Weapon => Vec3::new(0.12, 0.5, 0.12),
                        ItemSlot::Helmet => Vec3::splat(0.22),
                        ItemSlot::Chest => Vec3::new(0.42, 0.3, 0.3),
                        ItemSlot::Boots => Vec3::new(0.32, 0.12, 0.25),
                        _ => Vec3::splat(0.2),
                    };

                    if *obj_idx < renderer.objects.len() {
                        renderer.objects[*obj_idx].model_matrix =
                            Mat4::from_translation(world_pos) * Mat4::from_scale(scale);
                    }
                }
                None => {
                    // Hide the piece
                    if *obj_idx < renderer.objects.len() {
                        renderer.objects[*obj_idx].model_matrix = Mat4::ZERO;
                    }
                    *shown_rarity = None;
                }
            }
        }
    }

    pub fn clear(&mut self) {
        self.slots.clear();
    }
}

/// Generate a mesh for a specific equipment slot, tinted with a color.
fn slot_mesh(slot: ItemSlot, color: [f32; 3]) -> Mesh {
    let c = Vec3::from(color);
    match slot {
        ItemSlot::Helmet => helmet_mesh(c),
        ItemSlot::Chest => chest_mesh(c),
        ItemSlot::Boots => boots_mesh(c),
        ItemSlot::Weapon => weapon_mesh(c),
        _ => Mesh { vertices: Vec::new(), indices: Vec::new() },
    }
}

/// Helmet: a crown/dome shape on top of the head.
fn helmet_mesh(color: Vec3) -> Mesh {
    use glam::Vec2;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let segments = 8u32;
    let radius = 1.0_f32;
    let height = 0.8_f32;
    let rim_color = color * 1.2; // brighter rim

    // Dome (half sphere approximation)
    // Bottom ring
    let center_idx = vertices.len() as u32;
    vertices.push(Vertex { position: Vec3::new(0.0, height, 0.0), normal: Vec3::Y, color: rim_color.min(Vec3::ONE), uv: Vec2::new(0.5, 0.5) });

    for i in 0..segments {
        let angle0 = (i as f32 / segments as f32) * std::f32::consts::TAU;
        let angle1 = ((i + 1) as f32 / segments as f32) * std::f32::consts::TAU;
        let idx = vertices.len() as u32;
        let normal = Vec3::new(angle0.sin(), 0.3, angle0.cos()).normalize();

        // Rim verts
        vertices.push(Vertex { position: Vec3::new(angle0.sin() * radius, 0.0, angle0.cos() * radius), normal, color, uv: Vec2::new(0.5, 0.5) });
        vertices.push(Vertex { position: Vec3::new(angle1.sin() * radius, 0.0, angle1.cos() * radius), normal, color, uv: Vec2::new(0.5, 0.5) });
        // Mid ring (dome)
        vertices.push(Vertex { position: Vec3::new(angle0.sin() * radius * 0.85, height * 0.6, angle0.cos() * radius * 0.85), normal: Vec3::new(angle0.sin(), 0.5, angle0.cos()).normalize(), color, uv: Vec2::new(0.5, 0.5) });
        vertices.push(Vertex { position: Vec3::new(angle1.sin() * radius * 0.85, height * 0.6, angle1.cos() * radius * 0.85), normal: Vec3::new(angle1.sin(), 0.5, angle1.cos()).normalize(), color, uv: Vec2::new(0.5, 0.5) });

        // Side quad (rim to mid)
        indices.extend_from_slice(&[idx, idx+1, idx+3, idx+3, idx+2, idx]);
        // Top triangles (mid to apex)
        indices.extend_from_slice(&[center_idx, idx+2, idx+3]);
    }

    Mesh { vertices, indices }
}

/// Chest: shoulder pads + body plate
fn chest_mesh(color: Vec3) -> Mesh {
    use glam::Vec2;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    let v = |pos: Vec3, normal: Vec3| -> Vertex {
        Vertex { position: pos, normal, color, uv: Vec2::new(0.5, 0.5) }
    };

    // Front plate (a trapezoid)
    let base_idx = vertices.len() as u32;
    vertices.push(v(Vec3::new(-1.0, -0.8, 0.55), Vec3::Z));
    vertices.push(v(Vec3::new( 1.0, -0.8, 0.55), Vec3::Z));
    vertices.push(v(Vec3::new( 0.8,  0.8, 0.55), Vec3::Z));
    vertices.push(v(Vec3::new(-0.8,  0.8, 0.55), Vec3::Z));
    indices.extend_from_slice(&[base_idx, base_idx+1, base_idx+2, base_idx+2, base_idx+3, base_idx]);

    // Back plate
    let base_idx = vertices.len() as u32;
    vertices.push(v(Vec3::new( 1.0, -0.8, -0.45), -Vec3::Z));
    vertices.push(v(Vec3::new(-1.0, -0.8, -0.45), -Vec3::Z));
    vertices.push(v(Vec3::new(-0.8,  0.8, -0.45), -Vec3::Z));
    vertices.push(v(Vec3::new( 0.8,  0.8, -0.45), -Vec3::Z));
    indices.extend_from_slice(&[base_idx, base_idx+1, base_idx+2, base_idx+2, base_idx+3, base_idx]);

    // Left shoulder pad
    let brighter = (color * 1.1).min(Vec3::ONE);
    let sv = |pos: Vec3, normal: Vec3| -> Vertex {
        Vertex { position: pos, normal, color: brighter, uv: Vec2::new(0.5, 0.5) }
    };
    let base_idx = vertices.len() as u32;
    vertices.push(sv(Vec3::new(-0.8, 0.5, -0.4), Vec3::new(-0.5, 0.5, 0.0).normalize()));
    vertices.push(sv(Vec3::new(-0.8, 0.5,  0.4), Vec3::new(-0.5, 0.5, 0.0).normalize()));
    vertices.push(sv(Vec3::new(-1.3, 0.9,  0.0), Vec3::new(-0.5, 0.8, 0.0).normalize()));
    indices.extend_from_slice(&[base_idx, base_idx+1, base_idx+2]);

    // Right shoulder pad
    let base_idx = vertices.len() as u32;
    vertices.push(sv(Vec3::new(0.8, 0.5,  0.4), Vec3::new(0.5, 0.5, 0.0).normalize()));
    vertices.push(sv(Vec3::new(0.8, 0.5, -0.4), Vec3::new(0.5, 0.5, 0.0).normalize()));
    vertices.push(sv(Vec3::new(1.3, 0.9,  0.0), Vec3::new(0.5, 0.8, 0.0).normalize()));
    indices.extend_from_slice(&[base_idx, base_idx+1, base_idx+2]);

    Mesh { vertices, indices }
}

/// Boots: two foot-shaped pieces.
fn boots_mesh(color: Vec3) -> Mesh {
    use glam::Vec2;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    // Simple box boot shape for each foot
    for side in [-1.0_f32, 1.0] {
        let ox = side * 0.4; // lateral offset
        let _base_idx = vertices.len() as u32;

        let w = 0.5_f32; // width
        let h = 0.8_f32; // height  
        let d = 0.9_f32; // depth (front-to-back)

        // 8 corners of a box
        let positions = [
            Vec3::new(ox - w*0.5, 0.0, -d*0.5),
            Vec3::new(ox + w*0.5, 0.0, -d*0.5),
            Vec3::new(ox + w*0.5, 0.0,  d*0.5),
            Vec3::new(ox - w*0.5, 0.0,  d*0.5),
            Vec3::new(ox - w*0.4, h,   -d*0.4),
            Vec3::new(ox + w*0.4, h,   -d*0.4),
            Vec3::new(ox + w*0.4, h,    d*0.3),
            Vec3::new(ox - w*0.4, h,    d*0.3),
        ];

        let faces: &[(usize, usize, usize, usize, Vec3)] = &[
            (0, 1, 5, 4, -Vec3::Z), // back
            (2, 3, 7, 6,  Vec3::Z), // front
            (0, 3, 7, 4, -Vec3::X), // left
            (1, 2, 6, 5,  Vec3::X), // right
            (4, 5, 6, 7,  Vec3::Y), // top
        ];

        for &(a, b, c, d, normal) in faces {
            let fi = vertices.len() as u32;
            vertices.push(Vertex { position: positions[a], normal, color, uv: Vec2::new(0.5, 0.5) });
            vertices.push(Vertex { position: positions[b], normal, color, uv: Vec2::new(0.5, 0.5) });
            vertices.push(Vertex { position: positions[c], normal, color, uv: Vec2::new(0.5, 0.5) });
            vertices.push(Vertex { position: positions[d], normal, color, uv: Vec2::new(0.5, 0.5) });
            indices.extend_from_slice(&[fi, fi+1, fi+2, fi+2, fi+3, fi]);
        }
    }

    Mesh { vertices, indices }
}

/// Weapon: a blade shape.
fn weapon_mesh(color: Vec3) -> Mesh {
    use glam::Vec2;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    let blade_color = color;
    let hilt_color = Vec3::new(0.3, 0.25, 0.1); // brown hilt

    // Blade — elongated diamond
    let blade_len = 1.8_f32;
    let blade_w = 0.2_f32;

    // Front face
    let bi = vertices.len() as u32;
    vertices.push(Vertex { position: Vec3::new(0.0, 0.0, 0.0), normal: Vec3::Z, color: hilt_color, uv: Vec2::new(0.5, 0.5) });        // base
    vertices.push(Vertex { position: Vec3::new(-blade_w, blade_len * 0.3, 0.02), normal: Vec3::Z, color: blade_color, uv: Vec2::new(0.5, 0.5) });  // left edge
    vertices.push(Vertex { position: Vec3::new(0.0, blade_len, 0.0), normal: Vec3::Z, color: blade_color, uv: Vec2::new(0.5, 0.5) });  // tip
    vertices.push(Vertex { position: Vec3::new(blade_w, blade_len * 0.3, 0.02), normal: Vec3::Z, color: blade_color, uv: Vec2::new(0.5, 0.5) });   // right edge
    indices.extend_from_slice(&[bi, bi+1, bi+2, bi, bi+2, bi+3]);

    // Back face
    let bi = vertices.len() as u32;
    vertices.push(Vertex { position: Vec3::new(0.0, 0.0, 0.0), normal: -Vec3::Z, color: hilt_color, uv: Vec2::new(0.5, 0.5) });
    vertices.push(Vertex { position: Vec3::new(blade_w, blade_len * 0.3, -0.02), normal: -Vec3::Z, color: blade_color, uv: Vec2::new(0.5, 0.5) });
    vertices.push(Vertex { position: Vec3::new(0.0, blade_len, 0.0), normal: -Vec3::Z, color: blade_color, uv: Vec2::new(0.5, 0.5) });
    vertices.push(Vertex { position: Vec3::new(-blade_w, blade_len * 0.3, -0.02), normal: -Vec3::Z, color: blade_color, uv: Vec2::new(0.5, 0.5) });
    indices.extend_from_slice(&[bi, bi+1, bi+2, bi, bi+2, bi+3]);

    // Crossguard
    let bi = vertices.len() as u32;
    let cg_w = 0.4_f32;
    let cg_h = 0.08_f32;
    vertices.push(Vertex { position: Vec3::new(-cg_w, -cg_h, -0.03), normal: Vec3::Y, color: hilt_color, uv: Vec2::new(0.5, 0.5) });
    vertices.push(Vertex { position: Vec3::new( cg_w, -cg_h, -0.03), normal: Vec3::Y, color: hilt_color, uv: Vec2::new(0.5, 0.5) });
    vertices.push(Vertex { position: Vec3::new( cg_w,  cg_h,  0.03), normal: Vec3::Y, color: hilt_color, uv: Vec2::new(0.5, 0.5) });
    vertices.push(Vertex { position: Vec3::new(-cg_w,  cg_h,  0.03), normal: Vec3::Y, color: hilt_color, uv: Vec2::new(0.5, 0.5) });
    indices.extend_from_slice(&[bi, bi+1, bi+2, bi+2, bi+3, bi]);

    Mesh { vertices, indices }
}
