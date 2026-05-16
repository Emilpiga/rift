//! Paperdoll equipment grid + flush-flat bag grid.
//!
//! Paperdoll: each `EquipSlotIdx` is laid out at the
//! position from `PAPERDOLL_LAYOUT`. Empty slots draw a
//! recessed well plus a flat-tinted loot atlas glyph (slot-
//! keyed); filled slots draw the item icon stretched to the
//! slot rectangle.
//!
//! Bag: a flush grid of `bag_cols × bag_rows` outlined cells.
//! Items are first-fit-packed by `pack_bag` and rendered as
//! `(cell_w × cell_h)` rectangles spanning multiple cells.
//!
//! IMPORTANT: empty bag cells AND empty paperdoll slots run
//! `ItemSlot::interact(payload=None)` so the engine's typed
//! drag system can resolve a drop into them. Without this the
//! click would draw an outline but `take_drop` would never
//! fire — that's the bug behind "I can't unequip by dragging
//! to an empty bag slot".

use rift_ui_im::{Color, Frame, Id, ItemSlot, Pad, Rect, Stroke, Ui};
use rift_ui_types::inventory::{
    DragSource, EnchantSourceView, EquipSlotIdx, InTransitSource, InventoryAction, ItemView,
};

use super::drag::{build_item_slot, route_slot_capture, route_slot_capture_equip_dest, DropTarget};
use super::grid_drop::{snap_preview_and_resolve, GridSpec};
use super::layout::{pack_bag, Layout};

pub struct BagPanelIn<'a> {
    pub items: &'a [Option<ItemView<'a>>],
    pub equipment: &'a [Option<ItemView<'a>>],
    /// Active stash tab items (used so a stash→bag drag
    /// previews with the correct multi-cell footprint while
    /// hovering the bag grid; see `source_footprint`).
    pub stash_active: &'a [Option<ItemView<'a>>],
    pub bag_cols: u8,
    pub bag_rows: u8,
    pub stash_open: bool,
    pub active_tab_u8: u8,
    /// Forge-selected bag index / equip slot while void forge is open.
    /// Drives snap-anchor rejection + slot chrome so the item stays put.
    pub forge_anchor: Option<EnchantSourceView>,
    /// Void forge panel visible — right-click gear sends [`InventoryAction::SelectEnchantSource`].
    pub void_forge_open: bool,
    /// Optimistic source-hide: when set, the renderer hides
    /// the matching slot so the player doesn't see the item
    /// pop back to its source between drop and the server's
    /// authoritative reply.
    pub in_transit: Option<rift_ui_types::inventory::InTransitSource>,
}

#[derive(Default)]
pub struct BagPanelOut {
    pub hovered_bag: Option<u32>,
    /// Screen rect of the hovered bag cell. Used by the
    /// tooltip to anchor next to the slot and pick the side
    /// (left vs right) with more available space.
    pub hovered_bag_rect: Option<Rect>,
    pub salvage_press_bag_idx: Option<u32>,
    pub salvage_release_bag_idx: Option<u32>,
    /// `Some` the frame the player released a drag onto a
    /// bag/equip slot. Carries the drag's source so the host
    /// can hide that source until the server reply lands
    /// (eliminates the visual "pop back" flash).
    pub in_transit_from_drop: Option<rift_ui_types::inventory::InTransitSource>,
    /// Screen rect (`[x, y, w, h]`) the in-flight drop
    /// resolved to. Used by the host to render a translucent
    /// destination ghost while the server reply is in flight
    /// (so the target slot doesn't read as empty).
    pub in_transit_dest_rect_from_drop: Option<[f32; 4]>,
}

/// Violet accent used for slot outlines (matches void chrome).
const ACCENT_OUTLINE: Color = Color::rgba(0.62, 0.48, 0.92, 0.88);
/// Inner highlight — cool lavender, not warm cream.
const INSET_HIGHLIGHT: Color = Color::rgba(0.78, 0.72, 1.0, 0.12);
/// 1px dark band painted along the top + left edges inside
/// a cell so it reads as recessed into the stone slab.
/// Pairs with [`CELL_INSET_LIGHT`] on the opposing edges.
const CELL_INSET_SHADOW: Color = Color::rgba(0.0, 0.0, 0.0, 0.55);
/// 1px cool highlight along the bottom + right edges of a
/// cell. Together with the top/left shadow it sells the
/// bevel without a full gradient.
const CELL_INSET_LIGHT: Color = Color::rgba(0.72, 0.68, 0.98, 0.10);
/// Inner-shadow band the [`draw_section_chrome`] helper
/// paints along the top + left edges of a section so the
/// niche reads as carved into the drawer rather than tiled
/// on top of it.
const SECTION_INNER_SHADOW: Color = Color::rgba(0.0, 0.0, 0.0, 0.42);
/// Per-slot rim vignette (drawn **on top of** the cell wash,
/// **under** the violet slot outline). The section-level slab
/// sits entirely beneath opaque / semi-opaque cell fills, so
/// a grid-wide radial never reads — this path hits every slot
/// that uses [`draw_cell_outline`].
const SLOT_VIGNETTE_EDGE: Color = Color::rgba(0.06, 0.04, 0.14, 0.45);
const SLOT_VIGNETTE_CENTRE: Color = Color::rgba(0.0, 0.0, 0.0, 0.0);
/// Highlight band along the bottom + right edges of a
/// section, matching `SECTION_INNER_SHADOW` on the opposing
/// edges so the niche reads as recessed at every viewing
/// distance.
const SECTION_INNER_LIGHT: Color = Color::rgba(0.70, 0.64, 0.96, 0.08);
/// Empty equipment-slot fill. The equipment container has a
/// stone texture; each slot gets a darker overlay so the
/// slot grid pops against the slab without losing the
/// underlying texture entirely. Darker than the bag empty-
/// cell wash so the equipped row reads as the focal column.
fn equip_slot_fill() -> Color {
    Color::rgba(0.0, 0.0, 0.0, 0.42)
}

/// Wash used under empty bag / stash slot chrome (re-exported
/// for stash grid parity with the bag).
pub(super) fn bag_empty_cell_fill() -> Color {
    Color::rgba(0.0, 0.0, 0.0, 0.22)
}

fn bag_empty_fill() -> Color {
    bag_empty_cell_fill()
}

/// Draw the textured stone backing used by both the bag and
/// the equipment container. `darker = true` darkens the
/// gradient slightly so the equipment slab reads as a
/// recessed niche behind the bag.
///
/// In addition to the carved-stone fill + border, paints the
/// thin top/left shadow and bottom/right highlight strips on
/// the section bounds. (Per-slot depth is handled in
/// [`draw_cell_outline`], which paints **after** each cell's
/// fill so it stays visible.)
/// This gives the section a clear recessed feel against the
/// drawer behind it — without it the section + drawer both
/// use the same stone fill and blend into one surface at
/// any distance.
pub(super) fn draw_section_chrome(
    ui: &mut Ui<'_>,
    theme: &rift_ui_im::Theme,
    rect: rift_ui_im::Rect,
    darker: bool,
) {
    let fill = if darker {
        let s = theme.colors.bg_stone.0;
        Color::rgba(s[0] * 0.62, s[1] * 0.62, s[2] * 0.62, s[3])
    } else {
        let s = theme.colors.bg_stone.0;
        Color::rgba(s[0] * 0.85, s[1] * 0.85, s[2] * 0.85, s[3])
    };
    Frame::stone(theme)
        .with_fill(fill)
        .with_stroke(Stroke::new(2.0, theme.colors.border_stone))
        .with_padding(Pad::all(0.0))
        .show_only(ui, rect);

    let w = rect.width();
    let h = rect.height();

    // Inner shadow / bevel. Two pixels thick to read at
    // normal viewing distance; thinner on cells (1px) so the
    // hierarchy stays section > cell when scanned.
    if w > 8.0 && h > 8.0 {
        // Top dark band
        ui.draw_rect(
            rift_ui_im::Rect::from_xywh(rect.x() + 2.0, rect.y() + 2.0, w - 4.0, 2.0),
            SECTION_INNER_SHADOW,
        );
        // Left dark band
        ui.draw_rect(
            rift_ui_im::Rect::from_xywh(rect.x() + 2.0, rect.y() + 2.0, 2.0, h - 4.0),
            SECTION_INNER_SHADOW,
        );
        // Bottom highlight
        ui.draw_rect(
            rift_ui_im::Rect::from_xywh(rect.x() + 2.0, rect.max.y - 3.0, w - 4.0, 1.0),
            SECTION_INNER_LIGHT,
        );
        // Right highlight
        ui.draw_rect(
            rift_ui_im::Rect::from_xywh(rect.max.x - 3.0, rect.y() + 2.0, 1.0, h - 4.0),
            SECTION_INNER_LIGHT,
        );
    }
}

/// Outline a single slot cell: soft rim vignette (so depth
/// survives the cell wash), then violet outer stroke, inset
/// highlight, and inner bevel bands.
pub(super) fn draw_cell_outline(ui: &mut Ui<'_>, rect: rift_ui_im::Rect) {
    let mw = rect.width();
    let mh = rect.height();
    if mw >= 4.0 && mh >= 4.0 {
        let pad = 1.0_f32;
        let inner = rift_ui_im::Rect::from_xywh(
            rect.x() + pad,
            rect.y() + pad,
            (mw - 2.0 * pad).max(0.0),
            (mh - 2.0 * pad).max(0.0),
        );
        if inner.width() >= 3.0 && inner.height() >= 3.0 {
            let cr = (inner.width().min(inner.height()) * 0.18).clamp(2.5_f32, 10.0_f32);
            ui.draw_rounded_radial_square_rect(inner, cr, SLOT_VIGNETTE_EDGE, SLOT_VIGNETTE_CENTRE);
        }
    }
    ui.draw_outline(rect, 1.0, ACCENT_OUTLINE);
    let inset = rift_ui_im::Rect::from_xywh(
        rect.x() + 1.0,
        rect.y() + 1.0,
        (rect.width() - 2.0).max(0.0),
        (rect.height() - 2.0).max(0.0),
    );
    ui.draw_outline(inset, 1.0, INSET_HIGHLIGHT);

    // Inner bevel: 1px dark band on top + left, 1px light
    // band on bottom + right. Sized off the inset rect so it
    // sits cleanly inside the accent outline.
    let w = inset.width();
    let h = inset.height();
    if w > 2.0 && h > 2.0 {
        ui.draw_rect(
            rift_ui_im::Rect::from_xywh(inset.x() + 1.0, inset.y() + 1.0, w - 2.0, 1.0),
            CELL_INSET_SHADOW,
        );
        ui.draw_rect(
            rift_ui_im::Rect::from_xywh(inset.x() + 1.0, inset.y() + 1.0, 1.0, h - 2.0),
            CELL_INSET_SHADOW,
        );
        ui.draw_rect(
            rift_ui_im::Rect::from_xywh(inset.x() + 1.0, inset.max.y - 2.0, w - 2.0, 1.0),
            CELL_INSET_LIGHT,
        );
        ui.draw_rect(
            rift_ui_im::Rect::from_xywh(inset.max.x - 2.0, inset.y() + 1.0, 1.0, h - 2.0),
            CELL_INSET_LIGHT,
        );
    }
}

/// Recessed “socket” inside each paperdoll cell — graded fill +
/// top/left shadow + opposing rim light (matches void-slot chrome).
fn draw_paperdoll_socket_inset(ui: &mut Ui<'_>, rect: Rect, fit: f32) {
    let pad = (rect.width().min(rect.height()) * 0.055)
        .clamp(2.5 * fit, 8.0 * fit)
        .max(2.0);
    let inner = Rect::from_xywh(
        rect.x() + pad,
        rect.y() + pad,
        (rect.width() - 2.0 * pad).max(0.0),
        (rect.height() - 2.0 * pad).max(0.0),
    );
    if inner.width() < 6.0 || inner.height() < 6.0 {
        return;
    }
    let cr = (inner.height() * 0.11).clamp(2.5 * fit, 9.0 * fit);
    ui.draw_rounded_gradient_rect(
        inner,
        cr,
        Color::rgba(0.026, 0.020, 0.052, 0.80),
        Color::rgba(0.068, 0.050, 0.108, 0.92),
    );
    let w = inner.width();
    let h = inner.height();
    let band = (2.0 * fit).max(1.0);
    ui.draw_rect(
        Rect::from_xywh(inner.x(), inner.y(), w, band.min(h * 0.24)),
        Color::rgba(0.0, 0.0, 0.0, 0.54),
    );
    ui.draw_rect(
        Rect::from_xywh(inner.x(), inner.y(), band.min(w * 0.22), h),
        Color::rgba(0.0, 0.0, 0.0, 0.54),
    );
    let lip = (1.6 * fit).max(1.0);
    ui.draw_rect(
        Rect::from_xywh(
            inner.x(),
            inner.max.y - lip.min(h * 0.18),
            w,
            lip.min(h * 0.18),
        ),
        Color::rgba(0.58, 0.52, 0.95, 0.072),
    );
    ui.draw_rect(
        Rect::from_xywh(
            inner.max.x - lip.min(w * 0.18),
            inner.y(),
            lip.min(w * 0.18),
            h,
        ),
        Color::rgba(0.58, 0.52, 0.95, 0.072),
    );
}

/// Matches [`draw_paperdoll_socket_inset`] padding so empty-slot art can sit inside the
/// gradient well rather than the outer cell chrome.
fn paperdoll_socket_inner_bounds(rect: Rect, fit: f32) -> Rect {
    let pad = (rect.width().min(rect.height()) * 0.055)
        .clamp(2.5 * fit, 8.0 * fit)
        .max(2.0);
    Rect::from_xywh(
        rect.x() + pad,
        rect.y() + pad,
        (rect.width() - 2.0 * pad).max(0.0),
        (rect.height() - 2.0 * pad).max(0.0),
    )
}

/// Atlas keys under `assets/icons/` (mirrors `rift_game::loot::items` base icons).
fn equip_slot_placeholder_icon_key(slot_idx: u8) -> &'static str {
    match slot_idx {
        0 => "loot/Weapons/1",
        1 => "loot/Helmets/Helmet_1",
        2 => "loot/BodyArmor/BodyArmor_1",
        3 => "loot/Pants/Pants_1",
        4 => "loot/Gloves/Gloves_1",
        5 => "loot/Boots/Boots_1",
        6 | 7 => "loot/Rings/Ring_1",
        8 => "loot/Necklaces/Necklace_1",
        9 => "loot/Shoulders/Shoulders_1",
        _ => "loot/BodyArmor/BodyArmor_1",
    }
}

/// Uniform multiply tint — slightly below the socket gradient floor (~0.068 / 0.05 / 0.108)
/// so the glyph reads as one muted silhouette on the paperdoll slate.
const PAPERDOLL_EMPTY_ICON_TINT: Color = Color::rgba(0.048, 0.042, 0.072, 0.76);

/// Icon rect filling the socket well with proportional horizontal / vertical
/// insets so tall slots (weapon / chest `2×3`) keep the silhouette's aspect.
fn paperdoll_empty_icon_rect(cell: Rect, fit: f32) -> Option<Rect> {
    let inner = paperdoll_socket_inner_bounds(cell, fit);
    if inner.width() < 12.0 || inner.height() < 12.0 {
        return None;
    }
    let mx = (inner.width() * 0.11).clamp(3.5 * fit, 11.0 * fit);
    let my = (inner.height() * 0.11).clamp(3.5 * fit, 11.0 * fit);
    let w = (inner.width() - 2.0 * mx).max(0.0);
    let h = (inner.height() - 2.0 * my).max(0.0);
    if w < 10.0 || h < 10.0 {
        return None;
    }
    let icon_r = Rect::from_xywh(inner.x() + mx, inner.y() + my, w, h);
    Some(optical_nudge_empty_icon_rect(icon_r, fit))
}

/// Socket shading is heavier top-left; nudge the square slightly down-right so it reads centered.
fn optical_nudge_empty_icon_rect(icon_r: Rect, fit: f32) -> Rect {
    let u = icon_r.width().min(icon_r.height());
    let nx = (u * 0.014).clamp(0.4 * fit, 1.6 * fit);
    let ny = (u * 0.022).clamp(0.6 * fit, 2.4 * fit);
    Rect::from_xywh(
        icon_r.x() + nx,
        icon_r.y() + ny,
        icon_r.width(),
        icon_r.height(),
    )
}

/// Small inset “chip” behind the tinted atlas glyph so it sits slightly recessed in the well.
fn draw_empty_paperdoll_icon_chip(ui: &mut Ui<'_>, icon_r: Rect, fit: f32) {
    let cr = (icon_r.height() * 0.14).clamp(2.5 * fit, 7.0 * fit);
    ui.draw_rounded_rect(icon_r, cr, Color::rgba(0.0, 0.0, 0.0, 0.22));
    let w = icon_r.width();
    let h = icon_r.height();
    if w <= 4.0 || h <= 4.0 {
        return;
    }
    ui.draw_rect(
        Rect::from_xywh(icon_r.x() + 1.0, icon_r.y() + 1.0, w - 2.0, 1.0),
        CELL_INSET_SHADOW,
    );
    ui.draw_rect(
        Rect::from_xywh(icon_r.x() + 1.0, icon_r.y() + 1.0, 1.0, h - 2.0),
        CELL_INSET_SHADOW,
    );
    ui.draw_rect(
        Rect::from_xywh(icon_r.x() + 1.0, icon_r.max.y - 2.0, w - 2.0, 1.0),
        CELL_INSET_LIGHT,
    );
    ui.draw_rect(
        Rect::from_xywh(icon_r.max.x - 2.0, icon_r.y() + 1.0, 1.0, h - 2.0),
        CELL_INSET_LIGHT,
    );
}

/// Covers a bag anchor with empty-cell chrome after a bag-to-equip drop, when the bag grid
/// was painted before the paperdoll consumed the release (same-frame ordering).
pub(super) fn paint_bag_in_transit_cover(
    ui: &mut Ui<'_>,
    layout: &Layout,
    bag_in: &BagPanelIn<'_>,
    items: &[Option<ItemView<'_>>],
) {
    let Some(InTransitSource::Bag(idx)) = bag_in.in_transit else {
        return;
    };
    let cols = bag_in.bag_cols;
    let rows = bag_in.bag_rows;
    let placements = pack_bag(
        items,
        |_, it: &ItemView<'_>| (it.cell_w.max(1), it.cell_h.max(1)),
        cols,
        rows,
    );
    let Some(Some((x, y, w, h))) = placements.get(idx as usize) else {
        return;
    };
    let rect = layout.bag_rect(*x, *y, *w, *h);
    ui.draw_rect(rect, bag_empty_fill());
    draw_cell_outline(ui, rect);
}

#[derive(Clone, Copy)]
enum DragPaperdollHint {
    Inactive,
    NonEquipPayload,
    Gear(Option<u8>),
}

fn drag_paperdoll_hint<'a>(ui: &Ui<'_>, bag_in: &BagPanelIn<'a>) -> DragPaperdollHint {
    let Some(src) = ui.drag_payload::<DragSource>().copied() else {
        return DragPaperdollHint::Inactive;
    };
    let cell = match src {
        DragSource::Bag(i) => bag_in.items.get(i as usize),
        DragSource::Equip(s) => bag_in.equipment.get(s.0 as usize),
        DragSource::Stash(i) => bag_in.stash_active.get(i as usize),
    };
    let Some(it) = cell.and_then(|o| o.as_ref()) else {
        return DragPaperdollHint::Inactive;
    };
    if it.is_consumable {
        return DragPaperdollHint::NonEquipPayload;
    }
    DragPaperdollHint::Gear(it.equip_native_slot)
}

fn draw_paperdoll_drag_affordance(
    ui: &mut Ui<'_>,
    rect: Rect,
    slot: EquipSlotIdx,
    hovered: bool,
    hint: DragPaperdollHint,
    fit: f32,
    theme: &rift_ui_im::Theme,
) {
    let DragPaperdollHint::Gear(native) = hint else {
        return;
    };
    let pad = (2.0 * fit).max(1.0);
    let shell = Rect::from_xywh(
        rect.x() + pad,
        rect.y() + pad,
        (rect.width() - 2.0 * pad).max(0.0),
        (rect.height() - 2.0 * pad).max(0.0),
    );
    if shell.width() < 4.0 || shell.height() < 4.0 {
        return;
    }
    let sr = (shell.height() * 0.085).clamp(2.0 * fit.min(6.0), 7.0);

    if slot.accepts_equip_drag(native) {
        let s = theme.colors.success.0;
        ui.draw_rounded_rect(shell, sr, Color::rgba(s[0], s[1], s[2], 0.055));
        return;
    }

    if native.is_some() && hovered {
        ui.draw_rounded_rect(shell, sr, Color::rgba(0.85, 0.20, 0.22, 0.045));
    }
}

#[derive(Default)]
pub struct PaperdollOut {
    pub hovered_equip: Option<EquipSlotIdx>,
    pub in_transit_from_drop: Option<rift_ui_types::inventory::InTransitSource>,
    pub in_transit_dest_rect_from_drop: Option<[f32; 4]>,
}

pub fn render_paperdoll(
    ui: &mut Ui<'_>,
    layout: &Layout,
    bag_in: &BagPanelIn<'_>,
    out_actions: &mut Vec<InventoryAction>,
    out: &mut PaperdollOut,
) {
    let theme = *ui.theme();
    out.hovered_equip = None;

    // Section background — carved-stone slab, darker than the
    // bag so the equipment niche reads as recessed.
    draw_section_chrome(ui, &theme, layout.paperdoll, true);

    let slot_fill = equip_slot_fill();
    let drag_hint = drag_paperdoll_hint(ui, bag_in);

    for i in 0..EquipSlotIdx::COUNT {
        let slot = EquipSlotIdx(i as u8);
        let rect = layout.paperdoll_slot_rect(slot.0);
        let id = Id::root("inv").child(("equip", i));
        let item = bag_in.equipment.get(i).and_then(|o| o.as_ref());

        // Cell chrome: darker fill + recessed inner well + accent outline.
        ui.draw_rect(rect, slot_fill);
        draw_paperdoll_socket_inset(ui, rect, layout.fit);
        draw_cell_outline(ui, rect);

        let hovered_prep = ui.interact_hover(id, rect);
        draw_paperdoll_drag_affordance(ui, rect, slot, hovered_prep, drag_hint, layout.fit, &theme);

        // ALWAYS call interact — `payload=None` for empty
        // slots still registers the rect as a drop target.
        let drag_payload = item.map(|_| DragSource::Equip(slot));
        let payload = if item.map(|it| it.forge_locked).unwrap_or(false) {
            None
        } else {
            drag_payload
        };
        // Hide the source slot while it's being dragged so
        // only the in-flight ghost is visible.
        let dragging_this = matches!(ui.drag_payload::<DragSource>().copied(), Some(DragSource::Equip(s)) if s.0 == slot.0);
        let in_transit_this = matches!(
            bag_in.in_transit,
            Some(rift_ui_types::inventory::InTransitSource::Equip(s)) if s == slot.0
        );
        let being_dragged = dragging_this || in_transit_this;
        let render_item = if being_dragged { None } else { item };
        let r = if render_item.is_some() {
            build_item_slot(render_item).interact::<DragSource>(ui, rect, id, payload)
        } else if item.is_some() {
            // Item is hidden because it's being dragged but
            // we still need a drop target with the original
            // payload so swaps land back here.
            ItemSlot::new(rect.width().min(rect.height()))
                .transparent_bg(true)
                .interact::<DragSource>(ui, rect, id, payload)
        } else {
            // Plain empty slot: no icon, no rarity tint.
            ItemSlot::new(rect.width().min(rect.height()))
                .transparent_bg(true)
                .interact::<DragSource>(ui, rect, id, payload)
        };
        let hovered = r.response.hovered;
        let forge_locked = item.map(|it| it.forge_locked).unwrap_or(false);
        if !forge_locked {
            route_slot_capture_equip_dest(
                r,
                DropTarget::Equip {
                    slot,
                    occupied: item.is_some(),
                },
                bag_in.stash_open,
                false,
                bag_in.active_tab_u8,
                false,
                bag_in.void_forge_open,
                out_actions,
                &mut out.in_transit_from_drop,
                &mut out.in_transit_dest_rect_from_drop,
                rect,
            );
        }

        if item.is_some() {
            if hovered {
                out.hovered_equip = Some(slot);
            }
        } else if let Some(icon_r) = paperdoll_empty_icon_rect(rect, layout.fit) {
            draw_empty_paperdoll_icon_chip(ui, icon_r, layout.fit);
            ui.draw_icon_silhouette(
                icon_r,
                equip_slot_placeholder_icon_key(slot.0),
                PAPERDOLL_EMPTY_ICON_TINT,
            );
        }
    }
}

pub fn render_bag_grid(
    ui: &mut Ui<'_>,
    layout: &Layout,
    bag_in: &BagPanelIn<'_>,
    armed_bag_idx_in: Option<u32>,
    out_actions: &mut Vec<InventoryAction>,
) -> BagPanelOut {
    let theme = *ui.theme();
    let mut out = BagPanelOut::default();

    // Section background — same carved-stone slab as the
    // paperdoll so the two read as parts of one container.
    draw_section_chrome(ui, &theme, layout.bag, false);

    let cols = bag_in.bag_cols;
    let rows = bag_in.bag_rows;

    // First-fit packing — work out which cells are filled so
    // we can paint empty-cell drop targets for the rest.
    let placements = pack_bag(
        bag_in.items,
        |_, it: &ItemView<'_>| (it.cell_w.max(1), it.cell_h.max(1)),
        cols,
        rows,
    );

    let cols_us = cols as usize;
    let rows_us = rows as usize;
    let mut filled = vec![false; cols_us * rows_us];
    // Per-cell owning anchor index: lets the snap-anchor
    // resolver look up "which item lives at this cell" so a
    // multi-cell ghost dropped onto another item's footprint
    // routes the swap against that item's anchor index, not a
    // random covered cell.
    let mut cell_owner: Vec<Option<u32>> = vec![None; cols_us * rows_us];
    for (idx, slot) in bag_in.items.iter().enumerate() {
        if slot.is_none() {
            continue;
        }
        let Some((x, y, w, h)) = placements[idx] else {
            continue;
        };
        for dy in 0..h as usize {
            for dx in 0..w as usize {
                let c = (y as usize + dy) * cols_us + (x as usize + dx);
                filled[c] = true;
                cell_owner[c] = Some(idx as u32);
            }
        }
    }

    // ── Snap-anchor drag preview & central drop resolver ──
    // Routes drops to a specific bag anchor (or the item
    // owning that anchor if the footprint overlaps it),
    // bypassing per-cell drop targets so swaps always land
    // where the user can see the green outline.
    let drag_pl = ui.drag_payload::<DragSource>().copied();
    if let Some(src) = drag_pl {
        let (src_w, src_h) = source_footprint(src, bag_in);
        let source_anchor_idx = match src {
            DragSource::Bag(i) => Some(i),
            _ => None,
        };
        let grid_rect = Rect::from_xywh(
            layout.bag_origin.x,
            layout.bag_origin.y,
            layout.bag_cell * cols as f32,
            layout.bag_cell * rows as f32,
        );
        let grid = GridSpec {
            rect: grid_rect,
            cell_px: layout.bag_cell,
            cols,
            rows,
            cell_owner: &cell_owner,
        };
        snap_preview_and_resolve(
            ui,
            &grid,
            src_w,
            src_h,
            source_anchor_idx,
            bag_in.stash_open,
            bag_in.active_tab_u8,
            bag_in.forge_anchor,
            DropTarget::Bag,
            out_actions,
            &mut out.in_transit_from_drop,
            &mut out.in_transit_dest_rect_from_drop,
        );
    }

    // Empty-cell pass — flush flat squares with drop targets.
    for cy in 0..rows {
        for cx in 0..cols {
            if filled[cy as usize * cols_us + cx as usize] {
                continue;
            }
            let cell_idx = cy as u32 * cols as u32 + cx as u32;
            let rect = layout.bag_rect(cx, cy, 1, 1);
            // Dark wash + outline so the empty cell reads as
            // a well in the stone slab. Transparent would
            // leave only the gold outline and the cell would
            // blend into the section's stone texture at
            // distance.
            ui.draw_rect(rect, bag_empty_fill());
            draw_cell_outline(ui, rect);

            // Map empty cell back to its inventory index 1:1
            // (bag storage is BAG_COLS × BAG_ROWS, row-major).
            // Drops resolve into this exact cell so the user
            // can position items deliberately.
            let inv_idx = cell_idx;
            let id = Id::root("inv").child(("bag_empty", inv_idx));
            let r = ItemSlot::new(layout.bag_cell)
                .transparent_bg(true)
                .interact::<DragSource>(ui, rect, id, None::<DragSource>);
            route_slot_capture(
                r,
                DropTarget::Bag(inv_idx),
                bag_in.stash_open,
                false,
                bag_in.active_tab_u8,
                false,
                bag_in.void_forge_open,
                out_actions,
                &mut out.in_transit_from_drop,
            );
        }
    }

    // Filled item pass — every filled item is rendered last so
    // its rect overlays the empty-cell outlines beneath it.
    for (idx, slot_opt) in bag_in.items.iter().enumerate() {
        let Some(item) = slot_opt.as_ref() else {
            continue;
        };
        let forge_locked = item.forge_locked;
        let Some((x, y, w, h)) = placements[idx] else {
            continue;
        };
        let rect = layout.bag_rect(x, y, w, h);
        let id = Id::root("inv").child(("bag", idx as u32));
        let payload = if forge_locked {
            None
        } else {
            Some(DragSource::Bag(idx as u32))
        };

        // Transparent: let the stone frame and the rarity
        // outline carry the visual weight.

        let dragging_this = matches!(
            ui.drag_payload::<DragSource>().copied(),
            Some(DragSource::Bag(i)) if i == idx as u32
        );
        // Combine the freshly-set drop result with the
        // (frame-stale) in-transit value so the source slot
        // stays hidden on the *same* frame the drop fires —
        // otherwise the drop arms in_transit too late and the
        // source briefly re-renders for one frame before the
        // hide takes effect on frame+1.
        let effective_in_transit = out.in_transit_from_drop.or(bag_in.in_transit);
        let in_transit_this = matches!(
            effective_in_transit,
            Some(rift_ui_types::inventory::InTransitSource::Bag(i)) if i == idx as u32
        );
        let being_dragged = dragging_this || in_transit_this;
        // Hidden source: paint empty-cell chrome (gold
        // outline + inset highlight) for every covered cell
        // so the grid still reads as a slot grid while the
        // ghost is in flight.
        if being_dragged {
            for dy in 0..h {
                for dx in 0..w {
                    let cr = layout.bag_rect(x + dx, y + dy, 1, 1);
                    draw_cell_outline(ui, cr);
                }
            }
        }
        let r = if being_dragged {
            ItemSlot::new(rect.width().min(rect.height()))
                .transparent_bg(true)
                .interact::<DragSource>(ui, rect, id, payload)
        } else {
            build_item_slot(Some(item)).interact::<DragSource>(ui, rect, id, payload)
        };
        let hovered = r.response.hovered;

        let salvage_allowed = !item.forge_locked;
        if salvage_allowed && r.response.pressed && ui.ctrl_held() {
            out.salvage_press_bag_idx = Some(idx as u32);
        }
        let armed_for_this =
            armed_bag_idx_in == Some(idx as u32) || out.salvage_press_bag_idx == Some(idx as u32);
        let ctrl_release =
            salvage_allowed && armed_for_this && (r.clicked || r.response.drag_released);
        if ctrl_release {
            out.salvage_release_bag_idx = Some(idx as u32);
        } else if !forge_locked {
            route_slot_capture(
                r,
                DropTarget::Bag(idx as u32),
                bag_in.stash_open,
                false,
                bag_in.active_tab_u8,
                item.is_consumable,
                bag_in.void_forge_open,
                out_actions,
                &mut out.in_transit_from_drop,
            );
        }
        if hovered {
            out.hovered_bag = Some(idx as u32);
            out.hovered_bag_rect = Some(rect);
        }

        // Rarity-tinted outline on top. Skipped while the
        // slot is the drag source so it reads as empty.
        if !being_dragged {
            let [rr, gg, bb, _] = item.rarity_color;
            ui.draw_outline(rect, 1.5, Color::rgba(rr, gg, bb, 0.95));
        }
    }

    out
}

/// Look up the dragged item's cell footprint from whichever
/// source view holds it. Stash items aren't carried through
/// [`BagPanelIn`], so a stash → bag drag falls back to a 1×1
/// preview (still anchor-correct, just visually conservative).
fn source_footprint(src: DragSource, bag_in: &BagPanelIn<'_>) -> (u8, u8) {
    match src {
        DragSource::Bag(idx) => bag_in
            .items
            .get(idx as usize)
            .and_then(|o| o.as_ref())
            .map(|it| (it.cell_w.max(1), it.cell_h.max(1)))
            .unwrap_or((1, 1)),
        DragSource::Equip(slot) => bag_in
            .equipment
            .get(slot.0 as usize)
            .and_then(|o| o.as_ref())
            .map(|it| (it.cell_w.max(1), it.cell_h.max(1)))
            .unwrap_or((1, 1)),
        DragSource::Stash(idx) => bag_in
            .stash_active
            .get(idx as usize)
            .and_then(|o| o.as_ref())
            .map(|it| (it.cell_w.max(1), it.cell_h.max(1)))
            .unwrap_or((1, 1)),
    }
}
