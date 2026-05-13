//! Spellbook screen — pick which six abilities live on the action
//! bar.
//!
//! Two ways in:
//! - Press `B` to toggle.
//! - Click any HUD action-bar slot to open with that slot
//!   pre-targeted; the next pool click assigns directly.
//!
//! The pool above the bar shows every player-castable ability
//! pulled from `rift_game::loadout::player_abilities()`. Locked
//! abilities (player level too low) are grayed out and reject
//! clicks. Locked bar slots (`SLOT_UNLOCK_LEVELS`) are also
//! grayed out and reject clicks; the player has to level up to
//! use them.
//!
//! The local UI doesn't optimistically mutate the loadout — the
//! click fires `ClientMsg::SetLoadoutSlot` and the server replies
//! with `ServerMsg::Loadout`, which the binary applies into
//! `PlayerState`. That keeps the bar always in sync with the
//! authoritative server state and matches the rest of the
//! request/reply UI patterns (inventory, equipment, stash).

use rift_engine::ui::im::{
    widgets::{title, Button, ItemSlot},
    Color, Frame, Id, Pos2, Rect, Ui, Vec2,
};
use rift_game::abilities::{Ability, Category};
use rift_game::loadout::{
    is_ability_unlocked, is_slot_unlocked, player_abilities, Loadout, SLOT_COUNT,
    SLOT_UNLOCK_LEVELS,
};

/// User intent emitted by [`SpellbookUi::frame`]. The binary
/// converts these into network commands. Mirrors the inventory
/// UI's "queue intent here, send-from-binary" split.
#[derive(Clone, Copy, Debug)]
pub enum SpellbookAction {
    /// Player asked to put `ability_id` into action-bar slot
    /// `slot_index`. `ability_id == EMPTY_SLOT` clears the slot.
    AssignSlot {
        slot_index: u8,
        ability_id: rift_game::abilities::AbilityWireId,
    },
}

#[derive(Default)]
pub struct SpellbookUi {
    pub open: bool,
    /// Wire id of the ability the user has selected from the
    /// pool. `None` means "no ability picked yet — clicking a
    /// slot is a no-op".
    selected_ability: Option<rift_game::abilities::AbilityWireId>,
    /// Bar slot the user pre-targeted by clicking the HUD.
    /// `Some(i)` means the next pool click assigns directly to
    /// slot `i`. Cleared on assign / close.
    target_slot: Option<u8>,
    /// Active category in the left rail. `None` means the user
    /// hasn't picked one yet, which we treat as `Category::All`.
    /// Persisted across frames so the player's last filter
    /// survives close/reopen, mirroring inventory behaviour.
    selected_category: Option<Category>,
}

impl SpellbookUi {
    pub fn new() -> Self {
        Self::default()
    }

    /// Toggle visibility. Closing also clears the current
    /// selection so reopening doesn't leave a stale highlight.
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if !self.open {
            self.selected_ability = None;
            self.target_slot = None;
        }
    }

    /// Open the spellbook with `slot_index` pre-targeted (the next
    /// pool click will assign to this slot). Used when the player
    /// clicks a HUD action-bar slot.
    pub fn open_for_slot(&mut self, slot_index: u8) {
        self.open = true;
        self.target_slot = Some(slot_index);
        self.selected_ability = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.selected_ability = None;
        self.target_slot = None;
    }

    /// Render one frame of the spellbook. Returns the user's
    /// intent (or `None` if nothing actionable happened). The
    /// binary fires the corresponding `ClientMsg` and the
    /// authoritative `ServerMsg::Loadout` reply mutates the
    /// `PlayerState`.
    pub fn frame(
        &mut self,
        ui: &mut Ui<'_>,
        loadout: &Loadout,
        player_level: u32,
        talents: &rift_game::talents::TalentTree,
    ) -> Option<SpellbookAction> {
        if !self.open {
            return None;
        }

        let theme = *ui.theme();
        let s = ui.screen_size();
        // Wider panel than before so the left category rail and
        // the wrap-grid both have room without overlapping.
        // Height grows too because the grid now wraps to as many
        // rows as the filtered category needs (up to ~5 rows of
        // 64-px tiles + the persistent detail/loadout strip at
        // the bottom).
        const PANEL_W_BASE: f32 = 960.0;
        const PANEL_H_BASE: f32 = 620.0;
        let margin = theme.spacing.panel_margin();
        let target_w = PANEL_W_BASE * theme.scale;
        let target_h = PANEL_H_BASE * theme.scale;
        let panel_w = target_w.min(s.x - margin * 2.0);
        let panel_h = target_h.min(s.y - margin * 2.0);
        // `fit` is the multiplier interior literals are
        // multiplied by — captures both the global theme scale
        // and any further shrink from the screen clamp so a
        // small viewport still gets a usable layout.
        let fit = (panel_w / PANEL_W_BASE)
            .min(panel_h / PANEL_H_BASE)
            .max(0.4);

        // Modal dim.
        ui.with_layer(rift_engine::ui::im::Layer::Modal, |ui| {
            ui.draw_rect(
                Rect::from_xywh(0.0, 0.0, s.x, s.y),
                Color::rgba(0.0, 0.0, 0.0, 0.55),
            );
        });

        let panel = Rect::from_xywh(
            (s.x - panel_w) * 0.5,
            (s.y - panel_h) * 0.5,
            panel_w,
            panel_h,
        );
        Frame::panel(&theme).show_only(ui, panel);

        // ── Header ────────────────────────────────────────────
        // All chrome paddings come from `theme.spacing` so this
        // panel matches inventory / character-select / etc.
        // exactly. `inner_pad` is the canonical panel gutter;
        // `section_gap` separates header → body → footer.
        let inner_pad = theme.spacing.inner_pad();
        let section_gap = theme.spacing.section_gap();
        let row_gap = theme.spacing.row_gap();
        title(ui, panel.min + Vec2::new(inner_pad, inner_pad), "Spellbook");
        let hint = match self.target_slot {
            Some(i) => format!("Pick an ability for slot {}.", i + 1),
            None => "Click an ability, then click a bar slot to equip it.".to_string(),
        };
        ui.draw_text(
            panel.min + Vec2::new(inner_pad, inner_pad + theme.fonts.size_lg + row_gap * 0.5),
            hint.as_str(),
            theme.fonts.size_sm,
            theme.colors.text_dim,
        );
        // Player level pill (top-right).
        let lvl_text = format!("Lv {player_level}");
        let lvl_w = ui.measure_text(&lvl_text, theme.fonts.size_md);
        ui.draw_text(
            Pos2::new(
                panel.max.x - lvl_w - inner_pad,
                panel.min.y + inner_pad + 4.0,
            ),
            lvl_text.as_str(),
            theme.fonts.size_md,
            theme.colors.accent,
        );

        // ── Layout regions ────────────────────────────────────
        // Header: title + hint + breathing room.
        let header_h =
            inner_pad + theme.fonts.size_lg + row_gap + theme.fonts.size_sm + section_gap;
        let rail_w = 140.0 * fit;
        let bar_strip_h = 110.0 * fit; // ACTION BAR row + label + breathing room
        let detail_h = 132.0 * fit;

        // Left category rail.
        let rail_rect = Rect::from_xywh(
            panel.min.x + inner_pad,
            panel.min.y + header_h,
            rail_w,
            panel_h - header_h - bar_strip_h - detail_h - inner_pad * 2.0,
        );
        // Right grid region.
        let grid_rect = Rect::from_xywh(
            rail_rect.max.x + section_gap,
            rail_rect.min.y,
            panel.max.x - inner_pad - (rail_rect.max.x + section_gap),
            rail_rect.height(),
        );
        // Detail panel (full width).
        let detail_rect = Rect::from_xywh(
            panel.min.x + inner_pad,
            grid_rect.max.y + inner_pad,
            panel_w - inner_pad * 2.0,
            detail_h,
        );
        // Action-bar strip (full width, bottom of panel).
        let bar_strip_rect = Rect::from_xywh(
            panel.min.x + inner_pad,
            detail_rect.max.y + inner_pad,
            panel_w - inner_pad * 2.0,
            panel.max.y - (detail_rect.max.y + inner_pad) - inner_pad,
        );

        // ── Category rail ─────────────────────────────────────
        let cat_btn_h = 36.0 * fit;
        let cat_btn_gap = 6.0 * fit;
        let active_cat = self.selected_category.unwrap_or(Category::All);
        // Per-category counts feed the small "(n)" tail on each
        // tab so the player knows which buckets are populated
        // without opening them.
        let pool_all: Vec<&Ability> = player_abilities().collect();
        for (i, cat) in Category::all().iter().enumerate() {
            let count = if *cat == Category::All {
                pool_all.len()
            } else {
                pool_all.iter().filter(|a| a.category() == *cat).count()
            };
            let label = format!("{}  ({})", cat.label(), count);
            let r = Rect::from_xywh(
                rail_rect.x(),
                rail_rect.y() + (i as f32) * (cat_btn_h + cat_btn_gap),
                rail_rect.width(),
                cat_btn_h,
            );
            let btn = if active_cat == *cat {
                Button::active(label.as_str())
            } else {
                Button::new(label.as_str())
            };
            let resp = btn.show_with_id(ui, Id::root("spellbook_cat").child(i), r);
            if resp.clicked {
                self.selected_category = Some(*cat);
            }
        }

        // ── Ability grid (wrapped) ────────────────────────────
        let tile = 64.0 * fit;
        let tile_gap = 8.0 * fit;
        let cols = ((grid_rect.width() + tile_gap) / (tile + tile_gap))
            .floor()
            .max(1.0) as usize;

        // Filter pool by active category.
        let pool: Vec<&Ability> = if active_cat == Category::All {
            pool_all.clone()
        } else {
            pool_all
                .iter()
                .copied()
                .filter(|a| a.category() == active_cat)
                .collect()
        };

        // Track what the user is pointing at so the detail panel
        // can show its info. Hover wins; selection is the sticky
        // fallback so the panel doesn't go blank when the cursor
        // leaves the grid.
        let mut hovered_ability: Option<&Ability> = None;
        let mut action: Option<SpellbookAction> = None;

        if pool.is_empty() {
            ui.draw_text(
                Pos2::new(grid_rect.x() + 8.0 * fit, grid_rect.y() + 8.0 * fit),
                "No abilities in this category.",
                theme.fonts.size_sm,
                theme.colors.text_muted,
            );
        }

        for (i, ab) in pool.iter().enumerate() {
            let col = i % cols;
            let row = i / cols;
            let pos = Pos2::new(
                grid_rect.x() + col as f32 * (tile + tile_gap),
                grid_rect.y() + row as f32 * (tile + tile_gap),
            );
            let id = Id::root("spellbook_pool").child(ab.wire_id.raw() as usize);
            let unlocked = is_ability_unlocked(ab.wire_id, talents);
            let mut t = ItemSlot::new(tile)
                .selected(self.selected_ability == Some(ab.wire_id))
                .enabled(unlocked);
            if let Some(name) = ab.icon {
                t = t.icon(name);
            } else if let Some(ch) = ab.name.chars().next() {
                t = t
                    .fallback_glyph(ch)
                    .fallback_color(Color::rgba(0.6, 0.85, 1.0, 0.95));
            }
            let resp = t.show(ui, pos, id);
            if resp.hovered {
                hovered_ability = Some(*ab);
            }
            if resp.clicked && unlocked {
                if let Some(slot) = self.target_slot.take() {
                    action = Some(SpellbookAction::AssignSlot {
                        slot_index: slot,
                        ability_id: ab.wire_id,
                    });
                    self.selected_ability = None;
                    self.open = false;
                } else {
                    self.selected_ability = Some(ab.wire_id);
                }
            }
            if !unlocked {
                draw_lock_badge(ui, pos, tile, ab.unlock_level, &theme);
            }
        }

        // ── Detail panel ──────────────────────────────────────
        let focus: Option<&Ability> = hovered_ability
            .or_else(|| self.selected_ability.and_then(rift_game::abilities::lookup));
        Frame::inset(&theme).show_only(ui, detail_rect);
        draw_ability_detail(ui, detail_rect, focus, player_level, &theme);

        // ── Action-bar mirror ─────────────────────────────────
        ui.draw_text(
            Pos2::new(bar_strip_rect.x(), bar_strip_rect.y()),
            "ACTION BAR",
            theme.fonts.size_sm,
            theme.colors.text_muted,
        );
        let bar_y = bar_strip_rect.y() + 22.0 * fit;
        const KEYS: [&str; SLOT_COUNT] = ["LMB", "1", "2", "3", "4", "5"];
        let bar_total_w = SLOT_COUNT as f32 * tile + (SLOT_COUNT - 1) as f32 * tile_gap;
        let bar_x = panel.min.x + (panel_w - bar_total_w) * 0.5;
        for slot_index in 0..SLOT_COUNT {
            let pos = Pos2::new(bar_x + slot_index as f32 * (tile + tile_gap), bar_y);
            let id = Id::root("spellbook_bar").child(slot_index);
            let wire_id = loadout.slots[slot_index];
            let ab = rift_game::abilities::lookup(wire_id);
            let slot_unlocked = is_slot_unlocked(slot_index, player_level);
            let mut t = ItemSlot::new(tile)
                .key_label(KEYS[slot_index])
                .selected(self.target_slot == Some(slot_index as u8))
                .enabled(slot_unlocked);
            if let Some(ab) = ab {
                if let Some(name) = ab.icon {
                    t = t.icon(name);
                } else if let Some(ch) = ab.name.chars().next() {
                    t = t
                        .fallback_glyph(ch)
                        .fallback_color(Color::rgba(0.6, 0.85, 1.0, 0.95));
                }
            }
            let resp = t.show(ui, pos, id);
            if resp.hovered {
                if let Some(ab) = ab {
                    hovered_ability = Some(ab);
                }
            }
            if resp.clicked && slot_unlocked {
                if let Some(picked) = self.selected_ability {
                    action = Some(SpellbookAction::AssignSlot {
                        slot_index: slot_index as u8,
                        ability_id: picked,
                    });
                    self.selected_ability = None;
                    self.target_slot = None;
                } else {
                    self.target_slot = Some(slot_index as u8);
                }
            }
            if !slot_unlocked {
                draw_lock_badge(ui, pos, tile, SLOT_UNLOCK_LEVELS[slot_index], &theme);
            }
        }

        // Re-resolve focus now that bar hover may have updated it.
        let focus = hovered_ability
            .or_else(|| self.selected_ability.and_then(rift_game::abilities::lookup));
        draw_ability_detail(ui, detail_rect, focus, player_level, &theme);

        // Close hint (bottom-right). Measured-anchored so the
        // text edge tracks the panel cleanly under any scale.
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
}

/// Draw a "Lv N" pill anchored to the bottom edge of a tile to
/// show the unlock level for a locked ability or bar slot.
/// Compact and self-contained so locked tiles read at a glance
/// without leaning on captions that overflow neighbouring tiles.
fn draw_lock_badge(
    ui: &mut Ui<'_>,
    tile_pos: Pos2,
    tile_size: f32,
    unlock_level: u32,
    theme: &rift_engine::ui::im::Theme,
) {
    let text = format!("Lv {unlock_level}");
    // Approximate text width — we don't have a measure helper
    // exposed, so size the pill from the digit count.
    let pill_w = 28.0 + (unlock_level >= 10) as i32 as f32 * 8.0;
    let pill_h = 16.0;
    let pill = Rect::from_xywh(
        tile_pos.x + (tile_size - pill_w) * 0.5,
        tile_pos.y + tile_size - pill_h - 4.0,
        pill_w,
        pill_h,
    );
    ui.draw_rect(pill, Color::rgba(0.18, 0.05, 0.05, 0.92));
    ui.draw_text(
        Pos2::new(pill.min.x + 6.0, pill.min.y + 2.0),
        text.as_str(),
        theme.fonts.size_sm,
        Color::rgba(0.95, 0.55, 0.55, 1.0),
    );
}

/// Draw the focused ability's name, level, stats and description
/// inside `rect`. When `focus` is `None` (nothing hovered or
/// selected) draws a placeholder hint.
fn draw_ability_detail(
    ui: &mut Ui<'_>,
    rect: Rect,
    focus: Option<&Ability>,
    player_level: u32,
    theme: &rift_engine::ui::im::Theme,
) {
    let pad = 14.0;
    let Some(ab) = focus else {
        ui.draw_text(
            Pos2::new(rect.min.x + pad, rect.min.y + pad + 4.0),
            "Hover an ability to see its details.",
            theme.fonts.size_sm,
            theme.colors.text_muted,
        );
        return;
    };
    let unlocked = player_level >= ab.unlock_level;

    // Name (large) on the left.
    ui.draw_text(
        Pos2::new(rect.min.x + pad, rect.min.y + pad),
        ab.name,
        theme.fonts.size_lg,
        if unlocked {
            theme.colors.text
        } else {
            theme.colors.text_muted
        },
    );
    // Lv pill on the right.
    let lvl_text = format!("Lv {}", ab.unlock_level);
    let pill_color = if unlocked {
        theme.colors.success
    } else {
        Color::rgba(0.85, 0.45, 0.45, 1.0)
    };
    ui.draw_text(
        Pos2::new(rect.max.x - pad - 64.0, rect.min.y + pad + 4.0),
        lvl_text.as_str(),
        theme.fonts.size_md,
        pill_color,
    );

    // Stats line.
    let stats = if ab.cooldown > 0.0 {
        format!(
            "Cooldown {:.1}s   |   Damage {:.0}%{}",
            ab.cooldown,
            ab.damage_mult * 100.0,
            if ab.projectile_count() > 1 {
                format!("   |   Projectiles {}", ab.projectile_count())
            } else {
                String::new()
            },
        )
    } else {
        format!(
            "Damage {:.0}%{}",
            ab.damage_mult * 100.0,
            if ab.projectile_count() > 1 {
                format!("   |   Projectiles {}", ab.projectile_count())
            } else {
                String::new()
            },
        )
    };
    ui.draw_text(
        Pos2::new(rect.min.x + pad, rect.min.y + pad + 30.0),
        stats.as_str(),
        theme.fonts.size_sm,
        theme.colors.accent,
    );

    // Description.
    ui.draw_text(
        Pos2::new(rect.min.x + pad, rect.min.y + pad + 56.0),
        ab.description,
        theme.fonts.size_sm,
        theme.colors.text_dim,
    );

    // Locked footer.
    if !unlocked {
        let lock_text = format!(
            "Locked. Reach character level {} to unlock.",
            ab.unlock_level
        );
        ui.draw_text(
            Pos2::new(rect.min.x + pad, rect.max.y - pad - 14.0),
            lock_text.as_str(),
            theme.fonts.size_sm,
            Color::rgba(0.85, 0.45, 0.45, 1.0),
        );
    }
}
