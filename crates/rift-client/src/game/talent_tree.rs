//! Host-side adapter for the talent panel.
//!
//! Builds a [`TalentTreeView`] every frame from the live
//! `rift_game::talents::TalentTree` so the widget in `rift-ui`
//! never sees `rift_game` types directly. Tooltip strings are
//! formatted here so the widget has no `rift_game` formatting
//! code either.

use rift_game::talents::{
    AbilityModifier, KeystoneId, Route, TalentEffect, TalentId, TalentStat, TalentTree,
};
use rift_ui_types::talents::{TalentNodeKind, TalentNodeView, TalentRouteView, TalentTreeView};

/// Build a flat view of `tree` ready to hand to
/// `rift_ui::talents::frame_talent_panel`. Cheap to do every
/// frame; the heaviest cost is the per-node tooltip string
/// formatting.
pub fn build_talent_view(tree: &TalentTree) -> TalentTreeView<'_> {
    // Pre-compute the `TalentId → index` map once so the
    // prereq lookup below is O(N) instead of O(N²).
    let n = tree.nodes.len();
    let mut id_to_idx: std::collections::HashMap<TalentId, u16> =
        std::collections::HashMap::with_capacity(n);
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

        let prereqs_met = node
            .prerequisites
            .iter()
            .all(|p| tree.nodes.iter().any(|n| n.id == *p && n.current_rank >= 1));
        let investable = tree.can_invest(node.id);

        nodes.push(TalentNodeView {
            id: node.id.0,
            name: node.name,
            description: node.description,
            route: route_view(node.route),
            kind: kind_view(&node.effect),
            current_rank: node.current_rank,
            max_rank: node.max_rank,
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

fn route_view(r: Route) -> TalentRouteView {
    match r {
        Route::Hub => TalentRouteView::Hub,
        Route::Warrior => TalentRouteView::Warrior,
        Route::Mage => TalentRouteView::Mage,
        Route::Healer => TalentRouteView::Healer,
        Route::Summoner => TalentRouteView::Summoner,
    }
}

fn kind_view(effect: &TalentEffect) -> TalentNodeKind {
    match effect {
        TalentEffect::PercentBonus { .. } | TalentEffect::FlatBonus { .. } => TalentNodeKind::Stat,
        TalentEffect::UnlockAbility { .. } => TalentNodeKind::Unlock,
        TalentEffect::AbilityMod { .. } => TalentNodeKind::Modifier,
        TalentEffect::PassiveProc { .. } => TalentNodeKind::Proc,
        TalentEffect::Keystone { .. } => TalentNodeKind::Keystone,
    }
}

fn tooltip_lines(node: &rift_game::talents::TalentNode) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    if !node.description.is_empty() {
        lines.push(node.description.to_string());
    }
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
    }
}
