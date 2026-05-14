//! `rift-game` — declarative gameplay data crate.
//!
//! "rift-game declares how the game should look and feel." This crate
//! owns the data types every other layer interprets:
//! - kinematic player movement integrator (`kinematic`)
//! - ability definitions, runtime effects (data only), and rosters
//!   (`abilities`)
//! - hero base stats + avatar (`hero`)
//! - talent trees (`talents`)
//! - character profiles + gender (`character`)
//! - monster role + wire byte mapping (`monsters`)
//! - attribute / experience / level-up (`attributes`, `experience`)
//! - ECS components owned by gameplay (`components` — `PlayerAction`)
//!
//! It depends only on `glam`, `hecs`, `rift-dungeon`, `serde`. It must
//! never depend on `rift-engine` (rendering) or `rift-net` (wire).

pub mod abilities;
pub mod attributes;
pub mod character;
pub mod components;
pub mod effects;
pub mod experience;
pub mod hero;
pub mod kinematic;
pub mod loadout;
pub mod loot;
pub mod minions;
pub mod monsters;
pub mod stats;
pub mod talents;
