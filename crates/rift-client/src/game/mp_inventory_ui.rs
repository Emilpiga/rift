//! Multiplayer inventory UI.
//!
//! Lightweight panel built around the new `rift_game::loot::Item`
//! type. Persistent items (loaded at Hello, appended on every loot
//! pickup) live in [`GameState::mp_inventory`]; this UI renders
//! them in a paginated grid and surfaces a hover tooltip using
//! [`Item::tooltip`].
//!
//! Out of scope for now: drag-and-drop, equip swap, ground-drop.
//! Those will land alongside the multiplayer equipment / wire
//! schema.

use rift_engine::input::Input;
use rift_engine::renderer::overlay::OverlayBatch;
use rift_game::loot::Item;
use winit::keyboard::KeyCode;

const SLOT_SIZE: f32 = 44.0;
const SLOT_GAP: f32 = 4.0;
const COLS: usize = 5;
const ROWS: usize = 4;
const PANEL_PAD: f32 = 14.0;
const HEADER_H: f32 = 28.0;

/// State + behaviour for the multiplayer inventory panel.
#[derive(Default)]
pub struct MpInventoryUI {
    pub open: bool,
}

impl MpInventoryUI {
    pub fn new() -> Self {
        Self { open: false }
    }

    /// Process Tab toggle. Returns `true` if the panel consumed
    /// the click this frame (so callers can suppress world clicks).
    pub fn update(&mut self, input: &Input) -> bool {
        if input.key_just_pressed(KeyCode::Tab) {
            self.open = !self.open;
        }
        if !self.open {
            return false;
        }
        // Future: handle drag/drop, right-click equip, etc. For
        // now the panel is read-only, so we still report
        // "consumed" while the mouse hovers over it to keep clicks
        // from punching through to the world.
        let (mx, my) = input.mouse_pos();
        let (px, py, pw, ph) = panel_rect(2560.0, 1440.0); // dummy; per-frame rect computed in render
        let _ = (px, py, pw, ph);
        let _ = (mx, my);
        // Caller-side hit-test deferred to render(); see below.
        true
    }

    /// Render the panel + tooltip. Skips entirely when closed.
    pub fn render(
        &self,
        batch: &mut OverlayBatch,
        items: &[Item],
        input: &Input,
        screen_w: f32,
        screen_h: f32,
    ) {
        if !self.open {
            return;
        }
        let (px, py, pw, ph) = panel_rect(screen_w, screen_h);
        // Backdrop + header.
        batch.rect_px(px, py, pw, ph, [0.04, 0.05, 0.08, 0.92], screen_w, screen_h);
        batch.rect_px(
            px,
            py,
            pw,
            HEADER_H,
            [0.10, 0.13, 0.20, 1.0],
            screen_w,
            screen_h,
        );
        let header = format!("INVENTORY ({} / {})", items.len(), COLS * ROWS);
        batch.text(
            &header,
            px + 12.0,
            py + 8.0,
            14.0,
            [0.85, 0.92, 1.0, 1.0],
            screen_w,
            screen_h,
        );

        // Slot grid. Empty slots render a dim outline; filled
        // slots tint with the item's rarity colour.
        let grid_x = px + PANEL_PAD;
        let grid_y = py + HEADER_H + PANEL_PAD;
        let (mx, my) = input.mouse_pos();
        let mut hovered: Option<&Item> = None;

        for row in 0..ROWS {
            for col in 0..COLS {
                let idx = row * COLS + col;
                let sx = grid_x + col as f32 * (SLOT_SIZE + SLOT_GAP);
                let sy = grid_y + row as f32 * (SLOT_SIZE + SLOT_GAP);
                // Slot background.
                batch.rect_px(
                    sx,
                    sy,
                    SLOT_SIZE,
                    SLOT_SIZE,
                    [0.08, 0.10, 0.15, 1.0],
                    screen_w,
                    screen_h,
                );
                if let Some(item) = items.get(idx) {
                    let c = item.rarity.color();
                    // Inner tinted square.
                    batch.rect_px(
                        sx + 3.0,
                        sy + 3.0,
                        SLOT_SIZE - 6.0,
                        SLOT_SIZE - 6.0,
                        [c[0] * 0.55, c[1] * 0.55, c[2] * 0.55, 1.0],
                        screen_w,
                        screen_h,
                    );
                    // First letter of base name as a poor man's icon.
                    let glyph = item
                        .base
                        .name
                        .chars()
                        .next()
                        .map(|c| c.to_ascii_uppercase())
                        .unwrap_or('?');
                    let mut buf = [0u8; 4];
                    let s: &str = glyph.encode_utf8(&mut buf);
                    batch.text(
                        s,
                        sx + SLOT_SIZE * 0.5 - 6.0,
                        sy + SLOT_SIZE * 0.5 - 8.0,
                        18.0,
                        [c[0], c[1], c[2], 1.0],
                        screen_w,
                        screen_h,
                    );
                    // Hover detection.
                    if mx >= sx && mx < sx + SLOT_SIZE && my >= sy && my < sy + SLOT_SIZE {
                        hovered = Some(item);
                    }
                }
            }
        }

        // Tooltip.
        if let Some(item) = hovered {
            render_tooltip(batch, item, mx + 16.0, my, screen_w, screen_h);
        }

        // "Press TAB to close" footer.
        batch.text(
            "TAB to close",
            px + 12.0,
            py + ph - 18.0,
            10.0,
            [0.55, 0.6, 0.7, 1.0],
            screen_w,
            screen_h,
        );
    }

    /// Returns true when the panel is open AND the mouse sits over
    /// it. Used by the caller to suppress world-click handling.
    pub fn consumes_mouse(&self, input: &Input, screen_w: f32, screen_h: f32) -> bool {
        if !self.open {
            return false;
        }
        let (mx, my) = input.mouse_pos();
        let (px, py, pw, ph) = panel_rect(screen_w, screen_h);
        mx >= px && mx < px + pw && my >= py && my < py + ph
    }
}

/// Centred panel rect — width fits the 5-col grid + padding.
fn panel_rect(screen_w: f32, screen_h: f32) -> (f32, f32, f32, f32) {
    let pw = COLS as f32 * (SLOT_SIZE + SLOT_GAP) - SLOT_GAP + PANEL_PAD * 2.0;
    let ph = ROWS as f32 * (SLOT_SIZE + SLOT_GAP) - SLOT_GAP
        + PANEL_PAD * 2.0
        + HEADER_H
        + 18.0;
    let px = (screen_w - pw) * 0.5;
    let py = (screen_h - ph) * 0.5;
    (px, py, pw, ph)
}

/// Multi-line tooltip — one rect background + one line per
/// `Item::tooltip()` entry. Header line is tinted by rarity.
fn render_tooltip(
    batch: &mut OverlayBatch,
    item: &Item,
    x: f32,
    y: f32,
    screen_w: f32,
    screen_h: f32,
) {
    let lines = item.tooltip();
    let line_h = 14.0;
    let pad = 6.0;
    let max_chars = lines.iter().map(|l| l.len()).max().unwrap_or(0) as f32;
    let w = (max_chars * 7.0 + pad * 2.0).max(140.0);
    let h = lines.len() as f32 * line_h + pad * 2.0;
    // Clamp inside the screen.
    let x = x.min(screen_w - w - 4.0);
    let y = y.min(screen_h - h - 4.0);
    batch.rect_px(x, y, w, h, [0.02, 0.03, 0.05, 0.95], screen_w, screen_h);
    let c = item.rarity.color();
    for (i, line) in lines.iter().enumerate() {
        let color = if i == 0 {
            [c[0], c[1], c[2], 1.0]
        } else {
            [0.85, 0.88, 0.95, 1.0]
        };
        batch.text(
            line,
            x + pad,
            y + pad + i as f32 * line_h,
            12.0,
            color,
            screen_w,
            screen_h,
        );
    }
}
