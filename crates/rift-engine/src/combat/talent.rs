/// Unique identifier for a talent node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TalentId(pub u16);

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
    /// Tier (row in tree). Must spend N points in lower tiers to unlock this tier.
    pub tier: u8,
    /// Prerequisites — IDs of nodes that must have at least 1 rank.
    pub prerequisites: Vec<TalentId>,
    /// Effect per rank (additive).
    pub effect: TalentEffect,
}

/// What a talent does per rank.
#[derive(Clone, Debug)]
pub enum TalentEffect {
    /// +X% bonus to a stat.
    PercentBonus { stat: TalentStat, per_rank: f32 },
    /// +X flat bonus to a stat.
    FlatBonus { stat: TalentStat, per_rank: f32 },
    /// Modify a specific ability.
    AbilityMod { ability: super::ability::AbilityId, modifier: AbilityModifier },
    /// Unlock a passive proc.
    PassiveProc { description: &'static str, chance: f32, per_rank: f32 },
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

/// The full talent tree for a class.
#[derive(Clone, Debug)]
pub struct TalentTree {
    pub nodes: Vec<TalentNode>,
    pub unspent_points: u32,
    /// Total points spent (used for tier gating).
    pub total_spent: u32,
}

impl TalentTree {
    /// Points required in lower tiers to unlock a given tier.
    fn points_required_for_tier(tier: u8) -> u32 {
        match tier {
            0 => 0,
            1 => 5,
            2 => 10,
            3 => 15,
            4 => 20,
            _ => 25,
        }
    }

    /// Can the player invest a point in this talent?
    pub fn can_invest(&self, id: TalentId) -> bool {
        let Some(node) = self.nodes.iter().find(|n| n.id == id) else {
            return false;
        };

        // Already maxed
        if node.current_rank >= node.max_rank {
            return false;
        }

        // No points available
        if self.unspent_points == 0 {
            return false;
        }

        // Tier gate
        if self.total_spent < Self::points_required_for_tier(node.tier) {
            return false;
        }

        // Prerequisites
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

    /// Compute aggregated bonuses from all invested talents.
    pub fn compute_bonuses(&self) -> TalentBonuses {
        let mut bonuses = TalentBonuses::default();
        for node in &self.nodes {
            if node.current_rank == 0 { continue; }
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
                TalentEffect::AbilityMod { .. } => {
                    // Ability mods handled separately when using abilities
                }
                TalentEffect::PassiveProc { .. } => {
                    // Passive procs handled in combat tick
                }
            }
        }
        bonuses
    }

    /// Create the Hunter talent tree.
    pub fn hunter() -> Self {
        let nodes = vec![
            // ─── Tier 0 (baseline) ──────────────────
            TalentNode {
                id: TalentId(0),
                name: "Sharp Eyes",
                description: "+2% crit chance per rank.",
                max_rank: 3,
                current_rank: 0,
                tier: 0,
                prerequisites: vec![],
                effect: TalentEffect::PercentBonus { stat: TalentStat::CritChance, per_rank: 0.02 },
            },
            TalentNode {
                id: TalentId(1),
                name: "Steady Aim",
                description: "+4% damage per rank.",
                max_rank: 3,
                current_rank: 0,
                tier: 0,
                prerequisites: vec![],
                effect: TalentEffect::PercentBonus { stat: TalentStat::Damage, per_rank: 0.04 },
            },
            TalentNode {
                id: TalentId(2),
                name: "Fleet Footed",
                description: "+3% movement speed per rank.",
                max_rank: 3,
                current_rank: 0,
                tier: 0,
                prerequisites: vec![],
                effect: TalentEffect::PercentBonus { stat: TalentStat::MoveSpeed, per_rank: 0.03 },
            },
            // ─── Tier 1 (5 points required) ──────────
            TalentNode {
                id: TalentId(3),
                name: "Deadly Draw",
                description: "+15% crit damage per rank.",
                max_rank: 3,
                current_rank: 0,
                tier: 1,
                prerequisites: vec![TalentId(0)],
                effect: TalentEffect::PercentBonus { stat: TalentStat::CritDamage, per_rank: 0.15 },
            },
            TalentNode {
                id: TalentId(4),
                name: "Quick Fingers",
                description: "+5% attack speed per rank.",
                max_rank: 3,
                current_rank: 0,
                tier: 1,
                prerequisites: vec![TalentId(1)],
                effect: TalentEffect::PercentBonus { stat: TalentStat::AttackSpeed, per_rank: 0.05 },
            },
            TalentNode {
                id: TalentId(5),
                name: "Long Range",
                description: "+10% projectile range per rank.",
                max_rank: 2,
                current_rank: 0,
                tier: 1,
                prerequisites: vec![TalentId(1)],
                effect: TalentEffect::PercentBonus { stat: TalentStat::Range, per_rank: 0.10 },
            },
            // ─── Tier 2 (10 points required) ─────────
            TalentNode {
                id: TalentId(6),
                name: "Multi-Shot Mastery",
                description: "Multi-Shot fires +1 arrow per rank.",
                max_rank: 2,
                current_rank: 0,
                tier: 2,
                prerequisites: vec![TalentId(4)],
                effect: TalentEffect::AbilityMod {
                    ability: super::ability::AbilityId::MultiShot,
                    modifier: AbilityModifier::ExtraProjectiles(1),
                },
            },
            TalentNode {
                id: TalentId(7),
                name: "Piercing Arrows",
                description: "Arrows pierce through 1 additional target per rank.",
                max_rank: 2,
                current_rank: 0,
                tier: 2,
                prerequisites: vec![TalentId(3)],
                effect: TalentEffect::AbilityMod {
                    ability: super::ability::AbilityId::SteadyShot,
                    modifier: AbilityModifier::Pierce(1),
                },
            },
            TalentNode {
                id: TalentId(8),
                name: "Thick Skin",
                description: "+5% defense per rank.",
                max_rank: 3,
                current_rank: 0,
                tier: 2,
                prerequisites: vec![TalentId(2)],
                effect: TalentEffect::PercentBonus { stat: TalentStat::Defense, per_rank: 0.05 },
            },
            // ─── Tier 3 (15 points required) ─────────
            TalentNode {
                id: TalentId(9),
                name: "Rapid Fire Mastery",
                description: "Rapid Fire cooldown reduced by 2s per rank.",
                max_rank: 2,
                current_rank: 0,
                tier: 3,
                prerequisites: vec![TalentId(4), TalentId(6)],
                effect: TalentEffect::AbilityMod {
                    ability: super::ability::AbilityId::RapidFire,
                    modifier: AbilityModifier::CooldownReduction(2.0),
                },
            },
            TalentNode {
                id: TalentId(10),
                name: "Death Mark",
                description: "Mark for Death also chains to 1 nearby enemy per rank.",
                max_rank: 2,
                current_rank: 0,
                tier: 3,
                prerequisites: vec![TalentId(7)],
                effect: TalentEffect::AbilityMod {
                    ability: super::ability::AbilityId::MarkForDeath,
                    modifier: AbilityModifier::Chain(1),
                },
            },
            // ─── Tier 4 (20 points required) ─────────
            TalentNode {
                id: TalentId(11),
                name: "Arrow Storm",
                description: "Rain of Arrows fires +4 arrows and deals +20% damage per rank.",
                max_rank: 1,
                current_rank: 0,
                tier: 4,
                prerequisites: vec![TalentId(9)],
                effect: TalentEffect::AbilityMod {
                    ability: super::ability::AbilityId::RainOfArrows,
                    modifier: AbilityModifier::DamageBonus(0.2),
                },
            },
        ];

        Self {
            nodes,
            unspent_points: 0,
            total_spent: 0,
        }
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
        // For now, flat and percent share the same fields (could split later)
        self.apply_percent(stat, value);
    }
}
