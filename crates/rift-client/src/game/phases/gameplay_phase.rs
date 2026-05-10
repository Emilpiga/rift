//! Phase 1 of the per-frame `GameState::update` pipeline.
//!
//! Reads input, advances per-frame interaction ticks (portals,
//! stash chest, ground loot, revive shrines), then runs the
//! movement / collision ECS pipeline. Authoritative gameplay
//! still lives server-side; this phase exists to surface
//! prompts, queue cast / transition requests, and step the
//! local prediction.

use rift_engine::ecs::systems::{
    collision_system, movement_system, player_action_post_system, player_action_pre_system,
    player_input_system, PlayerActionConfig,
};
use rift_engine::{Input, Renderer};

use crate::game::portal_system;
use crate::game::state::GameState;

pub fn tick(state: &mut GameState, renderer: &mut Renderer, input: &Input, dt: f32) {
    state.rift.timer += if state.floor.in_hub { 0.0 } else { dt };

    // Hub portal: spin the mesh and watch for the local player
    // walking up + pressing F to start a rift run.
    portal_system::tick_hub(
        &mut state.floor.hub_portal,
        &state.world,
        renderer,
        input,
        &mut state.net,
        &mut state.frame.hud_prompt,
        dt,
    );

    // Exit (descend) portal: appears in the dedicated portal
    // room after the boss dies; F-press advances to the next
    // rift floor (server opens a descend ready-check vote in
    // multiplayer). Anchor is the *left* of the two
    // pre-baked portal-room slots, falling back to the boss
    // room centre on synthetic floors that have no portal
    // room (the hub never reaches this branch because
    // `in_hub` short-circuits inside the tick).
    let (vote_active, vote_cd) = state
        .exit_vote
        .as_ref()
        .map(|v| (v.active, v.cooldown_remaining))
        .unwrap_or((false, 0.0));
    let (descend_anchor, return_anchor) = state
        .floor_mgr
        .portal_anchors
        .unwrap_or((state.floor_mgr.boss_room_center, state.floor_mgr.boss_room_center));
    portal_system::tick_exit(
        &mut state.floor.exit_portal,
        &state.world,
        renderer,
        input,
        &mut state.net,
        &mut state.frame.hud_prompt,
        &mut state.frame.descend_prompt,
        state.rift.floor_complete,
        state.floor.in_hub,
        descend_anchor,
        vote_active,
        vote_cd,
        dt,
    );

    // Rift exit portal (return-to-hub). Lazily spawned in
    // the dedicated portal room next to the descend portal
    // — the player walks one corridor from the boss kill
    // and picks "leave with loot" vs "push deeper" there.
    // No longer spawned at the floor's spawn point: once
    // you commit to a floor you must clear its boss to get
    // home.
    let is_ghost = state.net.local_ghost_cached;
    portal_system::tick_rift_spawn(
        &mut state.floor.rift_spawn_portal,
        &state.world,
        renderer,
        input,
        &mut state.net,
        &mut state.frame.hud_prompt,
        state.rift.floor_complete,
        state.floor.in_hub,
        return_anchor,
        vote_active,
        vote_cd,
        is_ghost,
        dt,
    );

    // Exit-vote Y/N keys: only act when a vote is active and
    // the local player is still Pending. The actual cast is
    // queued onto `NetState` and shipped by the binary as
    // `ClientMsg::RiftExitVoteCast`.
    if vote_active {
        use winit::keyboard::KeyCode;
        let our_id = state.net.our_net_id_cached;
        let we_pending = state
            .exit_vote
            .as_ref()
            .and_then(|v| {
                our_id.and_then(|nid| {
                    v.voters
                        .iter()
                        .find(|(id, _)| *id == nid)
                        .map(|(_, c)| *c)
                })
            })
            .map(|c| matches!(c, rift_net::messages::VoteChoice::Pending))
            .unwrap_or(false);
        if we_pending {
            if input.key_just_pressed(KeyCode::KeyY) {
                state.net.pending_exit_vote_casts.push(true);
            }
            if input.key_just_pressed(KeyCode::KeyN) {
                state.net.pending_exit_vote_casts.push(false);
            }
        }
    }

    // Hub stash chest: F-press toggles the stash panel
    // (queues `OpenStash` / `CloseStash` for the server,
    // forces the inventory UI open, and swaps bag-click
    // semantics from equip to deposit).
    crate::game::stash_system::tick(
        &state.world,
        &state.floor_mgr,
        input,
        &mut state.mp_inventory_ui,
        &mut state.net,
        &mut state.loot,
        &mut state.frame.hud_prompt,
    );

    // Ground loot: hover prompt + F-to-pick.
    crate::game::loot_system::tick(
        &state.world,
        &mut state.loot,
        &mut state.combat_text,
        input,
    );

    // Drive the per-drop pop / settle animation on any 3D
    // ground meshes attached to live loot drops. Cheap walk
    // over a small `Vec`; safe on empty.
    crate::game::loot_system::tick_drop_animation(&mut state.loot, renderer, dt);

    // Revive shrines: hover prompt + F-press toggles channel
    // intent. Server is authority on actual progress; we
    // surface the prompt + queue the toggle here.
    let local_ghost = state.net.local_ghost_cached;
    crate::game::shrine_system::tick(
        &mut state.shrines,
        &state.world,
        input,
        &mut state.net,
        &mut state.frame.hud_prompt,
        local_ghost,
    );
    // Drive the local player's channel pose + beam VFX.
    let pid = crate::game::ghost_system::player_id(&state.world);
    crate::game::shrine_system::tick_channel_pose(
        &mut state.shrines,
        &mut state.world,
        renderer,
        pid,
        local_ghost,
    );

    // ECS systems
    let action_cfg = PlayerActionConfig::default();
    let accept_input = !crate::game::ghost_system::is_dead(&state.world, state.net.local_ghost_cached);
    player_action_pre_system(&mut state.world, input, dt, &action_cfg, accept_input);
    player_input_system(&mut state.world, input, dt);
    movement_system(&mut state.world, dt, state.floor_mgr.dungeon.as_ref());
    player_action_post_system(&mut state.world, &action_cfg);
    collision_system(&mut state.world, &state.floor.wall_colliders);
}
