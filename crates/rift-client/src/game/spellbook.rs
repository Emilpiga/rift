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
    widgets::{title, ItemSlot},
    Color, Frame, Id, Pos2, Rect, Ui, Vec2,
};
use rift_game::abilities::Ability;
use rift_game::loadout::{
    is_ability_unlocked, is_slot_unlocked, player_abilities, Loadout,
    SLOT_COUNT, SLOT_UNLOCK_LEVELS,
};

/// User intent emitted by [`SpellbookUi::frame`]. The binary
/// converts these into network commands. Mirrors the inventory
/// UI's "queue intent here, send-from-binary" split.
#[derive(Clone, Copy, Debug)]
pub enum SpellbookAction {
    /// Player asked to put `ability_id` into action-bar slot
    /// `slot_index`. `ability_id == EMPTY_SLOT` clears the slot.
    AssignSlot { slot_index: u8, ability_id: u8 },
}

#[derive(Default)]
pub struct SpellbookUi {
    pub open: bool,
    /// Wire id of the ability the user has selected from the
    /// pool. `None` means "no ability picked yet — clicking a
    /// slot is a no-op".
    selected_ability: Option<u8>,
    /// Bar slot the user pre-targeted by clicking the HUD.
    /// `Some(i)` means the next pool click assigns directly to
    /// slot `i`. Cleared on assign / close.
    target_slot: Option<u8>,
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
    ) -> Option<SpellbookAction> {
        if !self.open {
            return None;
        }
        if ui.input().key_just_pressed(winit::keyboard::KeyCode::Escape) {
            self.close();
            return None;
        }

        let theme = *ui.theme();
        let s = ui.screen_size();

        // Modal dim.
        ui.with_layer(rift_engine::ui::im::Layer::Modal, |ui| {
            ui.draw_rect(
                Rect::from_xywh(0.0, 0.0, s.x, s.y),
                Color::rgba(0.0, 0.0, 0.0, 0.55),
            );
        });

        // Panel — taller than before so the detail panel has
        // room. Captions under tiles caused name overlap, so we
        // moved all per-ability text into a single detail panel
        // that updates as the user hovers / selects.
        const PANEL_W: f32 = 760.0;
        const PANEL_H: f32 = 520.0;
        let panel = Rect::from_xywh(
            (s.x - PANEL_W) * 0.5,
            (s.y - PANEL_H) * 0.5,
            PANEL_W,
            PANEL_H,
        );
        Frame::panel(&theme).show_only(ui, panel);

        title(ui, panel.min + Vec2::new(24.0, 18.0), "Spellbook");
        let hint = match self.target_slot {
            Some(i) => format!("Pick an ability for slot {}.", i + 1),
            None => "Click an ability, then click a bar slot to equip it.".to_string(),
        };
        ui.draw_text(
            panel.min + Vec2::new(24.0, 52.0),
            hint.as_str(),
            theme.fonts.size_sm,
            theme.colors.text_dim,
        );
        // Player level pill (top-right).
        let lvl_text = format!("Lv {player_level}");
        ui.draw_text(
            Pos2::new(panel.max.x - 80.0, panel.min.y + 22.0),
            lvl_text.as_str(),
            theme.fonts.size_md,
            theme.colors.accent,
        );

        // Section: pool of pickable abilities.
        const TILE: f32 = 64.0;
        const TILE_GAP: f32 = 8.0;
        ui.draw_text(
            panel.min + Vec2::new(24.0, 84.0),
            "ABILITIES",
            theme.fonts.size_sm,
            theme.colors.text_muted,
        );
        let pool: Vec<&Ability> = player_abilities().collect();
        let pool_y = panel.min.y + 108.0;
        let mut action: Option<SpellbookAction> = None;
        // Track what the user is pointing at so the detail panel
        // can show its info. Hover wins; selection is the sticky
        // fallback so the panel doesn't go blank when the cursor
        // leaves the grid.
        let mut hovered_ability: Option<&Ability> = None;
        for (i, ab) in pool.iter().enumerate() {
            let pos = Pos2::new(
                panel.min.x + 24.0 + i as f32 * (TILE + TILE_GAP),
                pool_y,
            );
            let id = Id::root("spellbook_pool").child(i);
            let unlocked = is_ability_unlocked(ab.wire_id, player_level);
            let mut tile = ItemSlot::new(TILE)
                .selected(self.selected_ability == Some(ab.wire_id))
                .enabled(unlocked);
            if let Some(name) = ab.icon {
                tile = tile.icon(name);
            } else if let Some(ch) = ab.name.chars().next() {
                tile = tile
                    .fallback_glyph(ch)
                    .fallback_color(Color::rgba(0.6, 0.85, 1.0, 0.95));
            }
            let resp = tile.show(ui, pos, id);
            if resp.hovered {
                hovered_ability = Some(*ab);
            }
            if resp.clicked && unlocked {
                if let Some(slot) = self.target_slot.take() {
                    // Pre-targeted from the HUD: assign right
                    // away and close.
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

            // Lock badge: small "Lv N" pill anchored to the
            // bottom of locked tiles. Replaces the per-tile
            // caption that was overlapping with neighbors.
            if !unlocked {
                draw_lock_badge(ui, pos, TILE, ab.unlock_level, &theme);
            }
        }

        // Section: detail panel for the focused ability.
        let focus: Option<&Ability> = hovered_ability.or_else(|| {
            self.selected_ability
                .and_then(rift_game::abilities::lookup)
        });
        let detail_rect = Rect::from_xywh(
            panel.min.x + 24.0,
            pool_y + TILE + 16.0,
            PANEL_W - 48.0,
            128.0,
        );
        Frame::inset(&theme).show_only(ui, detail_rect);
        draw_ability_detail(ui, detail_rect, focus, player_level, &theme);

        // Section: action-bar mirror.
        let bar_label_y = detail_rect.max.y + 16.0;
        ui.draw_text(
            Pos2::new(panel.min.x + 24.0, bar_label_y),
            "ACTION BAR",
            theme.fonts.size_sm,
            theme.colors.text_muted,
        );
        let bar_y = bar_label_y + 24.0;
        const KEYS: [&str; SLOT_COUNT] = ["LMB", "1", "2", "3", "4", "5"];
        // Centre the bar inside the panel.
        let bar_total_w = SLOT_COUNT as f32 * TILE + (SLOT_COUNT - 1) as f32 * TILE_GAP;
        let bar_x = panel.min.x + (PANEL_W - bar_total_w) * 0.5;
        for slot_index in 0..SLOT_COUNT {
            let pos = Pos2::new(
                bar_x + slot_index as f32 * (TILE + TILE_GAP),
                bar_y,
            );
            let id = Id::root("spellbook_bar").child(slot_index);
            let wire_id = loadout.slots[slot_index];
            let ab = rift_game::abilities::lookup(wire_id);
            let slot_unlocked = is_slot_unlocked(slot_index, player_level);
            let mut tile = ItemSlot::new(TILE)
                .key_label(KEYS[slot_index])
                .selected(self.target_slot == Some(slot_index as u8))
                .enabled(slot_unlocked);
            if let Some(ab) = ab {
                if let Some(name) = ab.icon {
                    tile = tile.icon(name);
                } else if let Some(ch) = ab.name.chars().next() {
                    tile = tile
                        .fallback_glyph(ch)
                        .fallback_color(Color::rgba(0.6, 0.85, 1.0, 0.95));
                }
            }
            let resp = tile.show(ui, pos, id);
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
                    // No ability picked yet — pre-target the
                    // slot so the next pool click assigns
                    // directly.
                    self.target_slot = Some(slot_index as u8);
                }
            }
            // Locked-slot lock badge replaces the previous
            // caption; the unlock level lives inside the tile
            // instead of below it.
            if !slot_unlocked {
                draw_lock_badge(ui, pos, TILE, SLOT_UNLOCK_LEVELS[slot_index], &theme);
            }
        }

        // Re-resolve focus now that bar hover may have updated it.
        let focus = hovered_ability.or_else(|| {
            self.selected_ability
                .and_then(rift_game::abilities::lookup)
        });
        // Redraw the detail panel contents on top with the (possibly
        // updated) focus — cheap: just a few text draws over the
        // existing background. Avoids a one-frame lag where bar
        // hover wouldn't update the detail panel until next frame.
        draw_ability_detail(ui, detail_rect, focus, player_level, &theme);

        // Close hint (bottom-right).
        let close_text = "B / Esc to close";
        ui.draw_text(
            Pos2::new(panel.max.x - 140.0, panel.max.y - 24.0),
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
        if unlocked { theme.colors.text } else { theme.colors.text_muted },
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
            if ab.projectile_count > 1 {
                format!("   |   Projectiles {}", ab.projectile_count)
            } else {
                String::new()
            },
        )
    } else {
        format!(
            "Damage {:.0}%{}",
            ab.damage_mult * 100.0,
            if ab.projectile_count > 1 {
                format!("   |   Projectiles {}", ab.projectile_count)
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
