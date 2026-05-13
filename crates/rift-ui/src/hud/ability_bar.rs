//! Bottom-center six-slot action bar.
//!
//! Pure widget: input is a flat `AbilityBarView`, output is
//! the index of any slot clicked this frame. Tooltip strings
//! are pre-formatted by the host (so the widget doesn't need
//! to read `CharacterStats`); we just render them with the
//! standard tooltip styling.

use rift_ui_im::{Color, Frame, Id, ItemSlot, Pad, Pos2, Rect, Tooltip, TooltipLine, Ui};
use rift_ui_types::hud::{AbilityBarView, HudAction};

const SLOT_SIZE_BASE: f32 = 64.0;
const SLOT_GAP_BASE: f32 = 6.0;
const BOTTOM_OFFSET_BASE: f32 = 16.0;
const PLAQUE_PAD_BASE: f32 = 6.0;
/// Extra gap inserted *before* the passive (Space) tile so it
/// reads as a separate cluster from the main 6-slot bar.
const PASSIVE_GAP_BASE: f32 = 14.0;

/// Full plaque width in baseline pixels (× `theme.scale` at
/// render time). Exposed so the vitals widget can pick the
/// same width and the two HUD plaques read as one column.
/// Sized for 6 main slots + the passive tile so the plaque
/// hugs the full cluster.
pub const PLAQUE_W_BASE: f32 =
    7.0 * SLOT_SIZE_BASE + 5.0 * SLOT_GAP_BASE + PASSIVE_GAP_BASE + 2.0 * PLAQUE_PAD_BASE;
/// Full plaque height in baseline pixels. Combined with
/// [`BOTTOM_OFFSET_BASE`] this gives the bottom-anchor the
/// vitals plaque needs so the two surfaces sit flush.
pub const PLAQUE_H_BASE: f32 = SLOT_SIZE_BASE + 2.0 * PLAQUE_PAD_BASE;
/// Gap (baseline px) between the ability bar plaque and the
/// screen's bottom edge.
pub const BOTTOM_GAP_BASE: f32 = BOTTOM_OFFSET_BASE;

/// Bottom anchor (baseline px from the screen's bottom edge)
/// the vitals plaque should use so it sits flush against the
/// top of the ability bar plaque with no gap.
pub const VITALS_BOTTOM_OFFSET_BASE: f32 = PLAQUE_H_BASE + BOTTOM_GAP_BASE;

// Cell chrome borrowed from the inventory's equipment grid so
// the action slots match the paperdoll slots. The same
// constants are duplicated here on purpose — the two
// surfaces share a visual language but the inventory widget
// crate is structured around its own private cell helpers
// (see `inventory::bag_panel::draw_cell_outline`).
const EMPTY_CELL_FILL: Color = Color::rgba(0.0, 0.0, 0.0, 0.32);
const GOLD_OUTLINE: Color = Color::rgba(0.78, 0.62, 0.30, 0.85);
const INSET_HIGHLIGHT: Color = Color::rgba(1.0, 0.95, 0.82, 0.10);
// Inset shadow inside the slot border. Painted as a 1px dark
// band along the top + left edges, with a matching cream
// highlight along the bottom + right, so the cell reads as
// recessed into the stone plaque rather than stamped on top.
const INSET_SHADOW: Color = Color::rgba(0.0, 0.0, 0.0, 0.55);
const INSET_LIGHT: Color = Color::rgba(1.0, 0.95, 0.82, 0.08);

/// Render the six-slot action bar centered horizontally,
/// anchored just above the bottom edge of the screen. Returns
/// `Some(HudAction::AbilitySlotClicked(idx))` when one of the
/// unlocked slots is clicked this frame.
pub fn frame_ability_bar(ui: &mut Ui<'_>, view: &AbilityBarView<'_>) -> Option<HudAction> {
    let theme = *ui.theme();
    let s = theme.scale;
    let slot_size = SLOT_SIZE_BASE * s;
    let slot_gap = SLOT_GAP_BASE * s;
    let passive_gap = PASSIVE_GAP_BASE * s;
    let plaque_pad = PLAQUE_PAD_BASE * s;
    let screen = ui.screen_size();
    // Plaque is sized for the full cluster (6 main slots +
    // the passive tile, with the extra gap between them).
    // Keeping it at that width unconditionally means the bar
    // doesn't visually shift between builds where the passive
    // happens to be unset (e.g. tests).
    let slots_w = 7.0 * slot_size + 5.0 * slot_gap + passive_gap;
    let plaque_w = slots_w + plaque_pad * 2.0;
    let plaque_h = slot_size + plaque_pad * 2.0;
    let plaque_x = (screen.x - plaque_w) * 0.5;
    let plaque_y = screen.y - plaque_h - BOTTOM_OFFSET_BASE * s;
    let plaque_rect = Rect::from_xywh(plaque_x, plaque_y, plaque_w, plaque_h);

    // Same carved-stone treatment as the vitals plaque above,
    // so the two HUD clusters read as one continuous surface.
    Frame::stone(&theme)
        .with_padding(Pad::all(plaque_pad))
        .with_radius(2.0 * s)
        .show_only(ui, plaque_rect);

    let origin_x = plaque_x + plaque_pad;
    let origin_y = plaque_y + plaque_pad;

    let mut hovered_idx: Option<usize> = None;
    let mut clicked_idx: Option<usize> = None;
    // `7` slot index is reserved for the passive tile so the
    // tooltip pass below can show it without leaking through
    // as a clickable bar slot.
    const PASSIVE_TOOLTIP_IDX: usize = 6;
    let mut passive_hovered = false;

    for (i, slot) in view.slots.iter().enumerate() {
        let pos = Pos2::new(origin_x + i as f32 * (slot_size + slot_gap), origin_y);
        let id = Id::root("ability_bar").child(i);
        let (hovered, clicked) = draw_one_slot(ui, slot, pos, slot_size, id);
        if hovered {
            hovered_idx = Some(i);
        }
        if clicked {
            clicked_idx = Some(i);
        }
    }

    // Passive tile (Evasive Roll on Space). Drawn with the
    // same chrome / cooldown overlay as a normal slot but
    // never clickable — assigning a passive isn't a thing,
    // and clicks here must not open the spellbook.
    let passive_x = origin_x + 6.0 * (slot_size + slot_gap) + (passive_gap - slot_gap);
    if let Some(slot) = view.passive.as_ref() {
        let pos = Pos2::new(passive_x, origin_y);
        let id = Id::root("ability_bar_passive").child(0usize);
        let (hovered, _clicked) = draw_one_slot(ui, slot, pos, slot_size, id);
        if hovered {
            passive_hovered = true;
        }
    }

    // Tooltip pass — runs after every slot has been laid out
    // so a hovered slot's tooltip sits on top of later slots
    // in the same row.
    let tooltip_for: Option<(usize, &rift_ui_types::hud::AbilityTooltip<'_>, Pos2)> =
        if passive_hovered {
            view.passive.as_ref().and_then(|s| s.tooltip.as_ref()).map(
                |tip: &rift_ui_types::hud::AbilityTooltip<'_>| {
                    (PASSIVE_TOOLTIP_IDX, tip, Pos2::new(passive_x, origin_y))
                },
            )
        } else if let Some(idx) = hovered_idx {
            view.slots.get(idx).and_then(|s| s.tooltip.as_ref()).map(
                |tip: &rift_ui_types::hud::AbilityTooltip<'_>| {
                    let slot_x = origin_x + idx as f32 * (slot_size + slot_gap);
                    (idx, tip, Pos2::new(slot_x, origin_y))
                },
            )
        } else {
            None
        };
    if let Some((_idx, tip, slot_pos)) = tooltip_for {
        // Sizing matches the inventory item tooltip: name
        // at `size_lg`, every body line at `size_md`, so
        // both tooltip surfaces read at the same scale
        // when the player flicks between them.
        let mut lines: Vec<TooltipLine<'_>> = Vec::with_capacity(6);
        lines.push(TooltipLine::new(
            tip.name,
            theme.fonts.size_lg,
            Color::rgba(1.0, 0.9, 0.5, 1.0),
        ));
        lines.push(TooltipLine::new(
            tip.description,
            theme.fonts.size_md,
            Color::rgba(0.8, 0.8, 0.8, 1.0),
        ));
        if let Some(ref d) = tip.damage_line {
            lines.push(TooltipLine::new(
                d.as_str(),
                theme.fonts.size_md,
                Color::rgba(0.95, 0.78, 0.55, 0.95),
            ));
        }
        if let Some(ref c) = tip.crit_line {
            lines.push(TooltipLine::new(
                c.as_str(),
                theme.fonts.size_md,
                Color::rgba(0.72, 0.68, 0.55, 0.85),
            ));
        }
        if let Some(ref p) = tip.projectiles_line {
            lines.push(TooltipLine::new(
                p.as_str(),
                theme.fonts.size_md,
                Color::rgba(0.7, 0.7, 0.7, 0.8),
            ));
        }
        if let Some(ref t) = tip.transform_line {
            lines.push(TooltipLine::new(
                t.as_str(),
                theme.fonts.size_md,
                // Legendary-orange to match unique-item
                // tooltip flavour lines.
                Color::rgba(0.95, 0.55, 0.25, 0.95),
            ));
        }
        if let Some(ref b) = tip.bonus_line {
            lines.push(TooltipLine::new(
                b.as_str(),
                theme.fonts.size_md,
                Color::rgba(0.95, 0.55, 0.25, 0.95),
            ));
        }
        if let Some(ref c) = tip.cost_line {
            let color = if tip.cost_affordable {
                Color::rgba(0.55, 0.75, 0.95, 0.95)
            } else {
                Color::rgba(0.85, 0.45, 0.45, 0.95)
            };
            lines.push(TooltipLine::new(c.as_str(), theme.fonts.size_md, color));
        }
        let slot_rect = Rect::from_xywh(slot_pos.x, slot_pos.y, slot_size, slot_size);
        Tooltip::new().min_width(220.0).anchor_to(slot_rect).show(
            ui,
            Pos2::new(slot_rect.x(), slot_rect.y() - 90.0 * s),
            &lines,
        );
    }

    clicked_idx.map(HudAction::AbilitySlotClicked)
}

/// Draw a single ability slot (chrome + icon + cooldown +
/// key hint) at `pos`. Returns `(hovered, clicked)`. Shared
/// between the main 6-slot loop and the passive-tile render
/// so they stay visually identical.
fn draw_one_slot(
    ui: &mut Ui<'_>,
    slot: &rift_ui_types::hud::AbilitySlotView<'_>,
    pos: Pos2,
    slot_size: f32,
    id: Id,
) -> (bool, bool) {
    let cell_rect = Rect::from_xywh(pos.x, pos.y, slot_size, slot_size);
    draw_slot_chrome(ui, cell_rect);

    let mut sb = ItemSlot::new(slot_size)
        .key_label(slot.key_hint)
        .transparent_bg(true)
        .icon_fills(true);
    if slot.selected {
        sb = sb.selected(true);
    }
    if !slot.unlocked {
        sb = sb
            .enabled(false)
            .fallback_glyph('\u{1F512}')
            .fallback_color(Color::rgba(0.74, 0.66, 0.48, 0.82));
    } else {
        if slot.cooldown_remaining > 0.001 {
            sb = sb.cooldown(slot.cooldown_remaining);
        }
        if !slot.affordable {
            sb = sb.unaffordable(true);
        }
        if let Some(icon) = slot.icon {
            sb = sb.icon(icon);
        } else if let Some(ch) = slot.fallback_glyph {
            sb = sb
                .fallback_glyph(ch)
                .fallback_color(Color::rgba(0.6, 0.85, 1.0, 0.95));
        }
    }

    let resp = sb.show(ui, pos, id);
    if !slot.unlocked {
        draw_locked_slot_badge(ui, cell_rect, slot.unlock_level);
    } else if slot.cooldown_remaining > 0.001 {
        draw_cooldown_accent(ui, cell_rect, slot.cooldown_remaining);
    }
    let hovered = resp.hovered && slot.tooltip.is_some() && slot.unlocked;
    let clicked = resp.clicked && slot.unlocked;
    (hovered, clicked)
}

fn draw_locked_slot_badge(ui: &mut Ui<'_>, rect: Rect, unlock_level: u32) {
    let s = ui.scale();
    ui.draw_grad4_rect(
        Rect::from_xywh(
            rect.x() + 2.0 * s,
            rect.y() + 2.0 * s,
            rect.width() - 4.0 * s,
            rect.height() - 4.0 * s,
        ),
        Color::rgba(0.03, 0.025, 0.020, 0.34),
        Color::rgba(0.06, 0.050, 0.040, 0.44),
        Color::rgba(0.00, 0.00, 0.00, 0.62),
        Color::rgba(0.00, 0.00, 0.00, 0.48),
    );

    let label = format!("Lv {unlock_level}");
    let font = 11.0 * s;
    let tw = ui.measure_text(&label, font);
    let chip_w = (tw + 10.0 * s).min(rect.width() - 8.0 * s);
    let chip_h = 16.0 * s;
    let chip = Rect::from_xywh(
        rect.x() + (rect.width() - chip_w) * 0.5,
        rect.max.y - chip_h - 5.0 * s,
        chip_w,
        chip_h,
    );
    ui.draw_gradient_rect(
        chip,
        Color::rgba(0.18, 0.145, 0.095, 0.94),
        Color::rgba(0.060, 0.050, 0.040, 0.96),
    );
    ui.draw_outline(chip, 1.0 * s, Color::rgba(0.72, 0.58, 0.32, 0.72));
    let text_pos = Pos2::new(
        chip.x() + (chip.width() - tw) * 0.5,
        chip.y() + (chip.height() - font) * 0.5,
    );
    ui.draw_text(
        Pos2::new(text_pos.x + 1.0 * s, text_pos.y + 1.0 * s),
        &label,
        font,
        Color::rgba(0.0, 0.0, 0.0, 0.72),
    );
    ui.draw_text(text_pos, &label, font, Color::rgba(0.92, 0.80, 0.56, 0.96));
}

fn draw_cooldown_accent(ui: &mut Ui<'_>, rect: Rect, remaining: f32) {
    let remaining = remaining.clamp(0.0, 1.0);
    let s = ui.scale();
    let inset = Rect::from_xywh(
        rect.x() + 2.0 * s,
        rect.y() + 2.0 * s,
        rect.width() - 4.0 * s,
        rect.height() - 4.0 * s,
    );
    let drain_h = inset.height() * remaining;
    if drain_h <= 0.5 {
        return;
    }
    let drain = Rect::from_xywh(inset.x(), inset.y(), inset.width(), drain_h);
    ui.draw_grad4_rect(
        drain,
        Color::rgba(0.02, 0.03, 0.05, 0.28),
        Color::rgba(0.03, 0.04, 0.06, 0.36),
        Color::rgba(0.0, 0.0, 0.0, 0.18),
        Color::rgba(0.0, 0.0, 0.0, 0.24),
    );

    let edge_y = (inset.y() + drain_h).min(inset.max.y - 1.0 * s);
    ui.draw_gradient_rect(
        Rect::from_xywh(inset.x(), edge_y - 1.0 * s, inset.width(), 2.0 * s),
        Color::rgba(1.0, 0.76, 0.34, 0.86),
        Color::rgba(0.70, 0.32, 0.10, 0.66),
    );
    ui.draw_grad4_rect(
        Rect::from_xywh(inset.x(), inset.y(), inset.width(), inset.height()),
        Color::rgba(0.95, 0.55, 0.18, 0.10),
        Color::rgba(0.95, 0.55, 0.18, 0.04),
        Color::rgba(0.0, 0.0, 0.0, 0.00),
        Color::rgba(0.0, 0.0, 0.0, 0.00),
    );
    ui.draw_outline(rect, 1.5 * s, Color::rgba(0.90, 0.56, 0.24, 0.62));
}

/// Inventory-style cell chrome shared by every ability slot,
/// empty or occupied: dark fill, gold outer outline, cream
/// inset highlight, plus a 1px inset shadow on the top and
/// left edges (with a matching highlight on the bottom and
/// right) so the cell reads as recessed into the stone
/// plaque. Mirrors the look of an equipment slot in the
/// paperdoll grid so the two HUD surfaces match.
fn draw_slot_chrome(ui: &mut Ui<'_>, rect: Rect) {
    // Base: dark wash + gold outline + cream inset.
    ui.draw_rect(rect, EMPTY_CELL_FILL);
    ui.draw_outline(rect, 1.0, GOLD_OUTLINE);
    let inset = Rect::from_xywh(
        rect.x() + 1.0,
        rect.y() + 1.0,
        (rect.width() - 2.0).max(0.0),
        (rect.height() - 2.0).max(0.0),
    );
    ui.draw_outline(inset, 1.0, INSET_HIGHLIGHT);

    // Inset shadow / bevel: 1px dark band along the top and
    // left edges of the inner rect, 1px cream highlight
    // along the bottom and right. Reads as a recessed cell
    // when scanned at a glance.
    let w = inset.width();
    let h = inset.height();
    if w > 2.0 && h > 2.0 {
        // Top dark band
        ui.draw_rect(
            Rect::from_xywh(inset.x() + 1.0, inset.y() + 1.0, w - 2.0, 1.0),
            INSET_SHADOW,
        );
        // Left dark band
        ui.draw_rect(
            Rect::from_xywh(inset.x() + 1.0, inset.y() + 1.0, 1.0, h - 2.0),
            INSET_SHADOW,
        );
        // Bottom highlight
        ui.draw_rect(
            Rect::from_xywh(inset.x() + 1.0, inset.max.y - 2.0, w - 2.0, 1.0),
            INSET_LIGHT,
        );
        // Right highlight
        ui.draw_rect(
            Rect::from_xywh(inset.max.x - 2.0, inset.y() + 1.0, 1.0, h - 2.0),
            INSET_LIGHT,
        );
    }
}
