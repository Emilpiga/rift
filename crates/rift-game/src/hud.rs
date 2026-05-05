use rift_engine::ecs::components::{Boss, Enemy, Health, Player, Transform};
use rift_engine::ai::NavGrid;
use rift_engine::loot::item::ItemSlot;
use rift_engine::loot::Equipment;
use rift_engine::renderer::OverlayBatch;
use glam::{Mat4, Vec3};

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
    in_hub: bool,
) {
    // HP + XP bars: stacked, centered above the ability bar so the
    // player's vital stats sit right under their character.
    let hp_pct = world
        .query::<(&Health, &Player)>()
        .iter()
        .map(|(_, (h, _))| h.current / (h.max + max_hp_bonus))
        .next()
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);

    // Ability bar lives at sh - 50; stack the bars 8 px above it.
    let bar_w = 280.0;
    let bar_h = 16.0;
    let xp_h = 6.0;
    let bars_total_h = bar_h + 2.0 + xp_h;
    let bar_x = (sw - bar_w) / 2.0;
    let bar_y = sh - 50.0 - 8.0 - bars_total_h;

    // HP bar
    batch.rect_px(bar_x, bar_y, bar_w, bar_h, [0.08, 0.08, 0.10, 0.85], sw, sh);
    let hp_color = if hp_pct > 0.5 {
        [0.45, 0.78, 0.30, 0.95]
    } else if hp_pct > 0.25 {
        [0.90, 0.70, 0.05, 0.95]
    } else {
        [0.92, 0.18, 0.18, 0.95]
    };
    batch.rect_px(bar_x, bar_y, bar_w * hp_pct, bar_h, hp_color, sw, sh);
    // Border
    batch.rect_px(bar_x, bar_y, bar_w, 1.5, [0.30, 0.30, 0.32, 0.9], sw, sh);
    batch.rect_px(bar_x, bar_y + bar_h - 1.5, bar_w, 1.5, [0.30, 0.30, 0.32, 0.9], sw, sh);
    batch.rect_px(bar_x, bar_y, 1.5, bar_h, [0.30, 0.30, 0.32, 0.9], sw, sh);
    batch.rect_px(bar_x + bar_w - 1.5, bar_y, 1.5, bar_h, [0.30, 0.30, 0.32, 0.9], sw, sh);

    // XP bar (slimmer, directly under the HP bar)
    let xp_pct = player_state.experience.progress();
    let xp_y = bar_y + bar_h + 2.0;
    batch.rect_px(bar_x, xp_y, bar_w, xp_h, [0.08, 0.08, 0.10, 0.85], sw, sh);
    batch.rect_px(bar_x, xp_y, bar_w * xp_pct, xp_h, [0.45, 0.30, 0.85, 0.95], sw, sh);

    // Level pip floats just to the left of the HP bar.
    let level_text = format!("Lv.{}", player_state.experience.level);
    batch.text(&level_text, bar_x - 42.0, bar_y + 1.0, 13.0, [0.92, 0.92, 0.92, 1.0], sw, sh);

    // Rift progress bar (top-center, 300x16 px). Hidden in the hub.
    if !in_hub {
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
    } else {
        // Hub label where the progress bar would normally sit.
        let label_w = 120.0;
        let label_h = 20.0;
        let lx = (sw - label_w) / 2.0;
        let ly = 10.0;
        batch.rect_px(lx, ly, label_w, label_h, [0.08, 0.10, 0.16, 0.8], sw, sh);
        batch.text("THE HUB", lx + 32.0, ly + 4.0, 13.0, [0.7, 0.85, 1.0, 1.0], sw, sh);
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

/// Fullscreen black quad used by the death→hub fade transition.
pub fn render_fade_to_black(batch: &mut OverlayBatch, alpha: f32, sw: f32, sh: f32) {
    let a = alpha.clamp(0.0, 1.0);
    if a <= 0.001 { return; }
    batch.rect_px(0.0, 0.0, sw, sh, [0.0, 0.0, 0.0, a], sw, sh);
}

/// Off-screen / far-away boss locator. When the boss is alive but the
/// player can't see them (off-screen, behind camera, or > ARROW_RANGE
/// world units away), draw a glowing arrow at the screen edge pointing
/// toward the boss in screen space.
pub fn render_boss_arrow(
    batch: &mut OverlayBatch,
    world: &hecs::World,
    view_proj: Mat4,
    sw: f32,
    sh: f32,
) {
    const ARROW_RANGE_SQ: f32 = 16.0 * 16.0; // show arrow if boss > 16 m away
    const EDGE_PAD: f32 = 110.0;

    // Find boss world position + player world position.
    let boss_pos: Option<Vec3> = world
        .query::<(&Transform, &Boss)>()
        .iter()
        .map(|(_, (t, _))| t.position + Vec3::new(0.0, 1.2, 0.0))
        .next();
    let Some(boss_pos) = boss_pos else { return };

    let player_pos: Option<Vec3> = world
        .query::<(&Transform, &Player)>()
        .iter()
        .map(|(_, (t, _))| t.position)
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
    let col = [1.00, 0.42, 0.05, (0.98 * pulse).clamp(0.7, 1.0)];

    // Tangent (perpendicular) to arrow heading; used to fan the head out.
    let tx = -ny;
    let ty = nx;

    // Helper: draw a tightly stamped 1-pixel-radius "dot" trail along
    // the line from local (u0,v0) to (u1,v1). Each dot is a tiny
    // axis-aligned rect; with `DOT_PITCH=1.5` they overlap into a clean
    // line, so the resulting shape reads as a single solid arrow rather
    // than a cloud of squares.
    let mut line = |u0: f32, v0: f32, u1: f32, v1: f32, thickness: f32| {
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
            batch.rect_px(
                sx_ - thickness * 0.5,
                sy_ - thickness * 0.5,
                thickness,
                thickness,
                col,
                sw,
                sh,
            );
        }
    };

    // Geometry of the arrow in local (u along heading, v perpendicular):
    //
    //                 tip (u = +HEAD_LEN, v = 0)
    //                  /\
    //                 /  \
    //   wing_l ──────/    \────── wing_r       (u = 0, v = ±HALF_W)
    //   shaft_l ────┤      ├──── shaft_r       (u = 0, v = ±SHAFT_W)
    //               │      │
    //               │      │
    //   tail_l ─────┴──────┴───── tail_r       (u = -SHAFT_LEN)
    const HEAD_LEN: f32 = 22.0;     // tip -> wings
    const SHAFT_LEN: f32 = 26.0;    // wings -> tail
    const HALF_W: f32 = 22.0;       // half-width at wings (head base)
    const SHAFT_W: f32 = 8.0;       // half-width of the shaft
    let tip_u = HEAD_LEN;
    let wing_u = 0.0;
    let tail_u = -SHAFT_LEN;
    let thick = 4.0;

    // Head outline (two leading edges of the V).
    line(tip_u, 0.0, wing_u, HALF_W, thick);
    line(tip_u, 0.0, wing_u, -HALF_W, thick);
    // Notch joining wings to shaft.
    line(wing_u, HALF_W, wing_u, SHAFT_W, thick);
    line(wing_u, -HALF_W, wing_u, -SHAFT_W, thick);
    // Shaft sides + tail cap.
    line(wing_u, SHAFT_W, tail_u, SHAFT_W, thick);
    line(wing_u, -SHAFT_W, tail_u, -SHAFT_W, thick);
    line(tail_u, SHAFT_W, tail_u, -SHAFT_W, thick);

    // Solid fill: scanlines parallel to the heading at uniform v steps.
    let fill = [col[0], col[1], col[2], (col[3] * 0.65).clamp(0.0, 1.0)];
    let v_steps = 28;
    for i in 1..v_steps {
        let v = -HALF_W + (HALF_W * 2.0) * (i as f32 / v_steps as f32);
        // Find left/right u bounds of the arrow at this v level.
        let av = v.abs();
        // Head region: linear taper from tip(u=HEAD_LEN, v=0) to wing(u=0, v=±HALF_W).
        // Shaft region: rectangle for |v| <= SHAFT_W between u=tail and u=wing.
        let in_head = av <= HALF_W;
        if !in_head { continue; }
        // u_right: forward-most u at this v. Inside the triangular head.
        let head_u = HEAD_LEN * (1.0 - av / HALF_W);
        // u_left: rear-most u. Equal to tail u inside the shaft, otherwise 0.
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
            batch.rect_px(sx_ - 1.5, sy_ - 1.5, 3.0, 3.0, fill, sw, sh);
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
    batch: &mut OverlayBatch,
    world: &hecs::World,
    nav: &NavGrid,
    player_facing: Vec3,
    portal_pos: Option<Vec3>,
    sw: f32,
    sh: f32,
) {
    const MAP_PX: f32 = 320.0;
    const PADDING: f32 = 14.0;
    const MARGIN: f32 = 14.0;

    let inner = MAP_PX - PADDING * 2.0;
    let cell = (inner / nav.width.max(nav.depth) as f32).max(1.0);
    let map_w = cell * nav.width as f32;
    let map_h = cell * nav.depth as f32;

    let map_x = sw - MAP_PX - MARGIN;
    let map_y = MARGIN;

    // Frame
    batch.rect_px(
        map_x,
        map_y,
        MAP_PX,
        MAP_PX,
        [0.04, 0.05, 0.07, 0.78],
        sw,
        sh,
    );
    // Border
    let border = [0.18, 0.20, 0.26, 0.95];
    batch.rect_px(map_x, map_y, MAP_PX, 1.5, border, sw, sh);
    batch.rect_px(map_x, map_y + MAP_PX - 1.5, MAP_PX, 1.5, border, sw, sh);
    batch.rect_px(map_x, map_y, 1.5, MAP_PX, border, sw, sh);
    batch.rect_px(map_x + MAP_PX - 1.5, map_y, 1.5, MAP_PX, border, sw, sh);

    // Centre the navgrid inside the framed area.
    let inner_x = map_x + (MAP_PX - map_w) * 0.5;
    let inner_y = map_y + (MAP_PX - map_h) * 0.5;

    // Walkable tiles
    let floor_col = [0.32, 0.30, 0.26, 0.92];
    for z in 0..nav.depth {
        for x in 0..nav.width {
            if nav.is_walkable(x, z) {
                batch.rect_px(
                    inner_x + x as f32 * cell,
                    inner_y + z as f32 * cell,
                    cell,
                    cell,
                    floor_col,
                    sw,
                    sh,
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
    // True iff (mx,my) lies inside the framed minimap window (so we
    // never paint dots on the surrounding HUD).
    let in_frame = |mx: f32, my: f32| -> bool {
        mx >= map_x && mx <= map_x + MAP_PX && my >= map_y && my <= map_y + MAP_PX
    };

    // Portal pip
    if let Some(p) = portal_pos {
        let (mx, my) = to_map(p);
        if in_frame(mx, my) {
            let s = (cell * 2.6).max(4.0);
            batch.rect_px(
                mx - s * 0.5,
                my - s * 0.5,
                s,
                s,
                [0.30, 0.75, 1.0, 0.95],
                sw,
                sh,
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
            ((cell * 2.4).max(4.0), [1.00, 0.55, 0.10, 1.0])
        } else {
            ((cell * 1.6).max(2.5), [0.92, 0.25, 0.22, 1.0])
        };
        batch.rect_px(mx - s * 0.5, my - s * 0.5, s, s, col, sw, sh);
    }

    // Player pip + facing tick
    if let Some((pp, _)) = world
        .query::<(&Transform, &Player)>()
        .iter()
        .map(|(_, (t, p))| (t.position, p.aim_dir))
        .next()
    {
        let (mx, my) = to_map(pp);
        if in_frame(mx, my) {
            let s = (cell * 1.9).max(3.0);
            batch.rect_px(
                mx - s * 0.5,
                my - s * 0.5,
                s,
                s,
                [0.95, 0.95, 0.98, 1.0],
                sw,
                sh,
            );
            // Facing line: 4 pixels long in the player's heading direction.
            let f = Vec3::new(player_facing.x, 0.0, player_facing.z);
            if f.length_squared() > 1e-4 {
                let f = f.normalize();
                let len = (cell * 3.5).max(6.0);
                let dx = f.x * len;
                let dz = f.z * len;
                // Approximate the line as a stack of small rects.
                let steps = 6;
                for i in 1..=steps {
                    let t = i as f32 / steps as f32;
                    batch.rect_px(
                        mx + dx * t - 1.0,
                        my + dz * t - 1.0,
                        2.0,
                        2.0,
                        [0.95, 0.95, 0.98, 0.85],
                        sw,
                        sh,
                    );
                }
            }
        }
    }
}

/// Generic interaction prompt centred just below mid-screen, used by
/// the rift / hub portals.  `text` is the message body (e.g.
/// "PRESS [F] TO ENTER THE RIFT").
pub fn render_portal_prompt(batch: &mut OverlayBatch, text: &str, sw: f32, sh: f32) {
    let tw = (text.len() as f32 * 8.5 + 36.0).max(220.0);
    let th = 22.0;
    let tx = (sw - tw) / 2.0;
    let ty = sh * 0.62;
    batch.rect_px(tx, ty, tw, th, [0.05, 0.08, 0.14, 0.78], sw, sh);
    batch.text(
        text,
        tx + 18.0,
        ty + 5.0,
        12.0,
        [0.55, 0.78, 1.0, 1.0],
        sw,
        sh,
    );
}

/// Red screen-edge vignette shown briefly after the player takes damage.
/// `strength` is in [0, 1]; the centre stays clear so combat readability
/// is preserved.  Implemented as four tapered borders + four corner
/// triangles approximated by stacked rects (cheap; the overlay batch
/// only supports rects).
pub fn render_damage_flash(batch: &mut OverlayBatch, strength: f32, sw: f32, sh: f32) {
    let s = strength.clamp(0.0, 1.0);
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
        let col = [0.78, 0.05, 0.05, alpha];
        // top
        batch.rect_px(0.0, 0.0, sw, band, col, sw, sh);
        // bottom
        batch.rect_px(0.0, sh - band, sw, band, col, sw, sh);
        // left
        batch.rect_px(0.0, 0.0, band, sh, col, sw, sh);
        // right
        batch.rect_px(sw - band, 0.0, band, sh, col, sw, sh);
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
