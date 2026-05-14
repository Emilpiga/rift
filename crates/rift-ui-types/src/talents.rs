//! Talent-tree view models + actions.
//!
//! Built every frame by `rift-client` from
//! `rift_game::talents::TalentTree`, consumed by
//! `rift_ui::talents::frame_talent_panel`. Plain data — no
//! `rift_game` types cross the hot-reload boundary.

// ─── Routing ─────────────────────────────────────────────────

/// Mirror of `rift_game::talents::Route`. Drives auto-layout
/// (Hub at centre, four routes radiating out) and route-
/// tinted node colours.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum TalentRouteView {
    Hub,
    Warrior,
    Mage,
    Healer,
    Summoner,
    Synergy,
    Fifth,
}

/// Coarse classification of a node, used for shape / glow.
/// The host derives this from the `TalentEffect` enum so the
/// widget doesn't need to pattern-match `rift_game` data.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum TalentNodeKind {
    /// `PercentBonus` / `FlatBonus`.
    Stat,
    /// `UnlockAbility` — bigger node, distinctive ring.
    Unlock,
    /// `AbilityMod` — small node, square-ish.
    Modifier,
    /// `PassiveProc`.
    Proc,
    /// `Keystone` — keystone glow flourish.
    Keystone,
    /// Cross-route hybrid payoff node.
    Synergy,
    /// Hub→route connector. Rendered slimmer / dimmer.
    Connector,
}

// ─── View models ─────────────────────────────────────────────

/// One node as the UI needs to see it. Indexes into
/// `TalentTreeView::nodes` are used inside this slice for
/// prereq linking — `prereq_indices` references positions in
/// the same `nodes` vector to avoid an `id → index` lookup
/// inside the widget.
#[derive(Clone, Debug)]
pub struct TalentNodeView<'a> {
    /// Stable id from `rift_game::talents::TalentId.0`. The
    /// widget hands this back via `TalentTreeAction::Invest`.
    pub id: u16,
    pub name: &'a str,
    pub description: &'a str,
    pub route: TalentRouteView,
    pub kind: TalentNodeKind,
    pub current_rank: u8,
    pub max_rank: u8,
    /// Blueprint graph coordinate. When present, the panel uses
    /// it directly instead of the legacy auto-layout fallback.
    pub position: Option<(f32, f32)>,
    /// Human-readable implementation status, e.g. "Ready" or
    /// "Needs System". Display-only; investability is still the
    /// authoritative gate.
    pub status: &'a str,
    /// Indices (into `TalentTreeView::nodes`) of prerequisite
    /// nodes. Empty for hub roots / connectors at the centre.
    pub prereq_indices: Vec<u16>,
    /// True iff `TalentTree::can_invest` returns true for this
    /// node (covers max-rank, unspent-points, and prereq
    /// rank≥1). The widget uses this for hit-test enable +
    /// brighten-on-investable styling.
    pub investable: bool,
    /// All prerequisites have rank ≥ 1. Distinct from
    /// `investable` (which also requires unspent points and
    /// not-yet-maxed). Drives the locked / unlocked dim level
    /// independent of the player's wallet.
    pub prereqs_met: bool,
    /// Pre-formatted "Rank X/Y" + per-rank effect summary
    /// lines for the hover tooltip. Owned by the host so the
    /// widget has no `rift_game` formatting code.
    pub tooltip_lines: Vec<String>,
}

/// Full tree snapshot. Built fresh every frame; cheap.
#[derive(Clone, Debug, Default)]
pub struct TalentTreeView<'a> {
    pub nodes: Vec<TalentNodeView<'a>>,
    pub unspent_points: u32,
    pub total_spent: u32,
}

// ─── Panel state ─────────────────────────────────────────────

/// UI-only persistent state for the talent panel. Lives on
/// `rift-client` (PlayerState) so the widget stays pure.
#[derive(Clone, Debug)]
pub struct TalentPanelState {
    pub open: bool,
    /// Canvas pan in panel-local pixels. (0,0) = hub
    /// auto-centred inside the canvas viewport.
    pub pan: (f32, f32),
    /// Zoom multiplier, clamped to `[Self::ZOOM_MIN,
    /// Self::ZOOM_MAX]`. 1.0 = the auto-layout base scale.
    pub zoom: f32,
    /// Current search filter. Lower-case prefix match
    /// against node names; empty = show everything.
    pub search: String,
    /// Persists the last node hovered for at least one frame
    /// — used to keep the tooltip steady when the cursor is
    /// over the tooltip itself.
    pub last_hover_id: Option<u16>,
    /// True while the user is mid-drag-pan (LMB held over an
    /// empty canvas region). Reset on release.
    pub dragging: bool,
    /// Last seen cursor pos (used to compute drag deltas).
    pub last_cursor: (f32, f32),
}

impl Default for TalentPanelState {
    fn default() -> Self {
        Self {
            open: false,
            pan: (0.0, 0.0),
            zoom: 1.0,
            search: String::new(),
            last_hover_id: None,
            dragging: false,
            last_cursor: (0.0, 0.0),
        }
    }
}

impl TalentPanelState {
    pub const ZOOM_MIN: f32 = 0.5;
    pub const ZOOM_MAX: f32 = 2.0;

    pub fn toggle(&mut self) {
        self.open = !self.open;
        if !self.open {
            self.dragging = false;
        }
    }

    pub fn close(&mut self) {
        self.open = false;
        self.dragging = false;
    }
}

// ─── Action ──────────────────────────────────────────────────

/// Emitted by `frame_talent_panel` when the player clicks a
/// node. The host pushes this into `NetState::pending_talent_invests`
/// and the main loop drains it to `request_invest_talent`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TalentTreeAction {
    Invest {
        talent_id: u16,
    },
    /// Lesser-respec: right-click on an invested node. Host
    /// routes this to `request_respec_talent`. The server
    /// rejects refunds that would orphan a downstream node
    /// (`TALENT_TREE.md` §7).
    Respec {
        talent_id: u16,
    },
    /// Greater-respec: footer button. Wipes every invested
    /// point. Host routes this to `request_respec_all_talents`.
    RespecAll,
    Close,
}
