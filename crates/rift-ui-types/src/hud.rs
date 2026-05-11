//! HUD view models + action enums.
//!
//! Read by the widget functions in `rift_ui::hud`, written by
//! the host (`rift_client::game::hud`) every frame from the
//! authoritative `PlayerState` / `Health` / `RiftState` data.
//! Plain data only — same hot-reload contract as the rest of
//! this crate.
//!
//! The view is a thin flattening of game state: numeric values
//! and pre-formatted tooltip lines. The widget crate is
//! responsible for layout + draw; the host owns all logic.

// ─── Vitals (HP / Essence / XP) ───────────────────────────────

/// One progress-bar row in the bottom-center vitals stack.
#[derive(Clone, Copy, Debug)]
pub struct VitalsRow {
    /// 0..1, clamped by the host.
    pub fraction: f32,
    /// Pre-formatted label drawn centered on the bar (e.g.
    /// `"427 / 540"` for HP, `"1180 / 2000 XP"` for XP).
    /// The host owns the wording so it stays consistent with
    /// the rest of the game text.
    pub label: &'static str,
}

/// HP / Essence / XP bar stack rendered above the ability bar.
/// `level` floats to the left of the HP bar; shard counts are
/// shown in the inventory header, not here.
#[derive(Clone, Copy, Debug)]
pub struct HudVitalsView<'a> {
    pub hp_fraction: f32,
    pub hp_label: &'a str,
    pub essence_fraction: f32,
    pub essence_label: &'a str,
    pub xp_fraction: f32,
    pub xp_label: &'a str,
    pub level: u32,
}

// ─── Ability bar ──────────────────────────────────────────────

/// Pre-formatted ability tooltip lines. The host builds these
/// (formatting damage / cost / cooldown values against the
/// player's live `CharacterStats`) so the widget doesn't have
/// to depend on `rift_game`.
#[derive(Clone, Debug, Default)]
pub struct AbilityTooltip<'a> {
    pub name: &'a str,
    pub description: &'a str,
    /// `"CD: 1.0s  |  142 damage"` — `None` for non-damaging
    /// abilities with no cooldown.
    pub damage_line: Option<String>,
    /// `"~165 avg  (12% crit, +50% dmg)"` — `None` when the
    /// player has no crit investment or the ability does no
    /// damage.
    pub crit_line: Option<String>,
    /// `"Essence: 25"` / `"Essence: 8 / sec"`. `None` for
    /// free abilities.
    pub cost_line: Option<String>,
    /// `affordable` controls the cost line's colour (blue if
    /// affordable, red if not).
    pub cost_affordable: bool,
    /// `"Projectiles: 3"` — only set when the ability fires
    /// more than one.
    pub projectiles_line: Option<String>,
}

/// One slot on the bottom-center action bar.
#[derive(Clone, Debug)]
pub struct AbilitySlotView<'a> {
    /// `"LMB"`, `"1"`, `"2"`, … rendered as the key hint.
    pub key_hint: &'a str,
    /// `None` for empty slots.
    pub icon: Option<&'a str>,
    /// 2-letter fallback when `icon` is `None`.
    pub fallback_glyph: Option<char>,
    /// 0..1 cooldown progress remaining (1.0 = fully on
    /// cooldown, 0.0 = ready). `0.0` for empty / unlocked-
    /// but-empty slots.
    pub cooldown_remaining: f32,
    /// Locked slots (player below the unlock level) render
    /// disabled with a padlock glyph; `unlock_level` is shown
    /// underneath.
    pub unlocked: bool,
    pub unlock_level: u32,
    /// Greys the slot icon + tints red when the player can't
    /// afford the cast cost.
    pub affordable: bool,
    /// Highlights the slot with a selection halo when the
    /// player is mid-targeting this ability.
    pub selected: bool,
    /// Set when the slot contains an ability and is unlocked.
    /// `None` for empty / locked slots — the tooltip is
    /// suppressed.
    pub tooltip: Option<AbilityTooltip<'a>>,
}

/// Bottom-center action bar.
#[derive(Clone, Debug)]
pub struct AbilityBarView<'a> {
    pub slots: [AbilitySlotView<'a>; 6],
}

// ─── Action ───────────────────────────────────────────────────

/// Returned by `frame_ability_bar` so the host can respond to
/// clicks on the bar (open the spellbook with the slot
/// pre-selected).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum HudAction {
    AbilitySlotClicked(usize),
}

// ─── Minimap ──────────────────────────────────────────────────

/// One non-boss / boss enemy pip on the minimap.
#[derive(Copy, Clone, Debug)]
pub struct MinimapEnemy {
    /// Position in nav-grid coords (x, z). The widget maps
    /// these to screen space using the grid dimensions in
    /// `MinimapView`.
    pub pos: (f32, f32),
    /// Drawn as a fat orange pip instead of the regular red
    /// when `true`.
    pub is_boss: bool,
}

/// Local player marker on the minimap.
#[derive(Copy, Clone, Debug)]
pub struct MinimapPlayer {
    pub pos: (f32, f32),
    /// Facing as a 2D vector in nav-grid space (x, z). The
    /// widget normalises and draws a short heading fan; pass
    /// `(0.0, 0.0)` to skip the fan.
    pub facing: (f32, f32),
}

/// View model for the top-right minimap. The host flattens the
/// hecs world + `NavGrid` into this so the widget doesn't link
/// `rift_engine` / `rift_dungeon`.
#[derive(Copy, Clone, Debug)]
pub struct MinimapView<'a> {
    /// Nav-grid dimensions in cells.
    pub grid_width: u32,
    pub grid_depth: u32,
    /// Row-major walkable mask. `len == grid_width * grid_depth`.
    /// `walkable[z * grid_width + x]` is `true` when the cell
    /// at `(x, z)` should be drawn as floor.
    pub walkable: &'a [bool],
    /// Optional rift / hub portal pip (grid coords).
    pub portal: Option<(f32, f32)>,
    /// Enemy / boss pips drawn over the floor.
    pub enemies: &'a [MinimapEnemy],
    /// Local player marker. `None` while no player exists
    /// (loading, post-death respawn flicker, …).
    pub player: Option<MinimapPlayer>,
}
