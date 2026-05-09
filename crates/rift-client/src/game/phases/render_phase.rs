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

    // Footprint trail: the decal system maintains a per-player
    // "blood charge" tracker that refills when the player walks
    // through fresh splatters and drains as it lays prints. Pass
    // the local player's foot position once per frame; the
    // tracker derives movement direction from the delta itself
    // and spawns prints every ~0.5 m of travel until the charge
    // runs out. No-op when the player has never stepped through
    // a splat.
    //
    // Skip the call entirely while airborne — otherwise a dodge-
    // jump through a fresh splatter leaves a chain of bloody
    // footprints floating ~2 m up at the apex of the jump until
    // they expire ~28 s later. Tracker state (last_pos / step_accum)
    // is reset by the call site below so resuming after the landing
    // doesn't dump the entire airborne travel as one giant step.
    let player_airborne = state
        .world
        .query::<(&Player, &LocalPlayer)>()
        .iter()
        .map(|(_, (p, _))| p.airborne)
        .next()
        .unwrap_or(false);
    if player_airborne {
        // Re-anchor so the next grounded frame doesn't see a huge
        // delta and dump every queued step at once on landing.
        renderer.blood_field.reset_step_tracker(0, player_pos);
    } else {
        let now = renderer.elapsed_secs();
        renderer.blood_field.track_player_step(0, player_pos, now);
    }

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
    // Pass the active fog band so each torch's intensity fades
    // in from 0 at the fog wall to full at the player's clear
    // zone — eliminates the "POOF, the corridor lit up" pop
    // when the player walks past a hard cutoff distance.
    let fog_start = renderer.fog_start;
    let fog_end = renderer.fog_end;
    let elapsed = renderer.elapsed_secs();
    state
        .floor_mgr
        .torches
        .update_lights(renderer, player_pos, elapsed, fog_start, fog_end);
    // Hub thunderstorm: when present, restores calm lighting
    // and overlays any in-progress lightning flash. Owns the
    // entire point-light vec while in the hub (no torches).
    if let Some(storm) = state.floor_mgr.hub_storm.as_mut() {
        storm.tick(renderer, dt);
    }
    // Push a hot-crimson light at every active portal AFTER
    // the torch / storm systems have rebuilt `point_lights`,
    // so the portal's environmental glow lands on the chest,
    // player, and surrounding floor every frame regardless of
    // which subsystem owns the per-frame light vec. See
    // `portal_system::push_lights` for the breathing/spasm
    // pulse maths (kept in sync with the rift shader).
    crate::game::portal_system::push_lights(
        renderer,
        &[
            state.floor.hub_portal.as_ref(),
            state.floor.exit_portal.as_ref(),
            state.floor.rift_spawn_portal.as_ref(),
        ],
        elapsed,
    );
    renderer.vfx_system.tick(dt);
    // After the simulation tick, walk every live effect with an
    // attached light and push a [`PointLight`] into the
    // renderer's per-frame list. This is what makes projectile
    // trails (fireballs, arcane bolts) and impact bursts
    // actually illuminate the corridor walls and enemies.
    // Done after `tick` so freshly-moved anchors (the trail's
    // `set_anchor` was called in `world_sync.sync_projectiles`
    // earlier this frame) drive correctly-positioned lights.
    // Split borrow: the system holds a `&self` while we mutate
    // `point_lights`, so we collect into a temp and append.
    {
        let mut effect_lights: Vec<rift_engine::PointLight> = Vec::new();
        renderer
            .vfx_system
            .collect_lights(elapsed, &mut effect_lights);
        renderer.point_lights.extend(effect_lights);
    }
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
