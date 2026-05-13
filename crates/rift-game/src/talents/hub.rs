//! Hub route — the central junction every route radiates from.
//!
//! Contains 6-10 generic-stat passives, the dodge-roll unlock,
//! and the short connector chains that bridge into each outer
//! route. See `TALENT_TREE.md` §3.1, §8 (connectors) and §11
//! (point 5).
//!
//! ## ID layout
//!
//! Reserved `TalentId` range: **0 .. 999**. Internal partitioning:
//!
//! | Range         | Purpose                                                         |
//! | ------------- | --------------------------------------------------------------- |
//! | `1 .. 99`     | Generic hub passives (open-access — no prereqs into the route). |
//! | `100 .. 109`  | Movement / dodge-roll cluster.                                  |
//! | `110 .. 119`  | Connector chain → Warrior entry.                                |
//! | `210 .. 219`  | Connector chain → Mage entry.                                   |
//! | `310 .. 319`  | Connector chain → Healer entry.                                 |
//! | `410 .. 419`  | Connector chain → Summoner entry.                               |
//!
//! Route entry nodes (in `warrior.rs` / `mage.rs` / …) list the
//! **tail** of the matching connector chain as their prerequisite,
//! which is what enforces the §3.3 "all cross-route travel goes
//! through the hub" rule.

use super::{Route, TalentEffect, TalentId, TalentNode, TalentStat};

/// Public IDs for the connector tails. The route files import
/// these to attach their entry node as a downstream prerequisite,
/// so the connector chain is the only path from the hub into a
/// route.
pub const CONNECTOR_WARRIOR_TAIL: TalentId = TalentId(111);
pub const CONNECTOR_MAGE_TAIL: TalentId = TalentId(211);
pub const CONNECTOR_HEALER_TAIL: TalentId = TalentId(311);
pub const CONNECTOR_SUMMONER_TAIL: TalentId = TalentId(411);

/// Hub node-set. See module docs for the layout.
pub fn nodes() -> Vec<TalentNode> {
    let mut out = Vec::new();

    // ─── Generic hub passives (1..99) ──────────────────────────────────
    //
    // Eight 3-rank passives covering the headline stat axes. No
    // prerequisites between them — they form the open cluster the
    // player walks through first regardless of build direction.
    // 24 total points if a player wanted to max every one, which is
    // more than a pure-build can afford to spend in the hub (per
    // §6 the player has ~75-85 points across a ~120-140 node tree,
    // so hub-only is intentionally a poor strategy).

    out.push(passive(
        1,
        "Vigor",
        "+3% maximum health per rank.",
        TalentStat::MaxHp,
        0.03,
        3,
        &[],
    ));
    out.push(passive(
        2,
        "Might",
        "+3% damage per rank.",
        TalentStat::Damage,
        0.03,
        3,
        &[],
    ));
    out.push(passive(
        3,
        "Keen Edge",
        "+1% critical strike chance per rank.",
        TalentStat::CritChance,
        0.01,
        3,
        &[],
    ));
    out.push(passive(
        4,
        "Focus",
        "+2% cooldown reduction per rank.",
        TalentStat::CooldownReduction,
        0.02,
        3,
        &[],
    ));
    out.push(passive(
        5,
        "Toughness",
        "+3% defense per rank.",
        TalentStat::Defense,
        0.03,
        3,
        &[],
    ));
    out.push(passive(
        6,
        "Swift Step",
        "+2% movement speed per rank.",
        TalentStat::MoveSpeed,
        0.02,
        3,
        &[],
    ));
    out.push(passive(
        7,
        "Reflexes",
        "+2% attack speed per rank.",
        TalentStat::AttackSpeed,
        0.02,
        3,
        &[],
    ));
    out.push(passive(
        8,
        "Precision",
        "+5% critical strike damage per rank.",
        TalentStat::CritDamage,
        0.05,
        3,
        &[],
    ));

    // ─── Movement / dodge-roll cluster (100..109) ─────────────────────
    //
    // Per `TALENT_TREE.md` §11 resolved-decision #1: the dodge roll
    // (`EVASIVE_ROLL`) is talent-gated rather than always-available,
    // and lives in the hub's movement cluster. One 3-rank lead-in
    // passive guards a single-rank unlock node, so the player
    // invests 2 points minimum to gain the dodge.

    out.push(passive(
        100,
        "Tumbler",
        "+3% movement speed per rank.",
        TalentStat::MoveSpeed,
        0.03,
        3,
        &[],
    ));
    out.push(TalentNode {
        id: TalentId(101),
        name: "Evasive Roll",
        description: "Unlock the Evasive Roll dodge (Space).",
        max_rank: 1,
        current_rank: 0,
        route: Route::Hub,
        prerequisites: vec![TalentId(100)],
        effect: TalentEffect::UnlockAbility {
            ability: crate::abilities::EVASIVE_ROLL,
        },
    });

    // ─── Connector chains (110+, 210+, 310+, 410+) ────────────────────
    //
    // Per §8 each chain is "2-3 cheap stat passives per route, each
    // +2-3% of a generic stat". Two single-rank nodes per chain
    // here; each route entry attaches as a downstream of the chain
    // tail. The first connector hangs off a thematically-flavoured
    // hub passive so the chain feels continuous on the screen graph
    // rather than free-floating.

    // Warrior connector — physical / endurance flavor.
    out.push(connector(
        110,
        "Strength",
        "+2% damage.",
        TalentStat::Damage,
        0.02,
        &[TalentId(2)],
    ));
    out.push(connector(
        111,
        "Endurance",
        "+2% maximum health.",
        TalentStat::MaxHp,
        0.02,
        &[TalentId(110)],
    ));

    // Mage connector — crit / cdr flavor.
    out.push(connector(
        210,
        "Insight",
        "+1% critical strike chance.",
        TalentStat::CritChance,
        0.01,
        &[TalentId(3)],
    ));
    out.push(connector(
        211,
        "Channeling",
        "+2% cooldown reduction.",
        TalentStat::CooldownReduction,
        0.02,
        &[TalentId(210)],
    ));

    // Healer connector — hp / cdr flavor.
    out.push(connector(
        310,
        "Compassion",
        "+2% maximum health.",
        TalentStat::MaxHp,
        0.02,
        &[TalentId(1)],
    ));
    out.push(connector(
        311,
        "Devotion",
        "+2% cooldown reduction.",
        TalentStat::CooldownReduction,
        0.02,
        &[TalentId(310)],
    ));

    // Summoner connector — crit / damage flavor.
    //
    // Hangs off `Precision` rather than `Keen Edge` so the
    // four route connectors prereq four *distinct* hub
    // passives. Otherwise Mage and Summoner share `Keen Edge`
    // (id 3), which forces the UI to draw an edge from one
    // hub passive to two opposite spokes — the resulting
    // crisscross dominates the hub graph reading.
    out.push(connector(
        410,
        "Command",
        "+1% critical strike chance.",
        TalentStat::CritChance,
        0.01,
        &[TalentId(8)],
    ));
    out.push(connector(
        411,
        "Bond",
        "+2% damage.",
        TalentStat::Damage,
        0.02,
        &[TalentId(410)],
    ));

    out
}

/// Stat-passive builder for hub nodes (route fixed to `Hub`).
fn passive(
    id: u16,
    name: &'static str,
    description: &'static str,
    stat: TalentStat,
    per_rank: f32,
    max_rank: u8,
    prerequisites: &[TalentId],
) -> TalentNode {
    TalentNode {
        id: TalentId(id),
        name,
        description,
        max_rank,
        current_rank: 0,
        route: Route::Hub,
        prerequisites: prerequisites.to_vec(),
        effect: TalentEffect::PercentBonus { stat, per_rank },
    }
}

/// Single-rank connector helper. Identical shape to [`passive`]
/// but always `max_rank = 1` to match the §8 "cheap stat passive"
/// connector contract.
fn connector(
    id: u16,
    name: &'static str,
    description: &'static str,
    stat: TalentStat,
    per_rank: f32,
    prerequisites: &[TalentId],
) -> TalentNode {
    TalentNode {
        id: TalentId(id),
        name,
        description,
        max_rank: 1,
        current_rank: 0,
        route: Route::Hub,
        prerequisites: prerequisites.to_vec(),
        effect: TalentEffect::PercentBonus { stat, per_rank },
    }
}
