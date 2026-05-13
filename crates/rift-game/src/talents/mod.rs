//! Talents — declarative tree shape + content.
//!
//! See `TALENT_TREE.md` for the design doc. The tree is a single
//! shared graph with a central **Hub** and four (extensible)
//! routes (Warrior / Mage / Healer / Summoner). Gating is purely
//! topological: a node is investable iff every prerequisite node
//! has rank ≥ 1 — there is no global "points-spent-in-lower-tier"
//! threshold.
//!
//! ## Layout
//!
//! - This `mod.rs` owns the schema (`TalentNode`, `TalentEffect`,
//!   `TalentTree`, validator) and the `fresh_character_tree()`
//!   aggregator that pulls all routes together.
//! - Each route lives in its own submodule (`hub`, `warrior`,
//!   `mage`, `healer`, `summoner`) exposing a single
//!   `pub fn nodes() -> Vec<TalentNode>`. Adding a fifth route is
//!   one new file + one line in `fresh_character_tree`.

use crate::abilities::AbilityId;
use crate::stats::{Stat, StatModifiers};

pub mod healer;
pub mod hub;
pub mod mage;
pub mod summoner;
pub mod warrior;

/// Unique identifier for a talent node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TalentId(pub u16);

/// Which route in the tree a node belongs to. Used for UI grouping,
/// route-local cluster passives, and (eventually) per-route point
/// tallies. `Hub` is the central junction every route connects
/// through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Route {
    Hub,
    Warrior,
    Mage,
    Healer,
    Summoner,
}

/// Identifier for a rule-changing keystone effect. Each variant
/// fans out to a hand-authored block at apply time (combat
/// pipeline reads this and toggles flags / inserts behaviour).
/// Listed here as placeholders matching `TALENT_TREE.md` §8;
/// content is authored when each route lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeystoneId {
    // Warrior
    Berserker,
    Executioner,
    // Mage
    BurningCrits,
    BeamConduit,
    // Healer
    BattlePrayer,
    Sanctuary,
    // Summoner
    Bonded,
    Necromancer,
}

/// A single node in the talent tree.
#[derive(Clone, Debug)]
pub struct TalentNode {
    pub id: TalentId,
    pub name: &'static str,
    pub description: &'static str,
    /// Max rank (1 = binary unlock, 3 = can invest up to 3 points).
    pub max_rank: u8,
    /// Current invested rank.
    pub current_rank: u8,
    /// Which route this node belongs to.
    pub route: Route,
    /// Prerequisites — IDs of nodes that must have at least 1 rank.
    pub prerequisites: Vec<TalentId>,
    /// Effect per rank (additive where stackable).
    pub effect: TalentEffect,
}

/// What a talent does per rank.
#[derive(Clone, Debug)]
pub enum TalentEffect {
    /// +X% bonus to a stat.
    PercentBonus { stat: TalentStat, per_rank: f32 },
    /// +X flat bonus to a stat.
    FlatBonus { stat: TalentStat, per_rank: f32 },
    /// Unlock an ability for casting. Single-rank, single-point.
    /// While the node has rank 0 the ability id behaves as locked;
    /// once rank ≥ 1 the ability is castable (subject to loadout
    /// slot unlocks).
    UnlockAbility { ability: AbilityId },
    /// Modify a specific ability (more projectiles, pierce, cdr, etc.).
    /// By validator rule, a node with this effect must be
    /// topologically downstream of an `UnlockAbility` node for the
    /// same ability — i.e. you cannot encounter a modifier before
    /// the unlock.
    AbilityMod {
        ability: AbilityId,
        modifier: AbilityModifier,
    },
    /// Unlock a passive proc.
    PassiveProc {
        description: &'static str,
        chance: f32,
        per_rank: f32,
    },
    /// Rule-changing keystone. Single-rank by convention; the
    /// `KeystoneId` discriminator is matched at the consumption
    /// site (combat / heal / proc pipelines) to apply both the
    /// positive effect and any paired drawback (see
    /// `TALENT_TREE.md` §13).
    Keystone { keystone: KeystoneId },
}

/// Stats that talents can modify.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TalentStat {
    Damage,
    CritChance,
    CritDamage,
    AttackSpeed,
    MoveSpeed,
    MaxHp,
    Defense,
    ProjectileSpeed,
    Range,
    CooldownReduction,
}

/// How a talent modifies an ability.
#[derive(Clone, Debug)]
pub enum AbilityModifier {
    /// Extra projectiles.
    ExtraProjectiles(u32),
    /// Reduced cooldown (flat seconds).
    CooldownReduction(f32),
    /// Extra damage multiplier.
    DamageBonus(f32),
    /// Pierce through targets.
    Pierce(u32),
    /// Chain to nearby enemies.
    Chain(u32),
}

/// The full talent tree for a character.
#[derive(Clone, Debug)]
pub struct TalentTree {
    pub nodes: Vec<TalentNode>,
    pub unspent_points: u32,
    /// Total points spent (for telemetry / UI).
    pub total_spent: u32,
}

impl TalentTree {
    /// Can the player invest a point in this talent?
    ///
    /// Pure-topological gating: requires unspent points, the node
    /// not being maxed, and every prerequisite node having
    /// `current_rank ≥ 1`. The old "X points spent in lower tiers"
    /// check is gone — depth is expressed by the prerequisite
    /// graph itself.
    pub fn can_invest(&self, id: TalentId) -> bool {
        let Some(node) = self.nodes.iter().find(|n| n.id == id) else {
            return false;
        };

        if node.current_rank >= node.max_rank {
            return false;
        }

        if self.unspent_points == 0 {
            return false;
        }

        for prereq in &node.prerequisites {
            let Some(prereq_node) = self.nodes.iter().find(|n| n.id == *prereq) else {
                return false;
            };
            if prereq_node.current_rank == 0 {
                return false;
            }
        }

        true
    }

    /// Invest a point in a talent. Returns false if not possible.
    pub fn invest(&mut self, id: TalentId) -> bool {
        if !self.can_invest(id) {
            return false;
        }
        if let Some(node) = self.nodes.iter_mut().find(|n| n.id == id) {
            node.current_rank += 1;
            self.unspent_points -= 1;
            self.total_spent += 1;
            true
        } else {
            false
        }
    }

    /// Lesser-respec: refund **every rank** of a single node and
    /// return its points to the unspent pool.
    ///
    /// Per `TALENT_TREE.md` §7 the refund is **rejected** if any
    /// other invested node lists this one as a prerequisite —
    /// orphaning would leave the tree in an inconsistent state
    /// (downstream nodes whose prereq closure is no longer
    /// satisfied). The player must lesser-respec leaves first or
    /// use a Greater token.
    ///
    /// Returns the number of points refunded (0 = rejected: node
    /// not found, no ranks invested, or would orphan a downstream
    /// node).
    pub fn refund_one(&mut self, id: TalentId) -> u32 {
        let Some(idx) = self.nodes.iter().position(|n| n.id == id) else {
            return 0;
        };
        let rank = self.nodes[idx].current_rank;
        if rank == 0 {
            return 0;
        }
        // Orphan check: any other invested node prereq-listing this id?
        let would_orphan = self
            .nodes
            .iter()
            .any(|n| n.current_rank >= 1 && n.id != id && n.prerequisites.contains(&id));
        if would_orphan {
            return 0;
        }
        self.nodes[idx].current_rank = 0;
        let refund = rank as u32;
        self.unspent_points += refund;
        self.total_spent = self.total_spent.saturating_sub(refund);
        refund
    }

    /// Greater-respec: refund every invested point, wiping the
    /// tree back to rank 0 across the board. Per `TALENT_TREE.md`
    /// §7 this never has to worry about orphaning — every node
    /// drops together. Returns the total points returned to the
    /// unspent pool.
    pub fn refund_all(&mut self) -> u32 {
        let mut refunded = 0u32;
        for n in self.nodes.iter_mut() {
            refunded += n.current_rank as u32;
            n.current_rank = 0;
        }
        self.unspent_points += refunded;
        self.total_spent = 0;
        refunded
    }

    /// Compute aggregated bonuses from all invested talents.
    pub fn compute_bonuses(&self) -> TalentBonuses {
        let mut bonuses = TalentBonuses::default();
        for node in &self.nodes {
            if node.current_rank == 0 {
                continue;
            }
            let rank = node.current_rank as f32;
            match &node.effect {
                TalentEffect::PercentBonus { stat, per_rank } => {
                    let val = per_rank * rank;
                    bonuses.apply_percent(*stat, val);
                }
                TalentEffect::FlatBonus { stat, per_rank } => {
                    let val = per_rank * rank;
                    bonuses.apply_flat(*stat, val);
                }
                TalentEffect::AbilityMod { .. } => {}
                TalentEffect::PassiveProc { .. } => {}
                TalentEffect::UnlockAbility { .. } => {}
                TalentEffect::Keystone { .. } => {}
            }
        }
        bonuses
    }

    /// True iff the tree contains an `UnlockAbility` node for
    /// `ability` with at least one rank invested. Use this at
    /// cast / loadout time to gate ability availability.
    pub fn is_ability_unlocked(&self, ability: AbilityId) -> bool {
        self.nodes.iter().any(|n| {
            n.current_rank >= 1
                && matches!(
                    n.effect,
                    TalentEffect::UnlockAbility { ability: a } if a == ability
                )
        })
    }

    /// Iterate every ability id currently unlocked by the tree.
    pub fn unlocked_abilities(&self) -> impl Iterator<Item = AbilityId> + '_ {
        self.nodes.iter().filter_map(|n| match n.effect {
            TalentEffect::UnlockAbility { ability } if n.current_rank >= 1 => Some(ability),
            _ => None,
        })
    }

    /// Iterate active keystones (rank ≥ 1) so the combat /
    /// heal pipelines can apply their effects.
    pub fn active_keystones(&self) -> impl Iterator<Item = KeystoneId> + '_ {
        self.nodes.iter().filter_map(|n| match n.effect {
            TalentEffect::Keystone { keystone } if n.current_rank >= 1 => Some(keystone),
            _ => None,
        })
    }
}

/// Aggregated bonuses from talent tree (percent and flat).
#[derive(Clone, Debug, Default)]
pub struct TalentBonuses {
    pub damage_pct: f32,
    pub crit_chance: f32,
    pub crit_damage_pct: f32,
    pub attack_speed_pct: f32,
    pub move_speed_pct: f32,
    pub max_hp_pct: f32,
    pub defense_pct: f32,
    pub projectile_speed_pct: f32,
    pub range_pct: f32,
    pub cooldown_reduction_pct: f32,
}

impl TalentBonuses {
    fn apply_percent(&mut self, stat: TalentStat, value: f32) {
        match stat {
            TalentStat::Damage => self.damage_pct += value,
            TalentStat::CritChance => self.crit_chance += value,
            TalentStat::CritDamage => self.crit_damage_pct += value,
            TalentStat::AttackSpeed => self.attack_speed_pct += value,
            TalentStat::MoveSpeed => self.move_speed_pct += value,
            TalentStat::MaxHp => self.max_hp_pct += value,
            TalentStat::Defense => self.defense_pct += value,
            TalentStat::ProjectileSpeed => self.projectile_speed_pct += value,
            TalentStat::Range => self.range_pct += value,
            TalentStat::CooldownReduction => self.cooldown_reduction_pct += value,
        }
    }

    fn apply_flat(&mut self, stat: TalentStat, value: f32) {
        self.apply_percent(stat, value);
    }
}

impl TalentTree {
    /// Convert this tree's invested ranks into a [`StatModifiers`]
    /// suitable for [`crate::stats::CharacterStats::compute`].
    /// Stats that have no `Stat` analogue (`Defense`, `Range`,
    /// `ProjectileSpeed`) are skipped — they're either driven
    /// elsewhere or not yet wired into the character sheet.
    pub fn stat_modifiers(&self) -> StatModifiers {
        let mut m = StatModifiers::new();
        for node in &self.nodes {
            if node.current_rank == 0 {
                continue;
            }
            let val = match &node.effect {
                TalentEffect::PercentBonus { stat, per_rank }
                | TalentEffect::FlatBonus { stat, per_rank } => {
                    (*stat, per_rank * node.current_rank as f32)
                }
                _ => continue,
            };
            let (tstat, v) = val;
            // Map TalentStat -> Stat. Percent-style bonuses go on
            // the percent channel; chance/multiplier stats go on
            // flat (additive in their native unit).
            match (tstat, &node.effect) {
                // `Damage` is now an aggregate — with `Stat::Power`
                // gone, route it to both `WeaponDamage` and
                // `SpellDamage` so a generic +damage talent buffs
                // every build path. Flat damage talents fold into
                // the same percent channel; we don't author flat
                // damage talents anyway.
                (TalentStat::MaxHp, TalentEffect::PercentBonus { .. }) => {
                    m.percent.add(Stat::Health, v);
                }
                (TalentStat::MaxHp, TalentEffect::FlatBonus { .. }) => {
                    m.flat.add(Stat::Health, v);
                }
                (TalentStat::CritChance, _) => m.flat.add(Stat::CritChance, v),
                (TalentStat::CritDamage, _) => m.flat.add(Stat::CritDamage, v),
                (TalentStat::AttackSpeed, _) => m.flat.add(Stat::AttackSpeed, v),
                (TalentStat::MoveSpeed, _) => m.flat.add(Stat::MoveSpeed, v),
                (TalentStat::CooldownReduction, _) => m.flat.add(Stat::CooldownReduction, v),
                // `Defense` is now folded into `Stat::Armor`'s
                // percent channel — a +5 % defense talent reads
                // as +5 % armor at compute time.
                (TalentStat::Defense, _) => m.percent.add(Stat::Armor, v),
                // `Range` rolls onto `Stat::Range` (global ability
                // range multiplier). `ProjectileSpeed` has no
                // `Stat` analogue yet — dropped on the floor.
                (TalentStat::Range, _) => m.flat.add(Stat::Range, v),
                _ => {}
            }
        }
        m
    }
}

// ─── Tree graph validator ────────────────────────────────────────────────

/// A structural problem with a `TalentTree`'s graph. Returned by
/// [`TalentTree::validate`]; intended for use as a debug-only
/// startup assertion when authoring content.
#[derive(Debug, PartialEq, Eq)]
pub enum TreeValidationError {
    /// A node lists a prerequisite id that does not exist in the tree.
    DanglingPrerequisite { node: TalentId, missing: TalentId },
    /// A node lists itself (directly or transitively) as a prereq.
    Cycle { node: TalentId },
    /// An `AbilityMod` node's prerequisite closure does not contain
    /// any `UnlockAbility` node for the same ability. Per
    /// `TALENT_TREE.md` §4.1: modifiers must be topologically
    /// downstream of the corresponding unlock node so the player
    /// cannot encounter "+1 projectile to Fireball" before they
    /// could have unlocked Fireball.
    ModifierBeforeUnlock { node: TalentId, ability: AbilityId },
    /// Two nodes share an id.
    DuplicateId(TalentId),
}

impl TalentTree {
    /// Validate the tree's graph shape. Cheap enough to run as a
    /// debug-only startup assertion on every `TalentTree`
    /// constructor.
    pub fn validate(&self) -> Result<(), Vec<TreeValidationError>> {
        let mut errors = Vec::new();

        // Duplicate ids.
        for (i, a) in self.nodes.iter().enumerate() {
            if self.nodes.iter().skip(i + 1).any(|b| b.id == a.id) {
                errors.push(TreeValidationError::DuplicateId(a.id));
            }
        }

        // Dangling prereqs.
        for node in &self.nodes {
            for prereq in &node.prerequisites {
                if !self.nodes.iter().any(|n| n.id == *prereq) {
                    errors.push(TreeValidationError::DanglingPrerequisite {
                        node: node.id,
                        missing: *prereq,
                    });
                }
            }
        }

        // Cycles + modifier-before-unlock (both use the prereq closure).
        for node in &self.nodes {
            // Closure walk; reject if we re-enter `node.id`.
            let mut stack: Vec<TalentId> = node.prerequisites.clone();
            let mut visited: Vec<TalentId> = Vec::new();
            let mut closure: Vec<TalentId> = Vec::new();
            let mut cyclic = false;
            while let Some(cur) = stack.pop() {
                if cur == node.id {
                    cyclic = true;
                    break;
                }
                if visited.contains(&cur) {
                    continue;
                }
                visited.push(cur);
                closure.push(cur);
                if let Some(n) = self.nodes.iter().find(|n| n.id == cur) {
                    for p in &n.prerequisites {
                        stack.push(*p);
                    }
                }
            }
            if cyclic {
                errors.push(TreeValidationError::Cycle { node: node.id });
                continue;
            }

            if let TalentEffect::AbilityMod { ability, .. } = &node.effect {
                let ok = closure.iter().any(|cid| {
                    self.nodes.iter().any(|n| {
                        n.id == *cid
                            && matches!(
                                n.effect,
                                TalentEffect::UnlockAbility { ability: a } if a == *ability
                            )
                    })
                });
                if !ok {
                    errors.push(TreeValidationError::ModifierBeforeUnlock {
                        node: node.id,
                        ability: *ability,
                    });
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

// ─── Assembly ────────────────────────────────────────────────────────────

/// Build the full talent tree a fresh character starts with —
/// every node at `current_rank = 0`, every route present so the
/// UI can render the full graph from the first frame.
///
/// Per `TALENT_TREE.md` §2 / §6 the constructor does **not** seed
/// `unspent_points`; the experience system grants the starter
/// point at level 1 and one point per level thereafter.
pub fn fresh_character_tree() -> TalentTree {
    let mut nodes = Vec::new();
    nodes.extend(hub::nodes());
    nodes.extend(warrior::nodes());
    nodes.extend(mage::nodes());
    nodes.extend(healer::nodes());
    nodes.extend(summoner::nodes());

    let tree = TalentTree {
        nodes,
        unspent_points: 0,
        total_spent: 0,
    };

    // Debug-only structural assertion. The route builders are
    // hand-authored; this catches dangling prereqs / cycles /
    // modifier-before-unlock bugs at the earliest point.
    debug_assert!(
        tree.validate().is_ok(),
        "talent tree failed validation: {:?}",
        tree.validate().err()
    );

    tree
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fresh-character tree must pass the structural validator
    /// — no dangling prereqs, no cycles, no AbilityMod nodes that
    /// can be reached before their UnlockAbility prerequisite, no
    /// duplicate ids. This is the same check the `debug_assert!`
    /// in `fresh_character_tree` performs, surfaced as a real test
    /// so a release build with content errors still fails CI.
    #[test]
    fn fresh_character_tree_validates() {
        let tree = fresh_character_tree();
        if let Err(errs) = tree.validate() {
            panic!("fresh character tree failed validation: {:?}", errs);
        }
    }

    /// Hub authoring sanity: the dodge-roll unlock must exist and
    /// must list its movement-cluster lead-in as a prereq, matching
    /// `TALENT_TREE.md` §11 resolved-decision #1 ("costs 1 point,
    /// single rank" — but reached via the cluster, never freely).
    #[test]
    fn hub_dodge_roll_is_gated() {
        let tree = fresh_character_tree();
        let dodge = tree
            .nodes
            .iter()
            .find(|n| {
                matches!(
                    n.effect,
                    TalentEffect::UnlockAbility {
                        ability: crate::abilities::EVASIVE_ROLL
                    }
                )
            })
            .expect("hub must define an EVASIVE_ROLL unlock node");
        assert_eq!(dodge.max_rank, 1, "dodge roll is a single-rank unlock");
        assert!(
            !dodge.prerequisites.is_empty(),
            "dodge roll must not be free — needs a movement-cluster prereq"
        );
    }
}
