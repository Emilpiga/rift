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
pub mod loot_labels;
pub mod voting;
pub mod world_overlays;

// Re-export submodule fns so callers continue to write
// `hud::render_minimap(...)`, `hud::render_exit_vote(...)`, etc.
pub use exploration::{render_descend_tooltip, render_hud_prompt, render_minimap};
pub use loot_labels::render_loot_labels;
pub use voting::{render_exit_vote, render_shrine_progress};
pub use world_overlays::{
    render_boss_arrow, render_enemy_health_bars, render_remote_player_health_bars,
};

use glam::Vec3;
use rift_engine::ecs::components::{Effects, Health, LocalPlayer, Player};
use rift_engine::ui::im::{Banner, Color, Id, Pos2, ProgressBar, Rect, Tooltip, TooltipLine, Ui};

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

    // HP / Essence / XP vitals stack. Rendered by
    // [`rift_ui::hud::frame_vitals`] inside a carved-stone
    // plaque so the cluster reads as one surface; the host's
    // only job here is to flatten the live game state into the
    // view (server-authoritative HP percent, mirrored essence
    // fraction, experience progress) and pre-format the inline
    // labels.
    //
    // Server-authoritative HP: `world_sync` writes
    // `h.current = h.max * snapshot.health_pct`. The server
    // already accounts for gear / level bonuses in its
    // `hp_max`, so `h.current / h.max` is the right 0..1
    // fraction — adding `max_hp_bonus` to the denominator
    // would double-count.
    let hp_pct = world
        .query::<(&Health, &Player, &LocalPlayer)>()
        .iter()
        .map(|(_, (h, _, _))| h.current / h.max.max(0.001))
        .next()
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);
    let resource_pct = player_state.resource_pct.clamp(0.0, 1.0);
    let xp_pct = player_state.experience.progress().clamp(0.0, 1.0);

    let stats = player_state.stats();
    let hp_max = stats.max_hp;
    let hp_now = (hp_pct * hp_max).round();
    let hp_label = format!("{hp_now:.0} / {hp_max:.0}");
    let essence_max = stats.max_resource;
    let essence_now = (resource_pct * essence_max).round();
    let essence_label = format!("{essence_now:.0} / {essence_max:.0}");
    let xp_label = format!(
        "{} / {} XP",
        player_state.experience.current_xp,
        player_state.experience.xp_to_next_level()
    );
    let vitals_view = rift_ui_types::hud::HudVitalsView {
        hp_fraction: hp_pct,
        hp_label: hp_label.as_str(),
        essence_fraction: resource_pct,
        essence_label: essence_label.as_str(),
        xp_fraction: xp_pct,
        xp_label: xp_label.as_str(),
        level: player_state.experience.level,
    };
    // Sit flush against the top of the ability bar plaque so
    // the two HUD surfaces read as one column. The widget
    // exports the exact offset (ability bar plaque height +
    // bottom gap) so this stays in sync if either constant
    // moves.
    let plaque_rect =
        rift_ui::hud::frame_vitals(ui, &vitals_view, rift_ui::hud::VITALS_BOTTOM_OFFSET_BASE);
    let bar_x = plaque_rect.x();
    let bar_y = plaque_rect.y();

    // Active buff / debuff strip — anchored above the vitals
    // plaque.
    let local_effects: Vec<rift_engine::ecs::components::ActiveEffect> = world
        .query::<(&Effects, &LocalPlayer)>()
        .iter()
        .map(|(_, (e, _))| e.effects.clone())
        .next()
        .unwrap_or_default();
    if !local_effects.is_empty() {
        // Local HUD pips: bigger so the icon reads at a glance
        // and the rect is comfortably hoverable. Bumped from
        // 28px (engine default) so the strip's top edge moves
        // up accordingly.
        let pip = 40.0 * s;
        draw_effect_pip_strip(
            ui,
            Pos2::new(bar_x, bar_y - pip - 6.0 * s),
            &local_effects,
            pip,
            true,
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
    // Current essence pool in raw units (`resource_pct *
    // max_resource`). Drives the unaffordable-slot tint and the
    // cost line in the tooltip.
    current_essence: f32,
    // Resolved character sheet for damage / crit tooltip math.
    // Reads only — used to reproduce the cast pipeline's
    // `base_damage × damage_scalar × ability_mult` against the
    // current gear / attribute state so the tooltip damage
    // number matches what the player will actually deal.
    stats: &rift_game::stats::CharacterStats,
    // Per-ability gear modifiers (extra projectiles, cooldown
    // scalar, damage scalar, transforms). Folded into the
    // tooltip numbers so legendary effects (e.g.
    // Cleavebreaker's `+2 projectiles to Fireball Volley`)
    // show up on the displayed damage / projectile / cooldown
    // lines instead of being invisible until the player
    // notices the cast difference.
    ability_mods: &rift_game::loot::ability_mods::AbilityMods,
    // Sink for the plaque rect so the next frame's combat tick
    // can recognise a click that landed on the bar as a UI
    // interaction (e.g. swap-slot opens the spellbook) rather
    // than a basic-attack cast.
    hud_consume_rects: &mut Vec<Rect>,
    targeting_slot: Option<usize>,
) -> Option<usize> {
    const AB_KEYS: [&str; 6] = ["LMB", "1", "2", "3", "4", "5"];

    // Build the flat view the widget consumes. Owned strings
    // (damage / crit / cost lines) live in the slot tooltips
    // themselves — same allocation lifetime as the view, so
    // they're trivially valid for the single `frame_ability_bar`
    // call.
    let abbrev_chars: Vec<Option<char>> = abilities
        .slots
        .iter()
        .map(|slot| {
            slot.as_ref().and_then(|state| {
                if state.ability.icon.is_some() {
                    None
                } else {
                    ability_abbrev(state.ability.name).chars().next()
                }
            })
        })
        .collect();

    let mut slots = std::array::from_fn::<rift_ui_types::hud::AbilitySlotView<'_>, 6, _>(|i| {
        let slot = &abilities.slots[i];
        let unlocked = rift_game::loadout::is_slot_unlocked(i, player_level);
        let unlock_level = rift_game::loadout::SLOT_UNLOCK_LEVELS[i];
        let (icon, fallback_glyph, cooldown_remaining, affordable, tooltip) = match slot {
            Some(state) if unlocked => {
                // Per-ability gear modifiers — folded into the
                // tooltip numbers so legendary effects (extra
                // projectiles, cooldown reductions, damage
                // amplifies, behaviour transforms) are
                // visible at a glance.
                let aid = state.ability.id;
                let dmg_mult = ability_mods.damage_for(aid);
                let cd_mult = ability_mods.cooldown_for(aid);
                let extra_projectiles = ability_mods.extra_projectiles_for(aid);
                let transform = ability_mods.transform_for(aid);

                let effective_cd = state.ability.cooldown * cd_mult;
                let total_projectiles = state
                    .ability
                    .projectile_count()
                    .saturating_add(extra_projectiles);

                let cd = (1.0 - state.cooldown_progress()).clamp(0.0, 1.0);
                let affordable = state.ability.resource_cost <= current_essence + 1e-3;

                let per_hit = stats.ability_effective_damage(&state.ability) * dmg_mult;
                let avg = stats.ability_avg_damage(&state.ability) * dmg_mult;
                let damage_line = if per_hit > 0.01 {
                    use rift_game::abilities::AbilityKind;
                    let unit = match state.ability.kind {
                        AbilityKind::Channel { .. } | AbilityKind::AoeZone { .. } => " / tick",
                        _ => "",
                    };
                    if effective_cd > 0.0 {
                        Some(format!(
                            "CD: {:.1}s  |  {:.0}{} damage",
                            effective_cd, per_hit, unit
                        ))
                    } else {
                        Some(format!("{:.0}{} damage", per_hit, unit))
                    }
                } else if effective_cd > 0.0 {
                    Some(format!("CD: {:.1}s", effective_cd))
                } else {
                    None
                };
                let crit_line = if per_hit > 0.01 && stats.crit_chance > 0.001 {
                    Some(format!(
                        "~{:.0} avg  ({:.0}% crit, +{:.0}% dmg)",
                        avg,
                        stats.crit_chance * 100.0,
                        stats.crit_damage * 100.0
                    ))
                } else {
                    None
                };
                let cost_line = if state.ability.channel_cost_per_sec > 0.0 {
                    Some(format!(
                        "Essence: {:.0} / sec",
                        state.ability.channel_cost_per_sec
                    ))
                } else if state.ability.resource_cost > 0.0 {
                    Some(format!("Essence: {:.0}", state.ability.resource_cost))
                } else {
                    None
                };
                let projectiles_line = if total_projectiles > 1 {
                    if extra_projectiles > 0 {
                        Some(format!(
                            "Projectiles: {} (+{} from gear)",
                            total_projectiles, extra_projectiles
                        ))
                    } else {
                        Some(format!("Projectiles: {}", total_projectiles))
                    }
                } else {
                    None
                };

                let transform_line = transform.map(|v| {
                    use rift_game::loot::AbilityVariant;
                    let desc = match v {
                        AbilityVariant::FireballToBeam => "Fireball channels into a piercing beam",
                        AbilityVariant::FrostRayShatter => {
                            "Frost Ray shatters into icy shards on release"
                        }
                        AbilityVariant::WhirlwindVortex => "Whirlwind pulls enemies into a vortex",
                    };
                    format!("★ {}", desc)
                });

                // Bonus summary — only show the parts that
                // actually contributed (deltas != neutral).
                let bonus_line = {
                    let mut parts: Vec<String> = Vec::new();
                    if (dmg_mult - 1.0).abs() > 1.0e-3 {
                        parts.push(format!("{:+.0}% damage", (dmg_mult - 1.0) * 100.0));
                    }
                    if (cd_mult - 1.0).abs() > 1.0e-3 {
                        // Negative cd_mult delta means *faster*
                        // cooldown — phrase it that way.
                        parts.push(format!("{:+.0}% cooldown", (cd_mult - 1.0) * 100.0));
                    }
                    if parts.is_empty() {
                        None
                    } else {
                        Some(format!("★ {}", parts.join(", ")))
                    }
                };

                let tip = rift_ui_types::hud::AbilityTooltip {
                    name: state.ability.name,
                    description: state.ability.description,
                    damage_line,
                    crit_line,
                    cost_line,
                    cost_affordable: affordable,
                    projectiles_line,
                    transform_line,
                    bonus_line,
                };
                (
                    state.ability.icon,
                    abbrev_chars[i],
                    cd,
                    affordable,
                    Some(tip),
                )
            }
            _ => (None, None, 0.0, true, None),
        };

        rift_ui_types::hud::AbilitySlotView {
            key_hint: AB_KEYS[i],
            icon,
            fallback_glyph,
            cooldown_remaining,
            unlocked,
            unlock_level,
            affordable,
            selected: targeting_slot == Some(i),
            tooltip,
        }
    });
    // Silence the inevitable "mut not needed" if all six slots
    // happen to be empty — the array is initialised in-place,
    // but the field-by-field assignment in `from_fn` keeps the
    // binding "mutable" semantically.
    let _ = &mut slots;
    // Passive (Space) tile — Evasive Roll lives on
    // `AbilitySlot::roll` outside the 6-slot loadout. Build
    // the same view shape so the widget renders it with the
    // same chrome and tooltip pipeline.
    let passive_abbrev: Option<char> = abilities.roll.as_ref().and_then(|state| {
        if state.ability.icon.is_some() {
            None
        } else {
            ability_abbrev(state.ability.name).chars().next()
        }
    });
    let passive = abilities.roll.as_ref().map(|state| {
        let cd = (1.0 - state.cooldown_progress()).clamp(0.0, 1.0);
        let effective_cd = state.ability.cooldown;
        let damage_line = if effective_cd > 0.0 {
            Some(format!("CD: {:.1}s", effective_cd))
        } else {
            None
        };
        let tip = rift_ui_types::hud::AbilityTooltip {
            name: state.ability.name,
            description: state.ability.description,
            damage_line,
            crit_line: None,
            cost_line: None,
            cost_affordable: true,
            projectiles_line: None,
            transform_line: None,
            bonus_line: None,
        };
        rift_ui_types::hud::AbilitySlotView {
            key_hint: "SPACE",
            icon: state.ability.icon,
            fallback_glyph: passive_abbrev,
            cooldown_remaining: cd,
            unlocked: true,
            unlock_level: 1,
            affordable: true,
            selected: false,
            tooltip: Some(tip),
        }
    });
    let view = rift_ui_types::hud::AbilityBarView { slots, passive };

    let result = rift_ui::hud::frame_ability_bar(ui, &view).map(|action| match action {
        rift_ui_types::hud::HudAction::AbilitySlotClicked(idx) => idx,
    });

    // Stash the plaque rect so the next frame's combat tick
    // can recognise a click on the bar as a UI interaction and
    // skip the basic-attack cast. Computed from the same
    // baseline constants the widget uses internally so the
    // rect tracks the live layout one-to-one.
    {
        let theme = *ui.theme();
        let s = theme.scale;
        let screen = ui.screen_size();
        let plaque_w = rift_ui::hud::PLAQUE_W_BASE * s;
        let plaque_h = rift_ui::hud::PLAQUE_H_BASE * s;
        let plaque_x = (screen.x - plaque_w) * 0.5;
        let plaque_y = screen.y - plaque_h - rift_ui::hud::BOTTOM_GAP_BASE * s;
        hud_consume_rects.push(Rect::from_xywh(plaque_x, plaque_y, plaque_w, plaque_h));
    }

    result
}

/// Screen-space buff / debuff pip strip. Anchors the strip's
/// top-left at `top_left` and renders one pip per active effect
/// (icon + cooldown drain).
///
/// `pip_size` lets the local HUD use a chunkier hover-friendly
/// size while the world-overlay variants keep the smaller pip
/// that fits above an enemy's HP bar without dwarfing it.
///
/// When `interactive` is `true`, each pip registers an
/// [`Ui::interact_hover`] and shows a tooltip describing the
/// effect (name, remaining duration, and a one-line summary
/// per `EffectKind`). Pass `false` for purely visual strips.
pub(crate) fn draw_effect_pip_strip(
    ui: &mut Ui<'_>,
    top_left: Pos2,
    effects: &[rift_engine::ecs::components::ActiveEffect],
    pip_size: f32,
    interactive: bool,
) {
    let pip_gap = (pip_size * 0.12).max(2.0);

    let mut x = top_left.x;
    let y = top_left.y;
    // Tooltip is drawn in a second pass after every pip rect
    // is laid out, so a hovered pip's tooltip sits on top of
    // any later pip in the same strip.
    let mut tooltip_for: Option<(usize, Pos2)> = None;
    for (i, eff) in effects.iter().enumerate() {
        let Some(def) = rift_game::effects::lookup(eff.id) else {
            continue;
        };
        let pip = Rect::from_xywh(x, y, pip_size, pip_size);
        ui.draw_rect(
            Rect::from_xywh(pip.x() - 1.0, pip.y() - 1.0, pip_size + 2.0, pip_size + 2.0),
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
            let drain_h = pip_size * frac;
            ui.draw_rect(
                Rect::from_xywh(pip.x(), pip.y(), pip_size, drain_h),
                Color::rgba(0.0, 0.0, 0.0, 0.55),
            );
        }
        // Remaining seconds — small numeric label so the player
        // doesn't have to estimate the drain height. Skip when
        // the pip is tiny (world-overlay variants) so the text
        // doesn't smear over the icon.
        if pip_size >= 26.0 && eff.remaining > 0.05 {
            let secs = eff.remaining.ceil() as i32;
            let lbl = if secs >= 10 {
                format!("{secs}")
            } else {
                format!("{:.1}", eff.remaining)
            };
            let lbl_size = (pip_size * 0.30).max(10.0);
            let lw = ui.measure_text(&lbl, lbl_size);
            // Outline-ish: draw a slightly offset shadow then
            // the foreground so the digit reads against either
            // a bright icon or a dark drain.
            let lx = pip.max.x - lw - 2.0;
            let ly = pip.max.y - lbl_size - 1.0;
            ui.draw_text(
                Pos2::new(lx + 1.0, ly + 1.0),
                &lbl,
                lbl_size,
                Color::rgba(0.0, 0.0, 0.0, 0.85),
            );
            ui.draw_text(
                Pos2::new(lx, ly),
                &lbl,
                lbl_size,
                Color::rgba(1.0, 1.0, 1.0, 0.95),
            );
        }
        if interactive {
            let id = Id::root("hud_effect_pip").child((eff.id as u32, i));
            let hovered = ui.interact_hover(id, pip);
            if hovered && tooltip_for.is_none() {
                tooltip_for = Some((i, Pos2::new(pip.x(), pip.max.y + 6.0)));
            }
        }
        x += pip_size + pip_gap;
    }

    if let Some((i, pos)) = tooltip_for {
        if let Some(eff) = effects.get(i) {
            if let Some(def) = rift_game::effects::lookup(eff.id) {
                draw_effect_tooltip(ui, pos, eff, def);
            }
        }
    }
}

/// One-shot tooltip for a single hovered effect pip. Pulled
/// out of `draw_effect_pip_strip` so the strip body stays
/// readable; called once per frame at most.
fn draw_effect_tooltip(
    ui: &mut Ui<'_>,
    pos: Pos2,
    eff: &rift_engine::ecs::components::ActiveEffect,
    def: &rift_game::effects::EffectDef,
) {
    use rift_game::effects::EffectKind;
    let theme = *ui.theme();
    let [r, g, b] = def.color;
    let header_col = Color::rgba(r, g, b, 1.0);

    // Owned strings live in this Vec so the borrow handed to
    // `TooltipLine` stays valid for the `Tooltip::show` call.
    let mut texts: Vec<(String, f32, Color)> = Vec::new();
    texts.push((
        format!("{:.1}s remaining", eff.remaining.max(0.0)),
        theme.fonts.size_md,
        theme.colors.text_dim,
    ));
    for kind in def.effects {
        let line = match kind {
            EffectKind::DamageOverTime { dps, interval } => {
                format!("{:.0} damage / sec (every {:.1}s)", dps, interval)
            }
            EffectKind::HealOverTime { hps, interval } => {
                format!("{:.0} heal / sec (every {:.1}s)", hps, interval)
            }
            EffectKind::MoveSpeedMult(m) => {
                let pct = ((m - 1.0) * 100.0).round() as i32;
                if pct >= 0 {
                    format!("+{pct}% movement speed")
                } else {
                    format!("{pct}% movement speed")
                }
            }
            EffectKind::IncomingDamageMult(m) => {
                let pct = ((m - 1.0) * 100.0).round() as i32;
                if pct >= 0 {
                    format!("+{pct}% damage taken")
                } else {
                    format!("{pct}% damage taken")
                }
            }
            EffectKind::HealingReceivedMult(m) => {
                let pct = ((m - 1.0) * 100.0).round() as i32;
                if pct >= 0 {
                    format!("+{pct}% healing received")
                } else {
                    format!("{pct}% healing received")
                }
            }
        };
        texts.push((line, theme.fonts.size_md, theme.colors.text));
    }
    let lines: Vec<TooltipLine<'_>> = texts
        .iter()
        .map(|(s, sz, c)| TooltipLine::new(s.as_str(), *sz, *c))
        .collect();
    Tooltip::new()
        .header(def.name)
        .header_color(header_col)
        .min_width(180.0)
        .pad(8.0)
        .show(ui, pos, &lines);
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
