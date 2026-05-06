//! Layered drawing for the immediate-mode UI.
//!
//! `OverlayBatch` records draw calls in submission order; that's
//! awkward when a tooltip wants to render *above* a modal that
//! was issued earlier in the frame. We solve that by collecting
//! draws into per-layer command buffers and flushing them
//! back-to-front at `Ui::end()` time.
//!
//! Each [`Layer`] is a small `enum` ordinal so adding new layers
//! later (e.g. `Layer::Debug` for an overlay HUD) only requires
//! extending the enum and `LAYERS_ORDERED`.
//!
//! Draw calls themselves stay primitive (`Rect`, `Text`, `Icon`)
//! to avoid coupling the layer system to widget composition;
//! widgets in L2/L3 lower themselves to these three commands.

use super::color::Color;
use super::rect::Rect;
use crate::renderer::OverlayBatch;

/// Z-ordering buckets. Lower variants render first (i.e. behind).
/// Order matters: it's also the iteration order used by `flush`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Layer {
    /// Static screen-space backdrops (vignettes, full-screen tints).
    Background = 0,
    /// Ordinary panels (HUD, inventory, character select).
    Panel      = 1,
    /// Above panels but below modals (notifications, banners).
    Foreground = 2,
    /// Modal dialogs and their dimmer.
    Modal      = 3,
    /// Tooltips, dropdown menus.
    Tooltip    = 4,
    /// In-flight drag ghost (always on top).
    DragGhost  = 5,
}

/// Iteration order for [`LayerBuf::flush`]. Keep in sync with the
/// enum declaration above.
const LAYERS_ORDERED: [Layer; 6] = [
    Layer::Background,
    Layer::Panel,
    Layer::Foreground,
    Layer::Modal,
    Layer::Tooltip,
    Layer::DragGhost,
];

/// Single deferred draw command. Lowered onto `OverlayBatch` at
/// flush time; widgets never construct these directly — they go
/// through `Ui::draw_*` helpers which forward here.
#[derive(Debug, Clone)]
pub(super) enum DrawCmd {
    Rect {
        rect: Rect,
        color: Color,
    },
    RoundedRect {
        rect: Rect,
        radius: f32,
        color: Color,
    },
    Text {
        text: String,
        x: f32,
        y: f32,
        size: f32,
        color: Color,
    },
    Icon {
        name: String,
        rect: Rect,
        tint: Color,
    },
}

/// Per-layer command buffer. Owned by [`Ui`](super::ui::Ui).
#[derive(Default)]
pub(super) struct LayerBuf {
    layers: [Vec<DrawCmd>; 6],
}

impl LayerBuf {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, layer: Layer, cmd: DrawCmd) {
        self.layers[layer as usize].push(cmd);
    }

    /// Drop every queued command without flushing. Used by the
    /// future modal-dismissal path which throws away in-progress
    /// tooltips when a higher-priority modal opens mid-frame.
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        for buf in &mut self.layers {
            buf.clear();
        }
    }

    /// Replay every queued command into `batch` in layer order.
    /// `screen_w/h` come from the renderer at flush time so we
    /// don't lock pixel→NDC math into queueing.
    pub fn flush(&mut self, batch: &mut OverlayBatch, screen_w: f32, screen_h: f32) {
        for layer in LAYERS_ORDERED {
            for cmd in self.layers[layer as usize].drain(..) {
                match cmd {
                    DrawCmd::Rect { rect, color } => {
                        batch.rect_px(
                            rect.x(), rect.y(),
                            rect.width(), rect.height(),
                            color.to_array(),
                            screen_w, screen_h,
                        );
                    }
                    DrawCmd::RoundedRect { rect, radius, color } => {
                        batch.rounded_rect_px(
                            rect.x(), rect.y(),
                            rect.width(), rect.height(),
                            radius,
                            color.to_array(),
                            screen_w, screen_h,
                        );
                    }
                    DrawCmd::Text { text, x, y, size, color } => {
                        batch.text(&text, x, y, size, color.to_array(), screen_w, screen_h);
                    }
                    DrawCmd::Icon { name, rect, tint } => {
                        batch.icon(
                            &name,
                            rect.x(), rect.y(),
                            rect.width(), rect.height(),
                            tint.to_array(),
                            screen_w, screen_h,
                        );
                    }
                }
            }
        }
    }
}
