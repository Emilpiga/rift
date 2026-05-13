//! Per-frame `GameState::update` pipeline split into four phases:
//! gameplay → combat → render → UI. Each phase is a free
//! function `tick(state: &mut GameState, ...)` that reaches into
//! `pub(super)` fields on `GameState`. The pipeline order +
//! dispatch lives in [`crate::game::state::GameState::update`].

pub mod combat_phase;
pub mod gameplay_phase;
pub mod render_phase;
pub mod ui_phase;
