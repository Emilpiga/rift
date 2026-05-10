//! Visual / UX layer of the rift client. Owns `GameState` plus all
//! rendering-driven systems (HUD, character-select, dungeon visuals,
//! outfit attachments, prop placement, monster asset cache).
//!
//! Authoritative gameplay still lives in `rift-server`; everything
//! here is what the local player sees and interacts with.

pub mod ability;
pub mod audio;
pub mod audio_banks;
pub mod avatar_cosmetics;
pub mod character_select;
pub mod character_spawn;
pub mod chat;
pub mod cursor;
pub mod environment;
pub mod equipment_visuals;
pub mod floor;
pub mod hud;
pub mod inventory;
pub mod loot_models;
pub mod meters;
pub mod monster_assets;
pub mod party;
pub mod phases;
pub mod props;
pub mod spellbook;
pub mod state;
pub mod states;
pub mod systems;
pub mod torches;
pub mod transition;

// Flatten the systems / phases / states hierarchies back into
// the `game` namespace so existing `crate::game::sub_state::*`,
// `super::ghost_system::...` and `super::gameplay_phase::tick`
// paths from sibling modules (state.rs, ability.rs, the binary)
// keep resolving without a crate-wide rename.
pub use phases::{combat_phase, gameplay_phase, render_phase, ui_phase};
pub use states::{floor_state, frame_state, player_state, rift_state, sub_state};
pub use systems::{
    combat_system, ghost_system, loot_system, portal_system, shrine_system, stash_system,
};

pub use player_state::PlayerState;
pub use state::GameState;
pub use sub_state::{EquipRequest, NetCastRequest, NetTransitionRequest, StashRequest};
