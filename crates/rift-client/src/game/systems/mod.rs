//! Per-frame subsystem ticks: portals, ground loot, stash chest,
//! revive shrines, ability combat, and the death/ghost lifecycle.
//! Each module exposes a `tick(...)` free function that takes
//! the slice of `GameState` it actually needs (rather than the
//! whole struct), and is composed by the [`crate::game::phases`]
//! pipeline.

pub mod combat_system;
pub mod ghost_system;
pub mod loot_system;
pub mod portal_system;
pub mod shrine_system;
pub mod stash_system;
