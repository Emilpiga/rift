//! Host-side adapter for the talent panel.
//!
//! Builds a [`TalentTreeView`] every frame from the live
//! `rift_game::talents::TalentTree` so the widget in `rift-ui`
//! never sees `rift_game` types directly. Tooltip strings are
//! formatted here so the widget has no `rift_game` formatting
//! code either.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::SystemTime,
};

use rift_engine::renderer::asset_decode::resolve_asset_path;
use rift_game::talents::{
    AbilityModifier, KeystoneId, Route, TalentEffect, TalentId, TalentStat, TalentTree,
};
use rift_ui_types::talents::{TalentNodeKind, TalentNodeView, TalentRouteView, TalentTreeView};

const TALENT_LAYOUT_CSV: &str = "assets/talents/talent_tree_layout.csv";

/// Build a flat view of `tree` ready to hand to
/// `rift_ui::talents::frame_talent_panel`. Cheap to do every
/// frame; the heaviest cost is the per-node tooltip string
/// formatting.
pub fn build_talent_view(tree: &TalentTree) -> TalentTreeView<'_> {
    let layout_positions = hot_layout_positions();

    // Pre-compute the `TalentId → index` map once so the
    // prereq lookup below is O(N) instead of O(N²).
    let n = tree.nodes.len();
    let mut id_to_idx: HashMap<TalentId, u16> = HashMap::with_capacity(n);
    for (i, node) in tree.nodes.iter().enumerate() {
        id_to_idx.insert(node.id, i as u16);
    }

    let mut nodes: Vec<TalentNodeView<'_>> = Vec::with_capacity(n);
    for node in &tree.nodes {
        let prereq_indices: Vec<u16> = node
            .prerequisites
            .iter()
            .filter_map(|id| id_to_idx.get(id).copied())
            .collect();

        let prereqs_met = tree.prerequisites_met(node);
        let investable = tree.can_invest(node.id);

        nodes.push(TalentNodeView {
            id: node.id.0,
            name: node.name,
            description: node.description,
            route: route_view(node.route),
            kind: kind_view(&node.effect),
            current_rank: node.current_rank,
            max_rank: node.max_rank,
            position: layout_positions
                .as_ref()
                .and_then(|positions| positions.get(&node.id.0).copied())
                .or_else(|| node.position.map(|p| (p.x, p.y))),
            status: node.status.label(),
            prereq_indices,
            investable,
            prereqs_met,
            tooltip_lines: tooltip_lines(node),
        });
    }

    TalentTreeView {
        nodes,
        unspent_points: tree.unspent_points,
        total_spent: tree.total_spent,
    }
}

#[derive(Default)]
struct TalentLayoutCache {
    path: Option<PathBuf>,
    modified: Option<SystemTime>,
    positions: HashMap<u16, (f32, f32)>,
    warned_missing: bool,
}

fn hot_layout_positions() -> Option<HashMap<u16, (f32, f32)>> {
    static CACHE: OnceLock<Mutex<TalentLayoutCache>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(TalentLayoutCache::default()));
    let mut cache = cache.lock().ok()?;
    cache.reload_if_changed();
    if cache.positions.is_empty() {
        None
    } else {
        Some(cache.positions.clone())
    }
}

impl TalentLayoutCache {
    fn reload_if_changed(&mut self) {
        let Some(path) = self.layout_path() else {
            if !self.warned_missing {
                log::debug!("talent layout CSV not found: {TALENT_LAYOUT_CSV}");
                self.warned_missing = true;
            }
            self.positions.clear();
            self.modified = None;
            return;
        };

        let modified = std::fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .ok();
        if self.modified.is_some() && self.modified == modified {
            return;
        }

        match std::fs::read_to_string(&path)
            .map_err(|err| err.to_string())
            .and_then(|raw| parse_layout_csv(&raw))
        {
            Ok(positions) => {
                self.positions = positions;
                self.modified = modified;
                log::info!(
                    "hot-reloaded {} talent layout positions from {}",
                    self.positions.len(),
                    path.display()
                );
            }
            Err(err) => {
                log::warn!(
                    "failed to hot-reload talent layout CSV {}; keeping last good layout: {}",
                    path.display(),
                    err
                );
            }
        }
    }

    fn layout_path(&mut self) -> Option<PathBuf> {
        if let Some(path) = &self.path {
            if path.exists() {
                return Some(path.clone());
            }
        }
        let path = resolve_asset_path(Path::new(TALENT_LAYOUT_CSV)).ok()?;
        self.path = Some(path.clone());
        Some(path)
    }
}

fn parse_layout_csv(raw: &str) -> Result<HashMap<u16, (f32, f32)>, String> {
    let mut lines = raw.lines().filter(|line| !line.trim().is_empty());
    let header = lines.next().ok_or_else(|| "empty CSV".to_string())?;
    let headers = split_csv_line(header);
    let id_idx = csv_col(&headers, "id")?;
    let opt_x_idx = csv_col(&headers, "optimized_x").ok();
    let opt_y_idx = csv_col(&headers, "optimized_y").ok();
    let x_idx = csv_col(&headers, "x")?;
    let y_idx = csv_col(&headers, "y")?;

    let mut positions = HashMap::new();
    for (line_no, line) in lines.enumerate() {
        let cols = split_csv_line(line);
        let row_no = line_no + 2;
        let id: u16 = csv_value(&cols, id_idx, row_no, "id")?
            .parse()
            .map_err(|_| {
                format!(
                    "row {row_no}: invalid talent id {:?}",
                    csv_value(&cols, id_idx, row_no, "id").unwrap_or_default()
                )
            })?;

        let x = csv_optional_f32(&cols, opt_x_idx)
            .or_else(|| csv_optional_f32(&cols, Some(x_idx)))
            .ok_or_else(|| format!("row {row_no}: missing x/optimized_x"))?;
        let y = csv_optional_f32(&cols, opt_y_idx)
            .or_else(|| csv_optional_f32(&cols, Some(y_idx)))
            .ok_or_else(|| format!("row {row_no}: missing y/optimized_y"))?;
        positions.insert(id, (x, y));
    }

    Ok(positions)
}

fn csv_col(headers: &[String], name: &str) -> Result<usize, String> {
    headers
        .iter()
        .position(|header| header == name)
        .ok_or_else(|| format!("missing CSV column {name:?}"))
}

fn csv_value(cols: &[String], idx: usize, row_no: usize, name: &str) -> Result<String, String> {
    cols.get(idx)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("row {row_no}: missing {name}"))
}

fn csv_optional_f32(cols: &[String], idx: Option<usize>) -> Option<f32> {
    let value = cols.get(idx?)?.trim();
    if value.is_empty() {
        None
    } else {
        value.parse().ok()
    }
}

fn split_csv_line(line: &str) -> Vec<String> {
    let mut cols = Vec::new();
    let mut col = String::new();
    let mut chars = line.chars().peekable();
    let mut quoted = false;

    while let Some(ch) = chars.next() {
        match ch {
            '"' if quoted && chars.peek() == Some(&'"') => {
                col.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => {
                cols.push(col.trim().to_string());
                col.clear();
            }
            _ => col.push(ch),
        }
    }
    cols.push(col.trim().to_string());
    cols
}

fn route_view(r: Route) -> TalentRouteView {
    match r {
        Route::Hub => TalentRouteView::Hub,
        Route::Warrior => TalentRouteView::Warrior,
        Route::Mage => TalentRouteView::Mage,
        Route::Healer => TalentRouteView::Healer,
        Route::Summoner => TalentRouteView::Summoner,
        Route::Synergy => TalentRouteView::Synergy,
        Route::Fifth => TalentRouteView::Fifth,
    }
}

fn kind_view(effect: &TalentEffect) -> TalentNodeKind {
    match effect {
        TalentEffect::PercentBonus { .. } | TalentEffect::FlatBonus { .. } => TalentNodeKind::Stat,
        TalentEffect::UnlockAbility { .. } => TalentNodeKind::Unlock,
        TalentEffect::AbilityMod { .. } => TalentNodeKind::Modifier,
        TalentEffect::PassiveProc { .. } => TalentNodeKind::Proc,
        TalentEffect::Keystone { .. } => TalentNodeKind::Keystone,
        TalentEffect::Synergy { .. } => TalentNodeKind::Synergy,
    }
}

fn tooltip_lines(node: &rift_game::talents::TalentNode) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    if !node.description.is_empty() {
        lines.push(node.description.to_string());
    }
    lines.push(format!("Status: {}", node.status.label()));
    match &node.effect {
        TalentEffect::PercentBonus { stat, per_rank } => {
            lines.push(format!(
                "+{:.0}% {} per rank",
                per_rank * 100.0,
                stat_label(*stat),
            ));
        }
        TalentEffect::FlatBonus { stat, per_rank } => {
            lines.push(format!("+{} {} per rank", per_rank, stat_label(*stat)));
        }
        TalentEffect::AbilityMod { modifier, .. } => {
            lines.push(format!("Modifier: {}", modifier_label(modifier)));
        }
        TalentEffect::PassiveProc {
            chance,
            per_rank,
            description,
        } => {
            if !description.is_empty() {
                lines.push(description.to_string());
            }
            lines.push(format!(
                "{:.0}% chance per rank, +{:.0}% per extra rank",
                chance * 100.0,
                per_rank * 100.0,
            ));
        }
        TalentEffect::UnlockAbility { .. } => {
            lines.push("Unlocks the ability for use on the bar.".to_string());
        }
        TalentEffect::Keystone { keystone } => {
            lines.push(format!("Keystone: {}", keystone_label(*keystone)));
        }
        TalentEffect::Synergy { description } => {
            lines.push(format!("Synergy: {description}"));
        }
    }
    lines
}

fn stat_label(s: TalentStat) -> &'static str {
    match s {
        TalentStat::Damage => "damage",
        TalentStat::CritChance => "crit chance",
        TalentStat::CritDamage => "crit damage",
        TalentStat::AttackSpeed => "attack speed",
        TalentStat::MoveSpeed => "move speed",
        TalentStat::MaxHp => "max HP",
        TalentStat::Defense => "defense",
        TalentStat::ProjectileSpeed => "projectile speed",
        TalentStat::Range => "range",
        TalentStat::CooldownReduction => "cooldown reduction",
    }
}

fn modifier_label(m: &AbilityModifier) -> String {
    match m {
        AbilityModifier::ExtraProjectiles(n) => format!("+{n} projectile(s)"),
        AbilityModifier::CooldownReduction(s) => format!("-{s:.1}s cooldown"),
        AbilityModifier::DamageBonus(p) => format!("+{:.0}% damage", p * 100.0),
        AbilityModifier::Pierce(n) => format!("Pierces +{n} target(s)"),
        AbilityModifier::Chain(n) => format!("Chains to +{n} target(s)"),
    }
}

fn keystone_label(k: KeystoneId) -> &'static str {
    match k {
        KeystoneId::Berserker => "Berserker",
        KeystoneId::Executioner => "Executioner",
        KeystoneId::BurningCrits => "Burning Crits",
        KeystoneId::BeamConduit => "Beam Conduit",
        KeystoneId::BattlePrayer => "Battle Prayer",
        KeystoneId::Sanctuary => "Sanctuary",
        KeystoneId::Bonded => "Bonded",
        KeystoneId::Necromancer => "Necromancer",
        KeystoneId::Named(name) => name,
    }
}
