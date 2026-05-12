//! Alt-hold loot nameplates (Path-of-Exile style).
//!
//! Held [`ImKey::AltLeft`] / [`ImKey::AltRight`] paints a small,
//! rarity-coloured rectangle floating above every ground-loot
//! drop on screen. The label is clickable — left-clicking it
//! queues a `PickUpLoot` for that drop's `NetId`, bypassing the
//! F-key proximity gate so the player can grab a specific item
//! out of a cluttered pile.
//!
//! Visual language:
//! - background: dark translucent slab.
//! - 2-px frame in the item's [`rift_game::loot::Rarity::color`].
//! - text: the item's `display_name` (already includes
//!   "Anchored " / "Unstable " prefixes).
//! - anchored items get a saturated gold border + a small `⚓`
//!   glyph in the left margin so they pop out of a busy pile.
//!
//! Pure client overlay. No new wire messages: clicks push into
//! the existing [`LootClientState::pending_pickups`] queue that
//! the F-key path already feeds.
//!
//! Layout: labels are anchored to each drop's projected pixel
//! position and stacked downward when two or more would overlap.
//! Behind-camera / off-screen anchors are silently culled.

use glam::{Mat4, Vec3};
use rift_engine::ui::im::{Color, Id, Pos2, Rect, Ui, Vec2};

use crate::game::states::sub_state::LootClientState;

/// Vertical pixel offset above the drop's world position. Picked
/// so the label clears the loot pillar VFX without floating so
/// high it disconnects from the item visually.
const Y_OFFSET_PX: f32 = -56.0;

/// Vertical gap between stacked labels (when two drops project to
/// nearly-overlapping pixel anchors).
const STACK_GAP_PX: f32 = 2.0;

/// Horizontal padding between the frame and the text on each side.
const PAD_X_BASE: f32 = 10.0;

/// Vertical padding between the frame and the text top/bottom.
const PAD_Y_BASE: f32 = 6.0;

/// Baseline text size for the label (Common rarity). Higher
/// rarities scale this up via [`rarity_scale`] so a Legendary
/// drop pops out of a cluttered pile at a glance.
const TEXT_SIZE_BASE: f32 = 18.0;

/// Frame thickness in pixels, scaled with the per-rarity size
/// multiplier so rare items get visibly thicker frames too.
const FRAME_PX_BASE: f32 = 2.0;

/// Rarity-driven size multiplier. Common is the baseline;
/// Legendary roughly 1.6× so it leaps off the screen even in a
/// crowded pile. Magic and Rare sit in between so the ladder
/// reads at a glance.
fn rarity_scale(rarity: rift_game::loot::Rarity) -> f32 {
    use rift_game::loot::Rarity;
    match rarity {
        Rarity::Common => 1.0,
        Rarity::Magic => 1.15,
        Rarity::Rare => 1.35,
        Rarity::Legendary => 1.6,
    }
}

/// Project a world-space position to a pixel anchor. Returns
/// `None` when the point is behind the camera or outside the
/// `(-1, 1)` clip cube. Mirrors `WorldUi::world_to_screen`,
/// inlined here so we can call it without holding a `WorldUi`
/// borrow across the per-drop measurement loop.
fn world_to_screen(view_proj: Mat4, world_pos: Vec3, screen: Vec2) -> Option<Pos2> {
    let clip = view_proj * world_pos.extend(1.0);
    if clip.w <= 0.0 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    if ndc.x < -1.0 || ndc.x > 1.0 || ndc.y < -1.0 || ndc.y > 1.0 {
        return None;
    }
    Some(Pos2::new(
        (ndc.x + 1.0) * 0.5 * screen.x,
        (ndc.y + 1.0) * 0.5 * screen.y,
    ))
}

fn rects_overlap(a: &Rect, b: &Rect) -> bool {
    a.min.x < b.max.x && b.min.x < a.max.x && a.min.y < b.max.y && b.min.y < a.max.y
}

/// One label's resolved layout + draw data. Cached between the
/// project / measure pass and the overlap-resolution + draw pass
/// so we can hit-test rects that already know their final stacked
/// y position.
struct Pending {
    net_id: rift_net::NetId,
    rect: Rect,
    name: String,
    rarity_color: [f32; 3],
    anchored: bool,
    text_size: f32,
    pad_x: f32,
    pad_y: f32,
    frame_px: f32,
}

/// Render alt-hold nameplates and submit clicks to the pickup
/// queue. Call once per frame from the UI phase, *after* the
/// world-overlay HP bars (so labels sort on top of them) and
/// *before* the inventory panel (so an open bag still occludes
/// the labels).
pub fn render_loot_labels(
    ui: &mut Ui<'_>,
    loot: &mut LootClientState,
    view_proj: Mat4,
    hud_consume_rects: &mut Vec<Rect>,
) {
    use rift_engine::ui::im::ImKey;

    let alt_held =
        ui.input().is_key_held(ImKey::AltLeft) || ui.input().is_key_held(ImKey::AltRight);
    if !alt_held {
        return;
    }
    if loot.drops.is_empty() {
        return;
    }

    let screen = ui.screen_size();

    let mut pending: Vec<Pending> = Vec::with_capacity(loot.drops.len());
    for drop in &loot.drops {
        // Anchor just above the item — the pillar VFX is ~1.5m
        // tall, so 1.1m + a screen-space lift reads as "label
        // hovers near the top of the column of light".
        let world_pos = drop.position + Vec3::new(0.0, 1.1, 0.0);
        let Some(anchor) = world_to_screen(view_proj, world_pos, screen) else {
            continue;
        };
        let name = drop.item.display_name();
        let scale = rarity_scale(drop.item.rarity);
        let text_size = TEXT_SIZE_BASE * scale;
        let pad_x = PAD_X_BASE * scale;
        let pad_y = PAD_Y_BASE * scale;
        let frame_px = FRAME_PX_BASE * scale;
        let tw = ui.measure_text(&name, text_size);
        let icon_w = if drop.item.anchored {
            text_size + 4.0
        } else {
            0.0
        };
        let w = tw + icon_w + pad_x * 2.0;
        let h = text_size + pad_y * 2.0;
        let rect = Rect::from_xywh(anchor.x - w * 0.5, anchor.y + Y_OFFSET_PX, w, h);
        pending.push(Pending {
            net_id: drop.net_id,
            rect,
            name,
            rarity_color: drop.item.rarity.color(),
            anchored: drop.item.anchored,
            text_size,
            pad_x,
            pad_y,
            frame_px,
        });
    }

    // Resolve overlaps: sort top-to-bottom, then push any rect
    // that vertically overlaps an earlier one down. Simple O(n²)
    // is fine — there are rarely more than a few dozen visible
    // drops at once.
    pending.sort_by(|a, b| {
        a.rect
            .min
            .y
            .partial_cmp(&b.rect.min.y)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for i in 0..pending.len() {
        let (left, right) = pending.split_at_mut(i);
        let mine = &mut right[0];
        loop {
            let mut bumped = false;
            for other in left.iter() {
                if rects_overlap(&mine.rect, &other.rect) {
                    let new_y = other.rect.max.y + STACK_GAP_PX;
                    if new_y > mine.rect.min.y {
                        let h = mine.rect.height();
                        mine.rect = Rect::from_xywh(mine.rect.min.x, new_y, mine.rect.width(), h);
                        bumped = true;
                    }
                }
            }
            if !bumped {
                break;
            }
        }
    }

    // Draw + interact. Reverse order so labels closer to the
    // camera (which projected lower on screen and thus stack on
    // top) get hover priority — matches the visual stacking.
    for p in pending.iter().rev() {
        let rc = p.rarity_color;
        let rarity = Color::rgba(rc[0], rc[1], rc[2], 1.0);
        let frame = if p.anchored {
            // Saturated gold border for the rare anchored trait
            // so it visually leaps off the pile even when the
            // base rarity is white/blue.
            Color::rgba(1.0, 0.78, 0.18, 1.0)
        } else {
            rarity
        };

        // Hover-aware background. Brightens on hover so the
        // player knows the click target is live.
        let id = Id::root("loot_label").child(p.net_id.0);
        let hovered = ui.interact_hover(id, p.rect);
        let bg_alpha = if hovered { 0.92 } else { 0.78 };
        let bg = Color::rgba(0.05, 0.05, 0.07, bg_alpha);

        // 1) Outer frame (solid rarity colour).
        ui.draw_rect(p.rect, frame);
        // 2) Inset background slab.
        let inner = Rect::from_xywh(
            p.rect.min.x + p.frame_px,
            p.rect.min.y + p.frame_px,
            (p.rect.width() - p.frame_px * 2.0).max(0.0),
            (p.rect.height() - p.frame_px * 2.0).max(0.0),
        );
        ui.draw_rect(inner, bg);
        // 3) Thin rarity-tinted accent along the top edge so
        //    blue/gold/orange reads even on anchored gold-framed
        //    items where the frame colour is overridden.
        let accent_h = (1.5 * (p.text_size / TEXT_SIZE_BASE)).max(1.5);
        let accent = Rect::from_xywh(inner.min.x, inner.min.y, inner.width(), accent_h);
        ui.draw_rect(
            accent,
            Color::rgba(rc[0], rc[1], rc[2], if hovered { 1.0 } else { 0.85 }),
        );

        // 4) Text + anchored glyph.
        let text_color = Color::rgba(rc[0], rc[1], rc[2], 1.0);
        let mut text_x = p.rect.min.x + p.pad_x;
        let text_y = p.rect.min.y + p.pad_y;
        if p.anchored {
            // Saturated gold anchor glyph.
            let anchor_color = Color::rgba(1.0, 0.82, 0.30, 1.0);
            ui.draw_text(
                Pos2::new(text_x, text_y),
                "\u{2693}",
                p.text_size,
                anchor_color,
            );
            text_x += p.text_size + 4.0;
        }
        ui.draw_text(Pos2::new(text_x, text_y), &p.name, p.text_size, text_color);

        // 5) Click → pickup queue. Mirrors the F-key path:
        //    de-dupe so a held click doesn't spam the channel.
        if hovered && ui.input().left_just_pressed() {
            if !loot.pending_pickups.contains(&p.net_id) {
                loot.pending_pickups.push(p.net_id);
            }
        }

        // 6) Swallow LMB clicks on the label so the gameplay
        //    tick's basic-attack cast doesn't also fire when
        //    the player clicks a label. Consumed by next
        //    frame's `combat_phase` gate.
        hud_consume_rects.push(p.rect);
    }
}
