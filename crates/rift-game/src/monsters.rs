//! Declarative monster role data. Asset loading + GPU caches live in
//! the client crate (`rift_client::game::monster_assets`).
//!
//! Wire byte mapping is defined here as the single source of truth;
//! both the server (when authoring snapshots) and the client (when
//! decoding them) agree on this enum.

use serde::{Deserialize, Serialize};

/// Logical role of a monster slot. Maps to a specific glTF + size.
/// Stable wire-byte ordering: never reorder, only append.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MonsterRole {
    Brute,
    Stalker,
    Caster,
    Elite,
    Boss,
    Wraith,
    Mindbinder,
}

/// Spawn-time stat multipliers applied on top of
/// [`crate::FloorConfig::enemy_speed`] / `enemy_health`. Picked
/// once per spawn; elite affixes (JUGGERNAUT/SWIFT) layer on
/// top of these in a second multiplicative pass.
#[derive(Clone, Copy, Debug)]
pub struct RoleStats {
    /// Multiplier on `FloorConfig::enemy_speed`.
    pub speed_mult: f32,
    /// Multiplier on `FloorConfig::enemy_health`.
    pub hp_mult: f32,
}

/// One row of monster-role metadata. Adding a new role should
/// primarily be one append here plus its server AI module.
#[derive(Clone, Copy, Debug)]
pub struct MonsterDef {
    pub role: MonsterRole,
    pub wire: u8,
    pub display_name: &'static str,
    pub gltf_path: &'static str,
    pub scale: f32,
    pub hit_radius: f32,
    pub stats: RoleStats,
}

pub const MONSTER_DEFS: [MonsterDef; 7] = [
    MonsterDef {
        role: MonsterRole::Brute,
        wire: 0,
        display_name: "Brute",
        gltf_path: "assets/models/animated-monsters/glTF/GreenDemon.gltf",
        scale: 0.60,
        hit_radius: 0.50,
        stats: RoleStats {
            speed_mult: 0.85,
            hp_mult: 1.15,
        },
    },
    MonsterDef {
        role: MonsterRole::Stalker,
        wire: 1,
        display_name: "Stalker",
        gltf_path: "assets/models/animated-monsters/glTF/Skull.gltf",
        scale: 0.55,
        hit_radius: 0.45,
        stats: RoleStats {
            speed_mult: 1.35,
            hp_mult: 0.75,
        },
    },
    MonsterDef {
        role: MonsterRole::Caster,
        wire: 2,
        display_name: "Caster",
        gltf_path: "assets/models/animated-monsters/glTF/Cyclops.gltf",
        scale: 0.60,
        hit_radius: 0.50,
        stats: RoleStats {
            speed_mult: 0.95,
            hp_mult: 0.65,
        },
    },
    MonsterDef {
        role: MonsterRole::Elite,
        wire: 3,
        display_name: "Elite",
        gltf_path: "assets/models/animated-monsters/glTF/Demon.gltf",
        scale: 0.85,
        hit_radius: 0.70,
        stats: RoleStats {
            speed_mult: 0.80,
            hp_mult: 1.00,
        },
    },
    MonsterDef {
        role: MonsterRole::Boss,
        wire: 4,
        display_name: "Boss",
        gltf_path: "assets/models/animated-monsters/glTF/Yeti.gltf",
        scale: 1.60,
        hit_radius: 1.05,
        stats: RoleStats {
            speed_mult: 1.00,
            hp_mult: 1.00,
        },
    },
    MonsterDef {
        role: MonsterRole::Wraith,
        wire: 5,
        display_name: "Wraith",
        gltf_path: "assets/models/animated-monsters/glTF/Ghost.gltf",
        scale: 0.72,
        hit_radius: 0.45,
        stats: RoleStats {
            speed_mult: 1.12,
            hp_mult: 0.70,
        },
    },
    MonsterDef {
        role: MonsterRole::Mindbinder,
        wire: 6,
        display_name: "Mindbinder",
        gltf_path: "assets/models/animated-monsters/glTF/Cthulhu.gltf",
        scale: 0.68,
        hit_radius: 0.55,
        stats: RoleStats {
            speed_mult: 0.82,
            hp_mult: 0.90,
        },
    },
];

impl MonsterRole {
    pub fn def(self) -> &'static MonsterDef {
        MONSTER_DEFS
            .iter()
            .find(|def| def.role == self)
            .expect("MonsterRole missing definition")
    }

    /// Path to the monster's glTF in the animated-monsters pack. Each
    /// model is hand-picked to match the role's silhouette + animation
    /// set (only ground monsters with full Walk/Idle/Bite/Death/Hit).
    pub fn gltf_path(self) -> &'static str {
        self.def().gltf_path
    }

    /// Visual scale multiplier. The monster pack is authored at roughly
    /// 1m height; the player is ~1.8m. Regulars are deliberately small
    /// (~half player height) so dense packs still read as a swarm
    /// rather than a wall of bodies. Elite + boss are the only sizes
    /// that visually outclass the player.
    pub fn scale(self) -> f32 {
        self.def().scale
    }

    /// Authoritative XZ hit-disc radius used by server collision.
    /// Values intentionally track visual footprint rather than raw
    /// mesh height: regular monsters stay compact, while elite and
    /// boss silhouettes get the broader body players can see.
    pub fn hit_radius(self) -> f32 {
        self.def().hit_radius
    }

    /// Encode the role as the byte we send on the wire.
    pub fn to_wire_byte(self) -> u8 {
        self.def().wire
    }

    /// Decode a wire byte back into a role. Returns `None` for unknown
    /// bytes so a future server can introduce new roles without
    /// crashing old clients.
    pub fn from_wire_byte(b: u8) -> Option<Self> {
        MONSTER_DEFS
            .iter()
            .find(|def| def.wire == b)
            .map(|def| def.role)
    }

    /// Human-readable name for HUD display (combat meter
    /// breakdown rows, etc.). Kept short so it fits in the
    /// meter column without truncation.
    pub fn display_name(self) -> &'static str {
        self.def().display_name
    }
}

pub const ALL_ROLES: [MonsterRole; MONSTER_DEFS.len()] = [
    MonsterRole::Brute,
    MonsterRole::Stalker,
    MonsterRole::Caster,
    MonsterRole::Elite,
    MonsterRole::Boss,
    MonsterRole::Wraith,
    MonsterRole::Mindbinder,
];

impl MonsterRole {
    /// Spawn-time stat multipliers. See [`RoleStats`].
    pub fn stats(self) -> RoleStats {
        self.def().stats
    }
}
