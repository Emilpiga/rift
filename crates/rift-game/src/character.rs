//! Character roster — in-memory list of created characters and their
//! profile data (name, gender, level). Persistence is deferred
//! until multiplayer/DB work begins; for now everything lives in this
//! struct for the lifetime of the process.

/// Hard cap on simultaneous character slots in the roster.
pub const MAX_CHARACTERS: usize = 5;

/// Stable wire-byte mapping for `Gender`. Decoupled from rift-net so
/// the conversion glue lives wherever the wire type is decoded.
pub mod gender_byte {
    pub const MALE: u8 = 0;
    pub const FEMALE: u8 = 1;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Gender {
    Male,
    Female,
}

impl Gender {
    pub fn label(self) -> &'static str {
        match self {
            Gender::Male => "Male",
            Gender::Female => "Female",
        }
    }

    /// Encode to the stable wire byte. Pair with [`Gender::from_wire_byte`].
    pub fn to_wire_byte(self) -> u8 {
        match self {
            Gender::Male => gender_byte::MALE,
            Gender::Female => gender_byte::FEMALE,
        }
    }

    /// Decode from a wire byte. Returns `None` for unknown bytes so
    /// future variants can be added without crashing old peers.
    pub fn from_wire_byte(b: u8) -> Option<Self> {
        Some(match b {
            gender_byte::MALE => Gender::Male,
            gender_byte::FEMALE => Gender::Female,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CharacterAppearance {
    pub skin_tone: u8,
    pub hair_style: u8,
    pub eyebrow_style: u8,
    pub hair_color: u8,
    pub eyebrow_color: u8,
    pub chest_size: u8,
}

impl Default for CharacterAppearance {
    fn default() -> Self {
        Self {
            skin_tone: 0,
            hair_style: 0,
            eyebrow_style: 0,
            hair_color: 16,
            eyebrow_color: 16,
            chest_size: 128,
        }
    }
}

/// Persistent (well, soon-to-be-persistent) per-character profile.
/// PlayerState is rebuilt from this each time the character is loaded.
#[derive(Clone, Debug)]
pub struct CharacterProfile {
    pub name: String,
    pub gender: Gender,
    pub appearance: CharacterAppearance,
    pub level: u32,
    /// Indices into `crate::loot::BASE_ITEMS` for the items
    /// this character currently has equipped, as advertised by
    /// the server in `RosterEntry`. Used by the character-select
    /// preview to dress the avatar before the player connects
    /// as that character. Empty for fresh characters and SP.
    pub equipped_base_ids: Vec<u16>,
}

impl CharacterProfile {
    pub fn new(name: String, gender: Gender) -> Self {
        Self {
            name,
            gender,
            appearance: CharacterAppearance::default(),
            level: 1,
            equipped_base_ids: Vec::new(),
        }
    }
}

/// In-memory list of characters. Add / remove / pick.
#[derive(Default)]
pub struct CharacterRoster {
    slots: Vec<CharacterProfile>,
}

impl CharacterRoster {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn slots(&self) -> &[CharacterProfile] {
        &self.slots
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_full(&self) -> bool {
        self.slots.len() >= MAX_CHARACTERS
    }

    pub fn get(&self, idx: usize) -> Option<&CharacterProfile> {
        self.slots.get(idx)
    }

    pub fn add(&mut self, profile: CharacterProfile) -> Option<usize> {
        if self.is_full() {
            return None;
        }
        self.slots.push(profile);
        Some(self.slots.len() - 1)
    }

    pub fn remove(&mut self, idx: usize) {
        if idx < self.slots.len() {
            self.slots.remove(idx);
        }
    }
}
