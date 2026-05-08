//! Phase 3 of the per-frame `GameState::update` pipeline.
//!
//! Animation + render-side systems: skinning, decals, camera,
//! fog, VFX, channel beams. Also drives the per-frame timer
//! decays for HUD overlays (damage flash, level-up flash,
//! transition fade).

use glam::Vec3;
use rift_engine::ecs::components::{LocalPlayer, Player, Transform};
use rift_engine::ecs::systems::{
    camera_follow_system, cast_advance_system, despawn_system, enemy_anim_system,
    locomotion_anim_system, render_sync_system, skinning_system,
};
use rift_engine::{Input, Renderer};

use crate::game::state::GameState;

pub fn tick(state: &mut GameState, renderer: &mut Renderer, input: &Input, dt: f32) {
    // Tick combat text
    state.combat_text.tick(dt);

    // Despawn dead entities (animation-finished kills, etc.)
    let _kills = despawn_system(&mut state.world, renderer);

    // Render sync
    render_sync_system(&state.world, renderer);

    locomotion_anim_system(&mut state.world);
    enemy_anim_system(&mut state.world, dt);

    // Spell-cast state machine: advances the upper-body cast layer.
    let _ = cast_advance_system(&mut state.world, dt);

    skinning_system(&mut state.world, renderer, dt);
    state.decals.update(dt, renderer);

    // Local-avatar ghost tint.
    crate::game::ghost_system::apply_tint(&state.world, renderer, state.net.local_ghost_cached);

    // Channel beam visuals (Frost Ray etc.).
    crate::game::ability::tick_channel_visuals(state, renderer, dt);

    // Equipment visual sync — locate player position for camera /
    // fog / torch lights below.
    let player_pos = state
        .world
        .query::<(&Transform, &Player, &LocalPlayer)>()
        .iter()
        .map(|(_, (t, _, _))| t.position)
        .next()
        .unwrap_or(Vec3::ZERO);

    // Skip aim updates while the player is dead — otherwise the
    // death pose's spine twist would keep tracking the cursor.
    if !crate::game::ghost_system::is_dead(&state.world, state.net.local_ghost_cached) {
        let arm_aim = crate::game::cursor::aim_dir(input, renderer, player_pos);
        if let Some(player_id) = crate::game::ghost_system::player_id(&state.world) {
            if let Ok(mut p) = state
                .world
                .get::<&mut rift_engine::ecs::components::Player>(player_id)
            {
                p.aim_dir = arm_aim;
            }
        }
    }

    camera_follow_system(&state.world, renderer, input, &state.floor.wall_aabbs, dt);
    // Anchor the distance fog on the player so zooming the
    // camera out doesn't pull the fog wall in over the
    // character.
    renderer.fog_origin = player_pos;
    // Push the 8 nearest wall-torch lights for this frame.
    state.floor_mgr.torches.update_lights(renderer, player_pos);
    // Hub thunderstorm: when present, restores calm lighting
    // and overlays any in-progress lightning flash. Owns the
    // entire point-light vec while in the hub (no torches).
    if let Some(storm) = state.floor_mgr.hub_storm.as_mut() {
        storm.tick(renderer, dt);
    }
    renderer.vfx_system.tick(dt);
    state.player_state.abilities.tick_all(dt);

    if state.frame.damage_flash > 0.0 {
        state.frame.damage_flash = (state.frame.damage_flash - dt * 2.2).max(0.0);
    }
    if state.frame.level_up_flash > 0.0 {
        state.frame.level_up_flash = (state.frame.level_up_flash - dt * 0.4).max(0.0);
    }
    if state.frame.transition_fade > 0.0 {
        // ~0.6 s fade-out from full black.
        state.frame.transition_fade = (state.frame.transition_fade - dt * 1.6).max(0.0);
    }
}
