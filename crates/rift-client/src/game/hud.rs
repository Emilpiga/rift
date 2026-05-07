use rift_engine::ecs::components::{
    Boss, Debuffs, Enemy, Health, LocalPlayer, Player, RemotePlayer, Transform,
};
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

/// Soft-haloed minimap pip: draws a low-alpha halo rounded-rect
/// then the opaque core rounded-rect on top. Free helper rather
/// than a closure so the caller can keep mutably borrowing `ui`
/// for other draws between pip calls.
fn draw_pip(
    ui: &mut Ui<'_>,
    mx: f32,
    my: f32,
    core_size: f32,
    core_col: Color,
    halo_col: Color,
) {
    let halo = core_size * 2.4;
    ui.draw_rounded_rect(
        Rect::from_xywh(mx - halo * 0.5, my - halo * 0.5, halo, halo),
        halo * 0.5,
        halo_col,
    );
    ui.draw_rounded_rect(
        Rect::from_xywh(mx - core_size * 0.5, my - core_size * 0.5, core_size, core_size),
        core_size * 0.5,
        core_col,
    );
}

/// Top-right minimap. Shows walkable tiles, the player (white pip with a
/// short heading fan), nearby enemies (red), the boss (orange), and the
/// active rift / hub portal (cyan).
///
/// Visual breakdown (top to bottom inside the panel):
///   ┌─────────────────────────────┐
///   │  MAP                     N  │  ← header strip, themed
///   ├─────────────────────────────┤
///   │  ┌───────────────────────┐  │  ← inset navgrid frame
///   │  │ walkable tiles + pips │  │
///   │  └───────────────────────┘  │
///   └─────────────────────────────┘
///
/// The map auto-scales: cell size is computed so the navgrid fits inside
/// the inset content area.
pub fn render_minimap(
    ui: &mut Ui<'_>,
    world: &hecs::World,
    nav: &NavGrid,
    player_facing: Vec3,
    portal_pos: Option<Vec3>,
) {
    use rift_engine::ui::im::{Frame, Pad};

    // ---- Layout constants ----
    const MAP_PX: f32 = 224.0;
    const HEADER_H: f32 = 18.0;
    const INSET: f32 = 6.0;
    const MARGIN: f32 = 14.0;
    const RADIUS: f32 = 6.0;

    let theme = *ui.theme();
    let screen = ui.screen_size();
    let sw = screen.x;

    let map_x = sw - MAP_PX - MARGIN;
    let map_y = MARGIN;
    let panel_rect = Rect::from_xywh(map_x, map_y, MAP_PX, MAP_PX);

    // ---- Outer frame: themed panel chrome ----
    // Subtle outer drop-shadow to lift the map off the world.
    ui.draw_rounded_rect(
        Rect::from_xywh(map_x + 2.0, map_y + 3.0, MAP_PX, MAP_PX),
        RADIUS + 1.0,
        Color::rgba(0.0, 0.0, 0.0, 0.32),
    );
    let frame = Frame::panel(&theme)
        .with_fill(Color::rgba(0.04, 0.05, 0.07, 0.94))
        .with_radius(RADIUS)
        .with_padding(Pad::all(0.0));
    frame.show(ui, panel_rect, |ui, body| {
        // ---- Header strip ----
        let header = Rect::from_xywh(body.x(), body.y(), body.width(), HEADER_H);
        ui.draw_rect(
            Rect::from_xywh(header.x(), header.y(), header.width(), header.height()),
            Color::rgba(0.07, 0.09, 0.12, 1.0),
        );
        // Header divider underline.
        ui.draw_rect(
            Rect::from_xywh(header.x(), header.max.y - 1.0, header.width(), 1.0),
            Color::rgba(0.16, 0.18, 0.24, 1.0),
        );
        // Title.
        ui.draw_text(
            Pos2::new(header.x() + 8.0, header.y() + 4.0),
            "MAP",
            10.0,
            theme.colors.text_dim,
        );
        // North indicator: "N" hugged to the right side of the
        // header strip. The minimap maps world Z to screen Y, so
        // up-on-map = -Z. The pip below the letter visually
        // anchors it as a compass marker.
        let n_w = ui.measure_text("N", 10.0);
        let n_x = header.max.x - n_w - 12.0;
        ui.draw_rect(
            Rect::from_xywh(n_x - 5.0, header.y() + 6.0, 3.0, 6.0),
            Color::rgba(0.55, 0.78, 1.0, 0.65),
        );
        ui.draw_text(
            Pos2::new(n_x, header.y() + 4.0),
            "N",
            10.0,
            Color::rgba(0.85, 0.92, 1.0, 0.95),
        );

        // ---- Navgrid area ----
        let inner_rect = Rect::from_xywh(
            body.x() + INSET,
            body.y() + HEADER_H + INSET,
            body.width() - INSET * 2.0,
            body.height() - HEADER_H - INSET * 2.0,
        );

        // Inset background (slightly darker than the panel so the
        // walkable tiles read as "lit").
        ui.draw_rounded_rect(
            inner_rect,
            RADIUS - 2.0,
            Color::rgba(0.025, 0.028, 0.035, 1.0),
        );

        // Centre the navgrid inside the inset.
        let cell = (inner_rect.width().min(inner_rect.height())
            / nav.width.max(nav.depth) as f32)
            .max(1.0);
        let map_w = cell * nav.width as f32;
        let map_h = cell * nav.depth as f32;
        let inner_x = inner_rect.x() + (inner_rect.width() - map_w) * 0.5;
        let inner_y = inner_rect.y() + (inner_rect.height() - map_h) * 0.5;

        // Walkable tiles. Two-tone fill driven by a cheap
        // checker on (x ^ z) so the map doesn't read as a flat
        // slab when cells are large enough to discern.
        let floor_a = Color::rgba(0.42, 0.36, 0.30, 0.95);
        let floor_b = Color::rgba(0.36, 0.30, 0.25, 0.95);
        for z in 0..nav.depth {
            for x in 0..nav.width {
                if nav.is_walkable(x, z) {
                    let col = if (x ^ z) & 1 == 0 { floor_a } else { floor_b };
                    ui.draw_rect(
                        Rect::from_xywh(
                            inner_x + x as f32 * cell,
                            inner_y + z as f32 * cell,
                            cell,
                            cell,
                        ),
                        col,
                    );
                }
            }
        }

        // Inner-edge vignette: four thin dark bands fading inward
        // so the map doesn't visually bleed into the panel
        // border. Cheap (4 rects) but goes a long way.
        const VIG_STEPS: i32 = 3;
        for i in 0..VIG_STEPS {
            let f = 1.0 - (i as f32 / VIG_STEPS as f32);
            let alpha = 0.28 * f;
            let band = (4.0 - i as f32 * 1.2).max(1.0);
            let col = Color::rgba(0.0, 0.0, 0.0, alpha);
            // top
            ui.draw_rect(
                Rect::from_xywh(inner_rect.x(), inner_rect.y() + i as f32, inner_rect.width(), band),
                col,
            );
            // bottom
            ui.draw_rect(
                Rect::from_xywh(
                    inner_rect.x(),
                    inner_rect.max.y - i as f32 - band,
                    inner_rect.width(),
                    band,
                ),
                col,
            );
            // left
            ui.draw_rect(
                Rect::from_xywh(inner_rect.x() + i as f32, inner_rect.y(), band, inner_rect.height()),
                col,
            );
            // right
            ui.draw_rect(
                Rect::from_xywh(
                    inner_rect.max.x - i as f32 - band,
                    inner_rect.y(),
                    band,
                    inner_rect.height(),
                ),
                col,
            );
        }

        // Inset outline.
        ui.draw_rounded_outline(
            inner_rect,
            RADIUS - 2.0,
            1.0,
            Color::rgba(0.18, 0.20, 0.26, 1.0),
        );

        // World → minimap coords helper
        let to_map = |p: Vec3| -> (f32, f32) {
            (inner_x + p.x * cell, inner_y + p.z * cell)
        };
        let in_inner = |mx: f32, my: f32| -> bool {
            mx >= inner_rect.x()
                && mx <= inner_rect.max.x
                && my >= inner_rect.y()
                && my <= inner_rect.max.y
        };

        // Portal pip — cyan, drawn first so enemy / player pips
        // overlap it cleanly when stacked.
        if let Some(p) = portal_pos {
            let (mx, my) = to_map(p);
            if in_inner(mx, my) {
                let s = (cell * 2.4).max(5.0);
                draw_pip(
                    ui,
                    mx,
                    my,
                    s,
                    Color::rgba(0.45, 0.85, 1.0, 1.0),
                    Color::rgba(0.30, 0.75, 1.0, 0.35),
                );
            }
        }

        // Enemy pips
        for (_id, (t, _e, boss, _)) in world
            .query::<(&Transform, &Enemy, Option<&Boss>, Option<&Health>)>()
            .iter()
        {
            let (mx, my) = to_map(t.position);
            if !in_inner(mx, my) {
                continue;
            }
            if boss.is_some() {
                let s = (cell * 2.6).max(5.0);
                draw_pip(
                    ui,
                    mx,
                    my,
                    s,
                    Color::rgba(1.00, 0.60, 0.10, 1.0),
                    Color::rgba(1.00, 0.55, 0.10, 0.40),
                );
            } else {
                let s = (cell * 1.7).max(3.0);
                draw_pip(
                    ui,
                    mx,
                    my,
                    s,
                    Color::rgba(0.94, 0.30, 0.26, 1.0),
                    Color::rgba(0.92, 0.20, 0.18, 0.30),
                );
            }
        }

        // Player pip + facing fan
        if let Some((pp, _)) = world
            .query::<(&Transform, &Player, &LocalPlayer)>()
            .iter()
            .map(|(_, (t, p, _))| (t.position, p.aim_dir))
            .next()
        {
            let (mx, my) = to_map(pp);
            if in_inner(mx, my) {
                // Facing fan: tapered dots along `player_facing`,
                // drawn before the player pip so the pip stays
                // crisp on top of the trail.
                let f = Vec3::new(player_facing.x, 0.0, player_facing.z);
                if f.length_squared() > 1e-4 {
                    let f = f.normalize();
                    let len = (cell * 4.5).max(8.0);
                    let dx = f.x * len;
                    let dz = f.z * len;
                    const STEPS: i32 = 5;
                    for i in 1..=STEPS {
                        let t = i as f32 / STEPS as f32;
                        let size = (3.2 * (1.0 - t * 0.6)).max(1.4);
                        let alpha = (1.0 - t) * 0.85 + 0.15;
                        ui.draw_rounded_rect(
                            Rect::from_xywh(
                                mx + dx * t - size * 0.5,
                                my + dz * t - size * 0.5,
                                size,
                                size,
                            ),
                            size * 0.5,
                            Color::rgba(0.95, 0.97, 1.0, alpha),
                        );
                    }
                }
                let s = (cell * 2.0).max(4.5);
                draw_pip(
                    ui,
                    mx,
                    my,
                    s,
                    Color::rgba(0.98, 0.99, 1.0, 1.0),
                    Color::rgba(0.55, 0.78, 1.0, 0.45),
                );
            }
        }
    });
}

/// Generic interaction prompt centred just below mid-screen, used by
/// the rift / hub portals. `text` is the message body (e.g.
/// "PRESS [F] TO ENTER THE RIFT"). Migrated onto the IM stack —
/// uses `Frame` so the panel chrome (rounded corners, border)
/// matches the rest of the UI without copy-pasting rect math.
pub fn render_hud_prompt(ui: &mut rift_engine::ui::im::Ui<'_>, text: &str) {
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

/// Loot-pickup prompt — same chrome as [`render_hud_prompt`] but
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
///
/// Returns `Some(slot_index)` if the player clicked one of the
/// six bar slots this frame. Caller uses that to open the
/// spellbook with the slot pre-targeted.
///
/// Locked slots (per `loadout::SLOT_UNLOCK_LEVELS` vs.
/// `player_level`) render disabled and reject clicks. The slot
/// shows a "Lv N" caption so the player knows when it unlocks.
pub fn render_ability_bar(
    ui: &mut Ui<'_>,
    abilities: &AbilitySlot,
    player_level: u32,
) -> Option<usize> {
    const AB_SIZE: f32 = 64.0;
    const AB_GAP: f32 = 6.0;
    const AB_KEYS: [&str; 6] = ["LMB", "1", "2", "3", "4", "5"];

    let screen = ui.screen_size();
    let ab_total_w = 6.0 * AB_SIZE + 5.0 * AB_GAP;
    let ab_x = (screen.x - ab_total_w) * 0.5;
    let ab_y = screen.y - AB_SIZE - 16.0;

    let mut hovered_idx: Option<usize> = None;
    let mut clicked_idx: Option<usize> = None;

    for (i, slot) in abilities.slots.iter().enumerate() {
        let pos = Pos2::new(ab_x + i as f32 * (AB_SIZE + AB_GAP), ab_y);
        let id = Id::root("ability_bar").child(i);
        let slot_unlocked =
            rift_game::loadout::is_slot_unlocked(i, player_level);

        let mut s = ItemSlot::new(AB_SIZE).key_label(AB_KEYS[i]);
        if !slot_unlocked {
            // Locked bar slot — render as a disabled "padlock"
            // tile with the unlock level glyph.
            s = s
                .enabled(false)
                .fallback_glyph('\u{1F512}')
                .fallback_color(Color::rgba(0.55, 0.25, 0.25, 0.9));
        } else if let Some(state) = slot {
            // `cooldown_progress()` returns elapsed/total; the
            // overlay drains from full → empty as the cooldown
            // ticks, so pass `1 - progress` (remaining fraction).
            let remaining = 1.0 - state.cooldown_progress();
            // Always keep the slot click-enabled so the player
            // can right-click-style swap via the spellbook
            // even mid-cooldown. The `ready()` flag only
            // affects whether the cast hotkey fires.
            s = s.cooldown(remaining);
            if let Some(name) = state.ability.icon {
                s = s.icon(name);
            } else {
                let abbrev = ability_abbrev(state.ability.name);
                if let Some(ch) = abbrev.chars().next() {
                    s = s.fallback_glyph(ch)
                        .fallback_color(Color::rgba(0.6, 0.85, 1.0, 0.95));
                }
            }
        }
        // Empty unlocked slot: leave it click-enabled with no
        // icon so the player can click to open the spellbook
        // and pick something for it.

        let resp = s.show(ui, pos, id);
        if resp.hovered && slot.is_some() && slot_unlocked {
            hovered_idx = Some(i);
        }
        if resp.clicked && slot_unlocked {
            clicked_idx = Some(i);
        }

        // Locked-slot caption.
        if !slot_unlocked {
            let lvl = rift_game::loadout::SLOT_UNLOCK_LEVELS[i];
            let theme = *ui.theme();
            ui.draw_text(
                Pos2::new(pos.x, pos.y + AB_SIZE + 2.0),
                format!("Lv {lvl}").as_str(),
                theme.fonts.size_sm,
                Color::rgba(0.65, 0.30, 0.30, 0.9),
            );
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

    clicked_idx
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

/// Render thin health bars above remote (party-member) avatars.
/// Mirrors `render_enemy_health_bars` styling but in green so the
/// player can tell teammates apart from enemies at a glance, and
/// always draws the bar (even at full HP) since seeing a teammate's
/// max HP is useful tactical info, unlike enemies where a full bar
/// is just visual noise.
///
/// Should only be called inside rift floors — in the hub, drawing
/// HP bars over idle teammates standing around is just clutter.
pub fn render_remote_player_health_bars(
    ui: &mut Ui<'_>,
    world: &hecs::World,
    view_proj: Mat4,
) {
    use rift_engine::ui::im::WorldUi;

    const BAR_W: f32 = 56.0;
    const BAR_H: f32 = 6.0;
    // A bit higher than enemies — players are taller than most
    // mobs and the bar would otherwise clip into the head.
    const Y_OFFSET: f32 = -32.0;

    let mut wui = WorldUi::new(ui, view_proj);
    for (_e, (transform, _rp, health)) in world
        .query::<(&Transform, &RemotePlayer, &Health)>()
        .iter()
    {
        let hp_pct = (health.current / health.max).clamp(0.0, 1.0);
        let world_pos = transform.position + Vec3::new(0.0, 1.6, 0.0);
        // Friendly green → amber → red as HP drops, so a low-HP
        // teammate visually pops the same way the local HP bar
        // does.
        let color = if hp_pct > 0.5 {
            Color::rgba(0.25, 0.75, 0.25, 0.9)
        } else if hp_pct > 0.25 {
            Color::rgba(0.85, 0.7, 0.1, 0.9)
        } else {
            Color::rgba(0.9, 0.25, 0.15, 0.9)
        };
        wui.bar_above_world(world_pos, Y_OFFSET, BAR_W, BAR_H, hp_pct, color);
    }
}
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

/// Full-screen "Entering World" overlay: title, progress bar,
/// and a tiny status label. Drawn on top of the live scene
/// during the staged-init steps after a hub↔rift transition so
/// the player sees something other than a frozen frame while
/// monsters / icons stream in.
pub fn draw_world_loading_overlay(renderer: &mut rift_engine::Renderer, progress: f32, label: &str) {
    let (sw, sh) = renderer.screen_size();
    let batch = &mut renderer.overlay_batch;

    batch.rect_px(0.0, 0.0, sw, sh, [0.02, 0.02, 0.03, 0.92], sw, sh);

    let title = "Entering World";
    let title_size = 30.0;
    let title_w = batch.measure_text(title, title_size);
    batch.text(
        title,
        (sw - title_w) * 0.5,
        sh * 0.40 - title_size,
        title_size,
        [0.85, 0.80, 0.65, 1.0],
        sw,
        sh,
    );

    let bar_w = (sw * 0.45).max(240.0);
    let bar_h = 18.0;
    let bar_x = (sw - bar_w) * 0.5;
    let bar_y = sh * 0.50;
    batch.rect_px(bar_x, bar_y, bar_w, bar_h, [0.10, 0.10, 0.14, 1.0], sw, sh);
    let fill_w = bar_w * progress.clamp(0.0, 1.0);
    if fill_w > 0.5 {
        batch.rect_px(bar_x, bar_y, fill_w, bar_h, [0.55, 0.45, 0.20, 1.0], sw, sh);
    }
    let border = [0.30, 0.28, 0.22, 1.0];
    let t = 1.5;
    batch.rect_px(bar_x, bar_y, bar_w, t, border, sw, sh);
    batch.rect_px(bar_x, bar_y + bar_h - t, bar_w, t, border, sw, sh);
    batch.rect_px(bar_x, bar_y, t, bar_h, border, sw, sh);
    batch.rect_px(bar_x + bar_w - t, bar_y, t, bar_h, border, sw, sh);

    let label_size = 14.0;
    let label_w = batch.measure_text(label, label_size);
    batch.text(
        label,
        (sw - label_w) * 0.5,
        bar_y + bar_h + 16.0,
        label_size,
        [0.65, 0.62, 0.55, 1.0],
        sw,
        sh,
    );
}
