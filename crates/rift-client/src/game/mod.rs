//! Visual / UX layer of the rift client. Owns `GameState` plus all
//! rendering-driven systems (HUD, character-select, dungeon visuals,
//! outfit attachments, prop placement, monster asset cache).
//!
//! Authoritative gameplay still lives in `rift-server`; everything
//! here is what the local player sees and interacts with.

pub mod state;
pub mod hud;
pub mod character_select;
pub mod character_spawn;
pub mod equipment_visuals;
pub mod environment;
pub mod props;
pub mod floor;
pub mod rift_state;
pub mod monster_assets;
pub mod mp_inventory_ui;

pub use state::{GameState, NetCastRequest, NetTransitionRequest, PlayerState};
