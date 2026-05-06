//! Player attributes (Agility/Strength/Intellect/Vitality) and the
//! scaling rules that turn raw attribute values into combat stats.

/// Core attribute types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AttributeType {
    /// Agility — primary for Hunter. Boosts damage, crit chance, attack speed.
    Agility,
    /// Strength — primary for melee classes. Boosts damage, armor, HP.
    Strength,
    /// Intellect — primary for caster classes. Boosts spell damage, resource.
    Intellect,
    /// Vitality — universal. Boosts HP, HP regen, defense.
    Vitality,
}

/// Player attribute values (base + bonus from gear).
#[derive(Clone, Debug)]
pub struct Attributes {
    pub agility: u32,
    pub strength: u32,
    pub intellect: u32,
    pub vitality: u32,
    /// Unspent attribute points.
    pub unspent_points: u32,
}

impl Default for Attributes {
    fn default() -> Self {
        Self {
            agility: 10,
            strength: 10,
            intellect: 10,
            vitality: 10,
            unspent_points: 0,
        }
    }
}

impl Attributes {
    /// Create starting attributes for a given class (primary attribute starts higher).
    pub fn for_class(primary: AttributeType) -> Self {
        let mut attrs = Self::default();
        match primary {
            AttributeType::Agility => attrs.agility = 15,
            AttributeType::Strength => attrs.strength = 15,
            AttributeType::Intellect => attrs.intellect = 15,
            AttributeType::Vitality => attrs.vitality = 15,
        }
        attrs
    }

    /// Get value of a specific attribute.
    pub fn get(&self, attr: AttributeType) -> u32 {
        match attr {
            AttributeType::Agility => self.agility,
            AttributeType::Strength => self.strength,
            AttributeType::Intellect => self.intellect,
            AttributeType::Vitality => self.vitality,
        }
    }

    /// Spend a point on an attribute. Returns false if no points available.
    pub fn spend_point(&mut self, attr: AttributeType) -> bool {
        if self.unspent_points == 0 {
            return false;
        }
        self.unspent_points -= 1;
        match attr {
            AttributeType::Agility => self.agility += 1,
            AttributeType::Strength => self.strength += 1,
            AttributeType::Intellect => self.intellect += 1,
            AttributeType::Vitality => self.vitality += 1,
        }
        true
    }
}

/// How attributes scale into combat stats.
#[derive(Clone, Debug)]
pub struct AttributeScaling {
    pub primary: AttributeType,
}

impl AttributeScaling {
    pub fn new(primary: AttributeType) -> Self {
        Self { primary }
    }

    /// Bonus damage from primary attribute.
    /// Each point of primary attribute gives 1% bonus damage.
    pub fn damage_bonus(&self, attrs: &Attributes) -> f32 {
        attrs.get(self.primary) as f32 * 0.01
    }

    /// Bonus defense from primary attribute + strength.
    /// Primary gives 0.5% per point, strength gives 0.8% per point.
    pub fn defense_bonus(&self, attrs: &Attributes) -> f32 {
        let primary_bonus = attrs.get(self.primary) as f32 * 0.005;
        let str_bonus = attrs.strength as f32 * 0.008;
        primary_bonus + str_bonus
    }

    /// Bonus max HP from vitality.
    /// Each point of vitality gives +3 max HP.
    pub fn hp_bonus(&self, attrs: &Attributes) -> f32 {
        attrs.vitality as f32 * 3.0
    }

    /// Bonus crit chance from agility (regardless of class).
    /// Each point of agility gives +0.1% crit.
    pub fn crit_bonus(&self, attrs: &Attributes) -> f32 {
        attrs.agility as f32 * 0.001
    }

    /// Bonus attack speed from agility.
    /// Each point gives +0.5% attack speed.
    pub fn attack_speed_bonus(&self, attrs: &Attributes) -> f32 {
        attrs.agility as f32 * 0.005
    }
}
