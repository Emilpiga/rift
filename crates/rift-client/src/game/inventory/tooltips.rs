//! Item tooltips and the side-by-side compare/delta panel.

use rift_engine::ui::im::{Color, Pos2, Rect, Tooltip, TooltipLine, Ui};
use rift_game::loot::Item;
use rift_game::stats::Stat;

/// Render a single item tooltip with the upgraded sizing —
/// header in `size_md`, body lines in `size_md`. Returns the
/// drawn rect (after screen clamping) so callers can stack
/// adjacent tooltips horizontally. `loadout` enables the
/// synergy footer ("→ Boosts <ability>") — pass `None` from
/// previews / character-select where the player has no slotted
/// abilities yet.
pub fn render_item_tooltip(
    ui: &mut Ui<'_>,
    item: &Item,
    header: &str,
    anchor: Pos2,
    loadout: Option<&rift_game::loadout::Loadout>,
) -> Rect {
    let theme = *ui.theme();
    let raw: Vec<String> = item.tooltip(loadout);
    let rarity = item.rarity.color();
    let rarity_col = Color::rgba(rarity[0], rarity[1], rarity[2], 1.0);
    let lines: Vec<TooltipLine<'_>> = raw
        .iter()
        .enumerate()
        .map(|(i, s)| TooltipLine {
            text: s.as_str(),
            // Name line gets `size_lg`; everything else
            // (`Item Level …`, implicits, affixes) sits at
            // `size_md` so the wall of stats is actually
            // legible at gameplay distance.
            size: if i == 0 { theme.fonts.size_lg } else { theme.fonts.size_md },
            color: if i == 0 {
                rarity_col
            } else if s.is_empty() {
                theme.colors.text
            } else if s.starts_with("Item Level") {
                theme.colors.text_dim
            } else if s.starts_with('\u{2500}') {
                // Divider between signature and bonus blocks.
                theme.colors.text_dim
            } else if s.starts_with('★') {
                // Legendary effect — gold tint.
                Color::rgba(1.00, 0.70, 0.20, 1.0)
            } else if s.starts_with('⚓') {
                // Anchored trait — saturated gold so the
                // chase-line reads at a glance.
                Color::rgba(1.00, 0.82, 0.25, 1.0)
            } else if s.starts_with('→') {
                // Synergy footer — accent.
                theme.colors.accent
            } else {
                theme.colors.text
            },
        })
        .collect();
    Tooltip::new()
        .header(header)
        .min_width(240.0)
        .pad(10.0)
        .show(ui, anchor, &lines)
}

/// Per-stat delta panel: for every stat that appears on either
/// item, render `+N`/`-N` in green / red so the player can see
/// at a glance what an equip swap would gain or cost.
pub fn render_compare_delta(
    ui: &mut Ui<'_>,
    hovered: &Item,
    equipped: &Item,
    anchor: Pos2,
) -> Rect {
    let theme = *ui.theme();
    let h_stats = hovered.stats();
    let e_stats = equipped.stats();

    // Union of stats touched by either side, in `Stat`
    // declaration order (kept stable so the column doesn't
    // dance frame to frame as a hovered item changes).
    const ORDER: &[Stat] = &[
        Stat::CritChance,
        Stat::CritDamage,
        Stat::AttackSpeed,
        Stat::Health,
        Stat::Vitality,
        Stat::Armor,
        Stat::Evasion,
        Stat::CooldownReduction,
        Stat::ResourceRegen,
        Stat::MoveSpeed,
        Stat::WeaponDamage,
        Stat::SpellDamage,
        Stat::PhysicalDamage,
        Stat::FireDamage,
        Stat::IceDamage,
        Stat::LightningDamage,
        Stat::ProjectileDamage,
        Stat::BeamDamage,
        Stat::AoeDamage,
        Stat::MeleeDamage,
    ];

    // Build the delta lines as owned strings; `TooltipLine`
    // borrows so we keep them in a local `Vec<String>`.
    let mut texts: Vec<(String, Color)> = Vec::new();
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
        let color = if delta > 0.0 {
            // Gain — soft green so it doesn't clash with
            // rarity highlights.
            Color::rgba(0.45, 0.92, 0.45, 1.0)
        } else {
            Color::rgba(0.96, 0.40, 0.40, 1.0)
        };
        texts.push((text, color));
    }

    if texts.is_empty() {
        // Both items roll the same stats with the same values
        // — surface that explicitly so the compare doesn't
        // look broken.
        texts.push((
            "No stat changes".to_string(),
            theme.colors.text_dim,
        ));
    }

    let lines: Vec<TooltipLine<'_>> = texts
        .iter()
        .map(|(t, c)| TooltipLine {
            text: t.as_str(),
            size: theme.fonts.size_md,
            color: *c,
        })
        .collect();

    Tooltip::new()
        .header("Change vs equipped")
        .header_color(Color::rgba(0.95, 0.85, 0.55, 1.0))
        .min_width(220.0)
        .pad(10.0)
        .show(ui, anchor, &lines)
}
