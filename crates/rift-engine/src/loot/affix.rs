/// Types of affixes that can roll on items.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AffixType {
    // ─── Offensive ───────────────────────────
    FlatDamage,       // +X damage
    PercentDamage,    // +X% damage (multiplicative with base)
    AttackSpeed,      // +X% attack speed
    CritChance,       // +X% critical hit chance
    CritDamage,       // +X% critical hit damage multiplier

    // ─── Defensive ──────────────────────────
    FlatDefense,      // +X armor/defense
    MaxHealth,        // +X max HP
    HealthRegen,      // +X HP/sec
    DamageReduction,  // +X% damage reduction

    // ─── Utility ─────────────────────────────
    MoveSpeed,        // +X% movement speed
    AttackRange,      // +X attack range
    AreaDamage,       // +X% area/splash damage
    LifeOnHit,        // +X HP per hit
}

/// A single rolled affix with a concrete value.
#[derive(Clone, Debug)]
pub struct Affix {
    pub affix_type: AffixType,
    /// The rolled value for this affix.
    pub value: f32,
    /// Whether this is a prefix (true) or suffix (false).
    pub is_prefix: bool,
}

impl Affix {
    pub fn flat_damage(&self) -> f32 {
        match self.affix_type {
            AffixType::FlatDamage => self.value,
            _ => 0.0,
        }
    }

    pub fn flat_defense(&self) -> f32 {
        match self.affix_type {
            AffixType::FlatDefense => self.value,
            _ => 0.0,
        }
    }

    pub fn speed_pct(&self) -> f32 {
        match self.affix_type {
            AffixType::MoveSpeed => self.value,
            _ => 0.0,
        }
    }

    pub fn attack_speed_pct(&self) -> f32 {
        match self.affix_type {
            AffixType::AttackSpeed => self.value,
            _ => 0.0,
        }
    }

    pub fn crit_chance(&self) -> f32 {
        match self.affix_type {
            AffixType::CritChance => self.value / 100.0, // stored as percent
            _ => 0.0,
        }
    }

    pub fn max_hp(&self) -> f32 {
        match self.affix_type {
            AffixType::MaxHealth => self.value,
            _ => 0.0,
        }
    }

    pub fn hp_regen(&self) -> f32 {
        match self.affix_type {
            AffixType::HealthRegen => self.value,
            _ => 0.0,
        }
    }

    pub fn percent_damage(&self) -> f32 {
        match self.affix_type {
            AffixType::PercentDamage => self.value / 100.0,
            _ => 0.0,
        }
    }

    pub fn damage_reduction(&self) -> f32 {
        match self.affix_type {
            AffixType::DamageReduction => self.value / 100.0,
            _ => 0.0,
        }
    }

    pub fn life_on_hit(&self) -> f32 {
        match self.affix_type {
            AffixType::LifeOnHit => self.value,
            _ => 0.0,
        }
    }

    /// Display name for this affix (e.g. "+12 Damage" or "+5% Move Speed").
    pub fn display(&self) -> String {
        match self.affix_type {
            AffixType::FlatDamage => format!("+{:.0} Damage", self.value),
            AffixType::PercentDamage => format!("+{:.0}% Damage", self.value),
            AffixType::AttackSpeed => format!("+{:.0}% Attack Speed", self.value),
            AffixType::CritChance => format!("+{:.1}% Crit Chance", self.value),
            AffixType::CritDamage => format!("+{:.0}% Crit Damage", self.value),
            AffixType::FlatDefense => format!("+{:.0} Defense", self.value),
            AffixType::MaxHealth => format!("+{:.0} Max HP", self.value),
            AffixType::HealthRegen => format!("+{:.1} HP/sec", self.value),
            AffixType::DamageReduction => format!("+{:.0}% Damage Reduction", self.value),
            AffixType::MoveSpeed => format!("+{:.0}% Move Speed", self.value),
            AffixType::AttackRange => format!("+{:.1} Range", self.value),
            AffixType::AreaDamage => format!("+{:.0}% Area Damage", self.value),
            AffixType::LifeOnHit => format!("+{:.1} Life on Hit", self.value),
        }
    }
}

/// Roll range for an affix at a given item level.
#[derive(Clone, Debug)]
pub struct AffixTier {
    pub affix_type: AffixType,
    pub is_prefix: bool,
    /// Minimum value at item level 1.
    pub min_base: f32,
    /// Maximum value at item level 1.
    pub max_base: f32,
    /// Added per item level to both min and max.
    pub scale_per_level: f32,
    /// Weight in the affix pool (higher = more common).
    pub weight: u32,
    /// Display name prefix/suffix for item naming.
    pub name_fragment: &'static str,
}

impl AffixTier {
    pub fn roll_range(&self, item_level: u32) -> (f32, f32) {
        let bonus = self.scale_per_level * (item_level.saturating_sub(1)) as f32;
        (self.min_base + bonus, self.max_base + bonus)
    }
}

/// The full pool of available affixes for item generation.
pub struct AffixPool {
    pub prefixes: Vec<AffixTier>,
    pub suffixes: Vec<AffixTier>,
}

impl AffixPool {
    pub fn standard() -> Self {
        Self {
            prefixes: vec![
                AffixTier {
                    affix_type: AffixType::FlatDamage,
                    is_prefix: true,
                    min_base: 2.0, max_base: 5.0, scale_per_level: 1.5,
                    weight: 100, name_fragment: "Sharp",
                },
                AffixTier {
                    affix_type: AffixType::PercentDamage,
                    is_prefix: true,
                    min_base: 5.0, max_base: 12.0, scale_per_level: 2.0,
                    weight: 60, name_fragment: "Brutal",
                },
                AffixTier {
                    affix_type: AffixType::FlatDefense,
                    is_prefix: true,
                    min_base: 3.0, max_base: 8.0, scale_per_level: 2.0,
                    weight: 90, name_fragment: "Sturdy",
                },
                AffixTier {
                    affix_type: AffixType::MaxHealth,
                    is_prefix: true,
                    min_base: 10.0, max_base: 25.0, scale_per_level: 5.0,
                    weight: 80, name_fragment: "Stalwart",
                },
                AffixTier {
                    affix_type: AffixType::AreaDamage,
                    is_prefix: true,
                    min_base: 8.0, max_base: 15.0, scale_per_level: 2.5,
                    weight: 40, name_fragment: "Devastating",
                },
                AffixTier {
                    affix_type: AffixType::LifeOnHit,
                    is_prefix: true,
                    min_base: 1.0, max_base: 3.0, scale_per_level: 0.5,
                    weight: 50, name_fragment: "Vampiric",
                },
            ],
            suffixes: vec![
                AffixTier {
                    affix_type: AffixType::AttackSpeed,
                    is_prefix: false,
                    min_base: 3.0, max_base: 8.0, scale_per_level: 1.0,
                    weight: 70, name_fragment: "of Haste",
                },
                AffixTier {
                    affix_type: AffixType::CritChance,
                    is_prefix: false,
                    min_base: 2.0, max_base: 5.0, scale_per_level: 0.8,
                    weight: 60, name_fragment: "of Precision",
                },
                AffixTier {
                    affix_type: AffixType::CritDamage,
                    is_prefix: false,
                    min_base: 10.0, max_base: 25.0, scale_per_level: 3.0,
                    weight: 50, name_fragment: "of Ferocity",
                },
                AffixTier {
                    affix_type: AffixType::MoveSpeed,
                    is_prefix: false,
                    min_base: 3.0, max_base: 8.0, scale_per_level: 1.0,
                    weight: 70, name_fragment: "of the Wind",
                },
                AffixTier {
                    affix_type: AffixType::HealthRegen,
                    is_prefix: false,
                    min_base: 1.0, max_base: 3.0, scale_per_level: 0.5,
                    weight: 60, name_fragment: "of Regeneration",
                },
                AffixTier {
                    affix_type: AffixType::DamageReduction,
                    is_prefix: false,
                    min_base: 2.0, max_base: 5.0, scale_per_level: 0.8,
                    weight: 55, name_fragment: "of the Fortress",
                },
                AffixTier {
                    affix_type: AffixType::AttackRange,
                    is_prefix: false,
                    min_base: 0.2, max_base: 0.5, scale_per_level: 0.1,
                    weight: 40, name_fragment: "of Reach",
                },
            ],
        }
    }
}
