//! Combat-meter HUD panel (bottom-right while in a rift).
//!
//! Shows authoritative per-player damage / healing / damage-
//! taken / threat scores pushed by the server roughly once per
//! second as `ServerMsg::MeterSnapshot`. Four tabs let the
//! player switch between metric views without cluttering the
//! HUD with four separate panels:
//!
//! - **DMG**: damage dealt to enemies. Headline metric.
//! - **HPS**: healing done (effective, excluding overheal).
//! - **TAKEN**: damage taken. Useful for "what killed me?"
//!   post-mortems.
//! - **THREAT**: live threat against the highest-aggro target
//!   (computed server-side as max threat across all alive
//!   enemies for that attacker).
//!
//! Per-row layout: a horizontal bar scaled to the top entry's
//! value, the player's name, and the formatted scalar. The
//! bar gives an at-a-glance ranking; the number gives the
//! exact figure for min-maxers.
//!
//! Ownership: drained from [`crate::net::NetClient`] by the
//! binary's frame loop and handed to [`MeterUi::apply_snapshot`]
//! (which also resolves `NetId → display name` once, so the
//! per-frame `frame()` call doesn't need `NetClient`). The
//! panel only renders while the player is inside a rift — the
//! hub gets no meter (no fight to score).

use rift_engine::ui::im::{Color, Pos2, Rect, Ui};
use rift_game::abilities;
use rift_game::monsters::MonsterRole;
use rift_net::messages::{
    MeterAbilityBreakdown, MeterEntry, MeterTakenAttackerBreakdown,
};
use rift_net::NetId;

use crate::net::NetClient;

/// Which metric the user has selected. Default is DMG since
/// that's what most players check first.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MeterTab {
    Dmg,
    Hps,
    Taken,
    Threat,
}

impl Default for MeterTab {
    fn default() -> Self {
        Self::Dmg
    }
}

impl MeterTab {
    fn label(self) -> &'static str {
        match self {
            Self::Dmg => "DMG",
            Self::Hps => "HPS",
            Self::Taken => "TAKEN",
            Self::Threat => "THREAT",
        }
    }

    const ALL: [MeterTab; 4] = [Self::Dmg, Self::Hps, Self::Taken, Self::Threat];
}

/// One pre-resolved row ready for direct rendering.
struct MeterRow {
    net_id: NetId,
    name: String,
    damage_dealt: f32,
    damage_taken: f32,
    healing_done: f32,
    threat: f32,
    /// Per-ability breakdown for the DMG and HPS tabs, sorted
    /// descending by total contribution server-side.
    abilities: Vec<MeterAbilityBreakdown>,
    /// Two-level breakdown for the TAKEN tab: outer rows are
    /// the attacker kind, inner rows are the abilities each
    /// kind hit you with. Sorted server-side descending.
    taken_attackers: Vec<MeterTakenAttackerBreakdown>,
}

/// Aggregate state for the combat-meter HUD. One per
/// `GameState`.
#[derive(Default)]
pub struct MeterUi {
    tab: MeterTab,
    /// Latest snapshot's elapsed clock (seconds since the
    /// instance started). `0.0` until the first push lands.
    elapsed: f32,
    /// Pre-resolved rows. Names are baked in so `frame` only
    /// needs the rows themselves, not a live `NetClient`.
    rows: Vec<MeterRow>,
    /// `Some(net_id)` when a player row has been clicked open
    /// to show its per-ability sub-rows. Cleared by clicking
    /// the same row again, switching to a tab that doesn't
    /// support breakdown (THREAT today), or losing the
    /// row from a fresh snapshot.
    expanded: Option<NetId>,
    /// Second-level expansion for the TAKEN tab: when an
    /// attacker-kind row is open, this holds its wire byte so
    /// the per-ability rows beneath it render. Reset whenever
    /// the parent player row collapses or the tab changes off
    /// TAKEN.
    expanded_attacker: Option<u8>,
    /// Vertical scroll offset, in pixels (pre-scaled). Clamped
    /// each frame to `[0, max_scroll]` so it never goes out of
    /// bounds when the row count shrinks. Mutated by the
    /// mouse-wheel reader inside `frame`.
    scroll: f32,
    /// Tab-button + panel rects from last frame — queried by
    /// `consumes_mouse` so a click on the panel doesn't also
    /// fire a basic attack underneath.
    cached_consume_rects: Vec<Rect>,
}

impl MeterUi {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the panel's data with the freshest server push.
    /// `net` is consulted once to resolve `NetId → name` so the
    /// per-frame draw can run without a `NetClient` reference.
    pub fn apply_snapshot(
        &mut self,
        elapsed: f32,
        entries: Vec<MeterEntry>,
        net: &NetClient,
    ) {
        self.elapsed = elapsed;
        let our_net_id = net.our_net_id();
        let our_name = net.character_name();
        self.rows = entries
            .into_iter()
            .map(|e| {
                let name = if Some(e.net_id) == our_net_id {
                    our_name.map(str::to_string).unwrap_or_else(|| "You".to_string())
                } else if let Some(n) = net.name_for_net_id(e.net_id) {
                    n.to_string()
                } else {
                    format!("#{}", e.net_id.0)
                };
                MeterRow {
                    net_id: e.net_id,
                    name,
                    damage_dealt: e.damage_dealt,
                    damage_taken: e.damage_taken,
                    healing_done: e.healing_done,
                    threat: e.threat,
                    abilities: e.abilities,
                    taken_attackers: e.taken_attackers,
                }
            })
            .collect();
        // Drop the expansion target if the previously-open row
        // is no longer present (player left, swapped instance).
        if let Some(open) = self.expanded {
            if !self.rows.iter().any(|r| r.net_id == open) {
                self.expanded = None;
                self.expanded_attacker = None;
            }
        }
    }

    /// True if `(mx, my)` lands on one of the tab buttons we
    /// drew last frame. The bar rows themselves don't claim
    /// input — clicking a player's row passes through.
    pub fn consumes_mouse(&self, mx: f32, my: f32) -> bool {
        let p = Pos2::new(mx, my);
        self.cached_consume_rects.iter().any(|r| r.contains(p))
    }

    /// Draw the panel. `in_rift` gates rendering so the hub
    /// stays clean.
    pub fn frame(&mut self, ui: &mut Ui<'_>, in_rift: bool) {
        self.cached_consume_rects.clear();
        if !in_rift || self.rows.is_empty() {
            return;
        }

        let theme = *ui.theme();
        let s = theme.scale;
        let screen = ui.screen_size();

        // Layout constants. Each row is ~22 px tall; the
        // header (title + tabs) eats title_h + tab_h + spacing.
        let row_h = 22.0 * s;
        let title_h = 18.0 * s;
        let tab_h = 20.0 * s;
        let header_h = title_h + tab_h + 8.0 * s;
        let pad = 8.0 * s;
        let w = 380.0 * s;

        // Cap panel height so it never exceeds ~60 % of the
        // screen. This is what enables the scrollable body:
        // if the player count grows beyond what fits, extra
        // rows are accessible by scrolling rather than by
        // expanding the panel up off-screen. We also reserve
        // room for at least ~10 rows of body even when the
        // current snapshot has fewer, so the panel doesn't
        // collapse to a tiny strip and clicking to expand a
        // row has somewhere to render its sub-bars.
        let max_panel_h = (screen.y * 0.6).max(header_h + row_h + pad * 2.0);
        let min_body_rows = 10.0;
        let min_body_h = min_body_rows * row_h;
        let want_rows_h = (self.rows.len() as f32) * row_h;
        let body_target = want_rows_h.max(min_body_h);
        let want_h = header_h + body_target + pad * 2.0;
        let h = want_h.min(max_panel_h);
        let inset = 12.0 * s;
        let rect = Rect::from_xywh(
            screen.x - w - inset,
            screen.y - h - inset,
            w,
            h,
        );

        // Background. Drawn directly rather than via `Frame`
        // because we want a subtle dark tint, not a panel-style
        // bordered card.
        ui.draw_rounded_rect(
            rect,
            theme.spacing.corner_radius,
            Color::rgba(0.05, 0.05, 0.07, 0.78),
        );
        ui.draw_rounded_outline(
            rect,
            theme.spacing.corner_radius,
            1.0,
            theme.colors.border,
        );
        // Whole-panel hit rect: lets the gameplay layer skip
        // basic-attack clicks under us.
        self.cached_consume_rects.push(rect);

        // ---- Title row: tab name on the left, elapsed clock
        //      on the right. Lives above the tabs so neither
        //      element fights with them for horizontal space.
        let title_y = rect.y() + pad;
        let _ = ui.draw_text(
            Pos2::new(rect.x() + pad, title_y),
            "Combat",
            theme.fonts.size_sm,
            theme.colors.text_dim,
        );
        let secs = self.elapsed;
        let mm = (secs / 60.0) as u32;
        let ss = (secs % 60.0) as u32;
        let elapsed_text = format!("{mm:02}:{ss:02}");
        // Measure the actual rendered width so the right edge
        // sits inside the panel padding regardless of font /
        // scale. (The earlier `len() * 6.5 * s` heuristic
        // overshot at higher UI scales and pushed the timer
        // off the right edge.)
        let elapsed_w = ui.measure_text(&elapsed_text, theme.fonts.size_sm);
        let elapsed_x = rect.x() + w - pad - elapsed_w;
        let _ = ui.draw_text(
            Pos2::new(elapsed_x, title_y),
            &elapsed_text,
            theme.fonts.size_sm,
            theme.colors.text_dim,
        );

        // ---- Tab row ------------------------------------------------------
        let tab_w = (w - pad * 2.0) / MeterTab::ALL.len() as f32;
        let tab_y = title_y + title_h + 4.0 * s;
        // `left_clicked()` is a one-shot read (consumes the
        // click), so check hover first and only call it when
        // the cursor is actually over a tab. Otherwise an
        // earlier UI layer's read would have already burned
        // the click before we get here.
        let mouse = ui.mouse_pos();
        let mut hovered_tab: Option<MeterTab> = None;
        for (i, tab) in MeterTab::ALL.iter().enumerate() {
            let tx = rect.x() + pad + (i as f32) * tab_w;
            let tab_rect = Rect::from_xywh(tx, tab_y, tab_w, tab_h);
            if tab_rect.contains(mouse) {
                hovered_tab = Some(*tab);
            }
            let active = *tab == self.tab;
            // Active tab: brighter fill so the selected metric
            // pops without needing extra chrome.
            let fill = if active {
                theme.colors.accent
            } else {
                Color::rgba(0.12, 0.12, 0.16, 0.85)
            };
            ui.draw_rounded_rect(tab_rect, 2.0 * s, fill);
            let label = tab.label();
            let text_color = if active {
                theme.colors.text
            } else {
                theme.colors.text_dim
            };
            // Approximate horizontal centring without measuring
            // the glyph run: the button rects are uniform, so a
            // fixed offset reads OK at every UI scale.
            let _ = ui.draw_text(
                Pos2::new(tx + tab_w * 0.5 - (label.len() as f32) * 3.5 * s, tab_y + 4.0 * s),
                label,
                theme.fonts.size_sm,
                text_color,
            );
        }
        if let Some(tab) = hovered_tab {
            if ui.input().left_clicked() {
                self.tab = tab;
            }
        }

        // DMG / HPS / TAKEN all carry per-ability breakdown
        // rows now (TAKEN is credited at the receiving player
        // by `apply_player_damage`). THREAT is still a single
        // instantaneous number with no per-ability slice.
        let supports_breakdown = matches!(self.tab, MeterTab::Dmg | MeterTab::Hps | MeterTab::Taken);
        if !supports_breakdown {
            self.expanded = None;
        }

        // ---- Sorted rows --------------------------------------------------
        // Pull the value the active tab cares about, sort
        // descending (highest at top), then bar-scale every row
        // against the leader so the visual bar is "share of #1".
        let mut indexed: Vec<(usize, f32)> = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| (i, value_for(self.tab, r)))
            .collect();
        indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let top = indexed.first().map(|r| r.1).unwrap_or(0.0);
        // Per-second display for DMG / HPS / TAKEN. THREAT is
        // an instantaneous value so we leave it alone.
        let per_second = secs > 0.5
            && matches!(self.tab, MeterTab::Dmg | MeterTab::Hps | MeterTab::Taken);

        // Body region: everything below the tabs is scrollable.
        let body_top = tab_y + tab_h + 8.0 * s;
        let body_bottom = rect.y() + h - pad;
        let body_h = (body_bottom - body_top).max(0.0);

        // Total content height accounts for any expanded row's
        // sub-rows. We only count abilities the active tab
        // actually shows (non-zero `ability_value`); zero
        // entries are hidden by the row walk below, so
        // counting them here would leave phantom slots in
        // the scroll math (and visually).
        let sub_row_h = 18.0 * s;
        let mut total_rows_h = (self.rows.len() as f32) * row_h;
        if let Some(open) = self.expanded {
            if let Some(open_row) = self.rows.iter().find(|r| r.net_id == open) {
                total_rows_h += taken_or_ability_sub_count(
                    self.tab,
                    open_row,
                    self.expanded_attacker,
                ) as f32
                    * sub_row_h;
            }
        }
        let max_scroll = (total_rows_h - body_h).max(0.0);

        // Mouse-wheel: only consume when the cursor is over
        // the panel so background scroll (camera zoom) still
        // works elsewhere. Wheel delta is in lines; multiply
        // out to one row per notch so a single tick advances
        // the list by one entry.
        if rect.contains(mouse) {
            let delta = ui.input().scroll_delta();
            if delta.abs() > 0.0 {
                self.scroll -= delta * row_h;
            }
        }
        self.scroll = self.scroll.clamp(0.0, max_scroll);

        let row_x = rect.x() + pad;
        let row_w = w - pad * 2.0;
        // Walk sorted rows top-down, advancing `cursor_y` by
        // either `row_h` (collapsed) or `row_h + sub_rows *
        // sub_row_h` (expanded). Click detection is rolled into
        // the same loop so we don't miss a row clipped by
        // scroll.
        let mut cursor_y = body_top - self.scroll;
        let mut click_target: Option<NetId> = None;
        let mut attacker_click_target: Option<u8> = None;
        for (idx, value) in indexed.into_iter() {
            let y = cursor_y;
            // Skip drawing rows entirely outside the visible
            // body region, but still advance the cursor so
            // later rows land at the right Y.
            let row_visible = y + row_h > body_top && y < body_bottom;
            if row_visible {
                let bar_rect = Rect::from_xywh(row_x, y, row_w, row_h - 4.0 * s);
                let frac = if top > 0.0 { (value / top).clamp(0.0, 1.0) } else { 0.0 };
                draw_meter_bar(ui, bar_rect, frac, bar_color(self.tab), 1.0);

                let row = &self.rows[idx];
                let is_open = self.expanded == Some(row.net_id);
                // Caret hint when the tab supports breakdown,
                // so players can tell rows are clickable.
                let caret = if supports_breakdown {
                    if is_open { "v " } else { "> " }
                } else {
                    ""
                };
                let _ = ui.draw_text(
                    Pos2::new(row_x + 6.0 * s, y + 3.0 * s),
                    &format!("{caret}{}", row.name),
                    theme.fonts.size_sm,
                    theme.colors.text,
                );

                // Right-aligned value, split into two fixed
                // columns so the cumulative total never
                // collides with the per-second rate as either
                // grows: rate at the far right, then total in
                // its own column to the left of it. When we're
                // not in per-second mode (THREAT) we just draw
                // the total in the right slot.
                let rate_col_w = 80.0 * s;
                let col_gap = 6.0 * s;
                let total_text = format_short(value);
                let total_w = ui.measure_text(&total_text, theme.fonts.size_sm);
                if per_second && secs > 0.0 {
                    let rate_text = format!("{}/s", format_short(value / secs));
                    let rate_w = ui.measure_text(&rate_text, theme.fonts.size_sm);
                    let rate_x = row_x + row_w - 6.0 * s - rate_w;
                    let _ = ui.draw_text(
                        Pos2::new(rate_x, y + 3.0 * s),
                        &rate_text,
                        theme.fonts.size_sm,
                        theme.colors.text,
                    );
                    // Total: anchored to the right of its own
                    // fixed-width column so it doesn't drift
                    // into the rate column when it grows.
                    let total_col_right = row_x + row_w - 6.0 * s - rate_col_w - col_gap;
                    let total_x = total_col_right - total_w;
                    let _ = ui.draw_text(
                        Pos2::new(total_x, y + 3.0 * s),
                        &total_text,
                        theme.fonts.size_sm,
                        theme.colors.text_dim,
                    );
                } else {
                    let total_x = row_x + row_w - 6.0 * s - total_w;
                    let _ = ui.draw_text(
                        Pos2::new(total_x, y + 3.0 * s),
                        &total_text,
                        theme.fonts.size_sm,
                        theme.colors.text,
                    );
                }

                // Click-to-expand: hit-test against the bar
                // rect; the actual click read happens after the
                // loop so we only consume one click per frame.
                if supports_breakdown && bar_rect.contains(mouse) {
                    click_target = Some(row.net_id);
                }
            }
            cursor_y += row_h;

            // Inline sub-rows for the expanded player on the
            // current breakdown-capable tab. The TAKEN tab
            // gets its own two-level renderer (attacker →
            // ability); DMG / HPS use the flat per-ability
            // breakdown.
            let row = &self.rows[idx];
            if supports_breakdown && self.expanded == Some(row.net_id) {
                if matches!(self.tab, MeterTab::Taken) {
                    draw_taken_breakdown(
                        ui,
                        theme,
                        row,
                        row_x,
                        row_w,
                        sub_row_h,
                        body_top,
                        body_bottom,
                        s,
                        secs,
                        per_second,
                        self.expanded_attacker,
                        &mut cursor_y,
                        mouse,
                        &mut attacker_click_target,
                    );
                } else {
                    draw_ability_breakdown(
                        ui,
                        theme,
                        row,
                        self.tab,
                        row_x,
                        row_w,
                        sub_row_h,
                        body_top,
                        body_bottom,
                        s,
                        secs,
                        per_second,
                        &mut cursor_y,
                    );
                }
            }
        }
        // Single click consumption per frame — toggles whichever
        // row the cursor is hovering. Clicking the open row
        // collapses it; clicking another row replaces.
        if let Some(target) = click_target {
            if ui.input().left_clicked() {
                self.expanded = if self.expanded == Some(target) {
                    None
                } else {
                    Some(target)
                };
                // Switching player rows resets the inner
                // attacker expansion so the TAKEN tab opens
                // collapsed again.
                self.expanded_attacker = None;
            }
        } else if let Some(kind) = attacker_click_target {
            if ui.input().left_clicked() {
                self.expanded_attacker = if self.expanded_attacker == Some(kind) {
                    None
                } else {
                    Some(kind)
                };
            }
        }

        // ---- Scrollbar ----------------------------------------------------
        // Drawn last so it overlays any partially-visible row
        // edges at the top / bottom of the body region. Thin
        // and unobtrusive when not needed; only meaningful
        // when there's anything to scroll.
        if max_scroll > 0.0 {
            let track_w = 3.0 * s;
            let track_x = rect.x() + w - track_w - 2.0 * s;
            let track_rect = Rect::from_xywh(track_x, body_top, track_w, body_h);
            ui.draw_rect(track_rect, Color::rgba(0.12, 0.12, 0.16, 0.6));
            let thumb_h = (body_h * (body_h / total_rows_h)).max(12.0 * s);
            let thumb_y = body_top + (body_h - thumb_h) * (self.scroll / max_scroll);
            ui.draw_rect(
                Rect::from_xywh(track_x, thumb_y, track_w, thumb_h),
                theme.colors.accent,
            );
        }
    }
}

/// Per-tab metric extractor.
fn value_for(tab: MeterTab, r: &MeterRow) -> f32 {
    match tab {
        MeterTab::Dmg => r.damage_dealt,
        MeterTab::Hps => r.healing_done,
        MeterTab::Taken => r.damage_taken,
        MeterTab::Threat => r.threat,
    }
}

/// Per-tab metric extractor for the per-ability breakdown
/// rows. Used by the DMG and HPS sub-row pass; the TAKEN tab
/// has its own two-level renderer (attacker → ability) and
/// doesn't go through this.
fn ability_value(tab: MeterTab, a: &MeterAbilityBreakdown) -> f32 {
    match tab {
        MeterTab::Dmg => a.damage_dealt,
        MeterTab::Hps => a.healing_done,
        _ => 0.0,
    }
}

/// Resolve a wire ability id to a display name. Falls back to
/// "Other" for the unattributed sentinel and unknown ids.
fn ability_name(id: u8) -> &'static str {
    abilities::from_wire_id(abilities::AbilityWireId::new(id)).map(|a| a.name).unwrap_or("Other")
}

/// Resolve an attacker-kind wire byte to a display name.
/// Unknown bytes (and the `255` "Other" sentinel) collapse
/// to `"Other"` so future server roles don't crash old
/// clients.
fn attacker_name(kind: u8) -> &'static str {
    MonsterRole::from_wire_byte(kind)
        .map(|r| r.display_name())
        .unwrap_or("Other")
}

/// Draw one meter bar with consistent rounding regardless of
/// the fill fraction. The stock `ProgressBar` widget renders
/// the fill as a sharp rect when `frac < 0.999` (which gives a
/// "drained cap" look), so the leader's bar visually differs
/// from the rest. We want every row to read the same, so we
/// roll our own: rounded track, rounded fill, fill clipped
/// horizontally to `frac` of the rect (with a tiny minimum
/// width so a non-zero contribution still shows a sliver of
/// pill rather than nothing).
fn draw_meter_bar(ui: &mut Ui<'_>, rect: Rect, frac: f32, fill: Color, alpha: f32) {
    let theme = *ui.theme();
    let radius = theme.spacing.corner_radius;
    let track = Color::rgba(0.10, 0.10, 0.13, 0.85);
    ui.draw_rounded_rect(rect, radius, track);
    let frac = frac.clamp(0.0, 1.0);
    if frac > 0.0 {
        // Cap the fill at the row width and floor it at one
        // diameter so a small bar still shows a visible pill
        // instead of disappearing into the rounded corner.
        let min_w = (radius * 2.0).min(rect.width());
        let fw = (rect.width() * frac).max(min_w);
        let fill_rect = Rect::from_xywh(rect.x(), rect.y(), fw, rect.height());
        let c = if (alpha - 1.0).abs() < f32::EPSILON {
            fill
        } else {
            fill.with_alpha(alpha)
        };
        ui.draw_rounded_rect(fill_rect, radius, c);
    }
    ui.draw_rounded_outline(
        rect,
        radius,
        theme.spacing.border_thickness,
        theme.colors.border,
    );
}

/// Pick a hue per metric so the panel reads at a glance even
/// without looking at the active tab label.
fn bar_color(tab: MeterTab) -> Color {
    match tab {
        // Red-ish: the universal "damage" colour.
        MeterTab::Dmg => Color::rgba(0.78, 0.22, 0.22, 0.92),
        // Green-ish: healing.
        MeterTab::Hps => Color::rgba(0.28, 0.72, 0.34, 0.92),
        // Orange-ish: damage taken (warning, not heal).
        MeterTab::Taken => Color::rgba(0.85, 0.55, 0.18, 0.92),
        // Purple-ish: threat / aggro.
        MeterTab::Threat => Color::rgba(0.62, 0.36, 0.82, 0.92),
    }
}

/// Compact numeric formatting: 1234 → "1.2k", 1_234_567 →
/// "1.2M". Keeps the rightmost column narrow enough that the
/// player's name isn't squeezed.
fn format_short(v: f32) -> String {
    let v = v.max(0.0);
    if v >= 1_000_000.0 {
        format!("{:.1}M", v / 1_000_000.0)
    } else if v >= 1_000.0 {
        format!("{:.1}k", v / 1_000.0)
    } else if v >= 100.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

/// Count the visible sub-rows for an expanded player row on
/// the active tab. Used by the scroll-height pre-pass so
/// scrolling doesn't leave phantom slots when the inner row
/// list is sparse. For TAKEN, also counts the inner ability
/// rows beneath an expanded attacker.
fn taken_or_ability_sub_count(
    tab: MeterTab,
    row: &MeterRow,
    expanded_attacker: Option<u8>,
) -> usize {
    match tab {
        MeterTab::Taken => {
            let mut n = row.taken_attackers.len();
            if let Some(kind) = expanded_attacker {
                if let Some(att) = row
                    .taken_attackers
                    .iter()
                    .find(|a| a.attacker_kind == kind)
                {
                    n += att.abilities.len();
                }
            }
            n
        }
        MeterTab::Dmg | MeterTab::Hps => row
            .abilities
            .iter()
            .filter(|a| ability_value(tab, a) > 0.0)
            .count(),
        MeterTab::Threat => 0,
    }
}

/// Render the expanded TAKEN-tab breakdown for one player row:
/// outer rows are attacker kinds, inner rows (under an open
/// attacker) are the abilities that kind hit you with.
#[allow(clippy::too_many_arguments)]
fn draw_taken_breakdown(
    ui: &mut Ui<'_>,
    theme: rift_engine::ui::im::Theme,
    row: &MeterRow,
    row_x: f32,
    row_w: f32,
    sub_row_h: f32,
    body_top: f32,
    body_bottom: f32,
    s: f32,
    secs: f32,
    per_second: bool,
    expanded_attacker: Option<u8>,
    cursor_y: &mut f32,
    mouse: Pos2,
    attacker_click_target: &mut Option<u8>,
) {
    let outer_top = row
        .taken_attackers
        .iter()
        .map(|a| a.damage_taken)
        .fold(0.0_f32, f32::max);
    for att in &row.taken_attackers {
        let v = att.damage_taken;
        if v <= 0.0 {
            continue;
        }
        let sy = *cursor_y;
        let visible = sy + sub_row_h > body_top && sy < body_bottom;
        let is_open = expanded_attacker == Some(att.attacker_kind);
        if visible {
            let indent = 16.0 * s;
            let bar = Rect::from_xywh(
                row_x + indent,
                sy + 2.0 * s,
                row_w - indent,
                sub_row_h - 6.0 * s,
            );
            let frac = if outer_top > 0.0 {
                (v / outer_top).clamp(0.0, 1.0)
            } else {
                0.0
            };
            draw_meter_bar(ui, bar, frac, bar_color(MeterTab::Taken), 0.55);
            let caret = if is_open { "v " } else { "> " };
            let label = format!("{caret}{}", attacker_name(att.attacker_kind));
            let _ = ui.draw_text(
                Pos2::new(row_x + indent + 4.0 * s, sy + 3.0 * s),
                &label,
                theme.fonts.size_sm,
                theme.colors.text_dim,
            );
            draw_value_columns(
                ui,
                &theme,
                row_x,
                row_w,
                sy,
                s,
                v,
                secs,
                per_second,
            );
            if bar.contains(mouse) {
                *attacker_click_target = Some(att.attacker_kind);
            }
        }
        *cursor_y += sub_row_h;
        if is_open {
            let inner_top = att
                .abilities
                .iter()
                .map(|a| a.damage_taken)
                .fold(0.0_f32, f32::max);
            for ab in &att.abilities {
                let av = ab.damage_taken;
                if av <= 0.0 {
                    continue;
                }
                let isy = *cursor_y;
                let iv = isy + sub_row_h > body_top && isy < body_bottom;
                if iv {
                    let indent = 32.0 * s;
                    let bar = Rect::from_xywh(
                        row_x + indent,
                        isy + 2.0 * s,
                        row_w - indent,
                        sub_row_h - 6.0 * s,
                    );
                    let frac = if inner_top > 0.0 {
                        (av / inner_top).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    draw_meter_bar(ui, bar, frac, bar_color(MeterTab::Taken), 0.40);
                    let _ = ui.draw_text(
                        Pos2::new(row_x + indent + 4.0 * s, isy + 3.0 * s),
                        ability_name(ab.ability_id),
                        theme.fonts.size_sm,
                        theme.colors.text_dim,
                    );
                    draw_value_columns(
                        ui,
                        &theme,
                        row_x,
                        row_w,
                        isy,
                        s,
                        av,
                        secs,
                        per_second,
                    );
                }
                *cursor_y += sub_row_h;
            }
        }
    }
}

/// Render the expanded DMG / HPS sub-rows for one player row.
#[allow(clippy::too_many_arguments)]
fn draw_ability_breakdown(
    ui: &mut Ui<'_>,
    theme: rift_engine::ui::im::Theme,
    row: &MeterRow,
    tab: MeterTab,
    row_x: f32,
    row_w: f32,
    sub_row_h: f32,
    body_top: f32,
    body_bottom: f32,
    s: f32,
    secs: f32,
    per_second: bool,
    cursor_y: &mut f32,
) {
    let sub_top = row
        .abilities
        .iter()
        .map(|a| ability_value(tab, a))
        .fold(0.0_f32, f32::max);
    for ab in &row.abilities {
        let v = ability_value(tab, ab);
        if v <= 0.0 {
            continue;
        }
        let sy = *cursor_y;
        let visible = sy + sub_row_h > body_top && sy < body_bottom;
        if visible {
            let indent = 16.0 * s;
            let bar = Rect::from_xywh(
                row_x + indent,
                sy + 2.0 * s,
                row_w - indent,
                sub_row_h - 6.0 * s,
            );
            let frac = if sub_top > 0.0 {
                (v / sub_top).clamp(0.0, 1.0)
            } else {
                0.0
            };
            draw_meter_bar(ui, bar, frac, bar_color(tab), 0.55);
            let _ = ui.draw_text(
                Pos2::new(row_x + indent + 4.0 * s, sy + 3.0 * s),
                ability_name(ab.ability_id),
                theme.fonts.size_sm,
                theme.colors.text_dim,
            );
            draw_value_columns(
                ui,
                &theme,
                row_x,
                row_w,
                sy,
                s,
                v,
                secs,
                per_second,
            );
        }
        *cursor_y += sub_row_h;
    }
}

/// Shared right-aligned `total | rate/s` column renderer used
/// by every sub-row (DMG / HPS abilities, TAKEN attackers,
/// TAKEN abilities).
#[allow(clippy::too_many_arguments)]
fn draw_value_columns(
    ui: &mut Ui<'_>,
    theme: &rift_engine::ui::im::Theme,
    row_x: f32,
    row_w: f32,
    y: f32,
    s: f32,
    v: f32,
    secs: f32,
    per_second: bool,
) {
    let total_text = format_short(v);
    let total_w = ui.measure_text(&total_text, theme.fonts.size_sm);
    let rate_col_w = 80.0 * s;
    let col_gap = 6.0 * s;
    if per_second && secs > 0.0 {
        let rate_text = format!("{}/s", format_short(v / secs));
        let rate_w = ui.measure_text(&rate_text, theme.fonts.size_sm);
        let rate_x = row_x + row_w - 6.0 * s - rate_w;
        let _ = ui.draw_text(
            Pos2::new(rate_x, y + 3.0 * s),
            &rate_text,
            theme.fonts.size_sm,
            theme.colors.text_dim,
        );
        let total_col_right = row_x + row_w - 6.0 * s - rate_col_w - col_gap;
        let total_x = total_col_right - total_w;
        let _ = ui.draw_text(
            Pos2::new(total_x, y + 3.0 * s),
            &total_text,
            theme.fonts.size_sm,
            theme.colors.text_dim,
        );
    } else {
        let total_x = row_x + row_w - 6.0 * s - total_w;
        let _ = ui.draw_text(
            Pos2::new(total_x, y + 3.0 * s),
            &total_text,
            theme.fonts.size_sm,
            theme.colors.text_dim,
        );
    }
}
