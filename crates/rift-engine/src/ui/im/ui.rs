//! Per-frame immediate-mode UI context.
//!
//! Constructed once at the start of the UI draw step via
//! [`Ui::begin`], threaded through every screen / widget call,
//! and finalized with [`Ui::end`] which flushes layered draws
//! into the renderer's [`OverlayBatch`] and reports whether the
//! UI consumed mouse / keyboard input this frame.
//!
//! Landing 1 ships only the foundation: types, layered draw
//! recording, hover tracking, and the public draw helpers
//! (`draw_rect`, `draw_text`, `draw_icon`). Higher-level widgets
//! (button, text_field, item_slot, …) are added in subsequent
//! landings — but they all funnel into the same `Ui` so swapping
//! a hand-rolled panel for a real widget is a local edit.

use crate::input::Input;
use crate::renderer::OverlayBatch;

use super::color::Color;
use super::id::Id;
use super::layer::{DrawCmd, Layer, LayerBuf};
use super::rect::{Pos2, Rect, Vec2};
use super::response::Response;
use super::state::{DragState, UiState};
use super::theme::Theme;

/// Outcome of [`Ui::end`]. Consumed by the game loop to gate
/// world-space input (cast abilities, target picker) on the UI
/// not having eaten the click already.
#[derive(Debug, Clone, Copy, Default)]
pub struct UiOutput {
    /// `true` if any widget claimed the mouse this frame (hovered
    /// inside a panel rect, started a drag, button press, …).
    pub mouse_claimed: bool,
    /// `true` if a focused widget consumed keystrokes (text field,
    /// modal Esc handler).
    pub keyboard_claimed: bool,
}

/// Per-frame context. Holds borrows of the renderer batch, input
/// state, persistent UI state, and the theme; carries the
/// transient layered draw queue and the current draw layer.
pub struct Ui<'a> {
    batch: &'a mut OverlayBatch,
    input: &'a Input,
    state: &'a mut UiState,
    theme: &'a Theme,
    screen: Vec2,
    layers: LayerBuf,
    /// Layer that subsequent `draw_*` calls record into. Mutated
    /// by `with_layer` (RAII swap) so widgets can render their
    /// tooltip on `Layer::Tooltip` regardless of where they were
    /// invoked from.
    current_layer: Layer,
    /// Optional clip rect for hit-testing only (we don't actually
    /// scissor draws yet — `OverlayBatch` would need real clip
    /// support for that). For now widgets that scroll set this
    /// so out-of-rect children don't claim hover.
    clip: Rect,
}

impl<'a> Ui<'a> {
    /// Start a new frame. Resets transient bookkeeping in `state`
    /// (the previous frame's hover candidate is promoted into
    /// `hovered_last_frame` so this-frame interactions can read it).
    pub fn begin(
        batch: &'a mut OverlayBatch,
        input: &'a Input,
        state: &'a mut UiState,
        theme: &'a Theme,
        screen_w: f32,
        screen_h: f32,
    ) -> Self {
        // Promote the previous frame's hover candidate so widgets
        // checking `is_hovered` this frame see a stable value.
        state.hovered_last_frame = state.hovered_this_frame.take();
        state.mouse_claimed = false;
        state.keyboard_claimed = false;
        let screen = Vec2::new(screen_w, screen_h);
        Self {
            batch,
            input,
            state,
            theme,
            screen,
            layers: LayerBuf::new(),
            current_layer: Layer::Panel,
            clip: Rect::from_screen(screen_w, screen_h),
        }
    }

    /// Finalize: flush all queued draws into the renderer batch
    /// in layer order and return the input-consumption summary.
    /// After this call, `self` is dropped and `state` regains
    /// exclusive ownership.
    pub fn end(mut self) -> UiOutput {
        self.layers.flush(self.batch, self.screen.x, self.screen.y);
        UiOutput {
            mouse_claimed: self.state.mouse_claimed,
            keyboard_claimed: self.state.keyboard_claimed,
        }
    }

    // ─── accessors ──────────────────────────────────────────────

    pub fn input(&self) -> &Input {
        self.input
    }

    pub fn theme(&self) -> &Theme {
        self.theme
    }

    pub fn state(&self) -> &UiState {
        self.state
    }

    /// Mutable access to the persistent state, mostly for screens
    /// that need to push/pop modals or programmatically focus a
    /// widget at the start of the frame.
    pub fn state_mut(&mut self) -> &mut UiState {
        self.state
    }

    /// Screen rect spanning `(0,0)..(screen_w, screen_h)`.
    pub fn screen_rect(&self) -> Rect {
        Rect::from_xywh(0.0, 0.0, self.screen.x, self.screen.y)
    }

    pub fn screen_size(&self) -> Vec2 {
        self.screen
    }

    /// Current cursor position in pixel space.
    pub fn mouse_pos(&self) -> Pos2 {
        let (x, y) = self.input.mouse_pos();
        Pos2::new(x, y)
    }

    /// `true` if either Shift is held this frame. Provided so
    /// widgets / screens don't have to import winit just to
    /// branch on the shift modifier (compare-tooltips, splitting
    /// stacks, …).
    pub fn shift_held(&self) -> bool {
        use winit::keyboard::KeyCode;
        self.input.is_key_held(KeyCode::ShiftLeft) || self.input.is_key_held(KeyCode::ShiftRight)
    }

    /// `true` if either Ctrl is held this frame.
    pub fn ctrl_held(&self) -> bool {
        use winit::keyboard::KeyCode;
        self.input.is_key_held(KeyCode::ControlLeft)
            || self.input.is_key_held(KeyCode::ControlRight)
    }

    // ─── layer scoping ──────────────────────────────────────────

    /// Run `body` with `layer` as the active draw layer. Restores
    /// the previous layer on return, so it's safe to nest.
    pub fn with_layer<R>(&mut self, layer: Layer, body: impl FnOnce(&mut Ui<'_>) -> R) -> R {
        let prev = self.current_layer;
        self.current_layer = layer;
        let result = body(self);
        self.current_layer = prev;
        result
    }

    /// Run `body` with `clip` as the active hit-test clip rect.
    /// Restores the previous clip on return.
    pub fn with_clip<R>(&mut self, clip: Rect, body: impl FnOnce(&mut Ui<'_>) -> R) -> R {
        let prev = self.clip;
        self.clip = self.clip.intersect(clip);
        let result = body(self);
        self.clip = prev;
        result
    }

    // ─── interaction helpers (used by widgets) ──────────────────

    /// Mark the mouse as claimed for this frame. Widgets call
    /// this when they process a click / hover so the game's
    /// world-space picking knows to skip.
    pub fn claim_mouse(&mut self) {
        self.state.mouse_claimed = true;
    }

    /// Mark keyboard input as claimed (focused text field, modal
    /// Esc handler, …).
    pub fn claim_keyboard(&mut self) {
        self.state.keyboard_claimed = true;
    }

    /// Hit-test a widget's rect against the current clip and
    /// register the widget as the hover candidate if the cursor
    /// is over it. Returns `true` if hovered. Always call this
    /// before reading mouse-button state for the widget so the
    /// hover bookkeeping is consistent.
    pub fn interact_hover(&mut self, id: Id, rect: Rect) -> bool {
        let mp = self.mouse_pos();
        let inside = self.clip.contains(mp) && rect.contains(mp);
        if inside {
            // Last write wins — widgets drawn later (higher in
            // the layer order) take precedence as the hover
            // owner, matching how a player would expect.
            self.state.hovered_this_frame = Some(id);
            self.state.mouse_claimed = true;
        }
        inside
    }

    /// Convenience: hover-only interaction (no click/drag). Used
    /// by composite widgets that just want a hovered flag for
    /// styling.
    pub fn hover_only(&mut self, id: Id, rect: Rect) -> Response {
        let hovered = self.interact_hover(id, rect);
        Response {
            id,
            rect,
            hovered,
            pressed: false,
            clicked: false,
            drag_started: false,
            drag_released: false,
            focused: self.state.focus == Some(id),
        }
    }

    // ─── drag-and-drop ──────────────────────────────────────────
    //
    // The IM stack owns the drag state machine so screens
    // (inventory, hotbar, character paper-doll, …) only have to
    // declare their slots and the actions to take on drop. The
    // payload is type-erased: each source attaches a `T: Any`
    // when the drag begins and each target downcasts on release.
    //
    // The threshold-vs-click distinction is handled here: a
    // mouse press on a slot starts a *latent* drag; the drag
    // becomes *active* the first frame the cursor moves more
    // than `DRAG_THRESHOLD_PX` from the press point. If the
    // mouse is released before the threshold is crossed, the
    // source's `clicked` bit fires instead.

    /// Pixel distance the cursor must travel between press and
    /// release for an interaction to count as a drag (and not a
    /// click). Mirrors the value the legacy inventory used.
    pub const DRAG_THRESHOLD_PX: f32 = 6.0;

    /// Register `id` as a drag-source for the rect that was just
    /// hover-tested (`hovered`). Returns the interaction
    /// `Response` plus a pair of bits describing this frame's
    /// drag state (`drag_started`, `clicked_no_drag`). Call
    /// once per slot, immediately after `interact_hover`.
    ///
    /// `payload` is the value the drop target will receive on
    /// release — only invoked the frame the drag actually starts
    /// (so callers can capture e.g. an `Item` clone without
    /// paying for it on every hover).
    pub fn drag_source<T, F>(
        &mut self,
        id: Id,
        rect: Rect,
        hovered: bool,
        make_payload: F,
    ) -> DragSourceResponse
    where
        T: 'static + Send + Sync,
        F: FnOnce() -> T,
    {
        let mp = self.mouse_pos();
        let mut started = false;
        let mut clicked_no_drag = false;

        // Press: open a latent drag if we don't already have one.
        if hovered && self.input.left_just_pressed() && self.state.drag.is_none() {
            self.state.drag = Some(DragState::new(id, mp, make_payload()));
        }

        // Promote latent → active once threshold is crossed.
        if let Some(drag) = self.state.drag.as_mut() {
            if drag.source == id && !drag.active {
                let dx = mp.x - drag.press_pos.x;
                let dy = mp.y - drag.press_pos.y;
                if (dx * dx + dy * dy).sqrt() > Self::DRAG_THRESHOLD_PX {
                    drag.active = true;
                    started = true;
                }
            }
        }

        // Release while latent (no movement) → it was a click.
        // We *don't* clear the drag here; `take_drop` resolves
        // it on release for the destination side.
        if hovered && self.input.left_just_released() {
            if let Some(drag) = self.state.drag.as_ref() {
                if drag.source == id && !drag.active {
                    clicked_no_drag = true;
                }
            }
        }

        DragSourceResponse {
            response: Response {
                id,
                rect,
                hovered,
                pressed: hovered && self.input.left_just_pressed(),
                clicked: clicked_no_drag,
                drag_started: started,
                drag_released: hovered && self.input.left_just_released(),
                focused: self.state.focus == Some(id),
            },
            drag_started: started,
            clicked_no_drag,
        }
    }

    /// True iff a drag is in progress and the cursor is over
    /// `rect`. The widget should render its highlight off this
    /// (hover ring, dashed border, …).
    pub fn is_drag_target<T: 'static>(&self, rect: Rect) -> bool {
        if let Some(drag) = self.state.drag.as_ref() {
            if !drag.active {
                return false;
            }
            if drag.payload.downcast_ref::<T>().is_none() {
                return false;
            }
            let mp = self.mouse_pos();
            return rect.contains(mp);
        }
        false
    }

    /// If a drag of payload type `T` was released over `rect`
    /// this frame, consume it and return the payload + the
    /// source widget id. Otherwise returns `None`.
    pub fn take_drop<T: 'static + Send + Sync>(
        &mut self,
        rect: Rect,
    ) -> Option<DroppedPayload<T>> {
        if !self.input.left_just_released() {
            return None;
        }
        let drag = self.state.drag.as_ref()?;
        if !drag.active {
            return None;
        }
        let mp = self.mouse_pos();
        if !rect.contains(mp) {
            return None;
        }
        if drag.payload.downcast_ref::<T>().is_none() {
            return None;
        }
        let drag = self.state.drag.take().unwrap();
        let source = drag.source;
        let payload = match drag.payload.downcast::<T>() {
            Ok(b) => *b,
            Err(_) => return None,
        };
        Some(DroppedPayload { source, payload })
    }

    /// True iff the active drag's source widget id matches `id`.
    /// Useful for source slots that want to render themselves
    /// dimmed/empty while the drag ghost is in flight.
    pub fn is_being_dragged(&self, id: Id) -> bool {
        self.state
            .drag
            .as_ref()
            .map(|d| d.active && d.source == id)
            .unwrap_or(false)
    }

    /// True iff *any* drag is currently active. Game code reads
    /// this to suppress world-space click handling so a release
    /// outside every slot drops the item on the ground without
    /// also firing the player's basic attack.
    pub fn drag_active(&self) -> bool {
        self.state
            .drag
            .as_ref()
            .map(|d| d.active)
            .unwrap_or(false)
    }

    /// Cancel an in-flight drag without resolving any drop.
    /// Called by the screen on Esc, panel-close, etc.
    pub fn cancel_drag(&mut self) {
        self.state.drag = None;
    }

    /// Typed peek at the active drag's payload. Returns `None`
    /// if there is no drag, the drag is still latent (below
    /// threshold), or the payload is not of type `T`. Lets
    /// widgets render a drag ghost without reaching into
    /// [`UiState::drag`] directly.
    pub fn drag_payload<T: 'static>(&self) -> Option<&T> {
        let drag = self.state.drag.as_ref()?;
        if !drag.active {
            return None;
        }
        drag.payload.downcast_ref::<T>()
    }

    /// If a drag of payload `T` was released *outside* every
    /// drop target this frame, consume it and return the source
    /// id + payload. Used by the inventory to detect "drop on
    /// world" (drop loot to ground). Call this *after* every
    /// `take_drop` for the same payload type.
    pub fn take_drop_outside<T: 'static + Send + Sync>(
        &mut self,
    ) -> Option<DroppedPayload<T>> {
        if !self.input.left_just_released() {
            return None;
        }
        let drag = self.state.drag.as_ref()?;
        if !drag.active {
            return None;
        }
        if drag.payload.downcast_ref::<T>().is_none() {
            return None;
        }
        let drag = self.state.drag.take().unwrap();
        let payload = match drag.payload.downcast::<T>() {
            Ok(b) => *b,
            Err(_) => return None,
        };
        Some(DroppedPayload {
            source: drag.source,
            payload,
        })
    }

    // ─── primitive draw helpers ─────────────────────────────────
    //
    // Widgets in L2/L3 build on these. They funnel through the
    // current layer so tooltips/drag-ghosts sort correctly
    // without callers thinking about it.

    /// Filled rect.
    pub fn draw_rect(&mut self, rect: Rect, color: Color) {
        self.layers.push(self.current_layer, DrawCmd::Rect { rect, color });
    }

    /// Filled rect with rounded corners. `radius` is clamped to
    /// half the smaller side; pass `0.0` for a sharp rect (cheaper).
    pub fn draw_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
        if radius <= 0.0 {
            self.draw_rect(rect, color);
        } else {
            self.layers.push(
                self.current_layer,
                DrawCmd::RoundedRect { rect, radius, color },
            );
        }
    }

    /// Rounded outline composed of four edge rects + the four
    /// corner arcs. For sharp outlines (`radius == 0`) falls back
    /// to [`Self::draw_outline`].
    pub fn draw_rounded_outline(
        &mut self,
        rect: Rect,
        radius: f32,
        thickness: f32,
        color: Color,
    ) {
        if thickness <= 0.0 {
            return;
        }
        if radius <= 0.0 {
            self.draw_outline(rect, thickness, color);
            return;
        }
        // Outer ring drawn as the rounded rect minus an inner
        // shrunk rounded rect would require a real stencil; for a
        // 1\u20132 px stroke the cheap path is "draw the rounded fill
        // and immediately stamp a shrunk fill of the panel colour
        // on top". That couples the stroke to the underlying fill,
        // which we don't want. Instead, approximate the ring with
        // four sharp edge rects (corners rounded by the fill). For
        // 1\u20132 px borders against typical UI radii (\u22644 px) the
        // visual difference vs a true rounded ring is invisible.
        let t = thickness;
        // Top
        self.draw_rect(Rect::from_xywh(rect.x() + radius, rect.y(), rect.width() - 2.0 * radius, t), color);
        // Bottom
        self.draw_rect(
            Rect::from_xywh(rect.x() + radius, rect.max.y - t, rect.width() - 2.0 * radius, t),
            color,
        );
        // Left
        self.draw_rect(
            Rect::from_xywh(rect.x(), rect.y() + radius, t, rect.height() - 2.0 * radius),
            color,
        );
        // Right
        self.draw_rect(
            Rect::from_xywh(rect.max.x - t, rect.y() + radius, t, rect.height() - 2.0 * radius),
            color,
        );
    }

    /// 1-pixel-thick rect outline composed of four edges.
    pub fn draw_outline(&mut self, rect: Rect, thickness: f32, color: Color) {
        let t = thickness.max(0.0);
        if t <= 0.0 {
            return;
        }
        // Top
        self.draw_rect(Rect::from_xywh(rect.x(), rect.y(), rect.width(), t), color);
        // Bottom
        self.draw_rect(
            Rect::from_xywh(rect.x(), rect.max.y - t, rect.width(), t),
            color,
        );
        // Left
        self.draw_rect(Rect::from_xywh(rect.x(), rect.y() + t, t, rect.height() - 2.0 * t), color);
        // Right
        self.draw_rect(
            Rect::from_xywh(rect.max.x - t, rect.y() + t, t, rect.height() - 2.0 * t),
            color,
        );
    }

    /// Draw text at `pos` (top-left). Returns the rendered width.
    pub fn draw_text(&mut self, pos: Pos2, text: &str, size: f32, color: Color) -> f32 {
        // Width measurement matches OverlayBatch's bitmap font:
        // fixed-width glyphs scaled by `size / glyph_height`.
        let measured = self.measure_text(text, size);
        self.layers.push(
            self.current_layer,
            DrawCmd::Text {
                text: text.to_string(),
                x: pos.x,
                y: pos.y,
                size,
                color,
            },
        );
        measured
    }

    /// Draw a registered icon by atlas name. Silently no-ops on
    /// an unknown name (matches `OverlayBatch::icon`).
    pub fn draw_icon(&mut self, rect: Rect, name: &str, tint: Color) {
        self.layers.push(
            self.current_layer,
            DrawCmd::Icon {
                name: name.to_string(),
                rect,
                tint,
            },
        );
    }

    /// Measure text width without recording a draw. Goes through
    /// the underlying batch's font metrics so widgets stay
    /// consistent with what `draw_text` will produce.
    pub fn measure_text(&self, text: &str, size: f32) -> f32 {
        self.batch.measure_text(text, size)
    }

    /// Draw text at `pos` truncated to fit within `max_width`,
    /// appending `…` if it doesn't fit. Returns the width
    /// actually drawn. Useful for footer hints, slot labels,
    /// and any layout that can't reflow.
    pub fn draw_text_ellipsized(
        &mut self,
        pos: Pos2,
        text: &str,
        size: f32,
        max_width: f32,
        color: Color,
    ) -> f32 {
        let full = self.measure_text(text, size);
        if full <= max_width {
            return self.draw_text(pos, text, size, color);
        }
        let ell = "\u{2026}";
        let ell_w = self.measure_text(ell, size);
        if ell_w >= max_width {
            return 0.0;
        }
        // Binary search the longest prefix that still fits with
        // the ellipsis appended. Cheap because text is short and
        // measure_text is O(n).
        let chars: Vec<char> = text.chars().collect();
        let mut lo = 0usize;
        let mut hi = chars.len();
        while lo < hi {
            let mid = (lo + hi + 1) / 2;
            let candidate: String = chars[..mid].iter().collect();
            let w = self.measure_text(&candidate, size) + ell_w;
            if w <= max_width {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        let mut s: String = chars[..lo].iter().collect();
        s.push_str(ell);
        self.draw_text(pos, &s, size, color)
    }
}

/// Outcome of [`Ui::drag_source`]. Bundles the standard
/// [`Response`] with the two drag-specific bits a typical caller
/// branches on (start of drag vs click-without-drag) so the
/// match arms read top-to-bottom.
#[derive(Debug, Clone, Copy)]
pub struct DragSourceResponse {
    /// Standard interaction response. `clicked` is set only when
    /// the press / release pair stayed within the drag threshold.
    pub response: Response,
    /// `true` the *one* frame the drag transitions from latent
    /// (mouse pressed) to active (cursor moved past threshold).
    pub drag_started: bool,
    /// `true` the frame the press was released on the source
    /// without ever crossing the threshold. Mirrors
    /// `Response::clicked` for callers that only want this bit.
    pub clicked_no_drag: bool,
}

/// Payload returned by [`Ui::take_drop`] / [`Ui::take_drop_outside`].
#[derive(Debug)]
pub struct DroppedPayload<T> {
    /// Widget id the drag originated on.
    pub source: Id,
    /// The value the source attached at drag-start. Owned;
    /// callers can move it freely.
    pub payload: T,
}
