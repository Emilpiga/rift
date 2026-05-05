//! Character roster — in-memory list of created characters and their
//! profile data (name, gender, class, level). Persistence is deferred
//! until multiplayer/DB work begins; for now everything lives in this
//! struct for the lifetime of the process.

use rift_engine::combat::ClassId;

/// Hard cap on simultaneous character slots in the roster.
pub const MAX_CHARACTERS: usize = 5;

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
}

/// Persistent (well, soon-to-be-persistent) per-character profile.
/// PlayerState is rebuilt from this each time the character is loaded.
#[derive(Clone, Debug)]
pub struct CharacterProfile {
    pub name: String,
    pub gender: Gender,
    pub class: ClassId,
    pub level: u32,
}

impl CharacterProfile {
    pub fn new(name: String, gender: Gender, class: ClassId) -> Self {
        Self {
            name,
            gender,
            class,
            level: 1,
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
