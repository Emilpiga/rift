use rift_engine::ecs::components::{Boss, Debuffs, Enemy, Health, LocalPlayer, Player, Transform};
use rift_engine::ai::NavGrid;
use rift_engine::ui::im::{
    hp_color, Color, Id, ItemSlot, Pos2, ProgressBar, Rect, Tooltip, TooltipLine, Ui,
};
use glam::{Mat4, Vec3};

use crate::game::PlayerState;
use super::rift_state::RiftState;
use rift_game::abilities::AbilitySlot;

/// Render all HUD elements via the immediate-mode UI stack.
pub fn render_hud(
    ui: &mut Ui<'_>,
    world: &hecs::World,
    rift: &RiftState,
    player_state: &PlayerState,
    level_up_flash: f32,
    in_hub: bool,
) {
    let theme = *ui.theme();
    let screen = ui.screen_size();
    let sw = screen.x;
    let sh = screen.y;

    let stats = player_state.stats();
    let max_hp_bonus = stats.max_hp - player_state.config.base_hp
        - player_state.config.hp_per_level * player_state.experience.level as f32;
    // HP + XP bars: stacked, centered above the ability bar so the
    // player's vital stats sit right under their character.
    let hp_pct = world
        .query::<(&Health, &Player, &LocalPlayer)>()
        .iter()
        .map(|(_, (h, _, _))| h.current / (h.max + max_hp_bonus))
        .next()
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);

    // Ability bar lives at sh - 80 (see `render_ability_bar`);
    // stack the HP/XP pair 16 px above it for a bit more breathing
    // room than the original 8 px gap.
    let bar_w = 360.0;
    let bar_h = 22.0;
    let xp_h = 9.0;
    let bars_total_h = bar_h + 2.0 + xp_h;
    let bar_x = (sw - bar_w) / 2.0;
    let bar_y = sh - 80.0 - 16.0 - bars_total_h;

    // HP bar.
    ProgressBar::new(hp_pct)
        .fill(hp_color(hp_pct))
        .border(Color::rgba(0.30, 0.30, 0.32, 0.9))
        .show(ui, Rect::from_xywh(bar_x, bar_y, bar_w, bar_h));

    // XP bar (slimmer, directly under the HP bar).
    let xp_pct = player_state.experience.progress().clamp(0.0, 1.0);
    let xp_y = bar_y + bar_h + 2.0;
    let xp_now = player_state.experience.current_xp;
    let xp_need = player_state.experience.xp_to_next_level();
    let xp_label = format!("{xp_now} / {xp_need} XP");
    ProgressBar::new(xp_pct)
        .fill(Color::rgba(0.45, 0.30, 0.85, 0.95))
        .border(Color::rgba(0.30, 0.30, 0.32, 0.9))
        .rounded(false)
        .show(ui, Rect::from_xywh(bar_x, xp_y, bar_w, xp_h));
    // XP numerals sit just above the XP bar (the bar is too thin to
    // center text inside).
    let xp_text_size = 11.0;
    let xp_text_w = ui.measure_text(&xp_label, xp_text_size);
    ui.draw_text(
        Pos2::new(bar_x + (bar_w - xp_text_w) * 0.5, xp_y - 1.0),
        &xp_label,
        xp_text_size,
        Color::rgba(0.92, 0.92, 0.96, 0.95),
    );

    // Level pip floats just to the left of the HP bar.
    let level_text = format!("Lv.{}", player_state.experience.level);
    ui.draw_text(
        Pos2::new(bar_x - 50.0, bar_y + 4.0),
        &level_text,
        15.0,
        Color::rgba(0.92, 0.92, 0.92, 1.0),
    );

    // Level-up banner: appears top-center for ~2.5 s after the
    // server confirms a level-up.
    if level_up_flash > 0.001 {
        let banner = format!("LEVEL UP!  Lv.{}", player_state.experience.level);
        let size = theme.fonts.size_xl;
        let tw = ui.measure_text(&banner, size);
        let alpha = level_up_flash.min(1.0);
        ui.draw_text(
            Pos2::new((sw - tw) * 0.5, sh * 0.30),
            &banner,
            size,
            Color::rgba(1.0, 0.85, 0.30, alpha),
        );
    }

    // Rift progress bar (top-center). Hidden in the hub; replaced
    // by a small "THE HUB" label so the screen anchor stays consistent.
    if !in_hub {
        let prog_pct = rift.progress_percent() / 100.0;
        let prog_w = 300.0;
        let prog_h = 16.0;
        let prog_x = (sw - prog_w) / 2.0;
        let prog_y = 10.0;
        ProgressBar::new(prog_pct)
            .fill(theme.colors.accent)
            .track(Color::rgba(0.10, 0.10, 0.10, 0.80))
            .border(theme.colors.border)
            .show(ui, Rect::from_xywh(prog_x, prog_y, prog_w, prog_h));

        // Floor indicator (top-right) — segmented bar, one pip per
        // floor cleared.
        let floor_w = 40.0;
        let floor_h = 20.0;
        let floor_pct = (rift.floor as f32 / 10.0).clamp(0.0, 1.0);
        ProgressBar::new(floor_pct)
            .fill(Color::rgba(0.80, 0.70, 0.20, 0.90))
            .track(Color::rgba(0.20, 0.20, 0.30, 0.80))
            .border(theme.colors.border)
            .pips(10)
            .show(
                ui,
                Rect::from_xywh(sw - floor_w - 10.0, 10.0, floor_w, floor_h),
            );
    } else {
        // Hub label where the progress bar would normally sit.
        let label_w = 120.0;
        let label_h = 20.0;
        let lx = (sw - label_w) / 2.0;
        let ly = 10.0;
        ui.draw_rounded_rect(
            Rect::from_xywh(lx, ly, label_w, label_h),
            theme.spacing.corner_radius,
            Color::rgba(0.08, 0.10, 0.16, 0.80),
        );
        ui.draw_text(
            Pos2::new(lx + 32.0, ly + 4.0),
            "THE HUB",
            13.0,
            Color::rgba(0.7, 0.85, 1.0, 1.0),
        );
    }

    // Portal indicator (if floor complete).
    if rift.floor_complete {
        let tw = 200.0;
        let th = 16.0;
        let tx = (sw - tw) / 2.0;
        let ty = 35.0;
        ui.draw_rounded_rect(
            Rect::from_xywh(tx, ty, tw, th),
            theme.spacing.corner_radius,
            Color::rgba(0.10, 0.15, 0.25, 0.85),
        );
        ui.draw_text(
            Pos2::new(tx + 30.0, ty + 2.0),
            "ENTER THE PORTAL",
            12.0,
            theme.colors.accent,
        );
    }
}

/// Fullscreen black quad used by the death→hub fade transition.
pub fn render_fade_to_black(ui: &mut Ui<'_>, alpha: f32) {
    let a = alpha.clamp(0.0, 1.0);
    if a <= 0.001 { return; }
    ui.draw_rect(ui.screen_rect(), Color::rgba(0.0, 0.0, 0.0, a));
}

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

    // Find boss world position + player world position.
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

    // Project to clip space; figure out whether it's on screen.
    let clip = view_proj * boss_pos.extend(1.0);
    let on_screen = if clip.w > 0.0 {
        let ndc = clip.truncate() / clip.w;
        ndc.x.abs() <= 1.0 && ndc.y.abs() <= 1.0
    } else {
        false
    };

    if on_screen && dist_sq < ARROW_RANGE_SQ {
        return; // boss is right there, no need to guide
    }

    // Compute a screen-space direction from screen centre toward the boss.
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

    // Anchor the arrow at the screen edge in that direction.
    let max_x = sw * 0.5 - EDGE_PAD;
    let max_y = sh * 0.5 - EDGE_PAD;
    let scale = (max_x / nx.abs().max(1e-3)).min(max_y / ny.abs().max(1e-3));
    let ax = cx + nx * scale;
    let ay = cy + ny * scale;

    // Pulse a bit so it draws the eye.
    let dist = dist_sq.sqrt();
    let pulse = 0.75 + 0.25 * ((dist * 0.06).sin().abs());
    let col = Color::rgba(1.00, 0.42, 0.05, (0.98 * pulse).clamp(0.7, 1.0));

    // Tangent (perpendicular) to arrow heading; used to fan the head out.
    let tx = -ny;
    let ty = nx;

    // Helper: draw a tightly stamped 1-pixel-radius "dot" trail along
    // the line from local (u0,v0) to (u1,v1). Each dot is a tiny
    // axis-aligned rect; with `DOT_PITCH=1.5` they overlap into a clean
    // line, so the resulting shape reads as a single solid arrow rather
    // than a cloud of squares.
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

    // Geometry of the arrow in local (u along heading, v perpendicular):
    const HEAD_LEN: f32 = 22.0;     // tip -> wings
    const SHAFT_LEN: f32 = 26.0;    // wings -> tail
    const HALF_W: f32 = 22.0;       // half-width at wings (head base)
    const SHAFT_W: f32 = 8.0;       // half-width of the shaft
    let tip_u = HEAD_LEN;
    let wing_u = 0.0;
    let tail_u = -SHAFT_LEN;
    let thick = 4.0;

    // Head outline (two leading edges of the V).
    line(ui, tip_u, 0.0, wing_u, HALF_W, thick);
    line(ui, tip_u, 0.0, wing_u, -HALF_W, thick);
    // Notch joining wings to shaft.
    line(ui, wing_u, HALF_W, wing_u, SHAFT_W, thick);
    line(ui, wing_u, -HALF_W, wing_u, -SHAFT_W, thick);
    // Shaft sides + tail cap.
    line(ui, wing_u, SHAFT_W, tail_u, SHAFT_W, thick);
    line(ui, wing_u, -SHAFT_W, tail_u, -SHAFT_W, thick);
    line(ui, tail_u, SHAFT_W, tail_u, -SHAFT_W, thick);

    // Solid fill: scanlines parallel to the heading at uniform v steps.
    let fill_arr = col.0;
    let fill = Color::rgba(fill_arr[0], fill_arr[1], fill_arr[2], (fill_arr[3] * 0.65).clamp(0.0, 1.0));
    let v_steps = 28;
    for i in 1..v_steps {
        let v = -HALF_W + (HALF_W * 2.0) * (i as f32 / v_steps as f32);
        let av = v.abs();
        let in_head = av <= HALF_W;
        if !in_head { continue; }
        let head_u = HEAD_LEN * (1.0 - av / HALF_W);
        let in_shaft_band = av <= SHAFT_W;
        let left_u = if in_shaft_band { tail_u } else { 0.0 };
        let dot_pitch: f32 = 1.6;
        let span = head_u - left_u;
        if span <= 0.0 { continue; }
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

/// Top-right minimap. Shows walkable tiles, the player (white dot, with a
/// short heading line), nearby enemies (red), the boss (orange), and the
/// active rift / hub portal (cyan).
///
/// The map auto-scales: cell size is computed so the navgrid fits inside
/// `MAP_PX × MAP_PX`.
pub fn render_minimap(
    ui: &mut Ui<'_>,
    world: &hecs::World,
    nav: &NavGrid,
    player_facing: Vec3,
    portal_pos: Option<Vec3>,
) {
    const MAP_PX: f32 = 320.0;
    const PADDING: f32 = 14.0;
    const MARGIN: f32 = 14.0;

    let screen = ui.screen_size();
    let sw = screen.x;

    let inner = MAP_PX - PADDING * 2.0;
    let cell = (inner / nav.width.max(nav.depth) as f32).max(1.0);
    let map_w = cell * nav.width as f32;
    let map_h = cell * nav.depth as f32;

    let map_x = sw - MAP_PX - MARGIN;
    let map_y = MARGIN;

    // Frame
    ui.draw_rect(
        Rect::from_xywh(map_x, map_y, MAP_PX, MAP_PX),
        Color::rgba(0.04, 0.05, 0.07, 0.78),
    );
    // Border
    let border = Color::rgba(0.18, 0.20, 0.26, 0.95);
    ui.draw_rect(Rect::from_xywh(map_x, map_y, MAP_PX, 1.5), border);
    ui.draw_rect(Rect::from_xywh(map_x, map_y + MAP_PX - 1.5, MAP_PX, 1.5), border);
    ui.draw_rect(Rect::from_xywh(map_x, map_y, 1.5, MAP_PX), border);
    ui.draw_rect(Rect::from_xywh(map_x + MAP_PX - 1.5, map_y, 1.5, MAP_PX), border);

    // Centre the navgrid inside the framed area.
    let inner_x = map_x + (MAP_PX - map_w) * 0.5;
    let inner_y = map_y + (MAP_PX - map_h) * 0.5;

    // Walkable tiles
    let floor_col = Color::rgba(0.32, 0.30, 0.26, 0.92);
    for z in 0..nav.depth {
        for x in 0..nav.width {
            if nav.is_walkable(x, z) {
                ui.draw_rect(
                    Rect::from_xywh(
                        inner_x + x as f32 * cell,
                        inner_y + z as f32 * cell,
                        cell,
                        cell,
                    ),
                    floor_col,
                );
            }
        }
    }

    // World → minimap helper. Tile coords map 1:1 to world units.
    let to_map = |p: Vec3| -> (f32, f32) {
        let mx = inner_x + p.x * cell;
        let my = inner_y + p.z * cell;
        (mx, my)
    };
    // True iff (mx,my) lies inside the framed minimap window.
    let in_frame = |mx: f32, my: f32| -> bool {
        mx >= map_x && mx <= map_x + MAP_PX && my >= map_y && my <= map_y + MAP_PX
    };

    // Portal pip
    if let Some(p) = portal_pos {
        let (mx, my) = to_map(p);
        if in_frame(mx, my) {
            let s = (cell * 2.6).max(4.0);
            ui.draw_rect(
                Rect::from_xywh(mx - s * 0.5, my - s * 0.5, s, s),
                Color::rgba(0.30, 0.75, 1.0, 0.95),
            );
        }
    }

    // Enemy pips
    for (_id, (t, _e, boss, _)) in world
        .query::<(&Transform, &Enemy, Option<&Boss>, Option<&Health>)>()
        .iter()
    {
        let (mx, my) = to_map(t.position);
        if !in_frame(mx, my) { continue; }
        let (s, col) = if boss.is_some() {
            ((cell * 2.4).max(4.0), Color::rgba(1.00, 0.55, 0.10, 1.0))
        } else {
            ((cell * 1.6).max(2.5), Color::rgba(0.92, 0.25, 0.22, 1.0))
        };
        ui.draw_rect(Rect::from_xywh(mx - s * 0.5, my - s * 0.5, s, s), col);
    }

    // Player pip + facing tick
    if let Some((pp, _)) = world
        .query::<(&Transform, &Player, &LocalPlayer)>()
        .iter()
        .map(|(_, (t, p, _))| (t.position, p.aim_dir))
        .next()
    {
        let (mx, my) = to_map(pp);
        if in_frame(mx, my) {
            let s = (cell * 1.9).max(3.0);
            ui.draw_rect(
                Rect::from_xywh(mx - s * 0.5, my - s * 0.5, s, s),
                Color::rgba(0.95, 0.95, 0.98, 1.0),
            );
            // Facing line: short heading marker.
            let f = Vec3::new(player_facing.x, 0.0, player_facing.z);
            if f.length_squared() > 1e-4 {
                let f = f.normalize();
                let len = (cell * 3.5).max(6.0);
                let dx = f.x * len;
                let dz = f.z * len;
                let steps = 6;
                for i in 1..=steps {
                    let t = i as f32 / steps as f32;
                    ui.draw_rect(
                        Rect::from_xywh(mx + dx * t - 1.0, my + dz * t - 1.0, 2.0, 2.0),
                        Color::rgba(0.95, 0.95, 0.98, 0.85),
                    );
                }
            }
        }
    }
}

/// Generic interaction prompt centred just below mid-screen, used by
/// the rift / hub portals. `text` is the message body (e.g.
/// "PRESS [F] TO ENTER THE RIFT"). Migrated onto the IM stack —
/// uses `Frame` so the panel chrome (rounded corners, border)
/// matches the rest of the UI without copy-pasting rect math.
pub fn render_portal_prompt(ui: &mut rift_engine::ui::im::Ui<'_>, text: &str) {
    use rift_engine::ui::im::{Color, Frame, Pad, Pos2, Rect, Vec2};
    let theme = *ui.theme();
    let screen = ui.screen_size();
    let label_size = 12.0;
    let text_w = ui.measure_text(text, label_size);
    let inner = Vec2::new(text_w, label_size);
    let pad = Pad::symmetric(18.0, 5.0);
    let outer_w = inner.x + pad.left + pad.right;
    let outer_h = inner.y + pad.top + pad.bottom;
    let rect = Rect::from_xywh((screen.x - outer_w) / 2.0, screen.y * 0.62, outer_w, outer_h);
    let frame = Frame::panel(&theme)
        .with_fill(Color::rgba(0.05, 0.08, 0.14, 0.92))
        .with_padding(pad);
    frame.show(ui, rect, |ui, body| {
        ui.draw_text(
            Pos2::new(body.x(), body.y()),
            text,
            label_size,
            Color::rgba(0.55, 0.78, 1.0, 1.0),
        );
    });
}

/// Loot-pickup prompt — same chrome as [`render_portal_prompt`] but
/// the text colour follows the item's tier (rarity) so the player
/// can read the rarity at a glance. Placed slightly above the
/// portal prompt anchor so the two never overlap.
pub fn render_loot_prompt(
    ui: &mut rift_engine::ui::im::Ui<'_>,
    text: &str,
    color: rift_engine::ui::im::Color,
) {
    use rift_engine::ui::im::{Color, Frame, Pad, Pos2, Rect, Vec2};
    let theme = *ui.theme();
    let screen = ui.screen_size();
    let label_size = 12.0;
    let text_w = ui.measure_text(text, label_size);
    let inner = Vec2::new(text_w, label_size);
    let pad = Pad::symmetric(18.0, 5.0);
    let outer_w = inner.x + pad.left + pad.right;
    let outer_h = inner.y + pad.top + pad.bottom;
    let rect = Rect::from_xywh((screen.x - outer_w) / 2.0, screen.y * 0.70, outer_w, outer_h);
    let frame = Frame::panel(&theme)
        .with_fill(Color::rgba(0.05, 0.05, 0.07, 0.92))
        .with_padding(pad);
    frame.show(ui, rect, |ui, body| {
        ui.draw_text(Pos2::new(body.x(), body.y()), text, label_size, color);
    });
}

/// Red screen-edge vignette shown briefly after the player takes damage.
/// `strength` is in [0, 1]; the centre stays clear so combat readability
/// is preserved.  Implemented as four tapered borders + four corner
/// triangles approximated by stacked rects (cheap; the overlay batch
/// only supports rects).
pub fn render_damage_flash(ui: &mut Ui<'_>, strength: f32) {
    let s = strength.clamp(0.0, 1.0);
    if s <= 0.001 { return; }
    let screen = ui.screen_size();
    let sw = screen.x;
    let sh = screen.y;
    // Subtle border thickness; never grows large enough to obscure
    // gameplay near the screen edges.
    let t = 22.0 + 28.0 * s;
    // Stack layered rectangles per edge with falling alpha to fake a
    // soft gradient. Alpha is intentionally low so the effect reads
    // like a quick pulse, not a red filter.
    const STEPS: i32 = 4;
    for i in 0..STEPS {
        let f = 1.0 - (i as f32 / STEPS as f32);
        let alpha = (0.22 * s * f).clamp(0.0, 0.32);
        let band = t * (1.0 - i as f32 / STEPS as f32);
        let col = Color::rgba(0.78, 0.05, 0.05, alpha);
        // top
        ui.draw_rect(Rect::from_xywh(0.0, 0.0, sw, band), col);
        // bottom
        ui.draw_rect(Rect::from_xywh(0.0, sh - band, sw, band), col);
        // left
        ui.draw_rect(Rect::from_xywh(0.0, 0.0, band, sh), col);
        // right
        ui.draw_rect(Rect::from_xywh(sw - band, 0.0, band, sh), col);
    }
}

/// Render the ability bar (bottom-center) via the immediate-mode UI.
pub fn render_ability_bar(ui: &mut Ui<'_>, abilities: &AbilitySlot) {
    const AB_SIZE: f32 = 64.0;
    const AB_GAP: f32 = 6.0;
    const AB_KEYS: [&str; 6] = ["LMB", "1", "2", "3", "4", "5"];

    let screen = ui.screen_size();
    let ab_total_w = 6.0 * AB_SIZE + 5.0 * AB_GAP;
    let ab_x = (screen.x - ab_total_w) * 0.5;
    let ab_y = screen.y - AB_SIZE - 16.0;

    let mut hovered_idx: Option<usize> = None;

    for (i, slot) in abilities.slots.iter().enumerate() {
        let pos = Pos2::new(ab_x + i as f32 * (AB_SIZE + AB_GAP), ab_y);
        let id = Id::root("ability_bar").child(i);

        let mut s = ItemSlot::new(AB_SIZE).key_label(AB_KEYS[i]);
        if let Some(state) = slot {
            // `cooldown_progress()` returns elapsed/total; the
            // overlay drains from full → empty as the cooldown
            // ticks, so pass `1 - progress` (remaining fraction).
            let remaining = 1.0 - state.cooldown_progress();
            s = s.cooldown(remaining).enabled(state.ready());
            if let Some(name) = state.ability.icon {
                s = s.icon(name);
            } else {
                let abbrev = ability_abbrev(state.ability.name);
                if let Some(ch) = abbrev.chars().next() {
                    s = s.fallback_glyph(ch)
                        .fallback_color(Color::rgba(0.6, 0.85, 1.0, 0.95));
                }
            }
        } else {
            s = s.enabled(false);
        }

        let resp = s.show(ui, pos, id);
        if resp.hovered && slot.is_some() {
            hovered_idx = Some(i);
        }
    }

    // Tooltip for hovered ability.
    if let Some(idx) = hovered_idx {
        if let Some(Some(state)) = abilities.slots.get(idx) {
            let stats = if state.ability.cooldown > 0.0 {
                format!(
                    "CD: {:.1}s | Dmg: {:.0}%",
                    state.ability.cooldown,
                    state.ability.damage_mult * 100.0
                )
            } else {
                format!("Dmg: {:.0}%", state.ability.damage_mult * 100.0)
            };
            let proj = if state.ability.projectile_count > 1 {
                Some(format!("Projectiles: {}", state.ability.projectile_count))
            } else {
                None
            };
            let mut lines = vec![
                TooltipLine::new(state.ability.name, 14.0, Color::rgba(1.0, 0.9, 0.5, 1.0)),
                TooltipLine::new(state.ability.description, 11.0, Color::rgba(0.8, 0.8, 0.8, 1.0)),
                TooltipLine::new(stats.as_str(), 11.0, Color::rgba(0.6, 0.8, 1.0, 0.9)),
            ];
            if let Some(ref p) = proj {
                lines.push(TooltipLine::new(p.as_str(), 10.0, Color::rgba(0.7, 0.7, 0.7, 0.8)));
            }
            // Anchor centred above the bar, then let the
            // tooltip widget clamp inside the screen.
            let tip_x = (screen.x - 220.0) * 0.5;
            let tip_y = ab_y - 90.0;
            Tooltip::new()
                .min_width(220.0)
                .show(ui, Pos2::new(tip_x, tip_y), &lines);
        }
    }
}

/// Render thin health bars above enemies that have taken damage.
pub fn render_enemy_health_bars(ui: &mut Ui<'_>, world: &hecs::World, view_proj: Mat4) {
    use rift_engine::ui::im::WorldUi;

    const BAR_W: f32 = 52.0;
    const BAR_H: f32 = 6.0;
    const Y_OFFSET: f32 = -24.0;
    const PIP_SIZE: f32 = 6.0;
    const PIP_GAP: f32 = 2.0;

    let mut wui = WorldUi::new(ui, view_proj);

    for (entity, (transform, _enemy, health)) in world.query::<(&Transform, &Enemy, &Health)>().iter() {
        let debuff_mask = world
            .get::<&Debuffs>(entity)
            .map(|d| d.mask)
            .unwrap_or(0);
        let damaged = health.current < health.max;
        if !damaged && debuff_mask == 0 {
            continue;
        }

        let world_pos = transform.position + Vec3::new(0.0, 1.2, 0.0);

        let bar_rect = if damaged {
            let hp_pct = (health.current / health.max).clamp(0.0, 1.0);
            // Enemy HP gradient (more saturated red than the
            // friendly HP gradient since it's *their* HP draining).
            let color = if hp_pct > 0.5 {
                Color::rgba(0.8, 0.1, 0.1, 0.9)
            } else {
                Color::rgba(0.9, 0.3, 0.0, 0.9)
            };
            wui.bar_above_world(world_pos, Y_OFFSET, BAR_W, BAR_H, hp_pct, color)
        } else {
            // No bar drawn, but we still want the anchor for pips.
            wui.world_to_screen(world_pos)
                .map(|anchor| Rect::from_xywh(anchor.x - BAR_W * 0.5, anchor.y + Y_OFFSET, BAR_W, BAR_H))
        };

        // Debuff pips: one little square per active debuff,
        // coloured from the registered def. Stacked left-to-right
        // just above the bar.
        if debuff_mask != 0 {
            if let Some(rect) = bar_rect {
                let pips_y = rect.y() - PIP_SIZE - 2.0;
                let mut x = rect.x();
                for id in rift_game::debuffs::iter_mask(debuff_mask) {
                    let Some(def) = rift_game::debuffs::lookup(id) else { continue };
                    let [r, g, b] = def.color;
                    // 1 px black outline so pips read on light walls.
                    wui.ui().draw_rect(
                        Rect::from_xywh(x - 1.0, pips_y - 1.0, PIP_SIZE + 2.0, PIP_SIZE + 2.0),
                        Color::rgba(0.0, 0.0, 0.0, 0.85),
                    );
                    wui.ui().draw_rect(
                        Rect::from_xywh(x, pips_y, PIP_SIZE, PIP_SIZE),
                        Color::rgba(r, g, b, 0.95),
                    );
                    x += PIP_SIZE + PIP_GAP;
                }
            }
        }
    }
}

/// Two-letter shorthand for the ability-bar fallback when no icon
/// is registered. Picks the initials of the first two words; falls
/// back to the first two letters of a single-word name. Lower-case
/// connector words ("of", "for", "the") are skipped so
/// "Mark for Death" becomes `MD` instead of `MF`.
fn ability_abbrev(name: &str) -> String {
    const SKIP: &[&str] = &["of", "for", "the", "and", "to"];
    let initials: Vec<char> = name
        .split_whitespace()
        .filter(|w| !SKIP.contains(&w.to_ascii_lowercase().as_str()))
        .filter_map(|w| w.chars().next())
        .map(|c| c.to_ascii_uppercase())
        .take(2)
        .collect();
    if initials.len() >= 2 {
        initials.into_iter().collect()
    } else {
        // Single-word name — use the first two letters.
        let mut chars = name.chars();
        let a = chars.next().unwrap_or('?').to_ascii_uppercase();
        let b = chars.next().unwrap_or(a).to_ascii_uppercase();
        format!("{a}{b}")
    }
}
