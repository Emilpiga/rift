//! Non-combat HUD: minimap, interaction prompts, descend tooltip.
//! Everything in this module renders in screen space and is
//! agnostic of the active rift state — it just draws what the
//! caller passes in.

use glam::Vec3;
use rift_dungeon::{Floor, NavGrid, RoomType, StairDir, SurfaceKind, Tile};
use rift_engine::ecs::components::{Boss, Enemy, LocalPlayer, Player, RemotePlayer, Transform};
use rift_engine::ui::im::{Banner, Color, Pos2, Rect, Ui};
use rift_ui_types::hud::{
    MinimapCell, MinimapEnemy, MinimapPartyMember, MinimapPlayer, MinimapProp, MinimapPropKind,
    MinimapRoomKind, MinimapStairDir, MinimapSurface, MinimapTileKind, MinimapView,
};

/// Top-right minimap. Walks the hecs world to build a flat
/// [`MinimapView`], then delegates to the pure widget in
/// [`rift_ui::hud::frame_minimap`]. All visual layout +
/// drawing lives there; this shim is host glue only.
pub fn render_minimap(
    ui: &mut Ui<'_>,
    world: &hecs::World,
    nav: &NavGrid,
    floor: Option<&Floor>,
    seen: &mut Vec<bool>,
    zone_title: &str,
    zone_detail: &str,
    show_full_extent: bool,
    zoom: f32,
    player_facing: Vec3,
    portal_pos: Option<Vec3>,
) -> rift_engine::ui::im::Rect {
    // Walkable mask, row-major.
    let mut walkable = Vec::with_capacity(nav.width * nav.depth);
    for z in 0..nav.depth {
        for x in 0..nav.width {
            walkable.push(nav.is_walkable(x, z));
        }
    }

    // Enemy pips — separate non-boss / boss so the widget can
    // size them independently.
    let mut enemies: Vec<MinimapEnemy> = Vec::new();
    for (_id, (t, _e, boss)) in world.query::<(&Transform, &Enemy, Option<&Boss>)>().iter() {
        enemies.push(MinimapEnemy {
            pos: (t.position.x, t.position.z),
            is_boss: boss.is_some(),
        });
    }

    let mut party: Vec<MinimapPartyMember> = Vec::new();
    for (_id, (t, _rp)) in world.query::<(&Transform, &RemotePlayer)>().iter() {
        let facing = t.rotation * Vec3::Z;
        party.push(MinimapPartyMember {
            pos: (t.position.x, t.position.z),
            facing: (facing.x, facing.z),
        });
    }

    // Local player + facing flattened to 2D nav-grid space.
    let player = world
        .query::<(&Transform, &Player, &LocalPlayer)>()
        .iter()
        .map(|(_, (t, _, _))| MinimapPlayer {
            pos: (t.position.x, t.position.z),
            facing: (player_facing.x, player_facing.z),
        })
        .next();

    let mut revealers: Vec<(f32, f32, f32)> = Vec::new();
    if let Some(player) = player {
        revealers.push((player.pos.0, player.pos.1, 12.0));
    }
    for member in &party {
        revealers.push((member.pos.0, member.pos.1, 10.0));
    }

    let mut cells: Vec<MinimapCell> = Vec::new();
    let mut props: Vec<MinimapProp> = Vec::new();
    if let Some(floor) = floor {
        let needed = floor.width * floor.depth;
        if seen.len() != needed {
            seen.clear();
            seen.resize(needed, false);
        }
        let mut visible = vec![false; needed];
        if show_full_extent {
            for z in 0..floor.depth {
                for x in 0..floor.width {
                    let idx = z * floor.width + x;
                    if floor.tiles[idx] != Tile::Wall {
                        seen[idx] = true;
                        visible[idx] = true;
                    }
                }
            }
        } else {
            update_minimap_visibility(floor, seen, &mut visible, &revealers);
        }

        cells.reserve(needed);
        for z in 0..floor.depth {
            for x in 0..floor.width {
                let idx = z * floor.width + x;
                let tile = floor.tiles[idx];
                cells.push(MinimapCell {
                    kind: minimap_tile_kind(tile),
                    surface: minimap_surface(floor.surface_at(x as f32, z as f32)),
                    room: minimap_room_kind_at(floor, x, z),
                    elevation: floor.elevation.get(idx).copied().unwrap_or_default(),
                    stair_dir: match tile {
                        Tile::Stair { dir } => Some(minimap_stair_dir(dir)),
                        _ => None,
                    },
                    explored: seen.get(idx).copied().unwrap_or(false),
                    visible: visible.get(idx).copied().unwrap_or(false),
                });
            }
        }

        props.reserve(floor.props.len());
        for prop in &floor.props {
            props.push(MinimapProp {
                pos: (prop.pos.x, prop.pos.z),
                kind: minimap_prop_kind(prop),
            });
        }
    }

    let view = MinimapView {
        zone_title,
        zone_detail,
        grid_width: nav.width as u32,
        grid_depth: nav.depth as u32,
        walkable: &walkable,
        cells: &cells,
        props: &props,
        focus: player.map(|p| p.pos),
        zoom,
        show_full_extent,
        portal: portal_pos.map(|p| (p.x, p.z)),
        enemies: &enemies,
        party: &party,
        player,
    };
    rift_ui::hud::frame_minimap(ui, &view)
}

fn update_minimap_visibility(
    floor: &Floor,
    seen: &mut [bool],
    visible: &mut [bool],
    revealers: &[(f32, f32, f32)],
) {
    for &(cx, cz, radius) in revealers {
        let min_x = (cx - radius).floor().max(0.0) as usize;
        let max_x = (cx + radius)
            .ceil()
            .min((floor.width.saturating_sub(1)) as f32) as usize;
        let min_z = (cz - radius).floor().max(0.0) as usize;
        let max_z = (cz + radius)
            .ceil()
            .min((floor.depth.saturating_sub(1)) as f32) as usize;
        let radius_sq = radius * radius;
        for z in min_z..=max_z {
            for x in min_x..=max_x {
                let dx = x as f32 - cx;
                let dz = z as f32 - cz;
                if dx * dx + dz * dz > radius_sq {
                    continue;
                }
                let idx = z * floor.width + x;
                if let Some(v) = visible.get_mut(idx) {
                    *v = true;
                }
                if let Some(s) = seen.get_mut(idx) {
                    *s = true;
                }
            }
        }
    }
}

fn minimap_tile_kind(tile: Tile) -> MinimapTileKind {
    match tile {
        Tile::Wall => MinimapTileKind::Wall,
        Tile::Floor => MinimapTileKind::Floor,
        Tile::Stair { .. } => MinimapTileKind::Stair,
    }
}

fn minimap_surface(surface: SurfaceKind) -> MinimapSurface {
    match surface {
        SurfaceKind::Sand => MinimapSurface::Sand,
        SurfaceKind::Stone => MinimapSurface::Stone,
        SurfaceKind::Wood => MinimapSurface::Wood,
        SurfaceKind::Metal => MinimapSurface::Metal,
        SurfaceKind::Grass => MinimapSurface::Grass,
        SurfaceKind::Bone => MinimapSurface::Bone,
    }
}

fn minimap_stair_dir(dir: StairDir) -> MinimapStairDir {
    match dir {
        StairDir::PosX => MinimapStairDir::PosX,
        StairDir::NegX => MinimapStairDir::NegX,
        StairDir::PosZ => MinimapStairDir::PosZ,
        StairDir::NegZ => MinimapStairDir::NegZ,
    }
}

fn minimap_room_kind(room_type: RoomType) -> MinimapRoomKind {
    match room_type {
        RoomType::Arena => MinimapRoomKind::Arena,
        RoomType::BossRoom => MinimapRoomKind::Boss,
        RoomType::PortalRoom => MinimapRoomKind::Portal,
        RoomType::Corridor => MinimapRoomKind::Corridor,
    }
}

fn minimap_room_kind_at(floor: &Floor, x: usize, z: usize) -> MinimapRoomKind {
    floor
        .rooms
        .iter()
        .find(|room| {
            x >= room.x && x < room.x + room.width && z >= room.z && z < room.z + room.depth
        })
        .map(|room| minimap_room_kind(room.room_type))
        .unwrap_or(MinimapRoomKind::None)
}

fn minimap_prop_kind(prop: &rift_dungeon::PlacedProp) -> MinimapPropKind {
    if prop.id == rift_dungeon::props::PropId::StashChest {
        return MinimapPropKind::Chest;
    }
    if prop.light {
        return MinimapPropKind::Light;
    }
    let Some((min, max)) = prop.collider_aabb() else {
        return MinimapPropKind::Decoration;
    };
    let area = (max.x - min.x).abs() * (max.z - min.z).abs();
    if area >= 0.75 {
        MinimapPropKind::LargeSolid
    } else {
        MinimapPropKind::SmallSolid
    }
}
/// Generic interaction prompt centred just below mid-screen, used by
/// the rift / hub portals. `text` is the message body (e.g.
/// "PRESS [F] TO ENTER THE RIFT").
pub fn render_hud_prompt(ui: &mut Ui<'_>, text: &str) {
    let theme = *ui.theme();
    let s = theme.scale;
    Banner::new(text)
        .text_size(12.0 * s)
        .text_color(Color::rgba(0.55, 0.78, 1.0, 1.0))
        .fill(Color::rgba(0.05, 0.08, 0.14, 0.92))
        .y_factor(0.62)
        .show(ui);
}

/// Difficulty step-up tooltip drawn just above the descend
/// F-prompt. Computes the deltas between the current floor's
/// `FloorConfig` and the next floor's so the player can read
/// what they're walking into before pressing F.
pub fn render_descend_tooltip(ui: &mut Ui<'_>, current_floor: u32) {
    use rift_dungeon::FloorConfig;
    use rift_engine::ui::im::{Frame, Pad, Vec2};

    if current_floor == 0 {
        return;
    }
    let next = current_floor + 1;
    let cur_cfg = FloorConfig::for_floor(current_floor);
    let next_cfg = FloorConfig::for_floor(next);

    let title = format!("DESCEND TO FLOOR {next}");
    let cur_count = cur_cfg.enemy_count();
    let next_count = next_cfg.enemy_count();
    let count_pct = if cur_count > 0 {
        ((next_count as f32 / cur_count as f32) - 1.0) * 100.0
    } else {
        0.0
    };
    let hp_pct = (next_cfg.enemy_health / cur_cfg.enemy_health - 1.0) * 100.0;
    let dmg_pct = (next_cfg.enemy_damage_mult / cur_cfg.enemy_damage_mult - 1.0) * 100.0;
    let speed_pct = (next_cfg.enemy_speed / cur_cfg.enemy_speed - 1.0) * 100.0;

    let lines: [(&str, String); 4] = [
        (
            "Enemies",
            format!(
                "{} \u{2192} {}  (+{:.0}%)",
                cur_count, next_count, count_pct
            ),
        ),
        (
            "Enemy HP",
            format!(
                "{:.0} \u{2192} {:.0}  (+{:.0}%)",
                cur_cfg.enemy_health, next_cfg.enemy_health, hp_pct
            ),
        ),
        (
            "Enemy DMG",
            format!(
                "{:.2}\u{00d7} \u{2192} {:.2}\u{00d7}  (+{:.0}%)",
                cur_cfg.enemy_damage_mult, next_cfg.enemy_damage_mult, dmg_pct
            ),
        ),
        (
            "Enemy speed",
            format!(
                "{:.1} \u{2192} {:.1}  (+{:.0}%)",
                cur_cfg.enemy_speed, next_cfg.enemy_speed, speed_pct
            ),
        ),
    ];

    let theme = *ui.theme();
    let screen = ui.screen_size();
    let s = theme.scale;
    let title_size = 13.0 * s;
    let row_size = 11.0 * s;
    let row_gap = 3.0 * s;
    let key_w_max = lines
        .iter()
        .map(|(k, _)| ui.measure_text(k, row_size))
        .fold(0.0_f32, f32::max);
    let val_w_max = lines
        .iter()
        .map(|(_, v)| ui.measure_text(v, row_size))
        .fold(0.0_f32, f32::max);
    let col_gap = 18.0 * s;
    let inner_w = ui
        .measure_header_text(&title, title_size)
        .max(key_w_max + col_gap + val_w_max);
    let inner_h = title_size + 6.0 * s + (lines.len() as f32) * (row_size + row_gap) - row_gap;
    let pad = Pad::symmetric(18.0 * s, 8.0 * s);
    let outer_w = inner_w + pad.left + pad.right;
    let outer_h = inner_h + pad.top + pad.bottom;
    let portal_prompt_y = screen.y * 0.62;
    let rect = Rect::from_xywh(
        (screen.x - outer_w) / 2.0,
        portal_prompt_y - outer_h - 8.0 * s,
        outer_w,
        outer_h,
    );
    let frame = Frame::panel(&theme)
        .with_fill(Color::rgba(0.06, 0.04, 0.10, 0.92))
        .with_padding(pad);
    frame.show(ui, rect, |ui, body| {
        let title_w = ui.measure_header_text(&title, title_size);
        ui.draw_header_text(
            Pos2::new(body.x() + (inner_w - title_w) * 0.5, body.y()),
            &title,
            title_size,
            Color::rgba(0.95, 0.75, 0.55, 1.0),
        );
        let mut row_y = body.y() + title_size + 6.0 * s;
        for (key, val) in &lines {
            ui.draw_text(
                Pos2::new(body.x(), row_y),
                key,
                row_size,
                Color::rgba(0.65, 0.72, 0.82, 1.0),
            );
            let val_w = ui.measure_text(val, row_size);
            ui.draw_text(
                Pos2::new(body.x() + inner_w - val_w, row_y),
                val,
                row_size,
                Color::rgba(0.95, 0.55, 0.45, 1.0),
            );
            row_y += row_size + row_gap;
        }
        let _ = Vec2::ZERO;
    });
}
