//! Environment-themed presets — world objects and
//! non-combat effects: portals, loot pillars, shrines,
//! ambient props.

pub mod ambient;
pub mod loot;
pub mod player;
pub mod portal;
pub mod shrine;

pub use ambient::*;
pub use loot::*;
pub use player::*;
pub use portal::*;
pub use shrine::*;
