//! Inventory-screen host adapter.
//!
//! This module is a thin bridge between rift-game data and
//! the widget-only `rift_ui::inventory` crate. Each frame it:
//!
//! 1. Flattens `PlayerState` + bag + equipment + stash into
//!    a [`InventoryView`] of borrowed [`ItemView`]s. Strings
//!    backing those views live in per-frame [`String`] arenas
//!    on the stack.
//! 2. Calls [`rift_ui::inventory::frame_inventory`], which
//!    handles all rendering, input, drag/drop, and tooltip
//!    layout.
//! 3. Maps the returned [`InventoryAction`]s onto
//!    [`EquipRequest`] / [`StashRequest`] entries.
//!
//! All persistent UI state lives in
//! [`InventoryUiState`] on `GameState` so the rift-ui crate
//! can be hot-reloaded without losing it.

use std::time::Instant;

use rift_engine::ui::im::Ui;
use rift_game::loot::{
    salvage_yield, EquipSlot, Equipment, Item, Rarity, TooltipKind, TooltipLine,
};
use rift_game::stats::Stat;
use rift_ui::inventory::frame_inventory;
use rift_ui_types::inventory::{
    BulkSalvageView, CompareDeltaRow, InventoryAction, InventoryUiState, InventoryView, ItemView,
    RollBand, StashTabView, StashView, StatRow, StatSection, StatsView, TooltipLineKind,
    TooltipLineView,
};

use crate::game::sub_state::{EquipRequest, StashRequest, StashTabClient};
use crate::game::PlayerState;

/// Bag grid dimensions exposed to the UI. Items are
/// first-fit packed by the widget into a `BAG_COLS × BAG_ROWS`
/// cell grid each frame; the underlying storage stays a flat
/// `Vec<Option<Item>>` so wire / persistence paths are
/// unaffected.
const BAG_COLS: u8 = rift_net::messages::BAG_COLS as u8;
const BAG_ROWS: u8 = rift_net::messages::BAG_ROWS as u8;

/// Monotonic seconds since process start. Used for the
/// salvage 2-stage confirm window AND the inline rename
/// caret blink.
fn ui_now() -> f64 {
    use std::sync::OnceLock;
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = EPOCH.get_or_init(Instant::now);
    Instant::now().duration_since(*epoch).as_secs_f64()
}

/// Run one frame of the inventory UI. Returns `true` while
/// the panel is open (gameplay input should be suppressed
/// underneath).
#[allow(clippy::too_many_arguments)]
pub fn frame(
    ui: &mut Ui<'_>,
    inv_state: &mut InventoryUiState,
    items: &[Option<Item>],
    equipment: &Equipment,
    pending: &mut Vec<EquipRequest>,
    stash_open: bool,
    stash_tabs: &[StashTabClient],
    stash_pending: &mut Vec<StashRequest>,
    player_state: &PlayerState,
    pending_consume_bag_idx: &mut Option<u32>,
    open_talents_panel: &mut bool,
) -> bool {
    // ── Phase 1 ─ pre-allocate every owned String. ────────
    let viewer_level = player_state.experience.level;
    let loadout = Some(&player_state.loadout);

    let bag_tooltip_lines: Vec<Vec<TooltipLine>> = items
        .iter()
        .map(|slot| match slot {
            Some(it) => it.tooltip(loadout),
            None => Vec::new(),
        })
        .collect();
    let equip_tooltip_lines: Vec<Vec<TooltipLine>> = (0..EquipSlot::COUNT)
        .map(|i| match equipment.get(EquipSlot::ALL[i]) {
            Some(it) => it.tooltip(loadout),
            None => Vec::new(),
        })
        .collect();
    let stash_tooltip_lines: Vec<Vec<Vec<TooltipLine>>> = stash_tabs
        .iter()
        .map(|t| {
            t.items
                .iter()
                .map(|slot| match slot {
                    Some(it) => it.tooltip(loadout),
                    None => Vec::new(),
                })
                .collect()
        })
        .collect();

    let bag_compare_slots: Vec<Option<EquipSlot>> = items
        .iter()
        .map(|slot| slot.as_ref().and_then(|it| equipment.default_slot(it)))
        .collect();
    let stash_compare_slots: Vec<Vec<Option<EquipSlot>>> = stash_tabs
        .iter()
        .map(|t| {
            t.items
                .iter()
                .map(|slot| slot.as_ref().and_then(|it| equipment.default_slot(it)))
                .collect()
        })
        .collect();

    // Secondary compare slot — only meaningful for rings, the
    // single slot type with two interchangeable destinations.
    // For a ring item the primary slot is whichever ring
    // `default_slot` picked; the secondary is the OTHER ring
    // (when filled and distinct from primary). For every
    // other item this stays `None`.
    let other_ring = |primary: Option<EquipSlot>| -> Option<EquipSlot> {
        match primary? {
            EquipSlot::Ring1 => Some(EquipSlot::Ring2),
            EquipSlot::Ring2 => Some(EquipSlot::Ring1),
            _ => None,
        }
    };
    let bag_compare_slots_secondary: Vec<Option<EquipSlot>> = items
        .iter()
        .enumerate()
        .map(|(i, slot)| {
            slot.as_ref()?;
            let sec = other_ring(bag_compare_slots[i])?;
            // Only surface secondary when that ring slot is
            // actually filled — comparing against an empty
            // slot has no visual gain.
            equipment.get(sec).map(|_| sec)
        })
        .collect();
    let stash_compare_slots_secondary: Vec<Vec<Option<EquipSlot>>> = stash_tabs
        .iter()
        .enumerate()
        .map(|(ti, t)| {
            t.items
                .iter()
                .enumerate()
                .map(|(si, slot)| {
                    slot.as_ref()?;
                    let sec = other_ring(stash_compare_slots[ti][si])?;
                    equipment.get(sec).map(|_| sec)
                })
                .collect()
        })
        .collect();

    let bag_compare_strs: Vec<Vec<(String, bool)>> = items
        .iter()
        .enumerate()
        .map(|(i, slot)| match (slot, bag_compare_slots[i]) {
            (Some(it), Some(es)) => match equipment.get(es) {
                Some(eq) => compare_delta_rows(it, eq),
                None => Vec::new(),
            },
            _ => Vec::new(),
        })
        .collect();
    let bag_compare_strs_secondary: Vec<Vec<(String, bool)>> = items
        .iter()
        .enumerate()
        .map(|(i, slot)| match (slot, bag_compare_slots_secondary[i]) {
            (Some(it), Some(es)) => match equipment.get(es) {
                Some(eq) => compare_delta_rows(it, eq),
                None => Vec::new(),
            },
            _ => Vec::new(),
        })
        .collect();
    let stash_compare_strs: Vec<Vec<Vec<(String, bool)>>> = stash_tabs
        .iter()
        .enumerate()
        .map(|(ti, t)| {
            t.items
                .iter()
                .enumerate()
                .map(|(si, slot)| match (slot, stash_compare_slots[ti][si]) {
                    (Some(it), Some(es)) => match equipment.get(es) {
                        Some(eq) => compare_delta_rows(it, eq),
                        None => Vec::new(),
                    },
                    _ => Vec::new(),
                })
                .collect()
        })
        .collect();
    let stash_compare_strs_secondary: Vec<Vec<Vec<(String, bool)>>> = stash_tabs
        .iter()
        .enumerate()
        .map(|(ti, t)| {
            t.items
                .iter()
                .enumerate()
                .map(
                    |(si, slot)| match (slot, stash_compare_slots_secondary[ti][si]) {
                        (Some(it), Some(es)) => match equipment.get(es) {
                            Some(eq) => compare_delta_rows(it, eq),
                            None => Vec::new(),
                        },
                        _ => Vec::new(),
                    },
                )
                .collect()
        })
        .collect();

    let (stat_section_names, stat_rows_owned) = build_stat_rows(player_state);

    // Build TooltipLineView slices borrowing from the typed
    // line buffers above.
    let bag_lines: Vec<Vec<TooltipLineView<'_>>> = bag_tooltip_lines
        .iter()
        .map(|lines| view_tooltip(lines, viewer_level))
        .collect();
    let equip_lines: Vec<Vec<TooltipLineView<'_>>> = equip_tooltip_lines
        .iter()
        .map(|lines| view_tooltip(lines, viewer_level))
        .collect();
    let stash_lines: Vec<Vec<Vec<TooltipLineView<'_>>>> = stash_tooltip_lines
        .iter()
        .map(|tab| {
            tab.iter()
                .map(|lines| view_tooltip(lines, viewer_level))
                .collect()
        })
        .collect();
    let bag_compare_rows: Vec<Vec<CompareDeltaRow<'_>>> = bag_compare_strs
        .iter()
        .map(|rows| {
            rows.iter()
                .map(|(text, pos)| CompareDeltaRow {
                    text: text.as_str(),
                    delta_positive: *pos,
                })
                .collect()
        })
        .collect();
    let stash_compare_rows: Vec<Vec<Vec<CompareDeltaRow<'_>>>> = stash_compare_strs
        .iter()
        .map(|tab| {
            tab.iter()
                .map(|rows| {
                    rows.iter()
                        .map(|(text, pos)| CompareDeltaRow {
                            text: text.as_str(),
                            delta_positive: *pos,
                        })
                        .collect()
                })
                .collect()
        })
        .collect();
    let bag_compare_rows_secondary: Vec<Vec<CompareDeltaRow<'_>>> = bag_compare_strs_secondary
        .iter()
        .map(|rows| {
            rows.iter()
                .map(|(text, pos)| CompareDeltaRow {
                    text: text.as_str(),
                    delta_positive: *pos,
                })
                .collect()
        })
        .collect();
    let stash_compare_rows_secondary: Vec<Vec<Vec<CompareDeltaRow<'_>>>> =
        stash_compare_strs_secondary
            .iter()
            .map(|tab| {
                tab.iter()
                    .map(|rows| {
                        rows.iter()
                            .map(|(text, pos)| CompareDeltaRow {
                                text: text.as_str(),
                                delta_positive: *pos,
                            })
                            .collect()
                    })
                    .collect()
            })
            .collect();

    // ── Phase 3 ─ ItemViews. ──────────────────────────────
    // Filter-chip key arenas (per-frame). Owned `Vec<&'static str>`
    // because `Stat::name()` returns `&'static str` and the
    // non-stat affix categories are string literals.
    let equip_stat_keys: Vec<Vec<&'static str>> = (0..EquipSlot::COUNT)
        .map(|i| {
            equipment
                .get(EquipSlot::ALL[i])
                .map(build_item_stat_keys)
                .unwrap_or_default()
        })
        .collect();
    let bag_stat_keys: Vec<Vec<&'static str>> = items
        .iter()
        .map(|slot| slot.as_ref().map(build_item_stat_keys).unwrap_or_default())
        .collect();
    let stash_stat_keys: Vec<Vec<Vec<&'static str>>> = stash_tabs
        .iter()
        .map(|t| {
            t.items
                .iter()
                .map(|slot| slot.as_ref().map(build_item_stat_keys).unwrap_or_default())
                .collect()
        })
        .collect();

    let equip_views: Vec<Option<ItemView<'_>>> = (0..EquipSlot::COUNT)
        .map(|i| {
            let slot = EquipSlot::ALL[i];
            let it = equipment.get(slot)?;
            Some(build_item_view(
                it,
                &equip_lines[i],
                &[],
                None,
                &[],
                None,
                &equip_stat_keys[i],
            ))
        })
        .collect();
    let bag_views: Vec<Option<ItemView<'_>>> = items
        .iter()
        .enumerate()
        .map(|(i, slot)| {
            let it = slot.as_ref()?;
            let compare_with =
                bag_compare_slots[i].and_then(|es| equip_views[es.to_u8() as usize].as_ref());
            let compare_with_secondary = bag_compare_slots_secondary[i]
                .and_then(|es| equip_views[es.to_u8() as usize].as_ref());
            Some(build_item_view(
                it,
                &bag_lines[i],
                &bag_compare_rows[i],
                compare_with,
                &bag_compare_rows_secondary[i],
                compare_with_secondary,
                &bag_stat_keys[i],
            ))
        })
        .collect();
    let stash_views: Vec<Vec<Option<ItemView<'_>>>> = stash_tabs
        .iter()
        .enumerate()
        .map(|(ti, t)| {
            t.items
                .iter()
                .enumerate()
                .map(|(si, slot)| {
                    let it = slot.as_ref()?;
                    let compare_with = stash_compare_slots[ti][si]
                        .and_then(|es| equip_views[es.to_u8() as usize].as_ref());
                    let compare_with_secondary = stash_compare_slots_secondary[ti][si]
                        .and_then(|es| equip_views[es.to_u8() as usize].as_ref());
                    Some(build_item_view(
                        it,
                        &stash_lines[ti][si],
                        &stash_compare_rows[ti][si],
                        compare_with,
                        &stash_compare_rows_secondary[ti][si],
                        compare_with_secondary,
                        &stash_stat_keys[ti][si],
                    ))
                })
                .collect()
        })
        .collect();

    // ── Phase 4 ─ stats view. ──────────────────────────────
    let stat_rows_view: Vec<Vec<StatRow<'_>>> = stat_rows_owned
        .iter()
        .map(|rows| {
            rows.iter()
                .map(|(label, value, color, tooltip)| StatRow {
                    label: label.as_str(),
                    value: value.as_str(),
                    value_color: *color,
                    tooltip: tooltip.as_deref(),
                })
                .collect()
        })
        .collect();
    let stat_sections: Vec<StatSection<'_>> = stat_section_names
        .iter()
        .zip(stat_rows_view.iter())
        .map(|(name, rows)| StatSection {
            header: *name,
            rows: rows.as_slice(),
        })
        .collect();
    let stats_view = StatsView {
        name: player_state.name.as_str(),
        class_name: player_state.config.name,
        level: viewer_level,
        sections: stat_sections.as_slice(),
    };

    // ── Phase 5 ─ stash view. ──────────────────────────────
    let stash_tab_views: Vec<StashTabView<'_>> = stash_tabs
        .iter()
        .enumerate()
        .map(|(i, t)| StashTabView {
            name: t.name.as_str(),
            color: t.color,
            items: stash_views[i].as_slice(),
        })
        .collect();
    let stash_view = if stash_open {
        let owned_tabs = stash_tabs.len() as u32;
        let next_tab_cost: u32 = owned_tabs.saturating_mul(100);
        Some(StashView {
            tabs: stash_tab_views.as_slice(),
            max_tabs: rift_net::messages::MAX_STASH_TABS,
            slots_per_tab: rift_net::messages::STASH_TAB_SLOTS,
            player_shards: player_state.shards,
            next_tab_cost,
        })
    } else {
        None
    };

    // ── Phase 6 ─ assemble + invoke. ──────────────────────
    let view = InventoryView {
        items: bag_views.as_slice(),
        bag_cols: BAG_COLS,
        bag_rows: BAG_ROWS,
        equipment: equip_views.as_slice(),
        stash: stash_view,
        stats: stats_view,
        bulk_salvage: bulk_preview(items),
        currency_shards: player_state.shards,
    };

    let (open, actions) = frame_inventory(ui, &view, inv_state, ui_now());

    for act in actions {
        match act {
            InventoryAction::Close => {
                inv_state.open = false;
            }
            InventoryAction::Equip { inventory_index } => {
                pending.push(EquipRequest::Equip { inventory_index });
            }
            InventoryAction::Unequip { slot } => {
                pending.push(EquipRequest::Unequip { slot });
            }
            InventoryAction::UnequipToSlot {
                slot,
                inventory_index,
            } => {
                pending.push(EquipRequest::UnequipToSlot {
                    slot,
                    inventory_index,
                });
            }
            InventoryAction::SwapEquip { a, b } => {
                pending.push(EquipRequest::SwapEquip { a, b });
            }
            InventoryAction::SwapBag { a, b } => {
                pending.push(EquipRequest::SwapBag { a, b });
            }
            InventoryAction::DropToWorld { inventory_index } => {
                pending.push(EquipRequest::DropToWorld { inventory_index });
            }
            InventoryAction::DropEquipToWorld { slot } => {
                pending.push(EquipRequest::DropEquipToWorld { slot });
            }
            InventoryAction::Salvage { inventory_index } => {
                pending.push(EquipRequest::Salvage { inventory_index });
            }
            InventoryAction::SalvageBulk { rarity_max: _ } => {
                pending.push(EquipRequest::SalvageBulk {
                    rarity_max: Rarity::Magic as u8,
                });
            }
            InventoryAction::UseConsumable { inventory_index } => {
                use rift_game::loot::ConsumableKind;
                // Dispatch by `ConsumableKind` here so the
                // host owns the two-step vs. self-targeted
                // policy in one place. Self-targeted kinds
                // fire `UseItem` immediately with
                // `target_arg = u16::MAX`. Two-step kinds
                // (e.g. `LesserRespecToken`) arm a "pick a
                // node" mode and force-open the talent panel;
                // the panel emits the actual `UseItem` once
                // the player right-clicks an invested talent.
                let kind = items
                    .get(inventory_index as usize)
                    .and_then(|s| s.as_ref())
                    .and_then(|it| it.consumable_kind());
                match kind {
                    Some(ConsumableKind::GreaterRespecToken) => {
                        pending.push(EquipRequest::UseConsumable {
                            inventory_index,
                            target_arg: u16::MAX,
                        });
                    }
                    Some(ConsumableKind::LesserRespecToken) => {
                        *pending_consume_bag_idx = Some(inventory_index);
                        *open_talents_panel = true;
                    }
                    None => {
                        // UI thought it was a consumable but
                        // the host item disagrees \u2014
                        // silently drop. Most likely a stale
                        // `is_consumable` flag from a torn
                        // frame; the next frame will reflect
                        // the truth.
                    }
                }
            }
            InventoryAction::DepositToStash {
                inventory_index,
                tab_index,
            } => {
                stash_pending.push(StashRequest::Deposit {
                    inventory_index,
                    tab_index,
                });
            }
            InventoryAction::DepositToStashSlot {
                inventory_index,
                tab_index,
                stash_index,
            } => {
                stash_pending.push(StashRequest::DepositToSlot {
                    inventory_index,
                    tab_index,
                    stash_index,
                });
            }
            InventoryAction::WithdrawFromStash {
                tab_index,
                stash_index,
            } => {
                stash_pending.push(StashRequest::Withdraw {
                    tab_index,
                    stash_index,
                });
            }
            InventoryAction::WithdrawFromStashSlot {
                tab_index,
                stash_index,
                inventory_index,
            } => {
                stash_pending.push(StashRequest::WithdrawToSlot {
                    tab_index,
                    stash_index,
                    inventory_index,
                });
            }
            InventoryAction::EquipFromStash {
                tab_index,
                stash_index,
            } => {
                stash_pending.push(StashRequest::EquipFromStash {
                    tab_index,
                    stash_index,
                });
            }
            InventoryAction::UnequipToStashSlot {
                slot,
                tab_index,
                stash_index,
            } => {
                stash_pending.push(StashRequest::UnequipToStashSlot {
                    slot,
                    tab_index,
                    stash_index,
                });
            }
            InventoryAction::SwapStash { tab_index, a, b } => {
                stash_pending.push(StashRequest::Swap { tab_index, a, b });
            }
            InventoryAction::SwitchStashTab { .. } => {
                // Already mutated inside frame_inventory.
            }
            InventoryAction::RenameTab { tab_index, name } => {
                stash_pending.push(StashRequest::RenameTab { tab_index, name });
            }
            InventoryAction::RecolorTab { tab_index, color } => {
                stash_pending.push(StashRequest::RecolorTab { tab_index, color });
            }
            InventoryAction::BuyTab => {
                stash_pending.push(StashRequest::BuyTab);
            }
            InventoryAction::SortBag => {
                pending.push(EquipRequest::SortBag);
            }
            InventoryAction::SortStashTab { tab_index } => {
                stash_pending.push(StashRequest::SortTab { tab_index });
            }
        }
    }

    open
}

// ────────────────────────────────────────────────────────────
// Helpers
// ────────────────────────────────────────────────────────────

fn build_item_view<'a>(
    it: &'a Item,
    tooltip_lines: &'a [TooltipLineView<'a>],
    compare_delta: &'a [CompareDeltaRow<'a>],
    compare_with: Option<&'a ItemView<'a>>,
    compare_delta_secondary: &'a [CompareDeltaRow<'a>],
    compare_with_secondary: Option<&'a ItemView<'a>>,
    stat_keys: &'a [&'static str],
) -> ItemView<'a> {
    let c = it.rarity.color();
    let salvageable = !it.anchored && (it.rarity as u8) <= Rarity::Magic as u8;
    let fallback = if it.base.icon.is_empty() {
        it.base.name.chars().next().map(|c| c.to_ascii_uppercase())
    } else {
        None
    };
    // Multi-cell footprint mirrors `EquipSlot::inventory_size`
    // and matches the server's authoritative grid layout.
    // Bag-only items (consumables) fall through to the 1\u00d71
    // default via `Item::footprint`.
    let (cell_w, cell_h) = it.footprint();
    ItemView {
        rarity_color: [c[0], c[1], c[2], 1.0],
        anchored: it.anchored,
        required_level: it.required_level(),
        ilvl: it.ilvl,
        icon_key: it.base.icon,
        fallback_glyph: fallback,
        tooltip_lines,
        salvageable,
        salvage_yield: if it.anchored {
            0
        } else {
            salvage_yield(it.rarity, it.ilvl)
        },
        compare_with,
        compare_delta,
        compare_with_secondary,
        compare_delta_secondary,
        cell_w,
        cell_h,
        rarity_tier: it.rarity as u8,
        stat_keys,
        is_consumable: it.consumable_kind().is_some(),
    }
}

/// Distinct filter chips for one item: each affix contributes
/// either its `Stat::name()` (e.g. `"Crit Chance"`) or a
/// coarse category for non-stat affixes. Order is the affix
/// order on the item with duplicates suppressed; the stash
/// filter row de-duplicates further across all visible items.
fn build_item_stat_keys(it: &Item) -> Vec<&'static str> {
    use rift_game::loot::affixes::AffixEffect;
    let mut keys: Vec<&'static str> = Vec::new();
    let push = |k: &'static str, keys: &mut Vec<&'static str>| {
        if !keys.contains(&k) {
            keys.push(k);
        }
    };
    for a in &it.affixes {
        match &a.def.effect {
            AffixEffect::Stat(s) => push(s.name(), &mut keys),
            AffixEffect::AmplifyAbilityDamage(_) => push("Ability Damage", &mut keys),
            AffixEffect::ReduceAbilityCooldown(_) => push("Cooldown", &mut keys),
            AffixEffect::ExtraProjectiles(_) => push("Projectiles", &mut keys),
            AffixEffect::TransformAbility(_, _) => push("Ability Transform", &mut keys),
            AffixEffect::Proc(_, _) => push("On-Hit", &mut keys),
        }
    }
    keys
}

fn view_tooltip<'a>(lines: &'a [TooltipLine], viewer_level: u32) -> Vec<TooltipLineView<'a>> {
    // Trivial enum-to-enum adapter — the producer in
    // `rift-game/loot/tooltip.rs` stamps the semantic kind /
    // percentile directly, so this is just a remap into the
    // UI-side palette plus newline fragmenting. Continuation
    // fragments share the lead fragment's kind so multi-line
    // text (e.g. flavour, Shardspire's two-row legendary
    // description) renders with a single backdrop instead of
    // dropping back to `Stat` on the second row.
    let mut out: Vec<TooltipLineView<'a>> = Vec::with_capacity(lines.len());
    for ln in lines {
        let kind = match ln.kind {
            TooltipKind::Name => TooltipLineKind::Name,
            TooltipKind::Stat => TooltipLineKind::Stat,
            TooltipKind::Blank => TooltipLineKind::Blank,
            TooltipKind::Divider => TooltipLineKind::Divider,
            TooltipKind::ItemLevel => TooltipLineKind::ItemLevel,
            TooltipKind::RequiresLevel { required } => TooltipLineKind::RequiresLevel {
                ok: viewer_level >= required,
            },
            TooltipKind::Legendary => TooltipLineKind::Legendary,
            TooltipKind::LegendaryBannerEdge => TooltipLineKind::LegendaryBannerEdge,
            TooltipKind::LegendaryFlavor => TooltipLineKind::LegendaryFlavor,
            TooltipKind::Resonance => TooltipLineKind::Resonance,
            TooltipKind::RiftTouched => TooltipLineKind::RiftTouched,
            TooltipKind::Anchored => TooltipLineKind::Anchored,
            TooltipKind::Warning => TooltipLineKind::Warning,
            TooltipKind::Synergy => TooltipLineKind::Synergy,
        };
        let band = ln.percentile.map(RollBand::from_percentile);
        let mut frags = ln.text.split('\n');
        if let Some(first) = frags.next() {
            out.push(TooltipLineView {
                text: first,
                kind,
                band,
            });
        }
        for cont in frags {
            out.push(TooltipLineView {
                text: cont,
                kind,
                band: None,
            });
        }
    }
    out
}

fn bulk_preview(items: &[Option<Item>]) -> BulkSalvageView {
    let mut count: u32 = 0;
    let mut yld: u32 = 0;
    for slot in items {
        if let Some(it) = slot {
            if !it.anchored && (it.rarity as u8) <= Rarity::Magic as u8 {
                count += 1;
                yld = yld.saturating_add(salvage_yield(it.rarity, it.ilvl));
            }
        }
    }
    BulkSalvageView {
        count,
        yield_shards: yld,
    }
}

fn compare_delta_rows(hovered: &Item, equipped: &Item) -> Vec<(String, bool)> {
    const ORDER: &[Stat] = &[
        Stat::CritChance,
        Stat::CritDamage,
        Stat::AttackSpeed,
        Stat::Health,
        Stat::Armor,
        Stat::Evasion,
        Stat::CooldownReduction,
        Stat::ResourceRegen,
        Stat::MoveSpeed,
        Stat::PhysicalDamage,
        Stat::FireDamage,
        Stat::IceDamage,
        Stat::LightningDamage,
    ];
    let h_stats = hovered.stats();
    let e_stats = equipped.stats();
    let mut rows = Vec::new();
    for &stat in ORDER {
        let h = h_stats.get(stat);
        let e = e_stats.get(stat);
        let delta = h - e;
        if delta.abs() < 1e-4 {
            continue;
        }
        let text = if stat.is_percent() {
            format!("{:+.1}% {}", delta * 100.0, stat.name())
        } else {
            format!("{:+.0} {}", delta, stat.name())
        };
        rows.push((text, delta > 0.0));
    }
    rows
}

fn build_stat_rows(
    ps: &PlayerState,
) -> (
    Vec<&'static str>,
    Vec<Vec<(String, String, Option<[f32; 4]>, Option<String>)>>,
) {
    let s = ps.stats();
    let a = &ps.attributes;
    let pct = |v: f32| format!("{:.1}%", v * 100.0);
    let int = |v: f32| format!("{:.0}", v);
    let f1 = |v: f32| format!("{:.1}", v);
    let f2 = |v: f32| format!("{:.2}", v);
    let per_sec = |v: f32| format!("{:.1}/s", v);
    let tip = |t: &str| Some(t.to_string());

    let mut names: Vec<&'static str> = Vec::new();
    let mut rows: Vec<Vec<(String, String, Option<[f32; 4]>, Option<String>)>> = Vec::new();

    // ── ATTRIBUTES ───────────────────────────────────────────
    // Core stat investment. Item-rolled `+Strength` / `+Agility`
    // / `+Intellect` lines fold into these values 1:1 with
    // the manual point-spend screen, so the displayed numbers
    // are gear + manual combined.
    let primary_atype = ps.config.primary_attribute;
    let star = |a_type| {
        if a_type == primary_atype {
            " ★"
        } else {
            ""
        }
    };
    names.push("ATTRIBUTES");
    rows.push(vec![
        (
            format!(
                "Strength{}",
                star(rift_game::attributes::AttributeType::Strength)
            ),
            int(a.strength as f32),
            None,
            tip("Increases armor by 0.8% per point. \
                 If this is your primary attribute, also boosts damage by 1% per point."),
        ),
        (
            format!(
                "Agility{}",
                star(rift_game::attributes::AttributeType::Agility)
            ),
            int(a.agility as f32),
            None,
            tip(
                "Increases crit chance by 0.1% and attack speed by 0.5% per point. \
                 If this is your primary attribute, also boosts damage by 1% per point.",
            ),
        ),
        (
            format!(
                "Intellect{}",
                star(rift_game::attributes::AttributeType::Intellect)
            ),
            int(a.intellect as f32),
            None,
            tip("Increases maximum essence by 2 per point. \
                 If this is your primary attribute, also boosts damage by 1% per point."),
        ),
        (
            format!(
                "Vitality{}",
                star(rift_game::attributes::AttributeType::Vitality)
            ),
            int(a.vitality as f32),
            None,
            tip("Increases maximum health by 3 per point."),
        ),
    ]);

    names.push("OFFENSE");
    rows.push(vec![
        (
            "Damage".into(),
            int(s.damage),
            None,
            tip(
                "Base outgoing damage before element / archetype / ability multipliers. \
                 Each point in your primary attribute adds 1% to base damage.",
            ),
        ),
        (
            "Crit Chance".into(),
            pct(s.crit_chance),
            None,
            tip("Chance for any hit to deal extra damage."),
        ),
        (
            "Crit Damage".into(),
            pct(s.crit_damage),
            None,
            tip("Bonus damage dealt by a critical hit, on top of the base."),
        ),
        (
            "Attack Speed".into(),
            f2(s.attack_speed),
            None,
            tip("Attacks per second. Higher = faster basic attacks."),
        ),
    ]);

    names.push("DEFENSE");
    let mut def = vec![
        (
            "Health".into(),
            int(s.max_hp),
            None,
            tip("Your maximum life total. Reach 0 and you die."),
        ),
        (
            "Health Regen".into(),
            per_sec(s.health_regen),
            None,
            tip("Health restored per second out of combat."),
        ),
        (
            "Armor".into(),
            int(s.armor),
            None,
            tip(
                "Reduces incoming physical damage. Effectiveness scales with attacker level \
                 — stack it for melee fights.",
            ),
        ),
        (
            "Evasion".into(),
            pct(s.evasion),
            None,
            tip("Chance to completely dodge an incoming attack, taking no damage."),
        ),
    ];
    if s.elemental_resist > 0.0 {
        def.push((
            "Elemental Resist".into(),
            pct(s.elemental_resist),
            None,
            tip("Flat percent reduction to fire / ice / lightning damage taken (caps at 75%)."),
        ));
    }
    if s.healing_received > 0.0 {
        def.push((
            "Healing Received".into(),
            pct(s.healing_received),
            None,
            tip("Multiplier applied to all healing you receive from any source."),
        ));
    }
    rows.push(def);

    names.push("UTILITY");
    rows.push(vec![
        (
            "Move Speed".into(),
            f1(s.move_speed),
            None,
            tip("World units traveled per second while moving."),
        ),
        (
            "Cooldown Reduction".into(),
            pct(s.cooldown_reduction),
            None,
            tip("Reduces the cooldown of every ability by this percent (caps at 75%)."),
        ),
        (
            "Max Essence".into(),
            int(s.max_resource),
            None,
            tip("Essence is the resource pool spent on abilities. Grows with Intellect."),
        ),
        (
            "Essence Regen".into(),
            per_sec(s.resource_regen),
            None,
            tip("Essence restored per second while not casting."),
        ),
    ]);

    if s.fire_damage > 0.0 || s.ice_damage > 0.0 || s.lightning_damage > 0.0 {
        names.push("ELEMENTAL");
        let mut el = Vec::new();
        if s.fire_damage > 0.0 {
            el.push((
                "Fire".into(),
                pct(s.fire_damage),
                Some([0.96, 0.55, 0.30, 1.0]),
                tip("Bonus multiplier applied to fire damage you deal."),
            ));
        }
        if s.ice_damage > 0.0 {
            el.push((
                "Ice".into(),
                pct(s.ice_damage),
                Some([0.55, 0.85, 0.96, 1.0]),
                tip("Bonus multiplier applied to ice damage you deal."),
            ));
        }
        if s.lightning_damage > 0.0 {
            el.push((
                "Lightning".into(),
                pct(s.lightning_damage),
                Some([0.95, 0.85, 0.45, 1.0]),
                tip("Bonus multiplier applied to lightning damage you deal."),
            ));
        }
        rows.push(el);
    }

    (names, rows)
}
