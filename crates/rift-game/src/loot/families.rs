//! Item damage-family lock — Attribute × Element × Archetype.
//!
//! Phase 1 of the §2 itemisation refactor. See `ITEMS.md` §2.1 — every
//! item declares (at most) one **Attribute** (Strength / Agility /
//! Intellect), one or more permitted **Element** picks, and one or
//! more permitted **Archetype** picks. Phase 2's affix-roll code
//! restricts axis-line candidates to the base's family; cross-family
//! rolls become impossible rather than merely unlikely.
//!
//! These enums intentionally mirror — but do not re-use —
//! [`crate::abilities::Element`] and [`crate::abilities::Archetype`].
//! The ability versions carry `None` / `Utility` / `Movement` variants
//! that are meaningful for *casting* but meaningless for *gear* (an
//! item that drops with "no element" is just a non-elemental item;
//! same for archetype). Keeping the item vocabulary minimal pays for
//! itself the first time you write a `match` over it.

/// Which core attribute an item's identity is bound to. Weapons and
/// heavy armor commit to one; accessories and light armor stay
/// wildcard. Mirrors three of the four
/// [`crate::attributes::AttributeType`] variants — `Vitality` isn't
/// a class identity in this game, so no item locks to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Attribute {
    Strength,
    Agility,
    Intellect,
}

impl Attribute {
    pub fn name(self) -> &'static str {
        match self {
            Attribute::Strength => "Strength",
            Attribute::Agility => "Agility",
            Attribute::Intellect => "Intellect",
        }
    }
}

/// Damage element an item can roll for. Mirrors the four elements
/// that carry damage in [`crate::abilities::Element`] — utility /
/// movement abilities don't need representation here because items
/// are gear, not abilities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Element {
    Physical,
    Fire,
    Ice,
    Lightning,
}

impl Element {
    pub fn name(self) -> &'static str {
        match self {
            Element::Physical => "Physical",
            Element::Fire => "Fire",
            Element::Ice => "Ice",
            Element::Lightning => "Lightning",
        }
    }
}

/// Ability shape an item can roll archetype scaling for.
///
/// Intentionally narrow: only `Projectile` and `Melee` carry
/// dedicated archetype-damage affixes. Beam and AoE are covered
/// by their element axis (a Frost Ray scales off Ice; a Whirlwind
/// off Physical) and don't need an extra trio line of their own.
/// Kept basic; expand if a future archetype actually pulls its
/// weight on the item axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Archetype {
    Projectile,
    Melee,
}

impl Archetype {
    pub fn name(self) -> &'static str {
        match self {
            Archetype::Projectile => "Projectile",
            Archetype::Melee => "Melee",
        }
    }
}

/// Declared damage-axis lock for a base item. Phase 2's
/// [`crate::loot::Item::roll`] reads this to decide which axis lines
/// the item is *allowed* to roll.
///
/// - `None` on an axis means **wildcard** — the item has no lock on
///   that axis, and any roll from that axis pool is fair game. Used
///   by accessories (rings / amulets) and by armor pieces that
///   don't commit to an archetype.
/// - `Some(&[…])` is the **allowed set** for that axis. A roll
///   targeting an entry outside the set is filtered out.
///
/// `Attribute` is a scalar `Option<Attribute>` rather than a slice
/// because no realistic item commits to "either Strength or
/// Intellect" — that's what wildcard (`None`) is for.
#[derive(Clone, Copy, Debug)]
pub struct BaseFamily {
    pub attribute: Option<Attribute>,
    pub element: Option<&'static [Element]>,
    pub archetype: Option<&'static [Archetype]>,
}

impl BaseFamily {
    /// Wildcard family — accessories, anything that should roll
    /// from the full pool on every axis.
    pub const WILDCARD: BaseFamily = BaseFamily {
        attribute: None,
        element: None,
        archetype: None,
    };

    /// Convenience builder for an armor base that commits to a
    /// single attribute but stays open on the other axes.
    pub const fn attribute_only(a: Attribute) -> BaseFamily {
        BaseFamily {
            attribute: Some(a),
            element: None,
            archetype: None,
        }
    }

    /// `true` if `e` is permitted by this family's element lock.
    /// Wildcard (`None`) permits everything.
    pub fn allows_element(&self, e: Element) -> bool {
        match self.element {
            None => true,
            Some(list) => list.contains(&e),
        }
    }

    /// `true` if `a` is permitted by this family's archetype lock.
    pub fn allows_archetype(&self, a: Archetype) -> bool {
        match self.archetype {
            None => true,
            Some(list) => list.contains(&a),
        }
    }

    /// `true` if `a` is the family's attribute (or the family is
    /// attribute-wildcard).
    pub fn allows_attribute(&self, a: Attribute) -> bool {
        match self.attribute {
            None => true,
            Some(self_a) => self_a == a,
        }
    }
}

pub const ATTRIBUTES_ALL: &[Attribute] = &[
    Attribute::Strength,
    Attribute::Agility,
    Attribute::Intellect,
];

// Reusable element slice constants. Putting them here keeps the
// `BASE_ITEMS` table tidy and means a future "add an element"
// change is a single edit.
pub const ELEMENTS_ALL: &[Element] = &[
    Element::Physical,
    Element::Fire,
    Element::Ice,
    Element::Lightning,
];
pub const ELEMENTS_CASTER: &[Element] = &[Element::Fire, Element::Ice, Element::Lightning];
pub const ELEMENTS_PHYSICAL: &[Element] = &[Element::Physical];

pub const ARCHETYPES_ALL: &[Archetype] = &[Archetype::Projectile, Archetype::Melee];
pub const ARCHETYPES_MELEE: &[Archetype] = &[Archetype::Melee];
pub const ARCHETYPES_PROJECTILE: &[Archetype] = &[Archetype::Projectile];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_allows_everything() {
        let f = BaseFamily::WILDCARD;
        for a in ATTRIBUTES_ALL {
            assert!(f.allows_attribute(*a));
        }
        for e in ELEMENTS_ALL {
            assert!(f.allows_element(*e));
        }
        for a in ARCHETYPES_ALL {
            assert!(f.allows_archetype(*a));
        }
    }

    #[test]
    fn attribute_only_locks_attribute_but_not_other_axes() {
        let f = BaseFamily::attribute_only(Attribute::Strength);
        assert!(f.allows_attribute(Attribute::Strength));
        assert!(!f.allows_attribute(Attribute::Agility));
        assert!(!f.allows_attribute(Attribute::Intellect));
        assert!(f.allows_element(Element::Fire));
        assert!(f.allows_archetype(Archetype::Melee));
    }

    #[test]
    fn element_list_filters_correctly() {
        let f = BaseFamily {
            attribute: Some(Attribute::Intellect),
            element: Some(ELEMENTS_CASTER),
            archetype: None,
        };
        assert!(f.allows_element(Element::Fire));
        assert!(f.allows_element(Element::Ice));
        assert!(f.allows_element(Element::Lightning));
        assert!(!f.allows_element(Element::Physical));
    }
}
