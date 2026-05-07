//! Item rarity tier + per-rarity affix-count rules.
//!
//! Rarity is the single knob that controls how many affixes a drop
//! carries **and** which kinds of affixes it can roll (via
//! [`crate::loot::AffixDef::rarity_min`]).
//!
//! The intent isn't "Legendary = bigger numbers" — it's
//! "Legendary unlocks **patterns** Common/Magic/Rare can't roll":
//!
//! - **Common** — pure stats. Building blocks.
//! - **Magic** — stats with a synergistic clustering bias.
//! - **Rare** — stats + ability *amplifiers* (`+25 % Fireball damage`).
//! - **Legendary** — stats + ability *modifiers* (`Fireball gains +2
//!   projectiles`), *transforms* (`Fireball becomes a beam`), and
//!   *triggers* (`On crit: cast a mini fireball`).

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Rarity {
    Common = 0,
    Magic = 1,
    Rare = 2,
    Legendary = 3,
}

impl Rarity {
    /// Number of *bonus* affixes rolled at this rarity. Bonus
    /// affixes come on top of the deterministic per-slot
    /// signature (Vitality + slot-specific guaranteed lines)
    /// that every item gets regardless of rarity. Legendary
    /// additionally gets one effect affix from the legendary
    /// pool — see `Item::roll`.
    pub fn affix_count_range(self) -> (u32, u32) {
        match self {
            Rarity::Common => (0, 0),
            Rarity::Magic => (1, 1),
            Rarity::Rare => (2, 2),
            Rarity::Legendary => (3, 3),
        }
    }

    /// Tooltip / nameplate colour (sRGB 0..1).
    pub fn color(self) -> [f32; 3] {
        match self {
            Rarity::Common => [0.85, 0.85, 0.85],
            Rarity::Magic => [0.40, 0.65, 1.00],
            Rarity::Rare => [1.00, 0.85, 0.30],
            Rarity::Legendary => [1.00, 0.45, 0.10],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Rarity::Common => "Common",
            Rarity::Magic => "Magic",
            Rarity::Rare => "Rare",
            Rarity::Legendary => "Legendary",
        }
    }

    /// `true` if `self >= other`. Used to gate affixes by rarity_min.
    pub fn at_least(self, other: Rarity) -> bool {
        (self as u8) >= (other as u8)
    }
}
