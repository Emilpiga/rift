use rift_engine::ecs::components::{Enemy, Health, Player, Transform};
use rift_engine::loot::item::ItemSlot;
use rift_engine::loot::Equipment;
use rift_engine::renderer::OverlayBatch;
use glam::Mat4;

use crate::player::PlayerState;
use crate::rift_state::RiftState;
use rift_engine::combat::AbilitySlot;

/// Render all HUD elements.
pub fn render_hud(
    batch: &mut OverlayBatch,
    world: &hecs::World,
    rift: &RiftState,
    player_state: &PlayerState,
    equipment: &Equipment,
    sw: f32,
    sh: f32,
    max_hp_bonus: f32,
) {
    // HP bar (top-left, 200x20 px)
    let hp_pct = world
        .query::<(&Health, &Player)>()
        .iter()
        .map(|(_, (h, _))| h.current / (h.max + max_hp_bonus))
        .next()
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);

    let bar_x = 10.0;
    let bar_y = 10.0;
    let bar_w = 200.0;
    let bar_h = 20.0;

    batch.rect_px(bar_x, bar_y, bar_w, bar_h, [0.1, 0.1, 0.1, 0.8], sw, sh);
    let hp_color = if hp_pct > 0.5 {
        [0.1, 0.8, 0.1, 0.9]
    } else if hp_pct > 0.25 {
        [0.9, 0.7, 0.0, 0.9]
    } else {
        [0.9, 0.1, 0.1, 0.9]
    };
    batch.rect_px(bar_x, bar_y, bar_w * hp_pct, bar_h, hp_color, sw, sh);
    // Border
    batch.rect_px(bar_x, bar_y, bar_w, 2.0, [0.4, 0.4, 0.4, 0.9], sw, sh);
    batch.rect_px(bar_x, bar_y + bar_h - 2.0, bar_w, 2.0, [0.4, 0.4, 0.4, 0.9], sw, sh);
    batch.rect_px(bar_x, bar_y, 2.0, bar_h, [0.4, 0.4, 0.4, 0.9], sw, sh);
    batch.rect_px(bar_x + bar_w - 2.0, bar_y, 2.0, bar_h, [0.4, 0.4, 0.4, 0.9], sw, sh);

    // XP bar (below HP, 200x10 px)
    let xp_pct = player_state.experience.progress();
    let xp_x = 10.0;
    let xp_y = 34.0;
    let xp_w = 200.0;
    let xp_h = 10.0;
    batch.rect_px(xp_x, xp_y, xp_w, xp_h, [0.1, 0.1, 0.1, 0.7], sw, sh);
    batch.rect_px(xp_x, xp_y, xp_w * xp_pct, xp_h, [0.4, 0.2, 0.9, 0.9], sw, sh);
    let level_text = format!("Lv.{}", player_state.experience.level);
    batch.text(&level_text, xp_x + xp_w + 6.0, xp_y, 14.0, [0.9, 0.9, 0.9, 1.0], sw, sh);

    // Rift progress bar (top-center, 300x16 px)
    let prog_pct = rift.progress_percent() / 100.0;
    let prog_w = 300.0;
    let prog_h = 16.0;
    let prog_x = (sw - prog_w) / 2.0;
    let prog_y = 10.0;
    batch.rect_px(prog_x, prog_y, prog_w, prog_h, [0.1, 0.1, 0.1, 0.8], sw, sh);
    batch.rect_px(prog_x, prog_y, prog_w * prog_pct, prog_h, [0.3, 0.5, 0.9, 0.9], sw, sh);

    // Floor indicator (top-right)
    let floor_w = 40.0;
    let floor_h = 20.0;
    batch.rect_px(sw - floor_w - 10.0, 10.0, floor_w, floor_h, [0.2, 0.2, 0.3, 0.8], sw, sh);
    let bars = (rift.floor as f32).min(10.0);
    let bar_unit_w = (floor_w - 6.0) / 10.0;
    for i in 0..bars as u32 {
        batch.rect_px(
            sw - floor_w - 10.0 + 3.0 + i as f32 * bar_unit_w,
            14.0,
            bar_unit_w - 1.0,
            floor_h - 8.0,
            [0.8, 0.7, 0.2, 0.9],
            sw,
            sh,
        );
    }

    // Equipment slots (bottom-left, 6 slots: 32x32 each)
    let slot_size = 32.0;
    let slot_gap = 4.0;
    let eq_x = 10.0;
    let eq_y = sh - slot_size - 10.0;
    let slots = [
        equipment.get(ItemSlot::Weapon),
        equipment.get(ItemSlot::Helmet),
        equipment.get(ItemSlot::Chest),
        equipment.get(ItemSlot::Boots),
        equipment.get(ItemSlot::Ring),
        equipment.get(ItemSlot::Amulet),
    ];
    for (i, slot) in slots.iter().enumerate() {
        let sx = eq_x + i as f32 * (slot_size + slot_gap);
        batch.rect_px(sx, eq_y, slot_size, slot_size, [0.15, 0.15, 0.2, 0.8], sw, sh);
        if let Some(item) = slot {
            let [r, g, b] = item.rarity.color();
            batch.rect_px(
                sx + 3.0,
                eq_y + 3.0,
                slot_size - 6.0,
                slot_size - 6.0,
                [r, g, b, 0.9],
                sw,
                sh,
            );
        }
    }

    // Portal indicator (if floor complete)
    if rift.floor_complete {
        let tw = 200.0;
        let th = 16.0;
        let tx = (sw - tw) / 2.0;
        let ty = 35.0;
        batch.rect_px(tx, ty, tw, th, [0.1, 0.15, 0.25, 0.85], sw, sh);
        batch.text("ENTER THE PORTAL", tx + 30.0, ty + 2.0, 12.0, [0.4, 0.7, 1.0, 1.0], sw, sh);
    }
}

/// Render the ability bar (bottom-center).
pub fn render_ability_bar(
    batch: &mut OverlayBatch,
    abilities: &AbilitySlot,
    mouse_pos: (f32, f32),
    sw: f32,
    sh: f32,
) {
    let ab_size = 40.0;
    let ab_gap = 4.0;
    let ab_total_w = 6.0 * ab_size + 5.0 * ab_gap;
    let ab_x = (sw - ab_total_w) / 2.0;
    let ab_y = sh - ab_size - 10.0;
    let ab_keys = ["LMB", "1", "2", "3", "4", "5"];

    let mut hovered_slot: Option<usize> = None;

    for (i, slot) in abilities.slots.iter().enumerate() {
        let sx = ab_x + i as f32 * (ab_size + ab_gap);

        // Check hover
        if mouse_pos.0 >= sx && mouse_pos.0 <= sx + ab_size
            && mouse_pos.1 >= ab_y && mouse_pos.1 <= ab_y + ab_size
        {
            hovered_slot = Some(i);
        }

        batch.rect_px(sx, ab_y, ab_size, ab_size, [0.12, 0.12, 0.18, 0.85], sw, sh);

        if let Some(state) = slot {
            let ready = state.ready();
            let color = if hovered_slot == Some(i) {
                [0.4, 0.7, 1.0, 0.95] // brighter on hover
            } else if ready {
                [0.3, 0.6, 0.9, 0.9]
            } else {
                [0.15, 0.2, 0.3, 0.7]
            };
            batch.rect_px(sx + 2.0, ab_y + 2.0, ab_size - 4.0, ab_size - 4.0, color, sw, sh);

            if !ready {
                let cd_pct = 1.0 - state.cooldown_progress();
                let cd_h = (ab_size - 4.0) * cd_pct;
                batch.rect_px(sx + 2.0, ab_y + 2.0, ab_size - 4.0, cd_h, [0.0, 0.0, 0.0, 0.6], sw, sh);
            }

            // Ability icon abbreviation
            let abbrev = match state.ability.name {
                "Steady Shot" => "SS",
                "Multi-Shot" => "MS",
                "Evasive Roll" => "ER",
                "Rapid Fire" => "RF",
                "Mark for Death" => "MK",
                "Rain of Arrows" => "RA",
                _ => "??",
            };
            batch.text(abbrev, sx + 10.0, ab_y + 8.0, 14.0, [1.0, 1.0, 1.0, 0.9], sw, sh);
        }

        batch.text(ab_keys[i], sx + 2.0, ab_y + ab_size - 12.0, 10.0, [0.7, 0.7, 0.7, 0.8], sw, sh);
    }

    // Tooltip for hovered ability
    if let Some(idx) = hovered_slot {
        if let Some(Some(state)) = abilities.slots.get(idx) {
            let tooltip_w = 220.0;
            let tooltip_h = 70.0;
            let tx = (sw - tooltip_w) / 2.0;
            let ty = ab_y - tooltip_h - 8.0;

            // Background
            batch.rect_px(tx, ty, tooltip_w, tooltip_h, [0.08, 0.08, 0.12, 0.95], sw, sh);
            // Border
            batch.rect_px(tx, ty, tooltip_w, 1.0, [0.3, 0.5, 0.8, 0.8], sw, sh);
            batch.rect_px(tx, ty + tooltip_h - 1.0, tooltip_w, 1.0, [0.3, 0.5, 0.8, 0.8], sw, sh);
            batch.rect_px(tx, ty, 1.0, tooltip_h, [0.3, 0.5, 0.8, 0.8], sw, sh);
            batch.rect_px(tx + tooltip_w - 1.0, ty, 1.0, tooltip_h, [0.3, 0.5, 0.8, 0.8], sw, sh);

            // Name
            batch.text(state.ability.name, tx + 8.0, ty + 6.0, 14.0, [1.0, 0.9, 0.5, 1.0], sw, sh);
            // Description
            batch.text(state.ability.description, tx + 8.0, ty + 24.0, 11.0, [0.8, 0.8, 0.8, 1.0], sw, sh);
            // Stats line
            let stats_text = if state.ability.cooldown > 0.0 {
                format!("CD: {:.1}s | Dmg: {:.0}%", state.ability.cooldown, state.ability.damage_mult * 100.0)
            } else {
                format!("Dmg: {:.0}%", state.ability.damage_mult * 100.0)
            };
            batch.text(&stats_text, tx + 8.0, ty + 42.0, 11.0, [0.6, 0.8, 1.0, 0.9], sw, sh);
            // Projectile info
            if state.ability.projectile_count > 1 {
                let proj_text = format!("Projectiles: {}", state.ability.projectile_count);
                batch.text(&proj_text, tx + 8.0, ty + 55.0, 10.0, [0.7, 0.7, 0.7, 0.8], sw, sh);
            }
        }
    }
}

/// Render thin health bars above enemies that have taken damage.
pub fn render_enemy_health_bars(
    batch: &mut OverlayBatch,
    world: &hecs::World,
    view_proj: Mat4,
    sw: f32,
    sh: f32,
) {
    let bar_w = 52.0;
    let bar_h = 6.0;
    let y_offset = -24.0; // pixels above the projected position

    for (_, (transform, _enemy, health)) in world.query::<(&Transform, &Enemy, &Health)>().iter() {
        // Only show bar if enemy has taken damage
        if health.current >= health.max {
            continue;
        }

        // Project world position to screen
        let world_pos = transform.position + glam::Vec3::new(0.0, 1.2, 0.0); // above head
        let clip = view_proj * world_pos.extend(1.0);

        // Behind camera check
        if clip.w <= 0.0 {
            continue;
        }

        let ndc = clip.truncate() / clip.w;
        // Off-screen check
        if ndc.x < -1.0 || ndc.x > 1.0 || ndc.y < -1.0 || ndc.y > 1.0 {
            continue;
        }

        // NDC to pixel coords (top-left origin)
        let px = (ndc.x + 1.0) * 0.5 * sw;
        let py = (ndc.y + 1.0) * 0.5 * sh; // Vulkan Y is flipped in proj already

        let bx = px - bar_w * 0.5;
        let by = py + y_offset;

        let hp_pct = (health.current / health.max).clamp(0.0, 1.0);

        // Background
        batch.rect_px(bx, by, bar_w, bar_h, [0.0, 0.0, 0.0, 0.7], sw, sh);
        // Health fill
        let color = if hp_pct > 0.5 {
            [0.8, 0.1, 0.1, 0.9]
        } else {
            [0.9, 0.3, 0.0, 0.9]
        };
        batch.rect_px(bx, by, bar_w * hp_pct, bar_h, color, sw, sh);
    }
}
