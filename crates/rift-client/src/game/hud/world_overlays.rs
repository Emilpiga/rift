//! World-anchored HUD overlays — boss-locator arrow, enemy/ally
//! health bars, and the buff/debuff pip strips that ride above
//! them. Everything in this module reprojects world positions
//! into screen space and then defers to [`Ui`](rift_engine::ui::im::Ui)
//! / [`WorldUi`](rift_engine::ui::im::WorldUi) for actual drawing.

use glam::{Mat4, Vec3};
use rift_engine::ecs::components::{
    Boss, Effects, Enemy, Health, LocalPlayer, Player, RemotePlayer, Resource, Transform,
};
use rift_engine::ui::im::{Color, Pos2, Rect, ResourceBarAnim, Ui, Vec2 as UiVec2};

use super::draw_effect_pip_strip;

/// Boss locator shown during the active boss phase. Uses the same
/// projection convention as the portal compass so the marker sits on
/// the same side of the screen as the world-space boss.
pub fn render_boss_arrow(
    ui: &mut Ui<'_>,
    world: &hecs::World,
    view_proj: Mat4,
    boss_room_center: Vec3,
) {
    const EDGE_PAD: f32 = 106.0;
    const BOSS_ROOM_HIDE_DIST_SQ: f32 = 14.0 * 14.0;

    let screen = ui.screen_size();
    let sw = screen.x;
    let sh = screen.y;

    let boss_entity_pos: Option<Vec3> = world
        .query::<(&Transform, &Boss)>()
        .iter()
        .map(|(_, (t, _))| t.position + Vec3::new(0.0, 1.2, 0.0))
        .next();
    let (boss_pos, has_boss_entity) = boss_entity_pos
        .map(|pos| (pos, true))
        .unwrap_or((boss_room_center + Vec3::new(0.0, 1.2, 0.0), false));

    let player_pos: Option<Vec3> = world
        .query::<(&Transform, &Player, &LocalPlayer)>()
        .iter()
        .map(|(_, (t, _, _))| t.position)
        .next();
    let Some(player_pos) = player_pos else { return };

    let to_room = boss_room_center - player_pos;
    let room_dist_sq = to_room.x * to_room.x + to_room.z * to_room.z;
    if room_dist_sq < BOSS_ROOM_HIDE_DIST_SQ {
        return;
    }

    let to_boss = boss_pos - player_pos;
    let dist_sq = to_boss.x * to_boss.x + to_boss.z * to_boss.z;

    let clip = view_proj * boss_pos.extend(1.0);
    let cx = sw * 0.5;
    let cy = sh * 0.5;
    let (raw_x, raw_y, on_screen) = if clip.w > 0.0 {
        let ndc = clip.truncate() / clip.w;
        let sx = (ndc.x + 1.0) * 0.5 * sw;
        let sy = (ndc.y + 1.0) * 0.5 * sh;
        (sx - cx, sy - cy, ndc.x.abs() <= 0.86 && ndc.y.abs() <= 0.78)
    } else {
        let ndc_clip = clip.truncate() / clip.w.abs().max(1.0);
        (-ndc_clip.x * sw, ndc_clip.y * sh, false)
    };
    let len = (raw_x * raw_x + raw_y * raw_y).sqrt().max(1e-3);
    let nx = raw_x / len;
    let ny = raw_y / len;

    let (ax, ay) = if on_screen {
        (cx + raw_x, cy + raw_y)
    } else {
        let max_x = sw * 0.5 - EDGE_PAD;
        let max_y = sh * 0.5 - EDGE_PAD;
        let scale = (max_x / nx.abs().max(1e-3)).min(max_y / ny.abs().max(1e-3));
        (cx + nx * scale, cy + ny * scale)
    };

    let dist = dist_sq.sqrt();
    let theme = *ui.theme();
    let s = theme.scale;
    let pulse = 0.5 + 0.5 * ((dist * 0.10).sin().abs());
    let accent = Color::rgba(1.00, 0.22, 0.08, 0.92 + 0.08 * pulse);
    let warm = Color::rgba(1.00, 0.70, 0.24, 0.78 + 0.15 * pulse);
    let arrow_dir = UiVec2::new(nx, ny);
    ui.draw_arrow(
        Pos2::new(ax, ay),
        arrow_dir,
        if on_screen { 24.0 * s } else { 34.0 * s },
        if on_screen { 20.0 * s } else { 28.0 * s },
        accent,
    );
    ui.draw_arrow(
        Pos2::new(ax - nx * 3.0 * s, ay - ny * 3.0 * s),
        arrow_dir,
        if on_screen { 15.0 * s } else { 23.0 * s },
        if on_screen { 11.0 * s } else { 17.0 * s },
        warm,
    );

    let label = if has_boss_entity {
        format!("BOSS  {:.0}m", dist)
    } else {
        format!("BOSS ROOM  {:.0}m", dist)
    };
    let font = 11.0 * s;
    let label_w = ui.measure_text(&label, font);
    let label_x = (ax - label_w * 0.5).clamp(8.0 * s, sw - label_w - 8.0 * s);
    let label_y = (ay + 22.0 * s).clamp(48.0 * s, sh - 30.0 * s);
    ui.draw_text(
        Pos2::new(label_x, label_y),
        &label,
        font,
        Color::rgba(1.0, 0.76, 0.42, 0.98),
    );
}

/// Post-boss portal-room locator. Unlike the boss arrow this is
/// target-driven, so it can point at the generated portal room as
/// soon as the floor is complete, before either portal is on-screen.
pub fn render_portal_compass(
    ui: &mut Ui<'_>,
    world: &hecs::World,
    view_proj: Mat4,
    target_pos: Vec3,
) {
    const EDGE_PAD: f32 = 96.0;
    const NEAR_HIDE_DIST_SQ: f32 = 4.2 * 4.2;

    let screen = ui.screen_size();
    let sw = screen.x;
    let sh = screen.y;

    let player_pos: Option<Vec3> = world
        .query::<(&Transform, &Player, &LocalPlayer)>()
        .iter()
        .map(|(_, (t, _, _))| t.position)
        .next();
    let Some(player_pos) = player_pos else { return };

    let delta = target_pos - player_pos;
    let dist_sq = delta.x * delta.x + delta.z * delta.z;
    if dist_sq < NEAR_HIDE_DIST_SQ {
        return;
    }

    let marker_pos = target_pos + Vec3::new(0.0, 1.8, 0.0);
    let clip = view_proj * marker_pos.extend(1.0);
    let cx = sw * 0.5;
    let cy = sh * 0.5;
    let (raw_x, raw_y, on_screen) = if clip.w > 0.0 {
        let ndc = clip.truncate() / clip.w;
        let sx = (ndc.x + 1.0) * 0.5 * sw;
        let sy = (ndc.y + 1.0) * 0.5 * sh;
        (sx - cx, sy - cy, ndc.x.abs() <= 0.86 && ndc.y.abs() <= 0.78)
    } else {
        let ndc_clip = clip.truncate() / clip.w.abs().max(1.0);
        (-ndc_clip.x * sw, ndc_clip.y * sh, false)
    };

    let len = (raw_x * raw_x + raw_y * raw_y).sqrt().max(1e-3);
    let nx = raw_x / len;
    let ny = raw_y / len;
    let (ax, ay) = if on_screen {
        (cx + raw_x, cy + raw_y)
    } else {
        let max_x = sw * 0.5 - EDGE_PAD;
        let max_y = sh * 0.5 - EDGE_PAD;
        let scale = (max_x / nx.abs().max(1e-3)).min(max_y / ny.abs().max(1e-3));
        (cx + nx * scale, cy + ny * scale)
    };

    let dist = dist_sq.sqrt();
    let theme = *ui.theme();
    let s = theme.scale;
    let pulse = 0.5 + 0.5 * ((dist * 0.12).sin().abs());
    let accent = Color::rgba(0.96, 0.24, 0.15, 0.90 + 0.10 * pulse);
    let warm = Color::rgba(1.0, 0.72, 0.36, 0.80 + 0.14 * pulse);

    let arrow_dir = UiVec2::new(nx, ny);
    ui.draw_arrow(
        Pos2::new(ax, ay),
        arrow_dir,
        if on_screen { 22.0 * s } else { 31.0 * s },
        if on_screen { 18.0 * s } else { 25.0 * s },
        accent,
    );
    ui.draw_arrow(
        Pos2::new(ax - nx * 3.0 * s, ay - ny * 3.0 * s),
        arrow_dir,
        if on_screen { 14.0 * s } else { 21.0 * s },
        if on_screen { 10.0 * s } else { 15.0 * s },
        warm,
    );

    let label = format!("PORTAL  {:.0}m", dist);
    let font = 11.0 * s;
    let label_w = ui.measure_text(&label, font);
    let label_x = (ax - label_w * 0.5).clamp(8.0 * s, sw - label_w - 8.0 * s);
    let label_y = (ay + 20.0 * s).clamp(48.0 * s, sh - 30.0 * s);
    ui.draw_text(
        Pos2::new(label_x, label_y),
        &label,
        font,
        Color::rgba(1.0, 0.78, 0.48, 0.96),
    );
}

/// Render thin health bars above enemies that have taken damage.
pub fn render_enemy_health_bars(ui: &mut Ui<'_>, world: &hecs::World, view_proj: Mat4, dt: f32) {
    use rift_engine::ui::im::WorldUi;

    const BAR_W: f32 = 52.0;
    const BAR_H: f32 = 6.0;
    const Y_OFFSET: f32 = -24.0;

    let mut wui = WorldUi::new(ui, view_proj);

    for (entity, (transform, _enemy, health)) in
        world.query::<(&Transform, &Enemy, &Health)>().iter()
    {
        let effects_ref = world.get::<&Effects>(entity).ok();
        let effects = effects_ref
            .as_ref()
            .map(|d| d.effects.as_slice())
            .unwrap_or(&[]);
        let damaged = health.current < health.max;
        if !damaged && effects.is_empty() {
            continue;
        }

        let world_pos = transform.position + Vec3::new(0.0, 1.2, 0.0);

        let bar_rect = if damaged {
            let hp_pct = (health.current / health.max).clamp(0.0, 1.0);
            let style = if hp_pct > 0.5 {
                WorldBarStyle::enemy_healthy()
            } else {
                WorldBarStyle::enemy_wounded()
            };
            draw_animated_world_bar(
                &mut wui,
                entity_bar_key(entity, 0),
                WorldBarLane::Hp,
                world_pos,
                Y_OFFSET,
                BAR_W,
                BAR_H,
                hp_pct,
                dt,
                style,
            )
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
pub fn render_remote_player_health_bars(
    ui: &mut Ui<'_>,
    world: &hecs::World,
    view_proj: Mat4,
    dt: f32,
) {
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
        let style = if hp_pct > 0.25 {
            WorldBarStyle::ally_health()
        } else {
            WorldBarStyle::ally_critical()
        };
        let bar_rect = draw_animated_world_bar(
            &mut wui,
            entity_bar_key(entity, 1),
            WorldBarLane::Hp,
            world_pos,
            Y_OFFSET,
            BAR_W,
            BAR_H,
            hp_pct,
            dt,
            style,
        );

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
            draw_animated_world_bar(
                &mut wui,
                entity_bar_key(entity, 1),
                WorldBarLane::Essence,
                world_pos,
                Y_OFFSET + BAR_H + RESOURCE_GAP,
                BAR_W,
                RESOURCE_BAR_H,
                res_pct,
                dt,
                WorldBarStyle::ally_essence(),
            );
        }

        let effects_ref = world.get::<&Effects>(entity).ok();
        if let (Some(rect), Some(effects)) = (
            bar_rect,
            effects_ref
                .as_ref()
                .map(|d| d.effects.as_slice())
                .filter(|effects| !effects.is_empty()),
        ) {
            draw_effect_pips(&mut wui, rect, effects);
        }
    }
}

#[derive(Clone, Copy)]
enum WorldBarLane {
    Hp,
    Essence,
}

#[derive(Clone, Copy)]
struct WorldBarStyle {
    base: Color,
    hot: Color,
    chip: Color,
    glow: Color,
    border: Color,
}

impl WorldBarStyle {
    fn enemy_healthy() -> Self {
        Self {
            base: Color::rgba(0.68, 0.08, 0.08, 0.95),
            hot: Color::rgba(1.0, 0.24, 0.16, 1.0),
            chip: Color::rgba(1.0, 0.72, 0.50, 0.30),
            glow: Color::rgba(1.0, 0.16, 0.08, 1.0),
            border: Color::rgba(0.04, 0.02, 0.02, 0.92),
        }
    }

    fn enemy_wounded() -> Self {
        Self {
            base: Color::rgba(0.82, 0.22, 0.02, 0.96),
            hot: Color::rgba(1.0, 0.55, 0.16, 1.0),
            chip: Color::rgba(1.0, 0.88, 0.56, 0.34),
            glow: Color::rgba(1.0, 0.42, 0.08, 1.0),
            border: Color::rgba(0.05, 0.025, 0.01, 0.92),
        }
    }

    fn ally_health() -> Self {
        Self {
            base: Color::rgba(0.14, 0.58, 0.25, 0.96),
            hot: Color::rgba(0.42, 0.94, 0.34, 1.0),
            chip: Color::rgba(0.88, 1.0, 0.78, 0.30),
            glow: Color::rgba(0.58, 1.0, 0.44, 1.0),
            border: Color::rgba(0.025, 0.045, 0.025, 0.92),
        }
    }

    fn ally_critical() -> Self {
        Self {
            base: Color::rgba(0.58, 0.10, 0.08, 0.96),
            hot: Color::rgba(1.0, 0.30, 0.22, 1.0),
            chip: Color::rgba(1.0, 0.82, 0.74, 0.34),
            glow: Color::rgba(1.0, 0.22, 0.16, 1.0),
            border: Color::rgba(0.05, 0.02, 0.02, 0.92),
        }
    }

    fn ally_essence() -> Self {
        Self {
            base: Color::rgba(0.22, 0.46, 0.92, 0.96),
            hot: Color::rgba(0.40, 0.78, 1.0, 1.0),
            chip: Color::rgba(0.78, 0.92, 1.0, 0.34),
            glow: Color::rgba(0.30, 0.70, 1.0, 1.0),
            border: Color::rgba(0.02, 0.03, 0.06, 0.92),
        }
    }
}

#[derive(Clone, Copy)]
struct WorldBarAnimSnapshot {
    displayed: f32,
    trail: f32,
    pulse: f32,
}

fn entity_bar_key(entity: hecs::Entity, group: u64) -> u64 {
    u64::from(entity.to_bits()) ^ (group << 60)
}

fn draw_animated_world_bar(
    wui: &mut rift_engine::ui::im::WorldUi<'_, '_>,
    key: u64,
    lane: WorldBarLane,
    world_pos: Vec3,
    y_offset_px: f32,
    width: f32,
    height: f32,
    target: f32,
    dt: f32,
    style: WorldBarStyle,
) -> Option<Rect> {
    let anchor = wui.world_to_screen(world_pos)?;
    let rect = Rect::from_xywh(
        anchor.x - width * 0.5,
        anchor.y + y_offset_px,
        width,
        height,
    );
    let snapshot = {
        let state = wui.ui().state_mut();
        let anims = state.world_vitals.entry(key).or_default();
        let anim: &mut ResourceBarAnim = match lane {
            WorldBarLane::Hp => &mut anims.hp,
            WorldBarLane::Essence => &mut anims.essence,
        };
        anim.tick(target, dt);
        WorldBarAnimSnapshot {
            displayed: anim.displayed,
            trail: anim.trail,
            pulse: anim.pulse,
        }
    };
    draw_world_resource_bar(wui.ui(), rect, snapshot, style);
    Some(rect)
}

fn draw_world_resource_bar(
    ui: &mut Ui<'_>,
    rect: Rect,
    anim: WorldBarAnimSnapshot,
    style: WorldBarStyle,
) {
    let displayed = anim.displayed.clamp(0.0, 1.0);
    let trail = anim.trail.clamp(displayed, 1.0);
    let pulse = anim.pulse.clamp(0.0, 1.0);

    ui.draw_gradient_rect(
        rect,
        Color::rgba(0.025, 0.022, 0.024, 0.92),
        Color::rgba(0.006, 0.006, 0.008, 0.96),
    );

    let trail_w = rect.width() * trail;
    let fill_w = rect.width() * displayed;
    if trail_w > fill_w + 0.5 {
        ui.draw_grad4_rect(
            Rect::from_xywh(rect.x() + fill_w, rect.y(), trail_w - fill_w, rect.height()),
            style.chip,
            style.chip.fade(0.50),
            Color::rgba(0.0, 0.0, 0.0, 0.22),
            style.chip.fade(0.22),
        );
    }

    if fill_w > 0.5 {
        let fill = Rect::from_xywh(rect.x(), rect.y(), fill_w, rect.height());
        let lift = 1.0 + pulse * 0.18;
        ui.draw_grad4_rect(
            fill,
            scale_world_rgb(style.hot, lift),
            scale_world_rgb(style.base, 1.04 + pulse * 0.12),
            scale_world_rgb(style.base, 0.58),
            scale_world_rgb(style.base, 0.76 + pulse * 0.08),
        );
        ui.draw_gradient_rect(
            fill,
            Color::rgba(1.0, 1.0, 1.0, 0.20),
            Color::rgba(0.0, 0.0, 0.0, 0.22),
        );
        draw_world_bar_cursor(ui, rect, fill_w, style.glow, pulse);
    }

    if pulse > 0.01 && fill_w > 1.0 {
        ui.draw_grad4_rect(
            Rect::from_xywh(
                rect.x() - 1.0,
                rect.y() - 1.0,
                fill_w + 2.0,
                rect.height() + 2.0,
            ),
            style.glow.fade(0.08 * pulse),
            style.glow.fade(0.03 * pulse),
            style.glow.fade(0.02 * pulse),
            style.glow.fade(0.01 * pulse),
        );
    }

    ui.draw_outline(rect, 1.0, style.border);
}

fn draw_world_bar_cursor(ui: &mut Ui<'_>, rect: Rect, fill_w: f32, glow: Color, pulse: f32) {
    if fill_w <= 1.0 || fill_w >= rect.width() - 0.5 {
        return;
    }
    let x = rect.x() + fill_w;
    let halo_w = (rect.height() * 1.6).clamp(5.0, 12.0);
    ui.draw_grad4_rect(
        Rect::from_xywh(x - halo_w * 0.55, rect.y(), halo_w, rect.height()),
        Color::rgba(1.0, 1.0, 1.0, 0.0),
        glow.fade(0.16 + pulse * 0.18),
        Color::rgba(1.0, 1.0, 1.0, 0.0),
        glow.fade(0.04 + pulse * 0.06),
    );
    ui.draw_gradient_rect(
        Rect::from_xywh(
            x - 0.75,
            rect.y() + 1.0,
            1.5,
            (rect.height() - 2.0).max(1.0),
        ),
        Color::rgba(1.0, 1.0, 1.0, 0.66 + pulse * 0.14),
        glow.fade(0.34 + pulse * 0.18),
    );
}

fn scale_world_rgb(color: Color, mul: f32) -> Color {
    Color::rgba(
        (color.0[0] * mul).clamp(0.0, 1.0),
        (color.0[1] * mul).clamp(0.0, 1.0),
        (color.0[2] * mul).clamp(0.0, 1.0),
        color.0[3],
    )
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
