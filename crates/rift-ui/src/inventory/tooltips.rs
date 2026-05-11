//! Item tooltips and the side-by-side compare/delta panel.

use rift_ui_im::{Color, Pos2, Rect, Tooltip, TooltipLine, Ui};
use rift_ui_types::inventory::{CompareDeltaRow, ItemView, TooltipLineKind};

pub fn render_item_tooltip(
    ui: &mut Ui<'_>,
    item: &ItemView<'_>,
    header: &str,
    anchor: Pos2,
) -> Rect {
    render_item_tooltip_inner(ui, item, header, anchor, None, false)
}

/// Render the tooltip placed to the LEFT of `anchor_rect`
/// (falling back to the right if it doesn't fit). Used by the
/// "Equipped" compare panel so it chains leftward off the
/// "Hovered" tooltip rather than overlapping it.
pub fn render_item_tooltip_left_of(
    ui: &mut Ui<'_>,
    item: &ItemView<'_>,
    header: &str,
    anchor_rect: Rect,
) -> Rect {
    render_item_tooltip_inner(
        ui,
        item,
        header,
        Pos2::new(0.0, 0.0),
        Some(anchor_rect),
        true,
    )
}

/// Render the tooltip placed on the side of `anchor_rect`
/// indicated by `prefer_left`. Used to chain the "Equipped"
/// compare panel in the SAME direction the primary tooltip
/// extended away from its panel — otherwise it loops back
/// onto the slot grid.
pub fn render_item_tooltip_side_of(
    ui: &mut Ui<'_>,
    item: &ItemView<'_>,
    header: &str,
    anchor_rect: Rect,
    prefer_left: bool,
) -> Rect {
    render_item_tooltip_inner(
        ui,
        item,
        header,
        Pos2::new(0.0, 0.0),
        Some(anchor_rect),
        prefer_left,
    )
}

/// Render the tooltip anchored to `anchor_rect` with an
/// explicit side preference (`prefer_left = true` puts it on
/// the left of the slot, fall back to the right). The bare
/// `anchor` is used only as the legacy positioning fallback
/// in case `anchor_rect` somehow doesn't fit on either side.
pub fn render_item_tooltip_anchored(
    ui: &mut Ui<'_>,
    item: &ItemView<'_>,
    header: &str,
    anchor_rect: Rect,
    prefer_left: bool,
    anchor: Pos2,
) -> Rect {
    render_item_tooltip_inner(ui, item, header, anchor, Some(anchor_rect), prefer_left)
}

fn render_item_tooltip_inner(
    ui: &mut Ui<'_>,
    item: &ItemView<'_>,
    header: &str,
    anchor: Pos2,
    anchor_rect: Option<Rect>,
    prefer_left: bool,
) -> Rect {
    let theme = *ui.theme();
    let [rr, gg, bb, aa] = item.rarity_color;
    let rarity_col = Color::rgba(rr, gg, bb, aa);

    let lines: Vec<TooltipLine<'_>> = item
        .tooltip_lines
        .iter()
        .map(|l| {
            let size = match l.kind {
                TooltipLineKind::Name => theme.fonts.size_lg,
                _ => theme.fonts.size_md,
            };
            let color = match l.kind {
                TooltipLineKind::Name => rarity_col,
                TooltipLineKind::ItemLevel
                | TooltipLineKind::Divider
                | TooltipLineKind::RequiresLevel { ok: true } => theme.colors.text_dim,
                TooltipLineKind::RequiresLevel { ok: false } | TooltipLineKind::Warning => {
                    Color::rgba(0.96, 0.40, 0.40, 1.0)
                }
                TooltipLineKind::Legendary => Color::rgba(1.00, 0.70, 0.20, 1.0),
                TooltipLineKind::Resonance => Color::rgba(0.78, 0.55, 1.00, 1.0),
                TooltipLineKind::Anchored => Color::rgba(1.00, 0.82, 0.25, 1.0),
                TooltipLineKind::Synergy => theme.colors.accent,
                TooltipLineKind::Stat | TooltipLineKind::Blank => theme.colors.text,
            };
            TooltipLine {
                text: l.text,
                size,
                color,
            }
        })
        .collect();

    let mut t = Tooltip::new().header(header).min_width(240.0).pad(10.0);
    if let Some(r) = anchor_rect {
        t = t.anchor_to(r).prefer_left(prefer_left);
    }
    t.show(ui, anchor, &lines)
}

pub fn render_compare_delta(ui: &mut Ui<'_>, rows: &[CompareDeltaRow<'_>], anchor: Pos2) -> Rect {
    render_compare_delta_inner(ui, rows, anchor, None, true)
}

/// Place the delta panel to the LEFT of `anchor_rect`.
pub fn render_compare_delta_left_of(
    ui: &mut Ui<'_>,
    rows: &[CompareDeltaRow<'_>],
    anchor_rect: Rect,
) -> Rect {
    render_compare_delta_inner(ui, rows, Pos2::new(0.0, 0.0), Some(anchor_rect), true)
}

/// Place the delta panel on the side of `anchor_rect`
/// indicated by `prefer_left`.
pub fn render_compare_delta_side_of(
    ui: &mut Ui<'_>,
    rows: &[CompareDeltaRow<'_>],
    anchor_rect: Rect,
    prefer_left: bool,
) -> Rect {
    render_compare_delta_inner(
        ui,
        rows,
        Pos2::new(0.0, 0.0),
        Some(anchor_rect),
        prefer_left,
    )
}

fn render_compare_delta_inner(
    ui: &mut Ui<'_>,
    rows: &[CompareDeltaRow<'_>],
    anchor: Pos2,
    anchor_rect: Option<Rect>,
    prefer_left: bool,
) -> Rect {
    let theme = *ui.theme();
    let lines: Vec<TooltipLine<'_>>;
    if rows.is_empty() {
        lines = vec![TooltipLine {
            text: "No stat changes",
            size: theme.fonts.size_md,
            color: theme.colors.text_dim,
        }];
    } else {
        lines = rows
            .iter()
            .map(|r| TooltipLine {
                text: r.text,
                size: theme.fonts.size_md,
                color: if r.delta_positive {
                    Color::rgba(0.45, 0.92, 0.45, 1.0)
                } else {
                    Color::rgba(0.96, 0.40, 0.40, 1.0)
                },
            })
            .collect();
    }

    let mut t = Tooltip::new()
        .header("Change vs equipped")
        .header_color(Color::rgba(0.95, 0.85, 0.55, 1.0))
        .min_width(220.0)
        .pad(10.0);
    if let Some(r) = anchor_rect {
        t = t.anchor_to(r).prefer_left(prefer_left);
    }
    t.show(ui, anchor, &lines)
}
