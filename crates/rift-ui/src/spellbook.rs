//! Spellbook loadout editor widget.
//!
//! Pure rendering/action layer. The client host builds a
//! [`SpellbookView`] from game ability data and owns the
//! persistent [`SpellbookState`].

use rift_ui_im::{
    widgets::{Button, ItemSlot, PanelHeader},
    Color, Frame, Id, Layer, Pad, Pos2, Rect, Ui,
};
use rift_ui_types::spellbook::{
    SpellbookAbilityView, SpellbookAction, SpellbookState, SpellbookView,
};

use crate::icons::{draw_placeholder_icon, icon_rect_left, UiIcon};

const CATEGORY_ALL: u8 = 0;

pub fn frame_spellbook(
    ui: &mut Ui<'_>,
    view: &SpellbookView<'_>,
    state: &mut SpellbookState,
) -> Option<SpellbookAction> {
    if !state.open {
        return None;
    }

    let theme = *ui.theme();
    let screen = ui.screen_size();
    const PANEL_W_BASE: f32 = 960.0;
    const PANEL_H_BASE: f32 = 620.0;
    let margin = theme.spacing.panel_margin();
    let panel_w = (PANEL_W_BASE * theme.scale).min(screen.x - margin * 2.0);
    let panel_h = (PANEL_H_BASE * theme.scale).min(screen.y - margin * 2.0);
    let fit = (panel_w / PANEL_W_BASE)
        .min(panel_h / PANEL_H_BASE)
        .max(0.4);

    ui.with_layer(Layer::Modal, |ui| {
        ui.draw_rect(
            Rect::from_xywh(0.0, 0.0, screen.x, screen.y),
            Color::rgba(0.0, 0.0, 0.0, 0.58),
        );
    });

    let panel = Rect::from_xywh(
        (screen.x - panel_w) * 0.5,
        (screen.y - panel_h) * 0.5,
        panel_w,
        panel_h,
    );
    Frame::stone(&theme)
        .with_padding(Pad::all(0.0))
        .with_radius(5.0 * fit)
        .show_only(ui, panel);

    let inner_pad = theme.spacing.inner_pad();
    let section_gap = theme.spacing.section_gap();
    let row_gap = theme.spacing.row_gap();

    let hint = match state.target_slot {
        Some(i) => format!("Pick an ability for slot {}.", i + 1),
        None => "Click an ability, then click a bar slot to equip it.".to_string(),
    };
    let lvl_text = format!("Lv {}", view.player_level);

    let header_h = inner_pad + theme.fonts.size_lg + row_gap + theme.fonts.size_sm + section_gap;
    PanelHeader::new("SPELLBOOK")
        .subtitle(&hint)
        .right_text(&lvl_text)
        .show(
            ui,
            Rect::from_xywh(panel.x(), panel.y(), panel.width(), header_h),
        );
    let rail_w = 148.0 * fit;
    let bar_strip_h = 110.0 * fit;
    let detail_h = 136.0 * fit;
    let header_content_gap = 12.0 * fit;
    let content_y = panel.y() + header_h + header_content_gap;

    let rail_rect = Rect::from_xywh(
        panel.x() + inner_pad,
        content_y,
        rail_w,
        panel.max.y - content_y - bar_strip_h - detail_h - inner_pad * 2.0,
    );
    let grid_rect = Rect::from_xywh(
        rail_rect.max.x + section_gap,
        rail_rect.y(),
        panel.max.x - inner_pad - (rail_rect.max.x + section_gap),
        rail_rect.height(),
    );
    let detail_rect = Rect::from_xywh(
        panel.x() + inner_pad,
        grid_rect.max.y + inner_pad,
        panel_w - inner_pad * 2.0,
        detail_h,
    );
    let bar_strip_rect = Rect::from_xywh(
        panel.x() + inner_pad,
        detail_rect.max.y + inner_pad,
        panel_w - inner_pad * 2.0,
        panel.max.y - (detail_rect.max.y + inner_pad) - inner_pad,
    );

    draw_category_rail(ui, rail_rect, view, state, fit);

    let tile = 64.0 * fit;
    let tile_gap = 8.0 * fit;
    let cols = ((grid_rect.width() + tile_gap) / (tile + tile_gap))
        .floor()
        .max(1.0) as usize;
    let active_cat = state.selected_category;
    let mut hovered_ability: Option<SpellbookAbilityView<'_>> = None;
    let mut action: Option<SpellbookAction> = None;

    let mut visible_count = 0usize;
    for ability in view.abilities.iter().copied() {
        if active_cat != CATEGORY_ALL && ability.category != active_cat {
            continue;
        }
        let col = visible_count % cols;
        let row = visible_count / cols;
        visible_count += 1;
        let pos = Pos2::new(
            grid_rect.x() + col as f32 * (tile + tile_gap),
            grid_rect.y() + row as f32 * (tile + tile_gap),
        );
        let id = Id::root("spellbook_pool").child(ability.id as usize);
        let mut slot = ItemSlot::new(tile)
            .selected(state.selected_ability == Some(ability.id))
            .enabled(ability.unlocked);
        if let Some(icon) = ability.icon {
            slot = slot.icon(icon);
        } else if let Some(ch) = ability.name.chars().next() {
            slot = slot
                .fallback_glyph(ch)
                .fallback_color(Color::rgba(0.62, 0.82, 1.0, 0.95));
        }
        let resp = slot.show(ui, pos, id);
        if resp.hovered {
            hovered_ability = Some(ability);
        }
        if resp.clicked && ability.unlocked {
            if let Some(slot_index) = state.target_slot.take() {
                action = Some(SpellbookAction::AssignSlot {
                    slot_index,
                    ability_id: ability.id,
                });
                state.selected_ability = None;
                state.open = false;
            } else {
                state.selected_ability = Some(ability.id);
            }
        }
    }

    if visible_count == 0 {
        ui.draw_text(
            Pos2::new(grid_rect.x() + 8.0 * fit, grid_rect.y() + 8.0 * fit),
            "No abilities in this category.",
            theme.fonts.size_sm,
            theme.colors.text_muted,
        );
    }

    Frame::inset(&theme)
        .with_fill(Color::rgba(0.045, 0.035, 0.072, 0.88))
        .show_only(ui, detail_rect);
    let focus = hovered_ability.or_else(|| selected_ability(view, state.selected_ability));
    draw_ability_detail(ui, detail_rect, focus, view.player_level, fit);

    ui.draw_header_text(
        Pos2::new(bar_strip_rect.x(), bar_strip_rect.y()),
        "ACTION BAR",
        theme.fonts.size_sm,
        theme.colors.text_muted,
    );
    let bar_y = bar_strip_rect.y() + 22.0 * fit;
    let bar_total_w =
        view.slots.len() as f32 * tile + view.slots.len().saturating_sub(1) as f32 * tile_gap;
    let bar_x = panel.x() + (panel_w - bar_total_w) * 0.5;
    for (i, slot_view) in view.slots.iter().enumerate() {
        let pos = Pos2::new(bar_x + i as f32 * (tile + tile_gap), bar_y);
        let id = Id::root("spellbook_bar").child(slot_view.index as usize);
        let ability = slot_view.ability_id.and_then(|id| ability_by_id(view, id));
        let mut slot = ItemSlot::new(tile)
            .key_label(slot_view.key_label)
            .selected(state.target_slot == Some(slot_view.index))
            .enabled(slot_view.unlocked);
        if let Some(ability) = ability {
            if let Some(icon) = ability.icon {
                slot = slot.icon(icon);
            } else if let Some(ch) = ability.name.chars().next() {
                slot = slot
                    .fallback_glyph(ch)
                    .fallback_color(Color::rgba(0.62, 0.82, 1.0, 0.95));
            }
        }
        let resp = slot.show(ui, pos, id);
        if resp.hovered {
            hovered_ability = ability;
        }
        if resp.clicked && slot_view.unlocked {
            if let Some(picked) = state.selected_ability {
                action = Some(SpellbookAction::AssignSlot {
                    slot_index: slot_view.index,
                    ability_id: picked,
                });
                state.selected_ability = None;
                state.target_slot = None;
            } else {
                state.target_slot = Some(slot_view.index);
            }
        }
        if !slot_view.unlocked {
            draw_lock_badge(ui, pos, tile, slot_view.unlock_level, fit);
        }
    }

    let focus = hovered_ability.or_else(|| selected_ability(view, state.selected_ability));
    draw_ability_detail(ui, detail_rect, focus, view.player_level, fit);

    let close_text = "B / Esc to close";
    let close_w = ui.measure_text(close_text, theme.fonts.size_sm);
    ui.draw_text(
        Pos2::new(panel.max.x - close_w - inner_pad, panel.max.y - inner_pad),
        close_text,
        theme.fonts.size_sm,
        theme.colors.text_muted,
    );

    action
}

fn draw_category_rail(
    ui: &mut Ui<'_>,
    rail_rect: Rect,
    view: &SpellbookView<'_>,
    state: &mut SpellbookState,
    fit: f32,
) {
    let theme = *ui.theme();
    let cat_btn_h = 36.0 * fit;
    let cat_btn_gap = 6.0 * fit;
    for (i, category) in view.categories.iter().enumerate() {
        let label = format!("  {}  ({})", category.label, category.count);
        let rect = Rect::from_xywh(
            rail_rect.x(),
            rail_rect.y() + i as f32 * (cat_btn_h + cat_btn_gap),
            rail_rect.width(),
            cat_btn_h,
        );
        let button = if state.selected_category == category.id {
            Button::active(&label)
        } else {
            Button::new(&label)
        };
        let resp = button.show_with_id(
            ui,
            Id::root("spellbook_cat").child(category.id as usize),
            rect,
        );
        if resp.clicked {
            state.selected_category = category.id;
        }
        let icon = match i % 5 {
            0 => UiIcon::Book,
            1 => UiIcon::Damage,
            2 => UiIcon::Healing,
            3 => UiIcon::Shield,
            _ => UiIcon::Threat,
        };
        draw_placeholder_icon(
            ui,
            icon_rect_left(rect, 18.0 * fit, 8.0 * fit),
            icon,
            theme.colors.text,
        );
    }
}

fn draw_lock_badge(ui: &mut Ui<'_>, tile_pos: Pos2, tile_size: f32, unlock_level: u32, fit: f32) {
    let theme = *ui.theme();
    let text = format!("Lv {unlock_level}");
    let text_w = ui.measure_text(&text, theme.fonts.size_sm);
    let pill_w = text_w + 12.0 * fit;
    let pill_h = 18.0 * fit;
    let pill = Rect::from_xywh(
        tile_pos.x + (tile_size - pill_w) * 0.5,
        tile_pos.y + tile_size - pill_h - 5.0 * fit,
        pill_w,
        pill_h,
    );
    ui.draw_rect(pill, Color::rgba(0.12, 0.06, 0.22, 0.92));
    ui.draw_outline(pill, 1.0, Color::rgba(0.58, 0.38, 0.88, 0.86));
    ui.draw_text(
        Pos2::new(pill.x() + 6.0 * fit, pill.y() + 2.0 * fit),
        &text,
        theme.fonts.size_sm,
        Color::rgba(0.88, 0.72, 1.0, 1.0),
    );
}

fn draw_ability_detail(
    ui: &mut Ui<'_>,
    rect: Rect,
    focus: Option<SpellbookAbilityView<'_>>,
    player_level: u32,
    fit: f32,
) {
    let theme = *ui.theme();
    let pad = 14.0 * fit;
    let Some(ability) = focus else {
        ui.draw_text(
            Pos2::new(rect.x() + pad, rect.y() + pad + 4.0 * fit),
            "Hover an ability to see its details.",
            theme.fonts.size_sm,
            theme.colors.text_muted,
        );
        return;
    };

    ui.draw_header_text(
        Pos2::new(rect.x() + pad, rect.y() + pad),
        ability.name,
        theme.fonts.size_lg,
        if ability.unlocked {
            theme.colors.text
        } else {
            theme.colors.text_muted
        },
    );
    let lvl_text = format!("Lv {}", ability.unlock_level);
    let lvl_w = ui.measure_text(&lvl_text, theme.fonts.size_md);
    ui.draw_text(
        Pos2::new(rect.max.x - pad - lvl_w, rect.y() + pad + 4.0 * fit),
        &lvl_text,
        theme.fonts.size_md,
        if ability.unlocked {
            theme.colors.success
        } else {
            Color::rgba(0.76, 0.62, 0.95, 1.0)
        },
    );

    let mut stat_parts = Vec::new();
    if ability.cooldown > 0.0 {
        stat_parts.push(format!("Cooldown {:.1}s", ability.cooldown));
    }
    if ability.resource_cost > 0.0 {
        stat_parts.push(format!("Essence {:.0}", ability.resource_cost));
    } else if ability.channel_cost_per_sec > 0.0 {
        stat_parts.push(format!("Essence {:.0}/s", ability.channel_cost_per_sec));
    }
    if ability.effective_damage > 0.01 {
        stat_parts.push(format!("Damage {:.0}", ability.effective_damage));
        if ability.crit_chance > 0.001 {
            stat_parts.push(format!(
                "Avg {:.0} ({:.0}% crit)",
                ability.avg_damage,
                ability.crit_chance * 100.0
            ));
        }
    } else if ability.minion_health > 0.01 {
        if ability.minion_count > 1 {
            stat_parts.push(format!("Minions {}", ability.minion_count));
        }
        stat_parts.push(format!("Minion dmg {:.0}", ability.minion_damage));
        stat_parts.push(format!("HP {:.0}", ability.minion_health));
        stat_parts.push(format!("Duration {:.0}s", ability.minion_duration));
        stat_parts.push(format!("Attack {:.1}s", ability.minion_attack_interval));
        if ability.minion_inherits_crit && ability.crit_chance > 0.001 {
            stat_parts.push(format!(
                "Crit {:.0}% / +{:.0}%",
                ability.crit_chance * 100.0,
                ability.crit_damage * 100.0
            ));
        }
    } else {
        stat_parts.push(format!("Damage {:.0}%", ability.damage_mult * 100.0));
    }
    if ability.projectile_count > 1 {
        stat_parts.push(format!("Projectiles {}", ability.projectile_count));
    }
    if ability.pierce_count > 0 {
        stat_parts.push(format!("Pierce {}", ability.pierce_count));
    }
    let stats = stat_parts.join("   |   ");
    ui.draw_text(
        Pos2::new(rect.x() + pad, rect.y() + pad + 30.0 * fit),
        &stats,
        theme.fonts.size_sm,
        Color::rgba(0.92, 0.82, 1.0, 1.0),
    );
    ui.draw_text(
        Pos2::new(rect.x() + pad, rect.y() + pad + 58.0 * fit),
        ability.description,
        theme.fonts.size_sm,
        theme.colors.text_dim,
    );
    if player_level < ability.unlock_level {
        let lock_text = format!(
            "Locked. Reach character level {} to unlock.",
            ability.unlock_level
        );
        ui.draw_text(
            Pos2::new(rect.x() + pad, rect.max.y - pad - 14.0 * fit),
            &lock_text,
            theme.fonts.size_sm,
            Color::rgba(0.76, 0.62, 0.95, 1.0),
        );
    }
}

fn selected_ability<'a>(
    view: &'a SpellbookView<'a>,
    selected: Option<u8>,
) -> Option<SpellbookAbilityView<'a>> {
    selected.and_then(|id| ability_by_id(view, id))
}

fn ability_by_id<'a>(view: &'a SpellbookView<'a>, id: u8) -> Option<SpellbookAbilityView<'a>> {
    view.abilities
        .iter()
        .copied()
        .find(|ability| ability.id == id)
}
