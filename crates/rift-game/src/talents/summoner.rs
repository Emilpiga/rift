//! Summoner route — pets / minions.
//!
//! See `TALENT_TREE.md` §8.4.
//!
//! Reserved `TalentId` range: **4000 .. 4999**.
//!
//! §8.4 lists "Summon Wolf" / "Summon Golem" abilities that
//! haven't been authored yet. Until those ability registry rows
//! exist, the route ships with the stat passives, the
//! pet-damage scaling, and the two reserved keystones (Bonded /
//! Necromancer). The Bonded keystone gates the capstone — same
//! topological role as the §8.4 ring-3 entry — so the route is
//! still walkable end-to-end without dangling `UnlockAbility`
//! nodes pointing at non-existent `AbilityId`s.

use super::hub::CONNECTOR_SUMMONER_TAIL;
use super::{KeystoneId, Route, TalentEffect, TalentId, TalentNode, TalentStat};

pub fn nodes() -> Vec<TalentNode> {
    let mut out = Vec::new();

    // ─── Ring 1 ────────────────────────────────────────────────────────
    // Stand-in entry node: a 3-rank pet-damage stat passive (no
    // ability unlock, since "Summon Wolf" isn't authored yet).
    // Attaches to the hub connector tail so the cross-route
    // routing rule still holds.
    out.push(stat(
        4000,
        "Pet Mastery",
        "+5% damage per rank.",
        TalentStat::Damage,
        0.05,
        3,
        &[CONNECTOR_SUMMONER_TAIL],
    ));

    // ─── Ring 3 ────────────────────────────────────────────────────────
    out.push(TalentNode {
        id: TalentId(4010),
        name: "Bonded",
        description: "Pets inherit your critical strike chance.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Summoner,
        prerequisites: vec![TalentId(4000)],
        effect: TalentEffect::Keystone {
            keystone: KeystoneId::Bonded,
        },
    });

    // ─── Ring 4 ────────────────────────────────────────────────────────
    out.push(TalentNode {
        id: TalentId(4020),
        name: "Necromancer",
        description: "Killed enemies have a chance to rise as a minion.",
        max_rank: 1,
        current_rank: 0,
        route: Route::Summoner,
        prerequisites: vec![TalentId(4010)],
        effect: TalentEffect::Keystone {
            keystone: KeystoneId::Necromancer,
        },
    });

    out
}

fn stat(
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
        route: Route::Summoner,
        prerequisites: prerequisites.to_vec(),
        effect: TalentEffect::PercentBonus { stat, per_rank },
    }
}
