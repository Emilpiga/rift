//! Phase 2 of the per-frame `GameState::update` pipeline.
//!
//! Ability casting, death detection, ghost rise/clear, and the
//! hit-react one-shot. Heavy combat input (placed-AoE targeting,
//! channel hold, keybind dispatch, cast send) lives in
//! `combat_system::tick`; this phase wraps it with the death/ghost
//! lifecycle edges that need cross-frame state from `GameState`.

use rift_engine::{Input, Renderer};

use crate::game::state::GameState;

pub fn tick(state: &mut GameState, renderer: &mut Renderer, input: &Input, _dt: f32) {
    let dt = _dt;
    let (sw, sh) = renderer.screen_size();
    // Inventory input + draw is fused into the HUD render
    // pass below (single IM pass). Here we only gate gameplay
    // input: when the cursor is inside the inventory panel,
    // skip the combat tick so a click-to-equip doesn't also
    // fire a basic attack.
    let mp = input.mouse_pos();
    let pointer_in_inventory = state.mp_inventory_ui.consumes_mouse(mp.0, mp.1, sw, sh);

    // Ability-based combat (sends cast requests to the server).
    if !crate::game::ghost_system::is_dead(&state.world, state.net.local_ghost_cached)
        && !state.floor.in_hub
        && !pointer_in_inventory
    {
        crate::game::combat_system::tick(state, input, renderer, dt);
    }

    // Catch-all death detection: alive last frame, dead this
    // frame.
    let was_alive = state.frame.prev_player_hp.map_or(false, |p| p > 0.001);
    let is_dead = crate::game::ghost_system::is_dead(&state.world, state.net.local_ghost_cached);
    if was_alive && is_dead {
        crate::game::ghost_system::trigger_death(
            &mut state.world,
            &mut state.frame.damage_flash,
            state.rift.floor as u32,
        );
    }

    // Edge-detect the down-pose → ghost transition.
    let now_ghost = state.net.local_ghost_cached;
    if now_ghost && !state.frame.prev_local_ghost {
        crate::game::ghost_system::trigger_rise(&mut state.world, state.rift.floor as u32);
    }
    crate::game::ghost_system::tick_rise(&mut state.world, dt);
    if !now_ghost && state.frame.prev_local_ghost {
        crate::game::ghost_system::clear_markers(&mut state.world);
    }
    state.frame.prev_local_ghost = now_ghost;

    // Hit-react: detect a damage event on the local player and
    // play a one-shot reaction clip on the upper body.
    if !is_dead {
        crate::game::ghost_system::tick_hit_react(
            &mut state.world,
            &mut state.frame.prev_player_hp,
            state.rift.floor as u32,
        );
    } else {
        // Keep `prev_player_hp` pinned to the dying value so
        // the alive→dead edge above stays one-shot.
        state.frame.prev_player_hp = Some(0.0);
    }
}
