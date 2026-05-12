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
    let _ = renderer;
    // Inventory input + draw is fused into the HUD render
    // pass below (single IM pass). Here we only gate gameplay
    // input: when the cursor is inside the inventory panel,
    // skip the combat tick so a click-to-equip doesn't also
    // fire a basic attack.
    let mp = input.mouse_pos();
    let pointer_in_inventory =
        state
            .inventory_ui
            .consumes_mouse(mp.0, mp.1, state.loot.stash_session);
    // Same gate, but for the party UI (party frames, portal
    // modal, per-member confirm prompt, right-click context
    // menu) and the chat input field. Without this the
    // gameplay tick consumes `left_clicked()` /
    // `right_clicked()` in `tick_ability_keybinds` /
    // targeting tick before the UI phase ever sees them, so
    // buttons like "Enter" in the portal modal, "Kick" in
    // the context menu, and the chat field never register a
    // click.
    let pointer_in_party_ui = state.party.consumes_mouse(mp.0, mp.1);
    let pointer_in_chat = state.chat.consumes_mouse(mp.0, mp.1);
    // Combat-meter HUD (bottom-right). Same contract: when the
    // cursor is on the panel we want clicks to drive tab
    // switches / row expand-toggle, not basic attacks.
    let pointer_in_meters = state.meters.consumes_mouse(mp.0, mp.1);
    // HUD widgets (ability bar plaque, alt-hold loot labels)
    // publish their click-targets via `frame.hud_consume_rects`
    // each frame. A click landing on any of them is a UI
    // interaction (swap-slot, pickup) and must not also fire a
    // basic-attack cast.
    let pointer_in_hud = {
        let p = rift_engine::ui::im::Pos2::new(mp.0, mp.1);
        state.frame.hud_consume_rects.iter().any(|r| r.contains(p))
    };

    // Ability-based combat (sends cast requests to the server).
    // Two gates beyond the obvious "alive" check:
    //  * `is_dead` — covers the down-pose window before the
    //    server has flipped us to ghost.
    //  * `local_ghost_cached` — covers the ghost window itself.
    //    The server already rejects ghost casts, but without
    //    this gate the client still plays the cast pose / VFX
    //    locally, which is misleading. Cooldowns also tick on
    //    `try_use`, so a ghost spamming abilities would come
    //    back to life with everything on CD.
    let is_ghost = state.net.local_ghost_cached;
    let combat_blocked =
        crate::game::ghost_system::is_dead(&state.world, state.net.local_ghost_cached)
            || is_ghost
            || state.floor.in_hub
            || pointer_in_inventory
            || pointer_in_party_ui
            || pointer_in_chat
            || pointer_in_meters
            || pointer_in_hud;
    if !combat_blocked {
        crate::game::combat_system::tick(state, input, renderer, dt);
    } else if is_ghost
        && (state.frame.targeting.is_some() || state.frame.entity_targeting.is_some())
    {
        // Drop any in-flight targeting so a stale AoE indicator
        // or entity-pick cursor doesn't linger after death.
        // The indicator mesh's matrix is zeroed so it stops
        // drawing; the renderer slot is recycled lazily.
        if let Some(t) = state.frame.targeting.take() {
            if let Some(obj_idx) = t.indicator_obj {
                if obj_idx < renderer.objects.len() {
                    renderer.objects[obj_idx].model_matrix = glam::Mat4::ZERO;
                }
            }
        }
        if let Some(t) = state.frame.entity_targeting.take() {
            if let Some(obj_idx) = t.indicator_obj {
                if obj_idx < renderer.objects.len() {
                    renderer.objects[obj_idx].model_matrix = glam::Mat4::ZERO;
                }
            }
        }
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
