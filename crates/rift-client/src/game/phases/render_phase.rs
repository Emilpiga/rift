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

    skinning_system(
        &mut state.world,
        renderer,
        dt,
        state.floor_mgr.dungeon.as_ref(),
    );

    // Rigid weapon-prop follow: write `host_xform * hand_joint`
    // into each weapon attachment's renderer slot now that
    // `joint_worlds` is fresh for this frame.
    crate::game::weapon_visuals::update_weapon_transforms(&mut state.world, renderer, input);

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

    // Footstep audio: driven by the foot-IK pass, which
    // tracks each ankle's height above the avatar's grounded
    // plane and bumps a per-foot plant counter every time the
    // foot transitions from airborne to planted. We compare
    // those counters against the last-seen values cached in
    // `FrameState` and fire one one-shot per delta.
    //
    // Animation-driven detection (instead of velocity-derived
    // gait synthesis) means rolls, knockbacks, slides and any
    // other movement effect that decouples horizontal speed
    // from the visible foot motion automatically stay in sync
    // \u2014 if the foot doesn't visibly hit the ground, no sound
    // plays; if it does, the sound fires at the exact frame
    // and at the foot's actual world position.
    if !player_airborne {
        if let Some(audio) = state.audio.as_mut() {
            if let Some(player_id) = crate::game::ghost_system::player_id(&state.world) {
                if let Ok(ik) = state
                    .world
                    .get::<&rift_engine::ecs::components::FootIkState>(player_id)
                {
                    let plants = [
                        (
                            ik.left_plant_seq,
                            ik.left_plant_pos,
                            &mut state.frame.last_left_plant_seq,
                        ),
                        (
                            ik.right_plant_seq,
                            ik.right_plant_pos,
                            &mut state.frame.last_right_plant_seq,
                        ),
                    ];
                    for (seq, pos, last) in plants {
                        if seq == *last {
                            continue;
                        }
                        // Coalesce multi-step deltas (e.g. on
                        // first frame after regen, or after a
                        // teleport that reset the IK chain) to
                        // a single emission \u2014 we never want a
                        // burst of stacked one-shots.
                        *last = seq;
                        state.frame.step_rotation = state.frame.step_rotation.wrapping_add(1);
                        // Resolve the surface under the foot
                        // *at the moment of the plant* so a
                        // player crossing a material boundary
                        // hears the right material on each
                        // step. Falls back to the floor's
                        // default surface (Sand for hub,
                        // Stone for rift) when the dungeon
                        // hasn't been built yet \u2014 same default
                        // chain `Floor::surface_at` uses
                        // internally.
                        let surface = state
                            .floor_mgr
                            .dungeon
                            .as_ref()
                            .map(|f| f.surface_at(pos.x, pos.z))
                            .unwrap_or_default();
                        let bank = crate::game::audio_banks::footstep_paths(surface);
                        if bank.is_empty() {
                            // No authored samples for this
                            // surface yet \u2014 silent step. See
                            // the bank registry for what's
                            // missing.
                            continue;
                        }
                        let idx = state.frame.step_rotation as usize % bank.len();
                        let path = bank[idx];
                        // Slight per-step volume jitter so
                        // consecutive plants don't sound
                        // mechanically identical. Centred on
                        // 1.0 so jitter doesn't double-attenuate
                        // the already-quiet sample.
                        let jitter = 0.95 + (idx as f32) * 0.05;
                        let spec = rift_audio::SoundSpec {
                            path: path.into(),
                            // Loud: footsteps are *your*
                            // footsteps and need to read over
                            // the wind loop (~0.85 peak) at
                            // every camera distance. The raw
                            // wav peaks low so 1.6 brings them
                            // up to parity without clipping.
                            volume: 1.6 * jitter,
                            // Wide full-volume zone so the
                            // third-person camera (4\u20136 m
                            // behind the player) sits well
                            // inside `min_distance` and the
                            // sample plays at its authored
                            // level.
                            min_distance: 8.0,
                            max_distance: 25.0,
                            looping: false,
                            pitch: 1.0,
                        };
                        // Anchor at the foot's actual world
                        // position so spatialisation matches
                        // exactly where the contact happened.
                        audio.play_one_shot(&spec, pos);
                    }
                }
            }
        }
    } else {
        // Airborne: re-sync the cached counters to whatever
        // the IK pass currently has so the landing frame
        // doesn't dump every plant that fired during the
        // jump (the IK still ticks while airborne and may
        // count a touchdown the moment the kinematic resolves
        // back to grounded; the dedicated landing-impact SFX
        // belongs to a separate code path).
        if let Some(player_id) = crate::game::ghost_system::player_id(&state.world) {
            if let Ok(ik) = state
                .world
                .get::<&rift_engine::ecs::components::FootIkState>(player_id)
            {
                state.frame.last_left_plant_seq = ik.left_plant_seq;
                state.frame.last_right_plant_seq = ik.right_plant_seq;
            }
        }
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
    // Mirror the visual flicker into the audio mixer: each
    // torch's looping crackle volume tracks the same distance,
    // rank, and flicker curves as its light, so the soundscape
    // matches the screen.
    if let Some(audio) = state.audio.as_mut() {
        state
            .floor_mgr
            .torches
            .tick_audio(audio, player_pos, elapsed, fog_start, fog_end);
    }
    // Hub thunderstorm: when present, restores calm lighting
    // and overlays any in-progress lightning flash. Owns the
    // entire point-light vec while in the hub (no torches).
    if let Some(storm) = state.floor_mgr.hub_storm.as_mut() {
        storm.tick(renderer, dt);
    }
    state.floor_mgr.props.tick(renderer, dt);
    // Rift-floor void embers: re-anchor every frame ~10 m
    // below the player so the field of glowing motes
    // continuously spawns under whatever room the player is
    // standing in, then rises past the floor's outer edges.
    // `None` on the hub / char-select. The 10 m depth puts
    // the spawn plane well below the floor mesh so the floor
    // depth-test naturally hides every ember that's still
    // under it — only motes that drift past the silhouette
    // become visible, giving the dungeon a hot rim.
    if let Some(id) = state.floor_mgr.void_embers {
        renderer
            .vfx_system
            .set_anchor(id, player_pos - Vec3::new(0.0, 10.0, 0.0));
    }
    // Hub sandstorm haze: keep the haze emitter centred on
    // the player so the field of dust travels with the
    // camera rather than reading as a fixed volumetric panel
    // somewhere on the platform. Anchor lifted to chest
    // height so the spawn disc samples mostly ankle-to-head
    // altitude.
    //
    // Wind gust envelope: a slow sum-of-sines yields a
    // breathing curve in roughly [0.55 .. 1.45] that pulses
    // every ~6–18 seconds. We feed it into BOTH the haze
    // brightness (visual gust = thicker dust) and the wind
    // emitter volume (audio gust = louder roar) so the two
    // pulse together — the player sees the dust thicken at
    // the same instant the wind howls. Identical phase by
    // construction.
    if let Some(haze) = state.floor_mgr.hub_haze {
        let t = elapsed;
        let slow =
            (t * 0.35).sin() * 0.55 + (t * 0.17 + 1.7).sin() * 0.35 + (t * 0.08 + 0.4).sin() * 0.10;
        // Map [-1, +1] -> roughly [0.55, 1.45]: a 45 % dip
        // at the lull and a 45 % swell at the peak — wide
        // enough to feel obviously alive without hiding /
        // overpowering the rest of the scene.
        let gust = (1.0 + slow * 0.45).max(0.05);
        renderer
            .vfx_system
            .set_anchor(haze, player_pos + Vec3::new(0.0, 0.6, 0.0));
        renderer.vfx_system.set_brightness(haze, gust);
        // Drive the wind loop's volume from the same gust.
        // Anchor it on the listener (player ear height) so
        // the loop reads as a coherent wash of wind rather
        // than a directional point source — distance
        // attenuation stays at full while the gust does the
        // work.
        if let (Some(audio), Some(em)) = (state.audio.as_mut(), state.floor_mgr.hub_wind) {
            audio.set_emitter_position(em, player_pos + Vec3::new(0.0, 1.6, 0.0));
            audio.set_emitter_volume(em, gust);
        }
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
    // Lazy-attach + per-frame volume modulation for the
    // portal hum loops. Single integration point so the
    // various `tick_*` portal functions don't each have to
    // thread `&mut AudioSystem`.
    if let Some(audio) = state.audio.as_mut() {
        crate::game::portal_system::tick_audio(
            &mut [
                state.floor.hub_portal.as_mut(),
                state.floor.exit_portal.as_mut(),
                state.floor.rift_spawn_portal.as_mut(),
            ],
            audio,
            elapsed,
        );
    }
    renderer.vfx_system.tick(
        dt,
        // Cull off-screen VFX past a few metres beyond the
        // fog wall — anything farther is invisible to the
        // player but otherwise still pays the full per-
        // particle cost. Anchored on `fog_origin` (the
        // player) so zooming the camera doesn't pop nearby
        // effects in/out of simulation. The +5 m margin
        // avoids edge flicker when an effect anchor sits
        // right at the fog distance.
        Some((renderer.fog_origin, renderer.fog_end + 5.0)),
    );
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
        // VFX lights live in their own dedicated pool so that
        // ambient torches/portals/etc can't crowd them out of
        // the per-frame UBO. The renderer packs `vfx_lights`
        // first into both the light array and the shadow
        // atlas, then fills the remainder from `point_lights`.
        renderer.vfx_lights.clear();
        let mut effect_lights: Vec<rift_engine::PointLight> = Vec::new();
        renderer
            .vfx_system
            .collect_lights(elapsed, &mut effect_lights);
        renderer.vfx_lights.extend(effect_lights);
    }
    state.player_state.abilities.tick_all(dt);

    if state.frame.damage_flash > 0.0 {
        state.frame.damage_flash *= (-dt * 5.4).exp();
        if state.frame.damage_flash < 0.003 {
            state.frame.damage_flash = 0.0;
        }
    }
    if state.frame.level_up_flash > 0.0 {
        state.frame.level_up_flash = (state.frame.level_up_flash - dt * 0.4).max(0.0);
    }
    if state.frame.transition_fade > 0.0 {
        // ~0.6 s fade-out from full black.
        state.frame.transition_fade = (state.frame.transition_fade - dt * 1.6).max(0.0);
    }
}
