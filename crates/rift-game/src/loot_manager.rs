use glam::{Mat4, Vec3};
use rift_engine::ecs::components::{Health, Player};
use rift_engine::loot::item::{ItemKind, PotionType};
use rift_engine::loot::inventory::PlayerStats;
use rift_engine::loot::{DropTable, Equipment, Inventory, LootDrop};
use rift_engine::{Emitter, EmitterConfig, Mesh, Renderer};

/// Manages ground loot: rendering, physics, pickup, cleanup.
pub struct LootManager {
    pub ground_loot: Vec<LootDrop>,
    pub loot_seed: u64,
    /// Index of the item currently hovered by cursor (if any).
    pub hovered_index: Option<usize>,
}

impl LootManager {
    pub fn new() -> Self {
        Self {
            ground_loot: Vec::new(),
            loot_seed: 12345,
            hovered_index: None,
        }
    }

    pub fn clear(&mut self) {
        self.ground_loot.clear();
        self.hovered_index = None;
    }

    /// Roll drops for a kill and add them to ground loot.
    pub fn spawn_drops(&mut self, floor: u32, position: Vec3, is_boss: bool, is_elite: bool) {
        self.loot_seed = self.loot_seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let table = if is_boss {
            DropTable::boss()
        } else if is_elite {
            DropTable::elite()
        } else {
            DropTable::enemy()
        };
        let drops = table.roll(floor, position, self.loot_seed);
        for drop in drops {
            log::info!("  LOOT: {} ({:?})", drop.item.display_name, drop.item.rarity);
            self.ground_loot.push(drop);
        }
    }

    /// Spawn a single item on the ground at the given position.
    pub fn spawn_drop(&mut self, item: rift_engine::loot::item::Item, position: Vec3, _renderer: &mut Renderer) {
        log::info!("  DROPPED: {} ({:?})", item.display_name, item.rarity);
        self.ground_loot.push(LootDrop {
            item,
            position,
            lifetime: 120.0,
            velocity: Vec3::ZERO,
            grounded: true,
            age: 0.0,
            orb_obj_index: None,
            beam_obj_index: None,
            emitter_index: None,
            base_emitter_index: None,
        });
    }

    /// Try to pick up the nearest grounded item in range. Returns true if something was picked up.
    pub fn try_pickup(
        &mut self,
        player_pos: Vec3,
        pickup_radius: f32,
        inventory: &mut Inventory,
        equipment: &mut Equipment,
        stats: &PlayerStats,
        world: &mut hecs::World,
        renderer: &mut Renderer,
    ) -> bool {
        let nearest = self
            .ground_loot
            .iter()
            .enumerate()
            .filter(|(_, d)| d.grounded)
            .map(|(i, d)| (i, (d.position - player_pos).length()))
            .filter(|(_, dist)| *dist < pickup_radius)
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

        let Some((i, _)) = nearest else {
            return false;
        };

        let drop = self.ground_loot.remove(i);
        let item_name = drop.item.display_name.clone();
        let rarity = drop.item.rarity;

        // Hide render objects
        if let Some(idx) = drop.orb_obj_index {
            if idx < renderer.objects.len() {
                renderer.objects[idx].model_matrix = Mat4::ZERO;
            }
        }
        if let Some(idx) = drop.beam_obj_index {
            if idx < renderer.objects.len() {
                renderer.objects[idx].model_matrix = Mat4::ZERO;
            }
        }
        if let Some(idx) = drop.emitter_index {
            renderer.particle_system.deactivate_emitter(idx);
        }
        if let Some(idx) = drop.base_emitter_index {
            renderer.particle_system.deactivate_emitter(idx);
        }

        // Auto-equip if it's better, otherwise add to inventory
        if let Some(slot) = drop.item.slot() {
            let should_equip = match equipment.get(slot) {
                None => true,
                Some(current) => {
                    let current_power = current.total_damage() + current.total_defense();
                    let new_power = drop.item.total_damage() + drop.item.total_defense();
                    new_power > current_power
                }
            };

            if should_equip {
                let old = equipment.equip(drop.item);
                log::info!("  EQUIPPED: {} ({:?})", item_name, rarity);
                if let Some(old_item) = old {
                    inventory.add_item(old_item);
                }
            } else {
                inventory.add_item(drop.item);
                log::info!("  PICKED UP: {} ({:?})", item_name, rarity);
            }
        } else {
            // Potion — use immediately
            if let ItemKind::Potion(potion_type) = drop.item.base.kind {
                match potion_type {
                    PotionType::Health => {
                        for (_id, (health, _player)) in
                            world.query_mut::<(&mut Health, &Player)>()
                        {
                            let heal = drop.item.base.base_value;
                            health.current =
                                (health.current + heal).min(health.max + stats.max_hp_bonus);
                            log::info!("  HEALED: +{:.0} HP", heal);
                        }
                    }
                    _ => {
                        log::info!("  USED: {}", item_name);
                    }
                }
            }
        }

        true
    }

    /// Find the ground loot item closest to `cursor_pos` (XZ) within `radius`.
    /// Returns the index into `ground_loot`.
    pub fn item_under_cursor(&self, cursor_pos: Vec3, radius: f32) -> Option<usize> {
        let mut best: Option<(usize, f32)> = None;
        for (i, drop) in self.ground_loot.iter().enumerate() {
            if !drop.grounded {
                continue;
            }
            let delta = cursor_pos - drop.position;
            let dist = Vec3::new(delta.x, 0.0, delta.z).length();
            if dist < radius {
                if best.is_none() || dist < best.unwrap().1 {
                    best = Some((i, dist));
                }
            }
        }
        best.map(|(i, _)| i)
    }

    /// Update hover state: scale up the hovered item's orb, reset the previous one.
    pub fn update_hover(&mut self, new_hover: Option<usize>, _renderer: &mut Renderer) {
        // Un-hover the old one (will be fixed to correct transform in tick())
        if self.hovered_index != new_hover {
            self.hovered_index = new_hover;
        }
    }

    /// Returns true if an item is currently hovered.
    pub fn has_hover(&self) -> bool {
        self.hovered_index.is_some()
    }

    /// Pick up a specific item by index.
    pub fn pickup_at(
        &mut self,
        index: usize,
        inventory: &mut Inventory,
        equipment: &mut Equipment,
        stats: &PlayerStats,
        world: &mut hecs::World,
        renderer: &mut Renderer,
    ) -> bool {
        if index >= self.ground_loot.len() {
            return false;
        }

        let drop = self.ground_loot.remove(index);
        // Fix hover index if needed
        if self.hovered_index == Some(index) {
            self.hovered_index = None;
        } else if let Some(h) = self.hovered_index {
            if h > index {
                self.hovered_index = Some(h - 1);
            }
        }

        let item_name = drop.item.display_name.clone();
        let rarity = drop.item.rarity;

        // Hide render objects
        if let Some(idx) = drop.orb_obj_index {
            if idx < renderer.objects.len() {
                renderer.objects[idx].model_matrix = Mat4::ZERO;
            }
        }
        if let Some(idx) = drop.beam_obj_index {
            if idx < renderer.objects.len() {
                renderer.objects[idx].model_matrix = Mat4::ZERO;
            }
        }
        if let Some(idx) = drop.emitter_index {
            renderer.particle_system.deactivate_emitter(idx);
        }
        if let Some(idx) = drop.base_emitter_index {
            renderer.particle_system.deactivate_emitter(idx);
        }

        // Auto-equip if it's better, otherwise add to inventory
        if let Some(slot) = drop.item.slot() {
            let should_equip = match equipment.get(slot) {
                None => true,
                Some(current) => {
                    let current_power = current.total_damage() + current.total_defense();
                    let new_power = drop.item.total_damage() + drop.item.total_defense();
                    new_power > current_power
                }
            };

            if should_equip {
                let old = equipment.equip(drop.item);
                log::info!("  EQUIPPED: {} ({:?})", item_name, rarity);
                if let Some(old_item) = old {
                    inventory.add_item(old_item);
                }
            } else {
                inventory.add_item(drop.item);
                log::info!("  PICKED UP: {} ({:?})", item_name, rarity);
            }
        } else {
            // Potion — use immediately
            if let ItemKind::Potion(potion_type) = drop.item.base.kind {
                match potion_type {
                    PotionType::Health => {
                        for (_id, (health, _player)) in
                            world.query_mut::<(&mut Health, &Player)>()
                        {
                            let heal = drop.item.base.base_value;
                            health.current =
                                (health.current + heal).min(health.max + stats.max_hp_bonus);
                            log::info!("  HEALED: +{:.0} HP", heal);
                        }
                    }
                    _ => {
                        log::info!("  USED: {}", item_name);
                    }
                }
            }
        }

        true
    }

    /// Tick loot physics + create/update render objects.
    pub fn tick(&mut self, renderer: &mut Renderer, dt: f32) {
        for (i, drop) in self.ground_loot.iter_mut().enumerate() {
            drop.tick_physics(dt);
            drop.lifetime -= dt;

            // Create render objects for new drops
            if drop.orb_obj_index.is_none() {
                let color = drop.item.rarity.color();
                let orb_mesh = Mesh::loot_orb(color);
                if renderer
                    .add_mesh(&orb_mesh, Mat4::from_translation(drop.position))
                    .is_ok()
                {
                    drop.orb_obj_index = Some(renderer.objects.len() - 1);
                }
            }
            // Loot beam is purely particle-based (no mesh geometry)
            // Spawn particle emitters when grounded: column + base pulse
            if drop.emitter_index.is_none() && drop.grounded {
                let color = drop.item.rarity.color();
                let emitter = Emitter::new(drop.position, EmitterConfig::loot_beam(color));
                let idx = renderer.particle_system.add_emitter(emitter);
                drop.emitter_index = Some(idx);
                // Base pulse (bright glow at item's feet)
                let base = Emitter::new(drop.position + Vec3::new(0.0, 0.1, 0.0), EmitterConfig::loot_beam_base(color));
                let base_idx = renderer.particle_system.add_emitter(base);
                drop.base_emitter_index = Some(base_idx);
            }

            // Update orb position
            if let Some(idx) = drop.orb_obj_index {
                if idx < renderer.objects.len() {
                    let bob = drop.bob_offset();
                    let pos = drop.position + Vec3::new(0.0, bob, 0.0);
                    let spin = Mat4::from_rotation_y(drop.age * 3.0);
                    let scale = if self.hovered_index == Some(i) { 1.6 } else { 1.0 };
                    renderer.objects[idx].model_matrix =
                        Mat4::from_translation(pos) * spin * Mat4::from_scale(Vec3::splat(scale));
                }
            }
        }

        // Remove expired loot
        self.ground_loot.retain(|d| {
            if d.lifetime <= 0.0 {
                if let Some(idx) = d.orb_obj_index {
                    if idx < renderer.objects.len() {
                        renderer.objects[idx].model_matrix = Mat4::ZERO;
                    }
                }
                if let Some(idx) = d.beam_obj_index {
                    if idx < renderer.objects.len() {
                        renderer.objects[idx].model_matrix = Mat4::ZERO;
                    }
                }
                if let Some(idx) = d.emitter_index {
                    renderer.particle_system.deactivate_emitter(idx);
                }
                if let Some(idx) = d.base_emitter_index {
                    renderer.particle_system.deactivate_emitter(idx);
                }
                false
            } else {
                true
            }
        });
    }
}
