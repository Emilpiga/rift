//! Inventory screen view models + action enums.
//!
//! Read by the widget functions in `rift_ui::inventory`,
//! written by the host (`rift_client::game::inventory`) every
//! frame. The host pulls live data out of `rift_game::loot`
//! and friends and flattens it into the borrowed views below
//! so the UI crate doesn't need to depend on `rift_game`.
//!
//! See `crates/rift-ui/src/lib.rs` for the broader hot-reload
//! contract — short version: only plain data crosses the
//! boundary, no game types ever appear in this module.

// ─── Slot enum (mirror of game's EquipSlot) ──────────────────

/// Wire / persistence index of an equipment slot. Mirrors the
/// `u8` discriminants of `rift_game::loot::EquipSlot` in
/// declaration order; the adapter converts between the two
/// with `EquipSlotIdx::from_u8(slot.to_u8())`.
///
/// The widget crate uses these indices to identify drag
/// sources / drop targets without taking a `rift_game`
/// dependency.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct EquipSlotIdx(pub u8);

impl EquipSlotIdx {
    pub const COUNT: usize = 10;

    /// Human-friendly label drawn inside empty equipment slots.
    pub fn label(self) -> &'static str {
        match self.0 {
            0 => "Weapon",
            1 => "Helm",
            2 => "Chest",
            3 => "Legs",
            4 => "Hands",
            5 => "Boots",
            6 => "Ring 1",
            7 => "Ring 2",
            8 => "Amulet",
            9 => "Shoulders",
            _ => "?",
        }
    }
}

// ─── Tooltip line classification ─────────────────────────────

/// Pre-classified semantic role for a tooltip line. Keeps the
/// widget free of string-prefix sniffing — the host classifies
/// once at view build time and the UI just paints the right
/// colour.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TooltipLineKind {
    /// Item name (first line). Coloured by `rarity_color`.
    Name,
    /// Plain stat / affix / implicit line.
    Stat,
    /// Blank separator line.
    Blank,
    /// Thin divider between signature and bonus blocks.
    Divider,
    /// `Item Level …` line.
    ItemLevel,
    /// `Requires Level …` line. `ok` is false when the viewer
    /// can't meet the requirement — drawn red.
    RequiresLevel { ok: bool },
    /// `★ …` legendary effect line. Gold.
    Legendary,
    /// `╔` / `╚` sentinel line wrapping the legendary banner.
    /// Carries no text — the renderer paints a horizontal
    /// gold gradient rule and uses these as the top/bottom
    /// edges of a dark inset backdrop framing the legendary
    /// effect + flavour.
    LegendaryBannerEdge,
    /// Flavour string under the legendary effect (e.g.
    /// `"forged in the rift"`). Italic-leaning dim gold,
    /// rendered atop the same banner backdrop.
    LegendaryFlavor,
    /// `◆ …` resonance affix line — cross-family damage axis
    /// that intentionally breaks the trio's family lock.
    /// Distinct violet colour per ITEMS.md §2.5.
    Resonance,
    /// `✦ …` rift-touched memento line — the extra slot awarded
    /// to drops earned past `RIFT_TOUCHED_MIN_FLOOR`. Carries
    /// the floor depth suffix `(Floor N)`. Magenta tint so it
    /// reads as "this came from deep in the rift" without
    /// fighting resonance violet or anchored gold.
    RiftTouched,
    /// `⚓ …` anchored trait line. Saturated gold.
    Anchored,
    /// `⚠ …` warning line (e.g. unstable rift loot). Red.
    Warning,
    /// `→ Boosts …` synergy footer. Accent tint.
    Synergy,
}

/// Named tier for a single affix roll quality. Constructed from
/// the 0..1 percentile returned by
/// `rift_game::loot::roll_percentile` and used by tooltips to
/// replace the raw `[NN%]` suffix with a glyph + name pair the
/// player can scan at a glance. See `ITEMS.md` §Phase 6.
///
/// Thresholds (chosen so `Perfect` is rare and `Crude` is common
/// enough to feel meaningful as the "low end" tag):
///
/// | percentile p | band |
/// |--------------|------|
/// | `p < 0.20`   | `Crude` |
/// | `p < 0.50`   | `Fair` |
/// | `p < 0.80`   | `Fine` |
/// | `p < 0.95`   | `Pristine` |
/// | `p >= 0.95`  | `Perfect` |
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RollBand {
    Crude,
    Fair,
    Fine,
    Pristine,
    Perfect,
}

impl RollBand {
    /// Map a 0..1 roll percentile to a named tier. Values outside
    /// `[0, 1]` are clamped by saturating into the nearest tier.
    pub fn from_percentile(p: f32) -> Self {
        if p < 0.20 {
            Self::Crude
        } else if p < 0.50 {
            Self::Fair
        } else if p < 0.80 {
            Self::Fine
        } else if p < 0.95 {
            Self::Pristine
        } else {
            Self::Perfect
        }
    }

    /// Stable display name. Also the source of truth the host
    /// classifier matches against when parsing a tooltip line's
    /// trailing band suffix — keep these literals stable.
    pub fn name(self) -> &'static str {
        match self {
            Self::Crude => "Crude",
            Self::Fair => "Fair",
            Self::Fine => "Fine",
            Self::Pristine => "Pristine",
            Self::Perfect => "Perfect",
        }
    }

    /// Compact glyph rendered immediately before the name in the
    /// tooltip suffix. The chevron progression (down → right →
    /// up → double-up → triple-up) reads as quality climbing
    /// without needing colour.
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Crude => "▾",
            Self::Fair => "▸",
            Self::Fine => "▴",
            Self::Pristine => "▴▴",
            Self::Perfect => "▴▴▴",
        }
    }

    /// Straight-alpha RGB tint used to colour stat lines that
    /// carry this band. The tooltip renderer blends this with
    /// the theme `text` colour for `Stat` lines.
    pub fn color_rgb(self) -> [f32; 3] {
        match self {
            Self::Crude => [0.60, 0.60, 0.62],
            Self::Fair => [0.82, 0.82, 0.84],
            Self::Fine => [0.65, 0.92, 1.00],
            Self::Pristine => [1.00, 0.85, 0.45],
            Self::Perfect => [1.00, 0.72, 0.22],
        }
    }

    /// Inverse of [`Self::name`]: parse a band literal back into
    /// the enum. Used by the host tooltip classifier to recover
    /// the band from the trailing word of a rendered stat line.
    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "Crude" => Some(Self::Crude),
            "Fair" => Some(Self::Fair),
            "Fine" => Some(Self::Fine),
            "Pristine" => Some(Self::Pristine),
            "Perfect" => Some(Self::Perfect),
            _ => None,
        }
    }
}

/// One pre-classified tooltip line.
#[derive(Clone, Debug)]
pub struct TooltipLineView<'a> {
    pub text: &'a str,
    pub kind: TooltipLineKind,
    /// `Some` for stat / affix lines that carry a named roll
    /// band suffix; the renderer uses
    /// [`RollBand::color_rgb`] to tint the line. `None` for
    /// every non-affix line (Name, Divider, Anchored, …) and
    /// for affix effects with degenerate ranges (Transform)
    /// where percentile is meaningless.
    pub band: Option<RollBand>,
}

// ─── Compare delta (per-stat) ────────────────────────────────

/// One per-stat delta row inside the side-by-side compare
/// panel. `delta_positive` is just a sign flag the UI uses to
/// pick green vs red — the host pre-formats the numeric text.
#[derive(Clone, Debug)]
pub struct CompareDeltaRow<'a> {
    pub text: &'a str,
    pub delta_positive: bool,
}

// ─── Item view ───────────────────────────────────────────────

/// Per-frame snapshot of one bag / equipment / stash item.
///
/// Built by the host every frame from the live
/// `rift_game::loot::Item`. Borrowed strings reference data
/// owned by either the live `Item` (e.g. `base.icon`) or a
/// short-lived per-frame string arena the host allocates and
/// drops at frame end. The UI never mutates or stores these
/// past the frame.
#[derive(Clone, Debug)]
pub struct ItemView<'a> {
    /// Rarity tint applied to the slot border and the name
    /// line of the tooltip. Premultiplied alpha not required —
    /// the renderer expects straight-alpha RGBA.
    pub rarity_color: [f32; 4],
    /// `true` for items with the rare "Anchored" trait.
    pub anchored: bool,
    /// Required character level. Compared against the viewer's
    /// level when classifying `RequiresLevel` tooltip lines.
    pub required_level: u32,
    /// Item level (drops the icon overlay's rarity gem etc).
    pub ilvl: u32,
    /// Slot-icon registry key (e.g. `"loot/Boots/Boots_1"`).
    /// Empty string means the icon registry has nothing for
    /// this base — the UI falls back to `fallback_glyph`.
    pub icon_key: &'a str,
    /// Single-character glyph drawn inside the slot when the
    /// icon registry has no entry for `icon_key`. Empty when
    /// the host has no fallback to offer.
    pub fallback_glyph: Option<char>,
    /// Pre-classified tooltip lines. The first entry should
    /// always be the item name; subsequent lines are stats,
    /// implicits, affixes, legendary effects, etc.
    pub tooltip_lines: &'a [TooltipLineView<'a>],
    /// `true` for bag items whose rarity ≤ Magic and that
    /// aren't anchored — i.e. items the bulk-salvage path
    /// would consume. Drives the Ctrl-hover salvage hint.
    pub salvageable: bool,
    /// Salvage yield in shards, used only for the Ctrl-hover
    /// hint banner. `0` when the item is anchored / unrolled.
    pub salvage_yield: u32,
    /// Equipped item to compare this one against, when the
    /// player hovers it. `None` for stash/equip slot hovers
    /// where compare is suppressed.
    pub compare_with: Option<&'a ItemView<'a>>,
    /// Pre-computed per-stat compare rows shown when the
    /// player holds Shift while hovering. Empty when there's
    /// nothing to compare against or no stat differences.
    pub compare_delta: &'a [CompareDeltaRow<'a>],
    /// Second equipped comparison target — used only for
    /// rings, where both ring slots are equally valid
    /// destinations. `None` when the item isn't a ring, when
    /// the second ring slot is empty, or when the secondary
    /// compare is the same item as `compare_with`.
    pub compare_with_secondary: Option<&'a ItemView<'a>>,
    /// Delta rows for [`Self::compare_with_secondary`]. Same
    /// shape as [`Self::compare_delta`].
    pub compare_delta_secondary: &'a [CompareDeltaRow<'a>],
    /// Width in bag cells. `1` for ring/amulet, `2` for most
    /// armor, `2..=4` for weapons / chest. Determines the
    /// rectangle the item occupies in the bag grid AND on the
    /// paperdoll.
    pub cell_w: u8,
    /// Height in bag cells. See [`Self::cell_w`].
    pub cell_h: u8,
    /// Rarity tier as a 0..=3 ordinal: 0 = Common, 1 = Magic,
    /// 2 = Rare, 3 = Legendary. Used by the stash filter chips
    /// without forcing the UI crate to depend on `rift-game`.
    pub rarity_tier: u8,
    /// Filter keys for the stash filter row: each affix on the
    /// item contributes its `Stat::name()` (e.g. `"Crit Chance"`,
    /// `"Fire Damage"`) or a coarse category for non-stat
    /// affixes (`"Cooldown"`, `"Projectiles"`, `"Proc"`,
    /// `"Transform"`). The UI builds the filter chip set
    /// dynamically from the union of these keys, so adding a
    /// new `Stat` variant in `rift-game` shows up automatically.
    pub stat_keys: &'a [&'static str],
    /// `true` when the item is a `ItemSlot::Consumable(_)` \u2014
    /// drives bag right-click routing (`UseConsumable` instead
    /// of `Equip` / `DepositToStash`) and disables salvage /
    /// equip affordances.
    pub is_consumable: bool,
}

// ─── Stash views ─────────────────────────────────────────────

/// Per-frame view of a single stash tab.
#[derive(Clone, Debug)]
pub struct StashTabView<'a> {
    pub name: &'a str,
    /// Packed `0xRRGGBB`. Alpha implicit; the UI splits and
    /// premultiplies for the pill background.
    pub color: u32,
    /// Tab items in stash-index order. `None` entries are
    /// empty slots.
    pub items: &'a [Option<ItemView<'a>>],
}

/// Per-frame view of the stash side panel. Only present when
/// a stash session is active.
#[derive(Clone, Debug)]
pub struct StashView<'a> {
    pub tabs: &'a [StashTabView<'a>],
    /// Tabs slot capacity (mirrors `rift_net::MAX_STASH_TABS`).
    pub max_tabs: usize,
    /// Slot capacity per tab (mirrors `STASH_TAB_SLOTS`).
    pub slots_per_tab: usize,
    /// Viewer's shard balance — feeds the "+ Buy" tooltip.
    pub player_shards: u32,
    /// Pre-computed cost of the *next* tab the viewer can
    /// buy. The host owns the pricing formula.
    pub next_tab_cost: u32,
}

// ─── Stats view ──────────────────────────────────────────────

/// One row in the character-sheet panel. `value_color` is
/// `None` for the default body text colour.
#[derive(Clone, Debug)]
pub struct StatRow<'a> {
    pub label: &'a str,
    pub value: &'a str,
    /// `None` = use default text colour.
    pub value_color: Option<[f32; 4]>,
    /// Optional explanatory tooltip rendered when the player
    /// hovers the row. `None` = no tooltip.
    pub tooltip: Option<&'a str>,
}

/// One section of the character sheet. Section header drawn
/// in dim gold; rows follow underneath.
#[derive(Clone, Debug)]
pub struct StatSection<'a> {
    pub header: &'a str,
    pub rows: &'a [StatRow<'a>],
}

#[derive(Clone, Debug)]
pub struct StatsView<'a> {
    /// Player-chosen name; falls back to `class_name` when
    /// blank.
    pub name: &'a str,
    pub class_name: &'a str,
    pub level: u32,
    pub sections: &'a [StatSection<'a>],
}

// ─── Bulk-salvage preview ────────────────────────────────────

/// Pre-computed summary of every bag item the "Salvage Trash"
/// bulk path would consume.
#[derive(Copy, Clone, Debug, Default)]
pub struct BulkSalvageView {
    pub count: u32,
    pub yield_shards: u32,
}

// ─── Top-level inventory view ────────────────────────────────

/// Per-frame view of the full inventory screen.
#[derive(Clone, Debug)]
pub struct InventoryView<'a> {
    /// Bag items in inventory-index order. The widget runs a
    /// stable first-fit packing pass each frame to assign each
    /// non-`None` entry an `(x, y, w, h)` rectangle inside the
    /// `bag_cols × bag_rows` cell grid; `None` entries are
    /// skipped.
    pub items: &'a [Option<ItemView<'a>>],
    /// Bag grid width in cells (e.g. `10`).
    pub bag_cols: u8,
    /// Bag grid height in cells (e.g. `8`).
    pub bag_rows: u8,
    /// Equipped items indexed by `EquipSlotIdx.0`. Length is
    /// always `EquipSlotIdx::COUNT`.
    pub equipment: &'a [Option<ItemView<'a>>],
    /// `Some` while a stash session is active (player stood
    /// next to a stash chest).
    pub stash: Option<StashView<'a>>,
    /// Right-hand character sheet — only rendered when the
    /// player toggles it on via the drawer's Stats button.
    pub stats: StatsView<'a>,
    /// Bulk-salvage preview for the footer button.
    pub bulk_salvage: BulkSalvageView,
    /// Player shard balance, drawn in the currency bar at the
    /// bottom of the drawer. The bar is laid out wide enough
    /// to hold additional currencies as they're added.
    pub currency_shards: u32,
}

// ─── Persistent UI state (host-owned) ────────────────────────

/// Inventory-UI state the host owns and threads into
/// [`frame_inventory`] every frame. Everything here is plain
/// data so it can cross the hot-reload boundary safely.
///
/// [`frame_inventory`]: ../../rift_ui/inventory/fn.frame_inventory.html
#[derive(Clone, Debug, Default)]
pub struct InventoryUiState {
    /// Is the panel currently visible? Tab toggles it; the
    /// stash session forces it on.
    pub open: bool,
    /// Currently-selected stash tab. Clamped against the
    /// authoritative tab list every frame.
    pub active_stash_tab: u8,
    /// Monotonic seconds at which the 2-stage "Salvage Trash"
    /// button was armed; `None` when idle. The host advances
    /// time and the UI compares against the configurable
    /// `salvage_confirm_window_s` below.
    pub salvage_armed_at: Option<f64>,
    /// 2-stage confirm window in seconds. Defaults to 3.0 if
    /// left at zero on the first frame.
    pub salvage_confirm_window_s: f64,
    /// Bag slot the player Ctrl-pressed but hasn't released
    /// yet. Carried through a possible drag jiggle so the
    /// salvage fires reliably on release.
    pub salvage_armed_bag_idx: Option<u32>,
    /// Stash tab whose inline rename field is currently
    /// active. `None` when no rename is in progress.
    pub rename_target_tab: Option<u8>,
    /// Editable buffer behind the inline rename field.
    pub rename_buffer: String,
    /// `true` once the rename field has received focus at
    /// least one frame. Drives the click-away commit so a
    /// rename can't commit before the user has had a chance
    /// to focus it.
    pub rename_has_focused: bool,
    /// Stash tab whose color picker is currently open.
    /// `None` when no tab color palette is visible.
    pub color_picker_tab: Option<u8>,
    /// Bag-panel rect from the last rendered frame, used by
    /// the host's `consumes_mouse` check. `[x, y, w, h]` in
    /// screen pixels.
    pub cached_bag_rect: [f32; 4],
    /// Stats-panel rect from the last rendered frame.
    pub cached_stats_rect: [f32; 4],
    /// Stash-panel rect from the last rendered frame. `[0; 4]`
    /// when the stash is closed.
    pub cached_stash_rect: [f32; 4],
    /// Whether the right drawer's Stats subsection is
    /// expanded. Toggled by the `Stats` chip; persists across
    /// open/close.
    pub show_stats: bool,
    /// Whether the right drawer's Stash subsection is
    /// expanded. Forced `true` while a stash session is
    /// active.
    pub show_stash: bool,
    /// Stash filter: bitmask over rarity tiers (bit `n` =
    /// tier `n` is allowed). `0` means "no rarity filter
    /// active" (all tiers shown). Items whose tier bit is
    /// clear are rendered dimmed.
    pub stash_filter_rarity_mask: u8,
    /// Stash filter: active stat-key chips. An item passes
    /// when its `stat_keys` intersects this set, OR when this
    /// list is empty (no stat filter active). Strings own
    /// their data because the chip set persists across frames
    /// while the per-frame `stat_keys` slices do not.
    pub stash_filter_stats: Vec<String>,
    /// Slot the player just dropped this drag on. Set the
    /// frame the drop fires; cleared when the authoritative
    /// state changes or after a short timeout. The renderer
    /// hides the source slot's item while this is set so the
    /// item doesn't briefly "pop back" to the source between
    /// drop and the server's mutation reply.
    pub in_transit_source: Option<InTransitSource>,
    /// Monotonic time (seconds) at which `in_transit_source`
    /// was last armed. Compared against the frame's `time`
    /// to expire stale hides if the server reply gets lost.
    pub in_transit_set_at: f64,
    /// Screen rect (`[x, y, w, h]`) of the slot the in-flight
    /// drop is destined for. Set the same frame the drop
    /// fires; cleared together with `in_transit_source`.
    /// The renderer paints a translucent ghost of the source
    /// item here while the round-trip is pending so the
    /// destination doesn't read as "empty" between drop and
    /// the server's authoritative state.
    pub in_transit_dest_rect: Option<[f32; 4]>,
}

/// Compact mirror of the `DragSource` enum in `rift-ui` (kept
/// here so `InventoryUiState` doesn't have to depend on the
/// renderer crate). Used purely as a transient hint that the
/// renderer should hide a specific slot for one round-trip.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum InTransitSource {
    Bag(u32),
    Equip(u8),
    Stash { tab: u8, idx: u32 },
}

impl InTransitSource {
    /// Build from a `DragSource` plus the active stash tab
    /// (only used for the `Stash` variant).
    pub fn from_drag(src: DragSource, active_tab: u8) -> Self {
        match src {
            DragSource::Bag(i) => InTransitSource::Bag(i),
            DragSource::Equip(s) => InTransitSource::Equip(s.0),
            DragSource::Stash(i) => InTransitSource::Stash {
                tab: active_tab,
                idx: i,
            },
        }
    }
}

impl InventoryUiState {
    pub fn new() -> Self {
        Self {
            salvage_confirm_window_s: 3.0,
            ..Self::default()
        }
    }

    /// `true` while the inline rename field is active —
    /// drives the host's `set_text_capture` flag so typed
    /// letters don't leak into world bindings.
    pub fn wants_text_input(&self) -> bool {
        self.rename_target_tab.is_some()
    }

    /// `true` when `(mx, my)` falls inside any cached panel
    /// rect from the last frame. Lets the host suppress
    /// gameplay click handling without rerunning the layout.
    pub fn consumes_mouse(&self, mx: f32, my: f32, stash_visible: bool) -> bool {
        if !self.open {
            return false;
        }
        let hit = |r: [f32; 4]| {
            r[2] > 0.0
                && r[3] > 0.0
                && mx >= r[0]
                && mx < r[0] + r[2]
                && my >= r[1]
                && my < r[1] + r[3]
        };
        if hit(self.cached_bag_rect) || hit(self.cached_stats_rect) {
            return true;
        }
        if stash_visible && hit(self.cached_stash_rect) {
            return true;
        }
        false
    }
}

// ─── Drag payload ────────────────────────────────────────────

/// Where a drag started. The widget threads this through the
/// IM stack's typed drag payload so drop targets can branch
/// on the source.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum DragSource {
    Bag(u32),
    Equip(EquipSlotIdx),
    Stash(u32),
}

// ─── Actions ─────────────────────────────────────────────────

/// What the user did this frame. The host drains the returned
/// `Vec<InventoryAction>` and maps each entry onto a network
/// request or local state change. Names mirror the existing
/// `EquipRequest` / `StashRequest` shape so the adapter stays
/// a straight match-and-forward.
#[derive(Clone, Debug)]
pub enum InventoryAction {
    /// User pressed Tab (or the panel decided to close itself).
    Close,
    /// Equip a bag item into its canonical slot.
    Equip { inventory_index: u32 },
    /// Unequip into the next free bag slot.
    Unequip { slot: u8 },
    /// Unequip into a specific bag slot. The previous occupant,
    /// if any, swaps into the original equip slot.
    UnequipToSlot { slot: u8, inventory_index: u32 },
    /// Swap two equipment slots in place. Currently only used
    /// for ring1 ↔ ring2 (the only pair where both items
    /// remain legal after the swap); the server still
    /// validates both `Equipment::accepts` directions and
    /// rejects illegal combinations.
    SwapEquip { a: u8, b: u8 },
    /// Swap two bag slots in place (reorder).
    SwapBag { a: u32, b: u32 },
    /// Drop a bag item onto the ground at the player's pos.
    DropToWorld { inventory_index: u32 },
    /// Drop an equipped item directly onto the ground (skips
    /// the bag entirely). Emitted when the player drags an
    /// equipped slot outside the inventory drawer.
    DropEquipToWorld { slot: u8 },
    /// Salvage a single bag item for shards.
    Salvage { inventory_index: u32 },
    /// Bulk-salvage every non-anchored bag item ≤ `rarity_max`.
    SalvageBulk { rarity_max: u8 },
    /// Begin consuming the bag item at `inventory_index`. The
    /// host inspects the item's `ConsumableKind`: self-targeted
    /// kinds (e.g. `GreaterRespecToken`) fire the `UseItem`
    /// wire request immediately; two-step kinds (e.g.
    /// `LesserRespecToken`) enter a "pick a target" UI mode
    /// before pushing the request.
    UseConsumable { inventory_index: u32 },
    /// Deposit a bag item into the active stash tab.
    DepositToStash { inventory_index: u32, tab_index: u8 },
    /// Deposit a bag item into a specific stash slot.
    DepositToStashSlot {
        inventory_index: u32,
        tab_index: u8,
        stash_index: u32,
    },
    /// Withdraw a stash item back into the bag.
    WithdrawFromStash { tab_index: u8, stash_index: u32 },
    /// Withdraw a stash item into a specific bag slot.
    WithdrawFromStashSlot {
        tab_index: u8,
        stash_index: u32,
        inventory_index: u32,
    },
    /// Equip a stash item directly. Server swaps the
    /// previously-equipped item back into the freed stash cell.
    EquipFromStash { tab_index: u8, stash_index: u32 },
    /// Unequip an equipped item directly into a specific
    /// stash cell.
    UnequipToStashSlot {
        slot: u8,
        tab_index: u8,
        stash_index: u32,
    },
    /// Swap two slots inside one stash tab.
    SwapStash { tab_index: u8, a: u32, b: u32 },
    /// Switch the active stash tab. The widget also updates
    /// `state.active_stash_tab` so emission is purely
    /// informational; some hosts may want to telemetry it.
    SwitchStashTab { tab_index: u8 },
    /// Rename the given stash tab.
    RenameTab { tab_index: u8, name: String },
    /// Set the given stash tab's color. `color` is packed
    /// `0xRRGGBB`.
    RecolorTab { tab_index: u8, color: u32 },
    /// Purchase a new stash tab.
    BuyTab,
    /// Auto-sort the bag: server compacts by rarity desc,
    /// then ilvl desc, then footprint area desc.
    SortBag,
    /// Auto-sort one stash tab. Same ordering as `SortBag`.
    SortStashTab { tab_index: u8 },
}

/// `0xRRGGBB` palette offered by the stash tab color picker.
pub const STASH_TAB_PALETTE: &[u32] = &[
    0x6E6E78, // neutral grey (default)
    0xB95151, // muted red
    0xC68A3F, // amber
    0xC8B548, // yellow-gold
    0x6FAE5C, // green
    0x4DA0A8, // teal
    0x4E78C8, // blue
    0x9165B2, // violet
];
