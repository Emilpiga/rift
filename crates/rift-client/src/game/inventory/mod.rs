//! Multiplayer inventory UI.
//!
//! Built entirely on the engine's immediate-mode UI stack
//! ([`rift_engine::ui::im`]). All slot drawing, hover, drag /
//! drop, and tooltip logic comes from the shared widgets so
//! this directory is just *layout + action routing*: where
//! each panel sits, and how a release-on-target maps to a
//! server request.
//!
//! Module map:
//! * [`layout`] — constants + per-frame `Layout` struct.
//! * [`drag`] — `DragSource`/`DropTarget`, slot-builder,
//!   click + drop routing.
//! * [`bag_panel`] — bag + equipment row.
//! * [`stash_panel`] — tab strip, "+" button, slot grid,
//!   rename text field.
//! * [`stats_panel`] — read-only character sheet.
//! * [`tooltips`] — item tooltip + side-by-side delta.
//! * [`salvage`] — two-stage Salvage Trash timing helpers.
//!
//! Interactions:
//! * **Click** a bag slot → equip into its canonical slot
//!   (or deposit into the stash if a chest session is open).
//! * **Click** an equipment slot → unequip back into the bag.
//! * **Drag** a slot onto another to:
//!   - Bag → Bag: reorder
//!   - Bag → Equip: equip
//!   - Bag → Stash: deposit
//!   - Bag → World (released outside any panel): drop on ground
//!   - Equip → Bag(idx): unequip into a specific slot
//!   - Stash → Bag(idx): withdraw
//! * **Shift-click** a bag or stash item to render the
//!   currently-equipped item alongside the hovered item's
//!   tooltip for side-by-side stat comparison.

use rift_engine::ui::im::{Color, Pos2, Rect, Ui};
use rift_game::loot::{Equipment, Item};
use winit::keyboard::KeyCode;

use super::sub_state::{EquipRequest, StashRequest, StashTabClient};
use super::PlayerState;

pub mod bag_panel;
pub mod drag;
pub mod layout;
pub mod salvage;
pub mod stash_panel;
pub mod stats_panel;
pub mod tooltips;

use self::bag_panel::{render_bag_panel, BagPanelIn};
use self::drag::{compare_target, item_for_source, DragSource};
use self::layout::Layout;
use self::salvage::ui_now;
use self::stash_panel::{next_tab_color, render_stash_panel, RenameState, StashPanelIn};
use self::stats_panel::render_stats_panel;
use self::tooltips::{render_compare_delta, render_item_tooltip};

#[derive(Default)]
pub struct MpInventoryUI {
    pub open: bool,
    stash_visible: bool,
    /// Cached layout rects from the last `frame()` call so the
    /// input layer's [`Self::consumes_mouse`] check (which
    /// runs without a `Ui`) can hit-test the *actual* on-
    /// screen positions instead of recomputing them with a
    /// stale theme scale.
    cached_bag: Rect,
    cached_stats: Rect,
    cached_stash: Rect,
    /// Currently-selected stash tab index. Clamped against the
    /// authoritative `tabs` slice every frame so a server-side
    /// tab removal never leaves us pointing at thin air.
    /// `0` is the default starter tab.
    active_stash_tab: usize,
    /// Inline tab-rename state — wraps the engine's
    /// `InlineEditState` with the tab-index target so the
    /// rename can be cancelled when the player switches tabs
    /// or when the targeted tab is removed by the server.
    rename: RenameState,
    /// 2-stage commit state for the "Salvage Trash" bulk
    /// destructive action. First click arms (red label),
    /// second click within the configured window confirms.
    /// See [`rift_engine::ui::im::TwoStageConfirm`].
    salvage_confirm: rift_engine::ui::im::TwoStageConfirm,
    /// Bag slot whose press happened while Ctrl was held. The
    /// slot's `clicked` only resolves on release; if the player
    /// releases Ctrl before releasing the mouse, the naive
    /// "is Ctrl held *right now*" check at click time misses
    /// the intent and the click silently equips instead of
    /// salvaging. Latching the slot at press time and consuming
    /// it on the matching click closes that window so single
    /// salvages feel reliable.
    salvage_armed_bag_idx: Option<usize>,
}

impl MpInventoryUI {
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` while a text-input widget owned by the inventory
    /// is active (currently: the inline stash-tab rename
    /// field). Drives `Input::set_text_capture` from
    /// `GameState::update` so typed letters like W / A / S / D
    /// or T can't leak into world bindings before the rename
    /// has even rendered for the frame.
    pub fn wants_text_input(&self) -> bool {
        self.rename.is_active()
    }

    /// Run one frame of the inventory UI. Fuses input + draw
    /// through the engine's IM stack, so callers only need to
    /// hand it the data and the request queues.
    ///
    /// Returns `true` if the panel is open (i.e. it's currently
    /// claiming the screen for itself; gameplay input should be
    /// suppressed).
    pub fn frame(
        &mut self,
        ui: &mut Ui<'_>,
        items: &[Option<Item>],
        equipment: &Equipment,
        pending: &mut Vec<EquipRequest>,
        stash_open: bool,
        stash_tabs: &[StashTabClient],
        stash_pending: &mut Vec<StashRequest>,
        player_state: &PlayerState,
    ) -> bool {
        // Tab toggles only when no chest session owns the panel.
        if ui.input().key_just_pressed(KeyCode::Tab) && !stash_open {
            self.open = !self.open;
        }
        // Stash session forces the panel open while in range.
        self.stash_visible = stash_open;
        if stash_open {
            self.open = true;
        }
        if !self.open {
            // Cancel any drag whose source slot is no longer
            // visible — otherwise the drag ghost would persist
            // after a Tab close.
            ui.cancel_drag();
            return false;
        }

        let layout = Layout::compute(ui, stash_open);
        // Stash these for `consumes_mouse` (input layer has no
        // `Ui`, so it can't recompute the layout itself).
        self.cached_bag = layout.bag_panel;
        self.cached_stats = layout.stats_panel;
        self.cached_stash = layout.stash_panel;

        // Auto-disarm a stale confirm so a second click days
        // later doesn't surprise the player. Cheap to do every
        // frame; the comparison is just two f64 ops.
        self.salvage_confirm.tick(ui_now());

        // ─── Bag + equipment panel ──────────────────────────────
        let bag_out = render_bag_panel(
            ui,
            &layout,
            BagPanelIn {
                items,
                equipment,
                stash_open,
                active_tab_u8: self.active_stash_tab as u8,
                salvage_armed: self.salvage_confirm.armed(),
                salvage_armed_bag_idx: self.salvage_armed_bag_idx,
            },
            pending,
            stash_pending,
        );
        // Persist the press-time ctrl latch back into `self`
        // for the next frame; the bag closure could only mutate
        // a local cell.
        self.salvage_armed_bag_idx = bag_out.salvage_armed_bag_idx;

        // Resolve the Salvage Trash 2-stage button outside the
        // panel closure (where we can mutate `self`). First
        // click arms; second click within the window commits.
        if bag_out.pressed_salvage_trash && bag_out.bulk_preview.count > 0 {
            use rift_engine::ui::im::TwoStageOutcome;
            if let TwoStageOutcome::Confirmed = self.salvage_confirm.click(ui_now()) {
                pending.push(EquipRequest::SalvageBulk {
                    rarity_max: rift_game::loot::Rarity::Magic as u8,
                });
            }
        }

        // ─── Stash panel ────────────────────────────────────────
        // Clamp the active tab against the authoritative tab
        // list every frame — handles the rare case where the
        // server-pushed tab list shrinks (e.g. character
        // reset). Using `min` keeps us pointing at the last
        // tab instead of crashing.
        if !stash_tabs.is_empty() {
            self.active_stash_tab = self.active_stash_tab.min(stash_tabs.len() - 1);
        } else {
            self.active_stash_tab = 0;
        }
        // Cancel any in-flight rename whose tab vanished.
        if let Some(idx) = self.rename.tab_idx {
            if idx >= stash_tabs.len() {
                self.rename.cancel();
            }
        }

        let stash_hovered_item = if stash_open {
            let stash_out = render_stash_panel(
                ui,
                &layout,
                &mut self.rename,
                StashPanelIn {
                    stash_tabs,
                    player_state,
                    active_tab: self.active_stash_tab,
                },
                pending,
                stash_pending,
            );
            if let Some(i) = stash_out.switch_to {
                self.active_stash_tab = i;
                self.rename.cancel();
            }
            if let Some(i) = stash_out.recolor_request {
                if let Some(tab) = stash_tabs.get(i as usize) {
                    let next = next_tab_color(tab.color);
                    stash_pending.push(StashRequest::RecolorTab {
                        tab_index: i,
                        color: next,
                    });
                }
            }
            if stash_out.pressed_buy_tab {
                stash_pending.push(StashRequest::BuyTab);
            }
            if stash_out.pressed_rename {
                let current = stash_tabs
                    .get(self.active_stash_tab)
                    .map(|t| t.name.clone())
                    .unwrap_or_default();
                self.rename.begin(self.active_stash_tab, current);
            }
            stash_out.hovered_item
        } else {
            None
        };

        // ─── Stats panel ───────────────────────────────────────
        // Always shown alongside the bag panel so the player can
        // see their resolved character sheet without leaving the
        // inventory screen.
        render_stats_panel(ui, layout.stats_panel, player_state, &layout);

        // ─── Tooltip(s) ────────────────────────────────────────
        let tip_target = bag_out.hovered_item.as_ref().or(stash_hovered_item.as_ref());
        if let Some(item) = tip_target {
            let mp = ui.mouse_pos();
            let screen_w = ui.screen_size().x;
            // Default anchor: just to the right of the cursor.
            // If we're past the screen midpoint we *prefer* the
            // left side so the cursor stays free of the
            // tooltip slab and the compare/delta panes don't
            // immediately overflow the edge.
            let primary_anchor = if mp.x > screen_w * 0.5 {
                Pos2::new(mp.x - ui.s(18.0), mp.y)
            } else {
                Pos2::new(mp.x + ui.s(18.0), mp.y)
            };
            let primary = render_item_tooltip(
                ui,
                item,
                "Hovered",
                primary_anchor,
                Some(&player_state.loadout),
            );
            // Ctrl-hover salvage hint. Drawn as a small banner
            // beneath the primary tooltip so it doesn't push the
            // compare panel sideways. Suppressed when the hover
            // came from an equipment slot (you can't salvage
            // equipped gear) or a stash slot (deposit/withdraw
            // only — bulk salvage acts on bag items).
            if bag_out.hovered_from_bag && ui.ctrl_held() {
                let hint = if item.anchored {
                    "Anchored \u{2014} cannot be salvaged".to_string()
                } else {
                    let yld = rift_game::loot::salvage_yield(item.rarity, item.ilvl);
                    format!("Ctrl+click \u{2192} Salvage for {} \u{25C6}", yld)
                };
                let hint_size = ui.theme().fonts.size_md;
                let hw = ui.measure_text(&hint, hint_size);
                let pad = ui.s(8.0);
                let hint_rect = Rect::from_xywh(
                    primary.x(),
                    primary.max.y + ui.s(4.0),
                    hw + pad * 2.0,
                    hint_size + pad,
                );
                let bg = if item.anchored {
                    Color::rgba(0.40, 0.20, 0.18, 0.92)
                } else {
                    Color::rgba(0.18, 0.30, 0.22, 0.92)
                };
                ui.draw_rect(hint_rect, bg);
                ui.draw_text(
                    Pos2::new(hint_rect.x() + pad, hint_rect.y() + pad * 0.5),
                    &hint,
                    hint_size,
                    ui.theme().colors.text,
                );
            }
            // Compare side-by-side. Pick the side with more
            // remaining room so two- and three-pane tooltips
            // don't push past the screen edge on right-half
            // hovers. SHIFT additionally surfaces a per-stat
            // delta column so the player can see exactly what
            // they'd gain or lose. Skip the compare when the
            // hover originated from an equipment slot —
            // comparing an equipped item to itself is noise.
            if !bag_out.hovered_from_equip {
                if let Some(equipped) = compare_target(equipment, item) {
                    let stack_right = primary.max.x + ui.s(8.0) + 200.0 < screen_w;
                    let eq_anchor = if stack_right {
                        Pos2::new(primary.max.x + ui.s(8.0), primary.y())
                    } else {
                        // Stack to the left of `primary`. The
                        // tooltip's own width-clamp will pull it
                        // back if it can't fit there either.
                        Pos2::new(primary.x() - ui.s(8.0) - 200.0, primary.y())
                    };
                    let eq_rect = render_item_tooltip(
                        ui,
                        equipped,
                        "Equipped",
                        eq_anchor,
                        Some(&player_state.loadout),
                    );
                    if ui.shift_held() {
                        let delta_anchor = if stack_right {
                            Pos2::new(eq_rect.max.x + ui.s(8.0), eq_rect.y())
                        } else {
                            Pos2::new(eq_rect.x() - ui.s(8.0) - 200.0, eq_rect.y())
                        };
                        render_compare_delta(ui, item, equipped, delta_anchor);
                    }
                }
            }
        }

        // ─── Drag ghost + outside-drop (drop to world) ─────────
        // The ghost is just an `ItemSlot` rendered on the
        // DragGhost layer using the same builder as the
        // in-place slot, so what the player picks up is what
        // they see floating under the cursor.
        let active_stash_items: &[Option<Item>] = stash_tabs
            .get(self.active_stash_tab)
            .map(|t| t.items.as_slice())
            .unwrap_or(&[]);
        if let Some(payload) = ui.drag_payload::<DragSource>().copied() {
            if let Some(item) = item_for_source(payload, items, equipment, active_stash_items) {
                self::drag::build_item_slot(Some(item)).show_ghost(ui);
            }
        }
        // Released outside every slot? → drop-to-world (bag-source only).
        if let Some(drop) = ui.take_drop_outside::<DragSource>() {
            if let DragSource::Bag(idx) = drop.payload {
                pending.push(EquipRequest::DropToWorld { inventory_index: idx as u32 });
            }
        }

        true
    }

    pub fn consumes_mouse(&self, mx: f32, my: f32, _screen_w: f32, _screen_h: f32) -> bool {
        if !self.open {
            return false;
        }
        let hit = |r: Rect| {
            r.width() > 0.0
                && r.height() > 0.0
                && mx >= r.min.x
                && mx < r.max.x
                && my >= r.min.y
                && my < r.max.y
        };
        if hit(self.cached_bag) || hit(self.cached_stats) {
            return true;
        }
        if self.stash_visible && hit(self.cached_stash) {
            return true;
        }
        false
    }
}
