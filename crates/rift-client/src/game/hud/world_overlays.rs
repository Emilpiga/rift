//! World-anchored HUD overlays — boss-locator arrow, enemy/ally
//! health bars, and the buff/debuff pip strips that ride above
//! them. Everything in this module reprojects world positions
//! into screen space and then defers to [`Ui`](rift_engine::ui::im::Ui)
//! / [`WorldUi`](rift_engine::ui::im::WorldUi) for actual drawing.

use glam::{Mat4, Vec3};
use rift_engine::ecs::components::{
    Boss, Effects, Enemy, Health, LocalPlayer, Player, RemotePlayer, Resource, Transform,
};
use rift_engine::ui::im::{Color, Pos2, Rect, Ui};

use super::draw_effect_pip_strip;

/// Off-screen / far-away boss locator. When the boss is alive but the
/// player can't see them (off-screen, behind camera, or > ARROW_RANGE
/// world units away), draw a glowing arrow at the screen edge pointing
/// toward the boss in screen space.
pub fn render_boss_arrow(ui: &mut Ui<'_>, world: &hecs::World, view_proj: Mat4) {
    const ARROW_RANGE_SQ: f32 = 16.0 * 16.0; // show arrow if boss > 16 m away
    const EDGE_PAD: f32 = 110.0;

    let screen = ui.screen_size();
    let sw = screen.x;
    let sh = screen.y;

    let boss_pos: Option<Vec3> = world
        .query::<(&Transform, &Boss)>()
        .iter()
        .map(|(_, (t, _))| t.position + Vec3::new(0.0, 1.2, 0.0))
        .next();
    let Some(boss_pos) = boss_pos else { return };

    let player_pos: Option<Vec3> = world
        .query::<(&Transform, &Player, &LocalPlayer)>()
        .iter()
        .map(|(_, (t, _, _))| t.position)
        .next();
    let Some(player_pos) = player_pos else { return };

    let to_boss = boss_pos - player_pos;
    let dist_sq = to_boss.x * to_boss.x + to_boss.z * to_boss.z;

    let clip = view_proj * boss_pos.extend(1.0);
    let on_screen = if clip.w > 0.0 {
        let ndc = clip.truncate() / clip.w;
        ndc.x.abs() <= 1.0 && ndc.y.abs() <= 1.0
    } else {
        false
    };

    if on_screen && dist_sq < ARROW_RANGE_SQ {
        return;
    }

    let cx = sw * 0.5;
    let cy = sh * 0.5;
    let (dx, dy) = if clip.w > 0.0 {
        let ndc = clip.truncate() / clip.w;
        let bx = (ndc.x + 1.0) * 0.5 * sw - cx;
        let by = (ndc.y + 1.0) * 0.5 * sh - cy;
        (bx, by)
    } else {
        let ndc_clip = clip.truncate() / clip.w.abs().max(1.0);
        (-ndc_clip.x * sw, ndc_clip.y * sh)
    };
    let len = (dx * dx + dy * dy).sqrt().max(1e-3);
    let nx = dx / len;
    let ny = dy / len;

    let max_x = sw * 0.5 - EDGE_PAD;
    let max_y = sh * 0.5 - EDGE_PAD;
    let scale = (max_x / nx.abs().max(1e-3)).min(max_y / ny.abs().max(1e-3));
    let ax = cx + nx * scale;
    let ay = cy + ny * scale;

    let dist = dist_sq.sqrt();
    let pulse = 0.75 + 0.25 * ((dist * 0.06).sin().abs());
    let col = Color::rgba(1.00, 0.42, 0.05, (0.98 * pulse).clamp(0.7, 1.0));

    let tx = -ny;
    let ty = nx;

    let line = |ui: &mut Ui<'_>, u0: f32, v0: f32, u1: f32, v1: f32, thickness: f32| {
        let du = u1 - u0;
        let dv = v1 - v0;
        let line_len = (du * du + dv * dv).sqrt().max(1.0);
        let dot_pitch: f32 = 1.5;
        let count = (line_len / dot_pitch).ceil() as i32;
        for i in 0..=count {
            let t = i as f32 / count as f32;
            let u = u0 + du * t;
            let v = v0 + dv * t;
            let sx_ = ax + nx * u + tx * v;
            let sy_ = ay + ny * u + ty * v;
            ui.draw_rect(
                Rect::from_xywh(
                    sx_ - thickness * 0.5,
                    sy_ - thickness * 0.5,
                    thickness,
                    thickness,
                ),
                col,
            );
        }
    };

    const HEAD_LEN: f32 = 22.0;
    const SHAFT_LEN: f32 = 26.0;
    const HALF_W: f32 = 22.0;
    const SHAFT_W: f32 = 8.0;
    let tip_u = HEAD_LEN;
    let wing_u = 0.0;
    let tail_u = -SHAFT_LEN;
    let thick = 4.0;

    line(ui, tip_u, 0.0, wing_u, HALF_W, thick);
    line(ui, tip_u, 0.0, wing_u, -HALF_W, thick);
    line(ui, wing_u, HALF_W, wing_u, SHAFT_W, thick);
    line(ui, wing_u, -HALF_W, wing_u, -SHAFT_W, thick);
    line(ui, wing_u, SHAFT_W, tail_u, SHAFT_W, thick);
    line(ui, wing_u, -SHAFT_W, tail_u, -SHAFT_W, thick);
    line(ui, tail_u, SHAFT_W, tail_u, -SHAFT_W, thick);

    let fill_arr = col.0;
    let fill = Color::rgba(
        fill_arr[0],
        fill_arr[1],
        fill_arr[2],
        (fill_arr[3] * 0.65).clamp(0.0, 1.0),
    );
    let v_steps = 28;
    for i in 1..v_steps {
        let v = -HALF_W + (HALF_W * 2.0) * (i as f32 / v_steps as f32);
        let av = v.abs();
        let in_head = av <= HALF_W;
        if !in_head {
            continue;
        }
        let head_u = HEAD_LEN * (1.0 - av / HALF_W);
        let in_shaft_band = av <= SHAFT_W;
        let left_u = if in_shaft_band { tail_u } else { 0.0 };
        let dot_pitch: f32 = 1.6;
        let span = head_u - left_u;
        if span <= 0.0 {
            continue;
        }
        let count = (span / dot_pitch).ceil() as i32;
        for j in 0..=count {
            let t = j as f32 / count as f32;
            let u = left_u + span * t;
            let sx_ = ax + nx * u + tx * v;
            let sy_ = ay + ny * u + ty * v;
            ui.draw_rect(Rect::from_xywh(sx_ - 1.5, sy_ - 1.5, 3.0, 3.0), fill);
        }
    }
}

/// Render thin health bars above enemies that have taken damage.
pub fn render_enemy_health_bars(ui: &mut Ui<'_>, world: &hecs::World, view_proj: Mat4) {
    use rift_engine::ui::im::WorldUi;

    const BAR_W: f32 = 52.0;
    const BAR_H: f32 = 6.0;
    const Y_OFFSET: f32 = -24.0;

    let mut wui = WorldUi::new(ui, view_proj);

    for (entity, (transform, _enemy, health)) in
        world.query::<(&Transform, &Enemy, &Health)>().iter()
    {
        let effects: Vec<rift_engine::ecs::components::ActiveEffect> = world
            .get::<&Effects>(entity)
            .map(|d| d.effects.clone())
            .unwrap_or_default();
        let damaged = health.current < health.max;
        if !damaged && effects.is_empty() {
            continue;
        }

        let world_pos = transform.position + Vec3::new(0.0, 1.2, 0.0);

        let bar_rect = if damaged {
            let hp_pct = (health.current / health.max).clamp(0.0, 1.0);
            let color = if hp_pct > 0.5 {
                Color::rgba(0.8, 0.1, 0.1, 0.9)
            } else {
                Color::rgba(0.9, 0.3, 0.0, 0.9)
            };
            wui.bar_above_world(world_pos, Y_OFFSET, BAR_W, BAR_H, hp_pct, color)
        } else {
            wui.world_to_screen(world_pos).map(|anchor| {
                Rect::from_xywh(anchor.x - BAR_W * 0.5, anchor.y + Y_OFFSET, BAR_W, BAR_H)
            })
        };

        if let Some(rect) = bar_rect {
            draw_effect_pips(&mut wui, rect, &effects);
        }
    }
}

/// Render thin health bars above remote (party-member) avatars.
/// Mirrors `render_enemy_health_bars` styling but in green so the
/// player can tell teammates apart from enemies at a glance, and
/// always draws the bar (even at full HP) since seeing a teammate's
/// max HP is useful tactical info, unlike enemies where a full bar
/// is just visual noise.
///
/// A slimmer cyan-blue essence bar is drawn directly under the
/// HP bar so allies can see how much of the universal ability
/// resource a teammate has left before pulling the next pack —
/// same gameplay value as the HP bar, just tracking essence.
/// The fraction comes from the avatar's [`Resource`] component,
/// which `world_sync` mirrors from the snapshot's `resource_pct`
/// each tick (same pattern as the HP mirror above).
pub fn render_remote_player_health_bars(ui: &mut Ui<'_>, world: &hecs::World, view_proj: Mat4) {
    use rift_engine::ui::im::WorldUi;

    const BAR_W: f32 = 56.0;
    const BAR_H: f32 = 6.0;
    /// Vertical pixel gap between HP bar bottom and essence bar
    /// top. Smaller than `BAR_H` so the two bars read as a
    /// stacked vital-pair rather than two unrelated widgets.
    const RESOURCE_GAP: f32 = 1.0;
    /// Essence bar height. Slimmer than the HP bar so health
    /// stays the dominant readable cue.
    const RESOURCE_BAR_H: f32 = 4.0;
    const Y_OFFSET: f32 = -32.0;

    let mut wui = WorldUi::new(ui, view_proj);
    for (entity, (transform, _rp, health)) in
        world.query::<(&Transform, &RemotePlayer, &Health)>().iter()
    {
        let hp_pct = (health.current / health.max).clamp(0.0, 1.0);
        let world_pos = transform.position + Vec3::new(0.0, 1.6, 0.0);
        let color = if hp_pct > 0.5 {
            Color::rgba(0.25, 0.75, 0.25, 0.9)
        } else if hp_pct > 0.25 {
            Color::rgba(0.85, 0.7, 0.1, 0.9)
        } else {
            Color::rgba(0.9, 0.25, 0.15, 0.9)
        };
        let bar_rect = wui.bar_above_world(world_pos, Y_OFFSET, BAR_W, BAR_H, hp_pct, color);

        // Essence bar — same width as HP, slimmer, anchored
        // `BAR_H + RESOURCE_GAP` pixels under the HP bar's top
        // (i.e. `RESOURCE_GAP` under its bottom). Pure cyan-blue
        // so it can't be confused with the green HP bar at a
        // glance.
        if let Ok(resource) = world.get::<&Resource>(entity) {
            let res_pct = if resource.max > 0.0 {
                (resource.current / resource.max).clamp(0.0, 1.0)
            } else {
                0.0
            };
            wui.bar_above_world(
                world_pos,
                Y_OFFSET + BAR_H + RESOURCE_GAP,
                BAR_W,
                RESOURCE_BAR_H,
                res_pct,
                Color::rgba(0.30, 0.55, 0.95, 0.9),
            );
        }

        let effects: Vec<rift_engine::ecs::components::ActiveEffect> = world
            .get::<&Effects>(entity)
            .map(|d| d.effects.clone())
            .unwrap_or_default();
        if let (Some(rect), false) = (bar_rect, effects.is_empty()) {
            draw_effect_pips(&mut wui, rect, &effects);
        }
    }
}

/// Draw a horizontal strip of buff / debuff icon pips just above
/// `anchor` (typically the entity's HP bar). Each pip shows the
/// effect's icon (or a flat colored fill if no icon is defined)
/// plus a top-down dark drain overlay sized by `remaining /
/// duration` — same visual language as the action-bar cooldown
/// drain so players read remaining time the same way for both.
fn draw_effect_pips(
    wui: &mut rift_engine::ui::im::WorldUi<'_, '_>,
    anchor: Rect,
    effects: &[rift_engine::ecs::components::ActiveEffect],
) {
    const PIP_SIZE: f32 = 28.0;
    let pips_y = anchor.y() - PIP_SIZE - 3.0;
    // World-overlay pips ride above remote / enemy HP bars; we
    // still want hover tooltips so the player can identify a
    // friendly buff on a teammate at a glance.
    draw_effect_pip_strip(
        wui.ui(),
        Pos2::new(anchor.x(), pips_y),
        effects,
        PIP_SIZE,
        true,
    );
}
