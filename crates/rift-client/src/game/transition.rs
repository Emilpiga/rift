//! Transition state machines: character-select → in-game,
//! server-driven floor regen, and the per-regen reset.
//!
//! All entry points are free functions taking `&mut GameState`
//! plus the resources they actually need. `GameState::update`
//! dispatches into [`tick_entering_world`] / [`update_character_select`]
//! when the [`AppState`](super::state::AppState) variant matches;
//! the binary calls [`apply_net_transition`] directly when the
//! server hands us a `LoadFloor` packet.

use rift_engine::ecs::components::{Collider, Static, Transform};
use rift_engine::physics::Aabb;
use rift_engine::{Input, Renderer};

use rift_game::character;

use super::character_select;
use super::hud;
use super::player_state::PlayerState;
use super::rift_state::RiftState;
use super::state::{AppState, GameState};
use super::systems::portal_system;

/// One step of the character-select → in-game transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnterPhase {
    PrepareScene,
    PreloadHub,
    GenerateHub,
    AttachOutfits,
    LoadOutfits,
    RebuildWalls,
}

/// One step of a server-driven floor transition. Split from
/// [`EnterPhase`] because the inputs differ (we already have a
/// player + outfits + character-select is gone) and the visual
/// is a black-curtain "Entering Floor N…" screen rather than
/// the staged hub-entry overlay.
///
/// The phases are designed so the *first* one to run draws the
/// loading overlay before the heaviest one (`Generate`)
/// freezes the render thread — that way the user always sees
/// the loading screen instead of a frozen frame of the old
/// world. Each phase advances exactly one step per frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetEnterPhase {
    /// Black out + render the loading overlay one frame early
    /// so it presents before any heavy work runs. Carries the
    /// destination floor index forward through every phase.
    FadeOut { index: u32 },
    /// Drop per-floor visual state (decals, vfx, loot, …) so
    /// the new floor's regen sees a clean slate.
    Reset { index: u32 },
    /// Run the (single, unavoidably blocking) dungeon regen
    /// call. Frozen frame is hidden under the overlay we
    /// already presented in the prior phase.
    Generate { index: u32 },
    /// Rebuild static collider caches from the new ECS rows.
    RebuildWalls,
    /// One last frame of overlay before handing back to
    /// gameplay so the fade-in transition starts from a
    /// known-good frame.
    FadeIn,
}

/// Tick the character-select screen.
pub fn update_character_select(
    state: &mut GameState,
    renderer: &mut Renderer,
    input: &Input,
    dt: f32,
) {
    renderer.overlay_batch.clear();

    // Background-load the world's authored PBR material packs
    // while the user is browsing characters. Each call decodes
    // at most one pack (~1 s of 2 k PNG decode + GPU upload),
    // and we hit it every frame, so by the time the user
    // clicks Play the hub disc, mountains, and dungeon
    // surfaces are already warm in the texture cache. This
    // replaces the old "block in `generate_hub` for ~8 s"
    // approach which was tripping the netcode 5 s timeout
    // and forcing the server to drop the connect-token key.
    state.floor_mgr.env.tick_world_preload(renderer);

    // Preview avatar (independent of UI; needs &mut World/Renderer).
    state
        .character_select
        .tick_preview(&mut state.world, renderer, dt);
    // Apply head cosmetics (white eyes / eyebrows / hair) and
    // equipped gear so the preview matches what the player will
    // see in-world. Both passes are cache-backed so they
    // short-circuit on every frame after the first.
    if let Some((entity, gender)) = state.character_select.preview_entity() {
        super::avatar_cosmetics::apply_avatar_cosmetics(
            &mut state.world,
            renderer,
            &mut state.avatar_cosmetics_cache,
            entity,
            gender,
        );
        if let Some(base_ids) = state.character_select.preview_equipped_base_ids() {
            let desired = super::equipment_visuals::desired_visuals_for_base_ids(base_ids, gender);
            super::equipment_visuals::apply_equipment_visuals(
                &mut state.world,
                renderer,
                &mut state.equipment_visual_cache,
                entity,
                &desired,
            );
        }
    }
    rift_engine::ecs::systems::skinning_system(
        &mut state.world,
        renderer,
        dt,
        state.floor_mgr.dungeon.as_ref(),
    );

    // Fused input + render through the immediate-mode UI stack.
    let (sw, sh) = renderer.screen_size();
    let action = {
        use rift_engine::ui::im::{Ui, DEFAULT_THEME};
        let mut ui = Ui::begin(
            &mut renderer.overlay_batch,
            input,
            &mut state.ui_state,
            &DEFAULT_THEME,
            sw,
            sh,
        );
        let action = state.character_select.frame(&mut ui);
        let _ = ui.end();
        action
    };

    match action {
        character_select::SelectAction::None => {}
        character_select::SelectAction::AccountConfirmed { name } => {
            state.net.roster_request = Some(name);
        }
        character_select::SelectAction::Play {
            account_name,
            profile,
        } => {
            start_with_profile(state, account_name, profile);
        }
        character_select::SelectAction::Quit => {
            log::info!("Quit requested from character select");
        }
    }
}

/// Promote a confirmed character profile into in-game player
/// state and kick off the staged `EnteringWorld` sequence.
fn start_with_profile(
    state: &mut GameState,
    account_name: String,
    profile: character::CharacterProfile,
) {
    log::info!(
        "Entering world as '{}' on account '{}' ({:?})",
        profile.name,
        account_name,
        profile.gender,
    );
    state.player_state = PlayerState::with_profile(
        profile.gender,
        profile.name.clone(),
        rift_game::loadout::Loadout::default_hero(),
    );
    // Hand the profile + account to the binary so it can
    // advertise them on the wire. In SP this is just dropped.
    state.net.profile = Some(profile);
    state.net.account_name = Some(account_name);
    state.app_state = AppState::EnteringWorld(EnterPhase::PrepareScene);
}

/// Forward a server-supplied roster into the character-select
/// screen. Called by the binary once the net client receives
/// `ServerMsg::Roster` after we issued `RequestRoster`.
pub fn apply_server_roster(state: &mut GameState, entries: Vec<rift_net::messages::RosterEntry>) {
    state.character_select.apply_server_roster(entries);
}

/// Drive one step of the staged character-select → in-game
/// transition. The state machine is single-step-per-frame so
/// the netcode loop keeps pumping while heavy work runs (asset
/// decode, hub generation, outfit attach).
pub fn tick_entering_world(state: &mut GameState, renderer: &mut Renderer, phase: EnterPhase) {
    // Continue any pending world-PBR pre-warm that didn't
    // finish during character-select. Same one-pack-per-tick
    // budget as the character-select path, so each
    // EnteringWorld frame still pumps netcode between heavy
    // decodes.
    state.floor_mgr.env.tick_world_preload(renderer);

    let (label, next): (&'static str, Option<EnterPhase>) = match phase {
        EnterPhase::PrepareScene => {
            state
                .character_select
                .teardown_preview(&mut state.world, renderer);
            renderer.point_lights.clear();
            ("Preparing world…", Some(EnterPhase::PreloadHub))
        }
        EnterPhase::PreloadHub => {
            // Stream a few gltf assets per tick so the netcode
            // loop keeps running and the server doesn't time us
            // out while the hub forest decodes.
            let paths = super::props::hub_asset_paths();
            let loaded = state.floor_mgr.props.preload_step(&paths, 2);
            let total = super::props::hub_total_assets();
            let done = state.floor_mgr.props.loaded_count(&paths);
            let next = if done >= total || loaded == 0 {
                Some(EnterPhase::GenerateHub)
            } else {
                Some(EnterPhase::PreloadHub)
            };
            ("Loading environment…", next)
        }
        EnterPhase::GenerateHub => {
            state.floor.in_hub = true;
            state.rift = RiftState::new(1);
            // `generate_hub` calls `renderer.clear_objects()`,
            // which invalidates every renderer object index
            // we hold elsewhere. Drop loot ground-mesh /
            // VFX references *before* the regen so stale
            // `object_index = 0` entries can't stomp the new
            // hub platform's model matrix on the very next
            // frame.
            crate::game::systems::loot_system::clear_world_visuals(&mut state.loot, renderer);
            // Despawn the previous floor's torch audio
            // emitters before `generate_hub` drains the
            // torch table internally. Otherwise the emitter
            // ids would leak — the torches that owned them
            // are gone but the kira tracks would keep playing.
            if let Some(audio) = state.audio.as_mut() {
                state.floor_mgr.torches.clear_audio(audio);
            }
            match state.floor_mgr.generate_hub(
                &mut state.world,
                renderer,
                &state.player_state,
                &mut state.anim_cache,
                &mut state.avatar_cosmetics_cache,
            ) {
                Ok(portal_pos) => {
                    portal_system::spawn_hub(&mut state.floor.hub_portal, renderer, portal_pos)
                }
                Err(e) => log::error!("Hub generation failed: {}", e),
            }
            // Hub sandstorm wind loop: spawn the ambient
            // emitter that pairs with `hub_haze`. Anchored
            // on the player every frame in `render_phase`
            // and modulated by the same gust envelope that
            // brightens / dims the haze, so the audio and
            // visuals breathe together.
            spawn_hub_wind(state);
            // Fresh world / fresh local avatar — the
            // `SkinnedAttachments` component is empty. Mark
            // dirty so the binary's per-frame retry loop
            // re-applies the local equipment visuals on the
            // new entity.
            state.loot.equipment_visuals_dirty = true;
            ("Generating hub…", Some(EnterPhase::AttachOutfits))
        }
        EnterPhase::AttachOutfits => ("Preparing outfits…", Some(EnterPhase::LoadOutfits)),
        EnterPhase::LoadOutfits => ("Loading outfits…", Some(EnterPhase::RebuildWalls)),
        EnterPhase::RebuildWalls => {
            rebuild_wall_caches(state, renderer);
            ("Finalizing…", None)
        }
    };

    hud::draw_world_loading_overlay(renderer, 0.0, label);

    match next {
        Some(p) => state.app_state = AppState::EnteringWorld(p),
        None => state.app_state = AppState::Playing,
    }
}

/// Kick off a staged server-driven floor transition. Caller
/// (the binary) hands us the destination `index` from
/// `LoadFloor`; we set `app_state = NetEntering(FadeOut)` and
/// pin the screen to fully black so the next frame's render
/// presents the curtain *before* the heavy regen runs.
///
/// The actual regen happens in the `Generate` phase a frame
/// later, by which time the loading overlay has already been
/// presented. Net result: the player sees a clean fade-out →
/// "Entering Floor N…" overlay → fade-in, instead of
/// "world frozen for 3 s → snap to new world".
pub fn queue_net_transition(state: &mut GameState, _renderer: &mut Renderer, index: u32) {
    state.frame.transition_fade = 1.0;
    state.app_state = AppState::NetEntering(NetEnterPhase::FadeOut { index });
}

/// Drive one step of the staged net transition. Single
/// step-per-frame so the netcode loop keeps pumping and the
/// loading overlay is guaranteed to present before any
/// blocking work runs.
pub fn tick_net_entering(state: &mut GameState, renderer: &mut Renderer, phase: NetEnterPhase) {
    let (label, progress, next): (&'static str, f32, Option<NetEnterPhase>) = match phase {
        NetEnterPhase::FadeOut { index } => {
            // Re-pin in case the per-frame decay nibbled it.
            state.frame.transition_fade = 1.0;
            (
                "Entering world…",
                0.10,
                Some(NetEnterPhase::Reset { index }),
            )
        }
        NetEnterPhase::Reset { index } => {
            reset_for_regeneration(state, renderer);
            state.frame.transition_fade = 1.0;
            (
                if index == 0 {
                    "Returning to hub…"
                } else {
                    "Entering rift…"
                },
                0.30,
                Some(NetEnterPhase::Generate { index }),
            )
        }
        NetEnterPhase::Generate { index } => {
            // Heavy step. The overlay rendered in the prior
            // phase is what's currently on screen; this frame's
            // overlay below covers the post-regen state.
            if index == 0 {
                state.floor.in_hub = true;
                state.rift = RiftState::new(1);
                crate::game::systems::loot_system::clear_world_visuals(&mut state.loot, renderer);
                if let Some(audio) = state.audio.as_mut() {
                    state.floor_mgr.torches.clear_audio(audio);
                }
                match state.floor_mgr.generate_hub(
                    &mut state.world,
                    renderer,
                    &state.player_state,
                    &mut state.anim_cache,
                    &mut state.avatar_cosmetics_cache,
                ) {
                    Ok(portal_pos) => {
                        portal_system::spawn_hub(&mut state.floor.hub_portal, renderer, portal_pos)
                    }
                    Err(e) => log::error!("Hub regeneration failed: {}", e),
                }
                spawn_hub_wind(state);
            } else {
                state.floor.in_hub = false;
                state.rift = RiftState::new(index);
                crate::game::systems::loot_system::clear_world_visuals(&mut state.loot, renderer);
                if let Some(audio) = state.audio.as_mut() {
                    state.floor_mgr.torches.clear_audio(audio);
                }
                // Leaving the hub: despawn the wind loop
                // before the floor manager nulls the handle
                // in `generate`. Without this the kira
                // emitter would leak across the rift.
                if let (Some(audio), Some(em)) =
                    (state.audio.as_mut(), state.floor_mgr.hub_wind.take())
                {
                    audio.despawn_emitter(em);
                }
                if let Err(e) = state.floor_mgr.generate(
                    &mut state.world,
                    renderer,
                    &state.rift,
                    &state.player_state,
                    &mut state.anim_cache,
                    &mut state.avatar_cosmetics_cache,
                    state.net.floor_seed,
                ) {
                    log::error!("Net floor regeneration failed: {}", e);
                }
                // Spawn one looping flame-crackle emitter per
                // freshly-placed wall torch. No-op if audio
                // is unavailable or the asset is missing.
                if let Some(audio) = state.audio.as_mut() {
                    state.floor_mgr.torches.place_audio(audio);
                }
            }
            state.frame.transition_fade = 1.0;
            ("Generating world…", 0.70, Some(NetEnterPhase::RebuildWalls))
        }
        NetEnterPhase::RebuildWalls => {
            rebuild_wall_caches(state, renderer);
            state.frame.transition_fade = 1.0;
            ("Finalizing…", 0.90, Some(NetEnterPhase::FadeIn))
        }
        NetEnterPhase::FadeIn => {
            // Hand back to gameplay. `transition_fade` is left
            // at 1.0; the per-frame decay in `render_phase`
            // takes it down to 0 over ~0.6 s.
            ("Ready", 1.0, None)
        }
    };

    hud::draw_world_loading_overlay(renderer, progress, label);

    match next {
        Some(p) => state.app_state = AppState::NetEntering(p),
        None => state.app_state = AppState::Playing,
    }
}

/// Wipe per-frame and per-floor state that doesn't survive a
/// regen. Cross-floor state (inventory, level, account) is
/// preserved.
pub fn reset_for_regeneration(state: &mut GameState, renderer: &mut Renderer) {
    state.frame.reset();
    // Despawn portal hum loops BEFORE dropping the structs
    // that hold their emitter ids, otherwise the kira tracks
    // would leak across the floor regen.
    if let Some(audio) = state.audio.as_mut() {
        state.floor.detach_portal_audio(audio);
    }
    state.floor.reset_portals();
    // Exit-vote state is not cleared on regen: the server
    // re-broadcasts the authoritative `RiftExitVote` whenever
    // we land on a fresh floor (cooldown wipe → dirty flag
    // → broadcast), and the floor transition itself cancels
    // any in-flight vote on the server side.
    state.combat_text.clear();
    // Wipe every live particle / ribbon emitter so loot beams,
    // frost trails, channel ribbons, and any other long-lived
    // effect from the previous floor don't leak visuals into
    // the new one. The per-substate `reset_for_floor` calls
    // below rely on this happening first to invalidate the
    // emitter handles they're about to drop.
    renderer.vfx_system.clear_all();
    state.loot.reset_for_floor();
    state.shrines.reset_for_floor();
    state.mp_inventory_ui.open = false;
    // The world wipe just erased every `SkinnedAttachments`
    // component along with the player entity. Set the dirty
    // flag so the binary's per-frame retry loop re-applies the
    // local equipment visuals once the new avatar is spawned.
    state.loot.equipment_visuals_dirty = true;
}

/// Rebuild the wall collider + AABB caches from current
/// `Transform + Collider + Static` ECS rows. Called after every
/// floor regen + at the end of the in-game transition.
///
/// Also re-binds the renderer's per-floor blood field to the
/// freshly computed XZ extent so kill splats from this floor
/// don't leak across into the next, and so the field's
/// world\u2192UV transform is correct for sampling in the forward
/// shader. The hub disables the field outright (no kills there).
pub fn rebuild_wall_caches(state: &mut GameState, renderer: &mut Renderer) {
    state.floor.wall_colliders = state
        .world
        .query::<(&Transform, &Collider, &Static)>()
        .iter()
        .map(|(_, (t, c, _))| (t.position, *c))
        .collect();

    state.floor.wall_aabbs = state
        .floor
        .wall_colliders
        .iter()
        .map(|(pos, col)| Aabb::from_center(*pos, col.half_extents))
        .collect();

    if state.floor.in_hub || state.floor.wall_aabbs.is_empty() {
        renderer.unbind_blood_field();
    } else {
        let mut min = glam::Vec2::splat(f32::INFINITY);
        let mut max = glam::Vec2::splat(f32::NEG_INFINITY);
        for aabb in &state.floor.wall_aabbs {
            min.x = min.x.min(aabb.min.x);
            min.y = min.y.min(aabb.min.z);
            max.x = max.x.max(aabb.max.x);
            max.y = max.y.max(aabb.max.z);
        }

        // Derive the walkable Y-range from the dungeon's
        // elevation grid rather than from wall AABBs: walls
        // are spawned at a fixed Y=0 regardless of which
        // platform they border, so they don't reveal the
        // raised-dais / lowered-pit elevations the renderer
        // needs. The shader composite uses this band to keep
        // splats visible on every platform on the floor
        // (without it, only the base elevation receives
        // blood).
        let (floor_y_min, floor_y_max) = match &state.floor_mgr.dungeon {
            Some(d) if !d.elevation.is_empty() => {
                let mut lo = f32::INFINITY;
                let mut hi = f32::NEG_INFINITY;
                for &e in &d.elevation {
                    let y = e as f32 * rift_dungeon::ELEVATION_STEP;
                    if y < lo {
                        lo = y;
                    }
                    if y > hi {
                        hi = y;
                    }
                }
                if !lo.is_finite() {
                    (0.0, 0.0)
                } else {
                    (lo, hi)
                }
            }
            // No dungeon source (shouldn't happen outside the
            // hub, which we already short-circuited above) — fall
            // back to the wall-base Y so the field at least binds
            // somewhere sensible.
            _ => {
                let mut lo = f32::INFINITY;
                for aabb in &state.floor.wall_aabbs {
                    lo = lo.min(aabb.min.y);
                }
                if !lo.is_finite() {
                    (0.0, 0.0)
                } else {
                    (lo, lo)
                }
            }
        };
        renderer.recreate_blood_field(min, max, floor_y_min, floor_y_max);
    }
}

/// Spawn the looping hub-wind ambient emitter. Idempotent:
/// if a previous emitter is still around (e.g. re-entering
/// the hub from the rift) it is despawned first so we don't
/// stack two copies of the same loop. Anchored at the hub
/// origin as a placeholder; `render_phase` re-anchors it on
/// the player every frame, which keeps the source effectively
/// at the listener's position so the loop reads as a coherent
/// surrounding wind rather than a directional point source.
fn spawn_hub_wind(state: &mut GameState) {
    let Some(audio) = state.audio.as_mut() else {
        return;
    };
    if let Some(prev) = state.floor_mgr.hub_wind.take() {
        audio.despawn_emitter(prev);
    }
    // Wide falloff (1 m full -> 60 m silent) plus the
    // listener-anchored update means the player hears the
    // wind at full volume regardless of where they walk on
    // the platform. Volume is intentionally moderate (0.6)
    // so per-frame gust modulation in `tick_hub_wind` can
    // dip below it without sounding like the wind died.
    let spec = rift_audio::SoundSpec {
        path: "ambient/wind.mp3".into(),
        volume: 0.6,
        min_distance: 1.0,
        max_distance: 60.0,
        looping: true,
        pitch: 1.0,
    };
    state.floor_mgr.hub_wind = audio.spawn_emitter(&spec, glam::Vec3::ZERO);
}
