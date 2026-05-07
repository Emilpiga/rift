//! Visual / UX layer of the rift client. Owns `GameState` plus all
//! rendering-driven systems (HUD, character-select, dungeon visuals,
//! outfit attachments, prop placement, monster asset cache).
//!
//! Authoritative gameplay still lives in `rift-server`; everything
//! here is what the local player sees and interacts with.

pub mod state;
pub mod sub_state;
pub mod ability;
pub mod cursor;
pub mod player_state;
pub mod hud;
pub mod character_select;
pub mod character_spawn;
pub mod environment;
pub mod props;
pub mod floor;
pub mod torches;
pub mod rift_state;
pub mod monster_assets;
pub mod mp_inventory_ui;
pub mod loot_system;
pub mod portal_system;
pub mod stash_system;
pub mod shrine_system;
pub mod spellbook;

pub use player_state::PlayerState;
pub use state::GameState;
pub use sub_state::{EquipRequest, NetCastRequest, NetTransitionRequest, StashRequest};
