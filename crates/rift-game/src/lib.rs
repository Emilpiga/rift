//! `rift-game` — declarative gameplay data crate.
//!
//! "rift-game declares how the game should look and feel." This crate
//! owns the data types every other layer interprets:
//! - kinematic player movement integrator (`kinematic`)
//! - ability definitions, runtime effects (data only), and rosters
//!   (`abilities`)
//! - class roster + base stats (`classes`)
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
pub mod classes;
pub mod components;
pub mod debuffs;
pub mod experience;
pub mod kinematic;
pub mod loot;
pub mod monsters;
pub mod stats;
pub mod talents;
