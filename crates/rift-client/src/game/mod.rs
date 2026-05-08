//! Visual / UX layer of the rift client. Owns `GameState` plus all
//! rendering-driven systems (HUD, character-select, dungeon visuals,
//! outfit attachments, prop placement, monster asset cache).
//!
//! Authoritative gameplay still lives in `rift-server`; everything
//! here is what the local player sees and interacts with.

pub mod state;
pub mod states;
pub mod ability;
pub mod cursor;
pub mod hud;
pub mod character_select;
pub mod character_spawn;
pub mod environment;
pub mod props;
pub mod floor;
pub mod torches;
pub mod monster_assets;
pub mod mp_inventory_ui;
pub mod systems;
pub mod phases;
pub mod transition;
pub mod spellbook;
pub mod chat;
pub mod meters;
pub mod party;

// Flatten the systems / phases / states hierarchies back into
// the `game` namespace so existing `crate::game::sub_state::*`,
// `super::ghost_system::...` and `super::gameplay_phase::tick`
// paths from sibling modules (state.rs, ability.rs, the binary)
// keep resolving without a crate-wide rename.
pub use systems::{combat_system, ghost_system, loot_system, portal_system, shrine_system, stash_system};
pub use phases::{combat_phase, gameplay_phase, render_phase, ui_phase};
pub use states::{sub_state, frame_state, floor_state, player_state, rift_state};

pub use player_state::PlayerState;
pub use state::GameState;
pub use sub_state::{EquipRequest, NetCastRequest, NetTransitionRequest, StashRequest};
