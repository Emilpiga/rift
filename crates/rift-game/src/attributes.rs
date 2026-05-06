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
