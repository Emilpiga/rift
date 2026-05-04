use glam::Vec3;

use super::generation::{generate_item, generate_potion};
use super::item::{Item, ItemRarity, ItemSlot, PotionType};

/// A loot drop in the world (not yet picked up).
#[derive(Clone, Debug)]
pub struct LootDrop {
    pub item: Item,
    pub position: Vec3,
    /// Time remaining before auto-despawn (seconds).
    pub lifetime: f32,
    /// Velocity for the initial burst (explosion arc).
    pub velocity: Vec3,
    /// Whether the item has landed (stopped moving).
    pub grounded: bool,
    /// Time since this item spawned (for bob animation).
    pub age: f32,
    /// Render object index for the item orb (-1 if not yet rendered).
    pub orb_obj_index: Option<usize>,
    /// Render object index for the light beam.
    pub beam_obj_index: Option<usize>,
    /// Particle emitter index (if spawned).
    pub emitter_index: Option<usize>,
    /// Base pulse emitter index (if spawned).
    pub base_emitter_index: Option<usize>,
}

impl LootDrop {
    /// Tick physics: apply gravity and ground the item.
    pub fn tick_physics(&mut self, dt: f32) {
        self.age += dt;
        if !self.grounded {
            // Strong gravity to land quickly
            self.velocity.y -= 25.0 * dt;
            self.position += self.velocity * dt;

            // Hit the ground
            if self.position.y <= 0.3 {
                self.position.y = 0.3;
                self.grounded = true;
                self.velocity = Vec3::ZERO;
            }
        }
    }

    /// Get the bob offset for a grounded item (gentle float).
    pub fn bob_offset(&self) -> f32 {
        if self.grounded {
            (self.age * 2.5).sin() * 0.08
        } else {
            0.0
        }
    }

    /// Light beam height (scales up as item settles, then pulses).
    pub fn beam_height(&self) -> f32 {
        if !self.grounded {
            return 0.0; // No beam while in the air
        }
        let base = match self.item.rarity {
            ItemRarity::Common => 2.0,
            ItemRarity::Magic => 4.0,
            ItemRarity::Rare => 6.0,
            ItemRarity::Epic => 8.0,
            ItemRarity::Legendary => 12.0,
            ItemRarity::Ascended => 14.0,
            ItemRarity::Eternal => 16.0,
        };
        // Fade in over 0.3s after landing
        let age_since_land = (self.age - 0.3).max(0.0); // approx
        let fade_in = (age_since_land * 4.0).min(1.0);
        let pulse = 1.0 + (self.age * 3.0).sin() * 0.05;
        base * fade_in * pulse
    }
}

/// Drop table entry: what can drop and with what weight.
#[derive(Clone, Debug)]
pub struct DropEntry {
    pub slot: DropSlot,
    pub weight: u32,
}

/// What slot/type of item to drop.
#[derive(Clone, Copy, Debug)]
pub enum DropSlot {
    Weapon,
    Helmet,
    Chest,
    Boots,
    Ring,
    Amulet,
    HealthPotion,
    SpeedPotion,
    DamagePotion,
    Nothing, // Weighted "no drop"
}

/// Configuration for what a particular enemy type drops.
#[derive(Clone, Debug)]
pub struct DropTable {
    pub entries: Vec<DropEntry>,
    /// Number of rolls on the table per kill.
    pub rolls: u8,
}

impl DropTable {
    /// Standard enemy drop table.
    pub fn enemy() -> Self {
        Self {
            entries: vec![
                DropEntry { slot: DropSlot::Nothing, weight: 45 },
                DropEntry { slot: DropSlot::HealthPotion, weight: 20 },
                DropEntry { slot: DropSlot::Weapon, weight: 7 },
                DropEntry { slot: DropSlot::Helmet, weight: 5 },
                DropEntry { slot: DropSlot::Chest, weight: 5 },
                DropEntry { slot: DropSlot::Boots, weight: 5 },
                DropEntry { slot: DropSlot::Ring, weight: 5 },
                DropEntry { slot: DropSlot::Amulet, weight: 5 },
                DropEntry { slot: DropSlot::SpeedPotion, weight: 2 },
                DropEntry { slot: DropSlot::DamagePotion, weight: 1 },
            ],
            rolls: 1,
        }
    }

    /// Boss drop table — guaranteed drops, higher quality.
    pub fn boss() -> Self {
        Self {
            entries: vec![
                DropEntry { slot: DropSlot::Weapon, weight: 20 },
                DropEntry { slot: DropSlot::Helmet, weight: 15 },
                DropEntry { slot: DropSlot::Chest, weight: 15 },
                DropEntry { slot: DropSlot::Boots, weight: 15 },
                DropEntry { slot: DropSlot::Ring, weight: 12 },
                DropEntry { slot: DropSlot::Amulet, weight: 12 },
                DropEntry { slot: DropSlot::HealthPotion, weight: 8 },
                DropEntry { slot: DropSlot::SpeedPotion, weight: 3 },
            ],
            rolls: 3, // Bosses drop 3 items
        }
    }

    /// Elite enemy drop table — guaranteed drop, better odds than normal mobs.
    pub fn elite() -> Self {
        Self {
            entries: vec![
                DropEntry { slot: DropSlot::HealthPotion, weight: 15 },
                DropEntry { slot: DropSlot::Weapon, weight: 15 },
                DropEntry { slot: DropSlot::Helmet, weight: 12 },
                DropEntry { slot: DropSlot::Chest, weight: 12 },
                DropEntry { slot: DropSlot::Boots, weight: 12 },
                DropEntry { slot: DropSlot::Ring, weight: 10 },
                DropEntry { slot: DropSlot::Amulet, weight: 10 },
                DropEntry { slot: DropSlot::SpeedPotion, weight: 5 },
                DropEntry { slot: DropSlot::DamagePotion, weight: 5 },
            ],
            rolls: 2, // Elites always drop 2 items
        }
    }

    /// Roll this drop table and produce items with explosion physics.
    pub fn roll(&self, floor: u32, position: Vec3, base_seed: u64) -> Vec<LootDrop> {
        let mut drops = Vec::new();
        let mut seed = base_seed;
        let num_items_total = self.rolls;

        for i in 0..self.rolls {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let total_weight: u32 = self.entries.iter().map(|e| e.weight).sum();
            let mut roll = (seed % total_weight as u64) as u32;

            let mut selected_slot = DropSlot::Nothing;
            for entry in &self.entries {
                if roll < entry.weight {
                    selected_slot = entry.slot;
                    break;
                }
                roll -= entry.weight;
            }

            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(7);
            let item = match selected_slot {
                DropSlot::Nothing => continue,
                DropSlot::Weapon => generate_item(floor, ItemSlot::Weapon, seed),
                DropSlot::Helmet => generate_item(floor, ItemSlot::Helmet, seed),
                DropSlot::Chest => generate_item(floor, ItemSlot::Chest, seed),
                DropSlot::Boots => generate_item(floor, ItemSlot::Boots, seed),
                DropSlot::Ring => generate_item(floor, ItemSlot::Ring, seed),
                DropSlot::Amulet => generate_item(floor, ItemSlot::Amulet, seed),
                DropSlot::HealthPotion => generate_potion(floor, PotionType::Health, seed),
                DropSlot::SpeedPotion => generate_potion(floor, PotionType::Speed, seed),
                DropSlot::DamagePotion => generate_potion(floor, PotionType::Damage, seed),
            };

            // Compute burst velocity — items pop out in a tight radial pattern
            let angle = std::f32::consts::TAU * (i as f32 / num_items_total as f32)
                + (((seed >> 48) & 0xFFFF) as f32 / u16::MAX as f32) * 0.5;
            let burst_speed = 0.5 + (((seed >> 40) & 0xFF) as f32 / 255.0) * 0.5;
            let upward = 2.0 + ((seed >> 56) as f32 / 255.0) * 1.0;

            let velocity = Vec3::new(
                angle.cos() * burst_speed,
                upward,
                angle.sin() * burst_speed,
            );

            drops.push(LootDrop {
                item,
                position: Vec3::new(position.x, (position.y + 0.5).max(0.5), position.z),
                lifetime: 60.0,
                velocity,
                grounded: false,
                age: 0.0,
                orb_obj_index: None,
                beam_obj_index: None,
                emitter_index: None,
                base_emitter_index: None,
            });
        }

        drops
    }
}
