//! Generic square icon slot used by both the inventory grid
//! and the ability bar.
//!
//! A slot draws (in z-order):
//! 1. Background fill (idle / hovered / disabled / selected).
//! 2. Optional rarity tint inset (so item-rarity reads at a glance).
//! 3. Icon from the atlas, or a fallback glyph if the icon is
//!    missing / not registered.
//! 4. Cooldown drain overlay (a darkening rect that shrinks
//!    from the top down as the cooldown ticks).
//! 5. Optional key-bind label in the bottom-left corner
//!    (`"1"`, `"LMB"`, …).
//! 6. Hover / selected outline.
//!
//! All of those are configurable via the builder; only `size`
//! and the call to [`ItemSlot::show`] are required.
//!
//! The widget is interaction-aware: it routes hover via the
//! `Ui` so the inventory + hotbar both participate in the
//! single mouse-claim model, and returns a [`Response`] with
//! `clicked` / `pressed` / `drag_started` / `drag_released`
//! bits that callers can branch on.

use super::super::{
    color::Color, id::Id, layer::Layer, response::Response, theme::Theme,
    ui::{DroppedPayload, Ui},
};
use crate::ui::im::rect::{Pos2, Rect};

/// Builder for one square slot.
///
/// Lifetimes refer to the borrowed icon-name / key-label
/// string slices so callers don't have to allocate.
pub struct ItemSlot<'a> {
    size: f32,
    icon: Option<&'a str>,
    rarity_tint: Option<Color>,
    fallback_glyph: Option<char>,
    fallback_color: Option<Color>,
    key_label: Option<&'a str>,
    /// Cooldown remaining as a fraction in `[0, 1]`. `0.0`
    /// means "ready" (no overlay). The overlay is drawn from
    /// the top down so a slot that just fired shows a full
    /// drain that retreats as the cooldown elapses.
    cooldown: f32,
    selected: bool,
    enabled: bool,
    /// `true` for items carrying the rare "Anchored" trait.
    /// Renders a distinctive gold outer outline so the trait
    /// is recognisable at a glance even when the rarity tint
    /// already maxes the slot's chroma.
    anchored: bool,
}

impl<'a> ItemSlot<'a> {
    pub fn new(size: f32) -> Self {
        Self {
            size,
            icon: None,
            rarity_tint: None,
            fallback_glyph: None,
            fallback_color: None,
            key_label: None,
            cooldown: 0.0,
            selected: false,
            enabled: true,
            anchored: false,
        }
    }

    pub fn icon(mut self, name: &'a str) -> Self {
        self.icon = Some(name);
        self
    }

    /// Background tint shown behind the icon (and used as the
    /// fallback-glyph colour when no `fallback_color` is set).
    /// Inventory passes the item's rarity colour here.
    pub fn rarity_tint(mut self, c: Color) -> Self {
        self.rarity_tint = Some(c);
        self
    }

    pub fn fallback_glyph(mut self, ch: char) -> Self {
        self.fallback_glyph = Some(ch);
        self
    }

    pub fn fallback_color(mut self, c: Color) -> Self {
        self.fallback_color = Some(c);
        self
    }

    pub fn key_label(mut self, k: &'a str) -> Self {
        self.key_label = Some(k);
        self
    }

    /// Pass the *remaining* fraction of the cooldown
    /// (`1.0` = just started, `0.0` = ready). Clamped.
    pub fn cooldown(mut self, frac: f32) -> Self {
        self.cooldown = frac.clamp(0.0, 1.0);
        self
    }

    pub fn selected(mut self, on: bool) -> Self {
        self.selected = on;
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// Mark this slot as carrying an Anchored item. Draws an
    /// additional gold outer outline on top of the normal
    /// hover/selected outline so the trait is recognisable
    /// across the bag, equipment panel, and stash.
    pub fn anchored(mut self, on: bool) -> Self {
        self.anchored = on;
        self
    }

    /// Draw the slot with `pos` as its top-left corner and
    /// return the interaction response. The slot reports
    /// `clicked` from a non-consuming down-edge; it does not
    /// eat the click, so callers wiring a drag source on top
    /// (see [`Self::interact`]) still see the press.
    pub fn show(self, ui: &mut Ui<'_>, pos: Pos2, id: Id) -> Response {
        let rect = Rect::from_xywh(pos.x, pos.y, self.size, self.size);
        self.show_rect(ui, rect, id)
    }

    /// Same as [`Self::show`] but with an explicit `rect`.
    pub fn show_rect(self, ui: &mut Ui<'_>, rect: Rect, id: Id) -> Response {
        let hovered = ui.interact_hover(id, rect);
        self.render_into(ui, rect, hovered);
        let input = ui.input();
        let pressed = self.enabled && hovered && input.left_just_pressed();
        let drag_released = self.enabled && hovered && input.left_just_released();
        Response {
            id,
            rect,
            hovered,
            pressed,
            // Non-consuming click report; pairs cleanly with
            // [`Self::interact`] / `ui.drag_source`.
            clicked: pressed,
            drag_started: false,
            drag_released,
            focused: false,
        }
    }

    /// One-call draw + drag-source + drop-zone. The slot is a
    /// drag source iff `payload` is `Some`; either way it's a
    /// drop target for `T`. Returns a unified
    /// [`SlotInteraction`] so callers don't have to dance the
    /// `drag_source` + `take_drop` pair manually.
    ///
    /// `payload` is consumed only on the press edge \u2014 if no
    /// drag starts this frame, the value is dropped at the end
    /// of the call. Inventory typically passes `Some(src.clone())`
    /// for filled slots and `None` for empty ones.
    pub fn interact<T>(
        self,
        ui: &mut Ui<'_>,
        rect: Rect,
        id: Id,
        payload: Option<T>,
    ) -> SlotInteraction<T>
    where
        T: 'static + Send + Sync,
    {
        let hovered = ui.interact_hover(id, rect);
        self.render_into(ui, rect, hovered);

        // Drop side first: a release on this slot resolves a
        // drag started on a previous frame.
        let dropped = ui.take_drop::<T>(rect);

        // Drag-source side: only register if the slot is non-
        // empty. The closure captures `payload` by move; the
        // engine only invokes it on the press edge.
        let (clicked, drag_started) = if let Some(p) = payload {
            let r = ui.drag_source(id, rect, hovered, move || p);
            (r.clicked_no_drag, r.drag_started)
        } else {
            (false, false)
        };

        SlotInteraction {
            response: Response {
                id,
                rect,
                hovered,
                pressed: hovered && ui.input().left_just_pressed(),
                clicked,
                drag_started,
                drag_released: hovered && ui.input().left_just_released(),
                focused: false,
            },
            clicked,
            drag_started,
            dropped,
        }
    }

    /// Render this slot configuration as a drag-ghost centred
    /// on the cursor, on the [`Layer::DragGhost`] layer.
    /// Reuses the same builder so the in-flight ghost reads as
    /// the same item the user picked up.
    pub fn show_ghost(self, ui: &mut Ui<'_>) {
        let mp = ui.mouse_pos();
        let s = self.size * 0.85;
        let rect = Rect::from_xywh(mp.x - s * 0.5, mp.y - s * 0.5, s, s);
        ui.with_layer(Layer::DragGhost, |ui| {
            self.render_into(ui, rect, false);
        });
    }

    /// Pure paint pass: draws the slot at `rect` and reads no
    /// input. Used by [`Self::show_rect`], [`Self::interact`],
    /// and [`Self::show_ghost`] so all three share one render
    /// path.
    fn render_into(&self, ui: &mut Ui<'_>, rect: Rect, hovered: bool) {
        let theme = *ui.theme();

        // Background.
        let bg = pick_bg(&theme, hovered, self.selected, self.enabled);
        ui.draw_rounded_rect(rect, theme.spacing.corner_radius, bg);

        // Rarity inset behind the icon (slightly inset so the
        // outer slot frame still reads).
        let inner = inner_rect(rect);
        if let Some(tint) = self.rarity_tint {
            let dimmed = Color::rgba(
                tint.0[0] * 0.55,
                tint.0[1] * 0.55,
                tint.0[2] * 0.55,
                1.0,
            );
            ui.draw_rect(inner, dimmed);
        }

        // Icon or fallback.
        if let Some(name) = self.icon {
            let tint = if self.enabled {
                Color::rgba(1.0, 1.0, 1.0, 1.0)
            } else {
                Color::rgba(0.45, 0.50, 0.60, 1.0)
            };
            ui.draw_icon(inner, name, tint);
        } else if let Some(ch) = self.fallback_glyph {
            let mut buf = [0u8; 4];
            let s: &str = ch.encode_utf8(&mut buf);
            let glyph_size = (rect.height() * 0.45).max(12.0);
            let tw = ui.measure_text(s, glyph_size);
            let color = self
                .fallback_color
                .or(self.rarity_tint)
                .unwrap_or(theme.colors.text);
            ui.draw_text(
                Pos2::new(
                    rect.x() + (rect.width() - tw) * 0.5,
                    rect.y() + (rect.height() - glyph_size) * 0.5,
                ),
                s,
                glyph_size,
                color,
            );
        }

        // Cooldown drain overlay (top-down).
        if self.cooldown > 0.0 {
            let cd_h = inner.height() * self.cooldown;
            let cd_rect = Rect::from_xywh(inner.x(), inner.y(), inner.width(), cd_h);
            ui.draw_rect(cd_rect, Color::rgba(0.0, 0.0, 0.0, 0.55));
        }

        // Key-bind label.
        if let Some(k) = self.key_label {
            let size = theme.fonts.size_sm;
            ui.draw_text(
                Pos2::new(rect.x() + 3.0, rect.max.y - size - 2.0),
                k,
                size,
                Color::rgba(0.7, 0.7, 0.7, 0.85),
            );
        }

        // Anchored ring sits underneath the hover/selected
        // outline so the active interaction state still wins
        // visually but the gold border remains visible at
        // rest. Width 2.0 to match the selected outline.
        if self.anchored {
            ui.draw_rounded_outline(
                rect,
                theme.spacing.corner_radius,
                2.0,
                Color::rgba(1.00, 0.78, 0.20, 0.95),
            );
        }

        // Selected / hover outline (drawn last so nothing
        // overlaps the highlight).
        if self.selected {
            ui.draw_rounded_outline(
                rect,
                theme.spacing.corner_radius,
                2.0,
                theme.colors.accent,
            );
        } else if hovered {
            ui.draw_rounded_outline(
                rect,
                theme.spacing.corner_radius,
                1.0,
                Color::rgba(0.55, 0.85, 1.0, 0.6),
            );
        }
    }
}

/// Outcome of [`ItemSlot::interact`]. Bundles draw, click and
/// drag/drop into one struct so call sites stay flat.
pub struct SlotInteraction<T> {
    pub response: Response,
    /// True iff a click happened *without* turning into a drag.
    pub clicked: bool,
    /// True the frame the drag actually crosses the threshold.
    pub drag_started: bool,
    /// `Some` iff a drag of `T` was released over this slot
    /// this frame. Inventory matches on `(source, payload)` to
    /// route the action.
    pub dropped: Option<DroppedPayload<T>>,
}

fn pick_bg(theme: &Theme, hovered: bool, selected: bool, enabled: bool) -> Color {
    if !enabled {
        return Color::rgba(0.07, 0.07, 0.10, 0.85);
    }
    if selected {
        return theme.colors.bg_slot_hover;
    }
    if hovered {
        return theme.colors.bg_slot_hover;
    }
    theme.colors.bg_slot
}

fn inner_rect(rect: Rect) -> Rect {
    let pad = (rect.width().min(rect.height()) * 0.06).max(2.0);
    Rect::from_xywh(
        rect.x() + pad,
        rect.y() + pad,
        rect.width() - 2.0 * pad,
        rect.height() - 2.0 * pad,
    )
}


