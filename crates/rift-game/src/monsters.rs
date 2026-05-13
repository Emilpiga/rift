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
}

impl MonsterRole {
    /// Path to the monster's glTF in the animated-monsters pack. Each
    /// model is hand-picked to match the role's silhouette + animation
    /// set (only ground monsters with full Walk/Idle/Bite/Death/Hit).
    pub fn gltf_path(self) -> &'static str {
        match self {
            MonsterRole::Brute => "assets/models/animated-monsters/glTF/GreenDemon.gltf",
            MonsterRole::Stalker => "assets/models/animated-monsters/glTF/Skull.gltf",
            MonsterRole::Caster => "assets/models/animated-monsters/glTF/Cyclops.gltf",
            MonsterRole::Elite => "assets/models/animated-monsters/glTF/Demon.gltf",
            MonsterRole::Boss => "assets/models/animated-monsters/glTF/Yeti.gltf",
        }
    }

    /// Visual scale multiplier. The monster pack is authored at roughly
    /// 1m height; the player is ~1.8m. Regulars are deliberately small
    /// (~half player height) so dense packs still read as a swarm
    /// rather than a wall of bodies. Elite + boss are the only sizes
    /// that visually outclass the player.
    pub fn scale(self) -> f32 {
        match self {
            MonsterRole::Brute => 0.60,
            MonsterRole::Stalker => 0.55,
            MonsterRole::Caster => 0.60,
            MonsterRole::Elite => 0.85,
            MonsterRole::Boss => 1.60,
        }
    }

    /// Authoritative XZ hit-disc radius used by server collision.
    /// Values intentionally track visual footprint rather than raw
    /// mesh height: regular monsters stay compact, while elite and
    /// boss silhouettes get the broader body players can see.
    pub fn hit_radius(self) -> f32 {
        match self {
            MonsterRole::Brute => 0.50,
            MonsterRole::Stalker => 0.45,
            MonsterRole::Caster => 0.50,
            MonsterRole::Elite => 0.70,
            MonsterRole::Boss => 1.05,
        }
    }

    /// Encode the role as the byte we send on the wire.
    pub fn to_wire_byte(self) -> u8 {
        match self {
            MonsterRole::Brute => 0,
            MonsterRole::Stalker => 1,
            MonsterRole::Caster => 2,
            MonsterRole::Elite => 3,
            MonsterRole::Boss => 4,
        }
    }

    /// Decode a wire byte back into a role. Returns `None` for unknown
    /// bytes so a future server can introduce new roles without
    /// crashing old clients.
    pub fn from_wire_byte(b: u8) -> Option<Self> {
        Some(match b {
            0 => MonsterRole::Brute,
            1 => MonsterRole::Stalker,
            2 => MonsterRole::Caster,
            3 => MonsterRole::Elite,
            4 => MonsterRole::Boss,
            _ => return None,
        })
    }

    /// Human-readable name for HUD display (combat meter
    /// breakdown rows, etc.). Kept short so it fits in the
    /// meter column without truncation.
    pub fn display_name(self) -> &'static str {
        match self {
            MonsterRole::Brute => "Brute",
            MonsterRole::Stalker => "Stalker",
            MonsterRole::Caster => "Caster",
            MonsterRole::Elite => "Elite",
            MonsterRole::Boss => "Boss",
        }
    }
}

pub const ALL_ROLES: [MonsterRole; 5] = [
    MonsterRole::Brute,
    MonsterRole::Stalker,
    MonsterRole::Caster,
    MonsterRole::Elite,
    MonsterRole::Boss,
];

/// Spawn-time stat multipliers applied on top of
/// [`crate::FloorConfig::enemy_speed`] / `enemy_health`. Picked
/// once per spawn; elite affixes (JUGGERNAUT/SWIFT) layer on
/// top of these in a second multiplicative pass.
///
/// Centralised here so adding a new role is a single match arm
/// in [`MonsterRole::stats`] instead of two parallel match
/// arms across `spawn_summon` and `spawn_for_floor`.
#[derive(Clone, Copy, Debug)]
pub struct RoleStats {
    /// Multiplier on `FloorConfig::enemy_speed`. Reflects the
    /// role's footprint: brutes are slow, stalkers fast,
    /// casters average. Elites and bosses are tuned per-fight
    /// and so use 1.0 here (their HP / speed scaling lives
    /// in the floor config's elite block).
    pub speed_mult: f32,
    /// Multiplier on `FloorConfig::enemy_health`. Inverse of
    /// damage profile — squishy stalkers / casters compensate
    /// with mobility / range, brutes soak hits.
    pub hp_mult: f32,
}

impl MonsterRole {
    /// Spawn-time stat multipliers. See [`RoleStats`].
    pub fn stats(self) -> RoleStats {
        // Numbers preserved verbatim from the legacy per-role
        // match arms in `spawn_for_floor` / `spawn_summon`;
        // only the indirection moved.
        match self {
            MonsterRole::Brute => RoleStats {
                speed_mult: 0.85,
                hp_mult: 1.15,
            },
            MonsterRole::Stalker => RoleStats {
                speed_mult: 1.35,
                hp_mult: 0.75,
            },
            MonsterRole::Caster => RoleStats {
                speed_mult: 0.95,
                hp_mult: 0.65,
            },
            // Elite hp comes from `cfg.elite_hp_mult`, speed
            // from a separate 0.8× multiplier — stats here are
            // the neutral "no extra adjustment" fallback so
            // callers can layer those on uniformly.
            MonsterRole::Elite => RoleStats {
                speed_mult: 0.80,
                hp_mult: 1.00,
            },
            MonsterRole::Boss => RoleStats {
                speed_mult: 1.00,
                hp_mult: 1.00,
            },
        }
    }
}
