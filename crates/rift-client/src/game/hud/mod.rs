//! In-game HUD. The module is organised by concern:
//!
//! - [`render_hud`] — combat HUD: HP/XP bars, level pip, effect
//!   strip, rift progress, level-up banner, hub / portal banners.
//! - [`render_ability_bar`] — bottom-center action bar.
//! - [`render_loot_prompt`], [`render_damage_flash`], [`render_fade_to_black`]
//!   — small per-frame combat overlays.
//! - [`world_overlays`] — billboards anchored to world entities
//!   (boss arrow, enemy / ally HP bars).
//! - [`exploration`] — minimap, F-prompts, descend tooltip.
//! - [`voting`] — shrine progress, exit / descend vote panel.
//! - [`draw_world_loading_overlay`] — full-screen "Entering World"
//!   cover during hub↔rift transitions.
//!
//! Screen-space pixel literals are baseline values for a 1080p
//! reference frame; each render fn multiplies them by
//! `theme.scale` so the same call site renders 1:1 on 720p, 1080p,
//! and 4K.

pub mod exploration;
pub mod voting;
pub mod world_overlays;

// Re-export submodule fns so callers continue to write
// `hud::render_minimap(...)`, `hud::render_exit_vote(...)`, etc.
pub use exploration::{render_descend_tooltip, render_hud_prompt, render_minimap};
pub use voting::{render_exit_vote, render_shrine_progress};
pub use world_overlays::{
    render_boss_arrow, render_enemy_health_bars, render_remote_player_health_bars,
};

use glam::Vec3;
use rift_engine::ecs::components::{Effects, Health, LocalPlayer, Player};
use rift_engine::ui::im::{
    hp_color, Banner, Color, Id, ItemSlot, Pos2, ProgressBar, Rect, Tooltip, TooltipLine, Ui,
};

use super::rift_state::RiftState;
use crate::game::PlayerState;
use rift_game::abilities::AbilitySlot;

// `Vec3` is referenced by submodules but kept in scope for the
// occasional `Color::rgba` helper used inside this file.
const _: Option<Vec3> = None;

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
    // Single source of truth for "pixel literals are unscaled
    // baseline numbers". Every fixed dimension below is
    // multiplied by `s` so the HUD reads the same on 720p,
    // 1080p, and 4K. `theme.scale` is set once per frame in
    // `Ui::begin` from min(screen_w/1920, screen_h/1080).
    let s = theme.scale;

    // HP + XP bars: stacked, centered above the ability bar so the
    // player's vital stats sit right under their character.
    //
    // Server-authoritative HP: `world_sync` writes
    // `h.current = h.max * snapshot.health_pct`, where
    // `health_pct = server.hp / server.hp_max`. Because the
    // server already accounts for gear / level bonuses in its
    // `hp_max`, the client's `h.current / h.max` is the right
    // 0..1 fraction. Adding `max_hp_bonus` to the denominator
    // here would double-count the bonus and prevent the bar
    // from ever reaching 100% on a geared character.
    let hp_pct = world
        .query::<(&Health, &Player, &LocalPlayer)>()
        .iter()
        .map(|(_, (h, _, _))| h.current / h.max.max(0.001))
        .next()
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);

    let bar_w = 360.0 * s;
    let bar_h = 22.0 * s;
    let xp_h = 9.0 * s;
    let bars_total_h = bar_h + 2.0 * s + xp_h;
    let bar_x = (sw - bar_w) / 2.0;
    let bar_y = sh - 80.0 * s - 16.0 * s - bars_total_h;

    // HP bar.
    ProgressBar::new(hp_pct)
        .fill(hp_color(hp_pct))
        .border(Color::rgba(0.30, 0.30, 0.32, 0.9))
        .show(ui, Rect::from_xywh(bar_x, bar_y, bar_w, bar_h));
    let hp_max = player_state.stats().max_hp;
    let hp_now = (hp_pct * hp_max).round();
    let hp_label = format!("{hp_now:.0} / {hp_max:.0}");
    let hp_text_size = 13.0 * s;
    let hp_text_w = ui.measure_text(&hp_label, hp_text_size);
    ui.draw_text(
        Pos2::new(
            bar_x + (bar_w - hp_text_w) * 0.5,
            bar_y + (bar_h - hp_text_size) * 0.5,
        ),
        &hp_label,
        hp_text_size,
        Color::rgba(0.96, 0.96, 0.98, 0.95),
    );

    // XP bar (slimmer, directly under the HP bar).
    let xp_pct = player_state.experience.progress().clamp(0.0, 1.0);
    let xp_y = bar_y + bar_h + 2.0 * s;
    let xp_now = player_state.experience.current_xp;
    let xp_need = player_state.experience.xp_to_next_level();
    let xp_label = format!("{xp_now} / {xp_need} XP");
    ProgressBar::new(xp_pct)
        .fill(Color::rgba(0.45, 0.30, 0.85, 0.95))
        .border(Color::rgba(0.30, 0.30, 0.32, 0.9))
        .rounded(false)
        .show(ui, Rect::from_xywh(bar_x, xp_y, bar_w, xp_h));
    let xp_text_size = 11.0 * s;
    let xp_text_w = ui.measure_text(&xp_label, xp_text_size);
    ui.draw_text(
        Pos2::new(bar_x + (bar_w - xp_text_w) * 0.5, xp_y - 1.0 * s),
        &xp_label,
        xp_text_size,
        Color::rgba(0.92, 0.92, 0.96, 0.95),
    );

    // Level pip floats just to the left of the HP bar.
    let level_text = format!("Lv.{}", player_state.experience.level);
    ui.draw_text(
        Pos2::new(bar_x - 50.0 * s, bar_y + 4.0 * s),
        &level_text,
        15.0 * s,
        Color::rgba(0.92, 0.92, 0.92, 1.0),
    );

    // Active buff / debuff strip.
    let local_effects: Vec<rift_engine::ecs::components::ActiveEffect> = world
        .query::<(&Effects, &LocalPlayer)>()
        .iter()
        .map(|(_, (e, _))| e.effects.clone())
        .next()
        .unwrap_or_default();
    if !local_effects.is_empty() {
        draw_effect_pip_strip(
            ui,
            Pos2::new(bar_x, bar_y - 34.0 * s),
            &local_effects,
        );
    }

    // Level-up banner.
    if level_up_flash > 0.001 {
        let banner = format!("LEVEL UP!  Lv.{}", player_state.experience.level);
        let alpha = level_up_flash.min(1.0);
        Banner::new(&banner)
            .floating()
            .text_size(theme.fonts.size_xl)
            .text_color(Color::rgba(1.0, 0.85, 0.30, alpha))
            .y_factor(0.30)
            .show(ui);
    }

    // Rift progress bar (top-center) or hub label.
    if !in_hub {
        let prog_pct = rift.progress_percent() / 100.0;
        let prog_w = 300.0 * s;
        let prog_h = 16.0 * s;
        let prog_x = (sw - prog_w) / 2.0;
        let prog_y = 10.0 * s;
        ProgressBar::new(prog_pct)
            .fill(theme.colors.accent)
            .track(Color::rgba(0.10, 0.10, 0.10, 0.80))
            .border(theme.colors.border)
            .show(ui, Rect::from_xywh(prog_x, prog_y, prog_w, prog_h));

        let floor_w = 40.0 * s;
        let floor_h = 20.0 * s;
        let floor_pct = (rift.floor as f32 / 10.0).clamp(0.0, 1.0);
        ProgressBar::new(floor_pct)
            .fill(Color::rgba(0.80, 0.70, 0.20, 0.90))
            .track(Color::rgba(0.20, 0.20, 0.30, 0.80))
            .border(theme.colors.border)
            .pips(10)
            .show(
                ui,
                Rect::from_xywh(sw - floor_w - 10.0 * s, 10.0 * s, floor_w, floor_h),
            );
    } else {
        Banner::new("THE HUB")
            .pill()
            .text_size(13.0 * s)
            .text_color(Color::rgba(0.7, 0.85, 1.0, 1.0))
            .min_width(120.0 * s)
            .y_factor(10.0 * s / sh)
            .show(ui);
    }

    if rift.floor_complete {
        Banner::new("ENTER THE PORTAL")
            .pill()
            .fill(Color::rgba(0.10, 0.15, 0.25, 0.85))
            .text_size(12.0 * s)
            .text_color(theme.colors.accent)
            .min_width(200.0 * s)
            .y_factor(35.0 * s / sh)
            .show(ui);
    }
}

/// Fullscreen black quad used by the death→hub fade transition.
pub fn render_fade_to_black(ui: &mut Ui<'_>, alpha: f32) {
    let a = alpha.clamp(0.0, 1.0);
    if a <= 0.001 {
        return;
    }
    ui.draw_rect(ui.screen_rect(), Color::rgba(0.0, 0.0, 0.0, a));
}

/// Loot-pickup prompt — same chrome as [`render_hud_prompt`] but
/// the text colour follows the item's tier (rarity) so the player
/// can read the rarity at a glance.
pub fn render_loot_prompt(ui: &mut Ui<'_>, text: &str, color: Color) {
    let theme = *ui.theme();
    let s = theme.scale;
    Banner::new(text)
        .text_size(12.0 * s)
        .text_color(color)
        .fill(Color::rgba(0.05, 0.05, 0.07, 0.92))
        .y_factor(0.70)
        .show(ui);
}

/// Red screen-edge vignette shown briefly after the player takes damage.
/// `strength` is in [0, 1]; the centre stays clear so combat readability
/// is preserved.
pub fn render_damage_flash(ui: &mut Ui<'_>, strength: f32) {
    let s = strength.clamp(0.0, 1.0);
    if s <= 0.001 {
        return;
    }
    let screen = ui.screen_size();
    let sw = screen.x;
    let sh = screen.y;
    let t = 22.0 + 28.0 * s;
    const STEPS: i32 = 4;
    for i in 0..STEPS {
        let f = 1.0 - (i as f32 / STEPS as f32);
        let alpha = (0.22 * s * f).clamp(0.0, 0.32);
        let band = t * (1.0 - i as f32 / STEPS as f32);
        let col = Color::rgba(0.78, 0.05, 0.05, alpha);
        ui.draw_rect(Rect::from_xywh(0.0, 0.0, sw, band), col);
        ui.draw_rect(Rect::from_xywh(0.0, sh - band, sw, band), col);
        ui.draw_rect(Rect::from_xywh(0.0, 0.0, band, sh), col);
        ui.draw_rect(Rect::from_xywh(sw - band, 0.0, band, sh), col);
    }
}

/// Render the ability bar (bottom-center) via the immediate-mode UI.
///
/// Returns `Some(slot_index)` if the player clicked one of the
/// six bar slots this frame. Caller uses that to open the
/// spellbook with the slot pre-targeted.
pub fn render_ability_bar(
    ui: &mut Ui<'_>,
    abilities: &AbilitySlot,
    player_level: u32,
    targeting_slot: Option<usize>,
) -> Option<usize> {
    const AB_SIZE_BASE: f32 = 64.0;
    const AB_GAP_BASE: f32 = 6.0;
    const AB_KEYS: [&str; 6] = ["LMB", "1", "2", "3", "4", "5"];

    let theme = *ui.theme();
    let s = theme.scale;
    let ab_size = AB_SIZE_BASE * s;
    let ab_gap = AB_GAP_BASE * s;
    let screen = ui.screen_size();
    let ab_total_w = 6.0 * ab_size + 5.0 * ab_gap;
    let ab_x = (screen.x - ab_total_w) * 0.5;
    let ab_y = screen.y - ab_size - 16.0 * s;

    let mut hovered_idx: Option<usize> = None;
    let mut clicked_idx: Option<usize> = None;

    for (i, slot) in abilities.slots.iter().enumerate() {
        let pos = Pos2::new(ab_x + i as f32 * (ab_size + ab_gap), ab_y);
        let id = Id::root("ability_bar").child(i);
        let slot_unlocked = rift_game::loadout::is_slot_unlocked(i, player_level);

        let mut sb = ItemSlot::new(ab_size).key_label(AB_KEYS[i]);
        if targeting_slot == Some(i) {
            sb = sb.selected(true);
        }
        if !slot_unlocked {
            sb = sb
                .enabled(false)
                .fallback_glyph('\u{1F512}')
                .fallback_color(Color::rgba(0.55, 0.25, 0.25, 0.9));
        } else if let Some(state) = slot {
            let remaining = 1.0 - state.cooldown_progress();
            sb = sb.cooldown(remaining);
            if let Some(name) = state.ability.icon {
                sb = sb.icon(name);
            } else {
                let abbrev = ability_abbrev(state.ability.name);
                if let Some(ch) = abbrev.chars().next() {
                    sb = sb
                        .fallback_glyph(ch)
                        .fallback_color(Color::rgba(0.6, 0.85, 1.0, 0.95));
                }
            }
        }

        let resp = sb.show(ui, pos, id);
        if resp.hovered && slot.is_some() && slot_unlocked {
            hovered_idx = Some(i);
        }
        if resp.clicked && slot_unlocked {
            clicked_idx = Some(i);
        }

        if !slot_unlocked {
            let lvl = rift_game::loadout::SLOT_UNLOCK_LEVELS[i];
            ui.draw_text(
                Pos2::new(pos.x, pos.y + ab_size + 2.0 * s),
                format!("Lv {lvl}").as_str(),
                theme.fonts.size_sm,
                Color::rgba(0.65, 0.30, 0.30, 0.9),
            );
        }
    }

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
                TooltipLine::new(
                    state.ability.name,
                    theme.fonts.size_md,
                    Color::rgba(1.0, 0.9, 0.5, 1.0),
                ),
                TooltipLine::new(
                    state.ability.description,
                    theme.fonts.size_sm,
                    Color::rgba(0.8, 0.8, 0.8, 1.0),
                ),
                TooltipLine::new(
                    stats.as_str(),
                    theme.fonts.size_sm,
                    Color::rgba(0.6, 0.8, 1.0, 0.9),
                ),
            ];
            if let Some(ref p) = proj {
                lines.push(TooltipLine::new(
                    p.as_str(),
                    theme.fonts.size_sm,
                    Color::rgba(0.7, 0.7, 0.7, 0.8),
                ));
            }
            let slot_rect = Rect::from_xywh(
                ab_x + idx as f32 * (ab_size + ab_gap),
                ab_y,
                ab_size,
                ab_size,
            );
            Tooltip::new()
                .min_width(220.0)
                .anchor_to(slot_rect)
                .show(
                    ui,
                    Pos2::new(slot_rect.x(), slot_rect.y() - 90.0 * s),
                    &lines,
                );
        }
    }

    clicked_idx
}

/// Screen-space buff / debuff pip strip. Anchors the strip's
/// top-left at `top_left` and renders one pip per active effect
/// (icon + cooldown drain). Used by the local-player HUD and
/// re-exported to the world-overlay module so enemy / ally pips
/// share the exact same visual.
pub(crate) fn draw_effect_pip_strip(
    ui: &mut Ui<'_>,
    top_left: Pos2,
    effects: &[rift_engine::ecs::components::ActiveEffect],
) {
    const PIP_SIZE: f32 = 28.0;
    const PIP_GAP: f32 = 3.0;

    let mut x = top_left.x;
    let y = top_left.y;
    for eff in effects {
        let Some(def) = rift_game::effects::lookup(eff.id) else {
            continue;
        };
        let pip = Rect::from_xywh(x, y, PIP_SIZE, PIP_SIZE);
        ui.draw_rect(
            Rect::from_xywh(pip.x() - 1.0, pip.y() - 1.0, PIP_SIZE + 2.0, PIP_SIZE + 2.0),
            Color::rgba(0.0, 0.0, 0.0, 0.85),
        );
        if let Some(icon) = def.icon {
            ui.draw_icon(pip, icon, Color::rgba(1.0, 1.0, 1.0, 1.0));
        } else {
            let [r, g, b] = def.color;
            ui.draw_rect(pip, Color::rgba(r, g, b, 0.95));
        }
        let frac = if eff.duration > 0.001 {
            (eff.remaining / eff.duration).clamp(0.0, 1.0)
        } else {
            0.0
        };
        if frac > 0.0 {
            let drain_h = PIP_SIZE * frac;
            ui.draw_rect(
                Rect::from_xywh(pip.x(), pip.y(), PIP_SIZE, drain_h),
                Color::rgba(0.0, 0.0, 0.0, 0.55),
            );
        }
        x += PIP_SIZE + PIP_GAP;
    }
}

/// Compute a 1-2 letter abbreviation from an ability name for
/// the action-bar fallback glyph when no icon is set. Skips
/// short stop-words ("of", "for", "the") so "Mark for Death"
/// becomes `MD` instead of `MF`.
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
pub fn draw_world_loading_overlay(
    renderer: &mut rift_engine::Renderer,
    progress: f32,
    label: &str,
) {
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
