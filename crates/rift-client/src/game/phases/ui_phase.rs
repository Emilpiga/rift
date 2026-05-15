//! Phase 4 of the per-frame `GameState::update` pipeline.
//!
//! HUD + inventory immediate-mode UI pass. One `Ui::begin/end`
//! scope so layer order and `OverlayBatch` ownership stay
//! coherent across every widget.

use glam::Vec3;
use rift_engine::ecs::components::{LocalPlayer, Player, Transform};
use rift_engine::{Input, Renderer};
use std::cell::Cell;
use std::time::Instant;

use crate::game::hud;
use crate::game::spellbook;
use crate::game::state::GameState;

// --- FPS counter state ---
// Single-threaded: the UI phase runs only on the main thread,
// so a thread-local Cell is sound and avoids plumbing yet
// another field through GameState. Sampled with an EMA so
// the displayed value doesn't flicker per-frame.
thread_local! {
    static FPS_LAST: Cell<Option<Instant>> = const { Cell::new(None) };
    static FPS_EMA:  Cell<f32>             = const { Cell::new(60.0) };
}

pub fn tick(state: &mut GameState, renderer: &mut Renderer, input: &Input) {
    renderer.overlay_batch.clear();
    let (sw, sh) = renderer.screen_size();

    // ---- FPS sample ----
    // Measure wall-clock dt between this and the previous UI
    // phase. We use Instant rather than the gameplay `dt`
    // because the latter is clamped (anti-spiral-of-death)
    // and paused effectively when the window loses focus —
    // both would lie about real frame rate.
    let now = Instant::now();
    let (smoothed_fps, ui_dt) = FPS_LAST.with(|cell| {
        let last = cell.replace(Some(now));
        match last {
            Some(prev) => {
                let dt = now.duration_since(prev).as_secs_f32().max(1e-4);
                let inst_fps = 1.0 / dt;
                let smoothed = FPS_EMA.with(|e| {
                    // Exponential moving average with ~0.5 s
                    // time constant — fast enough to react to
                    // hitches, slow enough not to flicker.
                    let prev_ema = e.get();
                    let alpha = (dt / 0.5).clamp(0.0, 1.0);
                    let new_ema = prev_ema * (1.0 - alpha) + inst_fps * alpha;
                    e.set(new_ema);
                    new_ema
                });
                (smoothed, dt)
            }
            None => (60.0, 1.0 / 60.0),
        }
    });

    // ---- Draw FPS counter ----
    // Top-left corner, small bright text, dark drop shadow
    // for readability on bright sky / pale floor pixels.
    let fps_text = format!("{:>3} FPS", smoothed_fps.round() as i32);
    let fps_size = 16.0_f32;
    let fps_x = 8.0_f32;
    let fps_y = 8.0_f32;
    renderer.overlay_batch.text(
        &fps_text,
        fps_x + 1.0,
        fps_y + 1.0,
        fps_size,
        [0.0, 0.0, 0.0, 0.6],
        sw,
        sh,
    );
    renderer.overlay_batch.text(
        &fps_text,
        fps_x,
        fps_y,
        fps_size,
        [1.0, 1.0, 0.85, 0.95],
        sw,
        sh,
    );

    let nearest_loot = crate::game::loot_system::nearest_drop(&state.world, &state.loot);
    let view_proj = renderer.camera.view_projection();
    let player_facing = state
        .world
        .query::<(&Transform, &Player, &LocalPlayer)>()
        .iter()
        .map(|(_, (t, _, _))| t.rotation * Vec3::Z)
        .next()
        .unwrap_or(Vec3::Z);
    let hub_portal_pos = state.floor.hub_portal.as_ref().map(|p| p.position);
    let portal_compass_pos = if !state.floor.in_hub && state.rift.floor_complete {
        state
            .floor
            .exit_portal
            .as_ref()
            .map(|p| p.position)
            .or_else(|| {
                state
                    .floor_mgr
                    .portal_anchors
                    .map(|(descend, extract)| (descend + extract) * 0.5)
            })
            .or(Some(state.floor_mgr.boss_room_center))
    } else {
        None
    };

    let display_resolutions_snapshot: Vec<_> = renderer
        .display_resolutions()
        .iter()
        .map(|r| rift_ui_types::settings::DisplayResolution {
            width: r.width,
            height: r.height,
        })
        .collect();
    let selected_resolution_snapshot = {
        let r = renderer.selected_display_resolution();
        rift_ui_types::settings::DisplayResolution {
            width: r.width,
            height: r.height,
        }
    };

    use rift_engine::ui::im::{Color, Ui, DEFAULT_THEME};
    let mut ui = Ui::begin(
        &mut renderer.overlay_batch,
        input,
        &mut state.ui_state,
        &DEFAULT_THEME,
        sw,
        sh,
    );
    // Snapshot which sub-modals are open BEFORE any widget
    // gets a chance to mutate them this frame. The pause-menu
    // block at the bottom uses this snapshot to decide
    // whether Escape should open the pause menu or close the
    // top-most sub-modal — without it, a widget that closes
    // itself on Escape would flip `open` to false mid-frame
    // and the host would then mis-read the state as "no
    // modal open" and pop the pause menu.
    let pre_spellbook_open = state.spellbook.open();
    let pre_inventory_open = state.inventory_ui.open && !state.loot.stash_session;
    let pre_talents_open = state.talents_panel.open;
    // Clear last frame's HUD click-swallow rects before any
    // HUD widget repopulates them. `combat_phase` already read
    // them earlier this frame, so the slate is safe to wipe.
    state.frame.hud_consume_rects.clear();
    if state.frame.damage_flash > 0.001 {
        hud::render_damage_flash(&mut ui, state.frame.damage_flash);
    }
    hud::render_hud(
        &mut ui,
        &state.world,
        &state.rift,
        &state.player_state,
        ui_dt,
        state.frame.level_up_flash,
        state.floor.in_hub,
    );
    state.selection.frame_target_ui(
        &mut ui,
        &mut state.net,
        &mut state.chat,
        ui_dt,
        &mut state.frame.hud_consume_rects,
    );
    if let Some(slot_idx) = hud::render_ability_bar(
        &mut ui,
        &state.player_state.abilities,
        state.player_state.experience.level,
        state.player_state.resource_pct * state.player_state.stats().max_resource,
        state.player_state.stats(),
        state.player_state.ability_mods(),
        &state.player_state.talents,
        &mut state.frame.hud_consume_rects,
        // Highlight whichever slot is mid-targeting so the
        // player has a clear "you're aiming this one" cue.
        state
            .frame
            .targeting
            .as_ref()
            .map(|t| t.slot_index)
            .or_else(|| state.frame.entity_targeting.as_ref().map(|t| t.slot_index)),
    ) {
        // Click on a HUD bar slot opens the spellbook with that
        // slot pre-targeted; the next pool click assigns directly
        // without the two-step picker.
        state.spellbook.open_for_slot(slot_idx as u8);
    }
    let mut pending_bloom: Option<bool> = None;
    let mut pending_ssao: Option<bool> = None;
    let mut pending_volumetrics: Option<bool> = None;
    let mut pending_vsync: Option<bool> = None;
    let mut pending_resolution: Option<rift_ui_types::settings::DisplayResolution> = None;
    hud::render_enemy_health_bars(&mut ui, &state.world, view_proj, ui_dt);
    if state.floor.in_hub {
        hud::render_hub_remote_player_names(&mut ui, &state.world, view_proj, &state.selection);
    } else {
        if state.rift.boss_spawned && !state.rift.boss_killed && !state.rift.floor_complete {
            hud::render_boss_arrow(
                &mut ui,
                &state.world,
                view_proj,
                state.floor_mgr.boss_room_center,
            );
        }
        if let Some(portal_pos) = portal_compass_pos {
            hud::render_portal_compass(&mut ui, &state.world, view_proj, portal_pos);
        }
        hud::render_remote_player_health_bars(&mut ui, &state.world, view_proj, ui_dt);
        hud::render_minion_health_bars(&mut ui, &state.world, view_proj, ui_dt);
    }
    // Alt-hold loot nameplates. Drawn after world HP bars so
    // labels sort on top, before the minimap / inventory so an
    // open bag still occludes them.
    hud::render_loot_labels(
        &mut ui,
        &mut state.loot,
        view_proj,
        &mut state.frame.hud_consume_rects,
    );
    let (minimap_zone_title, minimap_zone_detail) = if state.floor.in_hub {
        ("HUB", String::from("SANCTUARY"))
    } else {
        let mood = state
            .floor_mgr
            .dungeon
            .as_ref()
            .map(|floor| floor.mood.display_name())
            .unwrap_or("UNKNOWN DEPTH");
        ("RIFT", format!("{} - LEVEL {}", mood, state.rift.floor))
    };
    let minimap_rect = hud::render_minimap(
        &mut ui,
        &state.world,
        &state.floor_mgr.nav_grid,
        state.floor_mgr.dungeon.as_ref(),
        &mut state.floor_mgr.minimap_seen,
        minimap_zone_title,
        &minimap_zone_detail,
        state.floor.in_hub,
        state.minimap_zoom,
        player_facing,
        hub_portal_pos,
    );
    let exit_vote_active_for_controls = state.exit_vote.as_ref().map(|v| v.active).unwrap_or(false);
    render_minimap_controls(
        &mut ui,
        minimap_rect,
        &mut state.minimap_zoom,
        state.player_state.talents.unspent_points,
        &mut state.talents_panel,
        &mut state.spellbook,
        &mut state.inventory_ui.open,
        &mut state.pause,
        state.loot.stash_session,
        exit_vote_active_for_controls,
        &mut state.frame.hud_consume_rects,
    );
    state.combat_text.render(&mut ui, view_proj);

    // Combat-meter panel (bottom-right). Only renders inside
    // a rift — the hub is meter-free. Drawn *before* the
    // inventory so the inventory's bag/equip panels sit on
    // top of the meter when the player opens the bag (the
    // panels overlap in the bottom-right of the screen).
    let in_rift = !state.floor.in_hub;
    state.meters.frame(&mut ui, in_rift);

    if !state.loot.stash_session
        && ui
            .input()
            .key_just_pressed(rift_engine::ui::im::ImKey::KeyI)
    {
        state.inventory_ui.open = !state.inventory_ui.open;
    }

    crate::game::inventory::frame(
        &mut ui,
        &mut state.inventory_ui,
        &state.loot.items,
        &state.loot.equipment,
        &mut state.loot.pending_equip_requests,
        state.loot.stash_session,
        &state.loot.stash_tabs,
        &mut state.loot.pending_stash_requests,
        &state.player_state,
        &mut state.net.pending_consume_bag_idx,
        &mut state.talents_panel.open,
    );

    // Spellbook toggle (B) — open / close the loadout editor.
    // Suppressed while a stash session is active so B doesn't
    // double-bind alongside the inventory drag context.
    if !state.loot.stash_session
        && ui
            .input()
            .key_just_pressed(rift_engine::ui::im::ImKey::KeyB)
    {
        state.spellbook.toggle();
    }
    if let Some(action) = state.spellbook.frame(
        &mut ui,
        &state.player_state.loadout,
        state.player_state.experience.level,
        &state.player_state.talents,
        state.player_state.stats(),
        state.player_state.ability_mods(),
    ) {
        match action {
            spellbook::SpellbookAction::AssignSlot {
                slot_index,
                ability_id,
            } => {
                state.net.pending_loadout_changes.push((
                    slot_index,
                    rift_game::abilities::AbilityWireId::new(ability_id),
                ));
            }
        }
    }

    // Talents panel toggle (N) — open / close the talent tree.
    // Suppressed while a stash session is active for the same
    // reason as the spellbook bind, and while an exit vote is
    // active (N doubles as the "No" vote in that flow).
    //
    // Open path uses the host's text-capture-gated polling so
    // typing "n" in chat / inventory rename can't open the
    // panel. Close path uses *raw* polling because the panel,
    // once open, sets text-capture itself (to silence WASD /
    // hotbar polling for the whole modal) — without the raw
    // read, the close N would be swallowed by the very flag
    // the panel set on its own behalf.
    let exit_vote_active = state.exit_vote.as_ref().map(|v| v.active).unwrap_or(false);
    if !state.loot.stash_session && !exit_vote_active {
        let n_open = !pre_talents_open
            && ui
                .input()
                .key_just_pressed(rift_engine::ui::im::ImKey::KeyN);
        let n_close = pre_talents_open
            && ui
                .input()
                .key_just_pressed_raw(rift_engine::ui::im::ImKey::KeyN);
        if n_open || n_close {
            state.talents_panel.toggle();
        }
    }
    {
        let view = crate::game::talent_tree::build_talent_view(&state.player_state.talents);
        if let Some(action) =
            rift_ui::talents::frame_talent_panel(&mut ui, &mut state.talents_panel, &view)
        {
            match action {
                rift_ui_types::talents::TalentTreeAction::Invest { talent_id } => {
                    state.net.pending_talent_invests.push(talent_id);
                }
                rift_ui_types::talents::TalentTreeAction::Respec { talent_id } => {
                    // While a two-step consumable is armed,
                    // right-click on an invested talent fires
                    // `UseItem` against that node instead of
                    // the free dev respec. The token is
                    // consumed server-side only on a valid
                    // (non-orphaning) refund; rejection just
                    // leaves the token armed for retry.
                    if let Some(inv_idx) = state.net.pending_consume_bag_idx.take() {
                        state.net.pending_use_item.push((inv_idx, talent_id));
                    } else {
                        state.net.pending_talent_respecs.push(talent_id);
                    }
                }
                rift_ui_types::talents::TalentTreeAction::RespecAll => {
                    state.net.pending_talent_respec_all = true;
                }
                rift_ui_types::talents::TalentTreeAction::Close => {
                    state.talents_panel.close();
                    // Closing the panel also cancels any armed
                    // two-step consumable so it doesn't bleed
                    // into the next time the player opens the
                    // tree.
                    state.net.pending_consume_bag_idx = None;
                }
            }
        }
        // Esc anywhere also cancels an armed consumable, even
        // when the panel itself doesn't emit a Close action.
        if state.net.pending_consume_bag_idx.is_some()
            && ui
                .input()
                .key_just_pressed(rift_engine::ui::im::ImKey::Escape)
        {
            state.net.pending_consume_bag_idx = None;
        }
    }

    // Portal prompt: rendered above the loot prompt so a player
    // standing inside both prompt radii sees both lines.
    if let Some(text) = state.frame.hud_prompt.take() {
        hud::render_hud_prompt(&mut ui, text);
    }

    // Difficulty step-up tooltip: shown whenever the local
    // player is in range of the boss-room exit portal and no
    // vote panel is currently up. Reads the next floor's
    // `FloorConfig` and renders the deltas above the F-prompt.
    let descend_prompt = std::mem::take(&mut state.frame.descend_prompt);
    let vote_active = state.exit_vote.as_ref().map(|v| v.active).unwrap_or(false);
    if descend_prompt && !vote_active {
        hud::render_descend_tooltip(&mut ui, state.rift.floor);
    }

    // Rift exit-vote panel.
    if let Some(vote) = state.exit_vote.as_ref() {
        if vote.active || vote.cooldown_remaining > 0.0 {
            hud::render_exit_vote(&mut ui, vote, state.net.our_net_id_cached);
        }
    }
    // Revive-shrine progress.
    if let Some(v) = state
        .shrines
        .visuals
        .iter()
        .filter(|v| v.channelers > 0 || v.progress > 0.0)
        .max_by(|a, b| {
            a.progress
                .partial_cmp(&b.progress)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    {
        hud::render_shrine_progress(&mut ui, v.progress, v.channelers, v.required.max(1));
    }
    if let Some((net_id, _)) = nearest_loot {
        if let Some(drop) = state.loot.drops.iter().find(|d| d.net_id == net_id) {
            let c = drop.item.rarity.color();
            let prompt = format!("PRESS [F]: {}", drop.item.display_name());
            hud::render_loot_prompt(&mut ui, &prompt, Color::rgba(c[0], c[1], c[2], 1.0));
        }
    }

    // Fade overlay sits on top of every other HUD element.
    if state.frame.transition_fade > 0.001 {
        hud::render_fade_to_black(&mut ui, state.frame.transition_fade);
    }

    // Chat HUD — rendered last so the input field overlays
    // every other widget but *before* the fade overlay so a
    // black-out hides it. Keys handled before the panel so
    // open/close edges land in the same frame as the draw.
    state
        .chat
        .handle_keys(&mut ui, &mut state.net.pending_chats_out);
    state.chat.frame(&mut ui, 0.0);

    // Drain any chat slash commands the chat HUD didn't
    // recognise, routing them through the party UI. Anything
    // the party UI also rejects becomes a local "Unknown
    // command" system line so the player gets feedback.
    let pending: Vec<(String, String)> = std::mem::take(&mut state.chat.pending_slash);
    for (head, body) in pending {
        match state.party.try_handle_slash(&head, &body) {
            Some(Ok(msg)) => state.net.pending_party_msgs.push(msg),
            Some(Err(local)) => state.chat.push_local_system(&local),
            None => state
                .chat
                .push_local_system(&format!("Unknown command: /{head}")),
        }
    }

    // Party HUD: top-left frames + portal/confirm modals + any
    // toasts. Rendered after chat so its modals sit above the
    // scrollback panel.
    state
        .party
        .frame(&mut ui, &mut state.net, &mut state.chat, &mut state.frame);

    // Pause menu / settings modal. Drawn last so it sits on
    // top of every other in-game HUD widget. Escape toggles
    // the menu open when no other modal already owns Escape
    // (chat typing, inventory text edit, spellbook open,
    // exit-vote panel, etc.). Inlined here rather than
    // delegated to a helper because `ui` is holding a mutable
    // borrow into `state.ui_state`, so a `&mut GameState`
    // helper signature would conflict on the second borrow.
    {
        let escape_busy = state.chat.is_typing() || state.inventory_ui.wants_text_input();
        // The talents panel sets text-capture for itself
        // (silences WASD / hotbar polling for the modal), so
        // its close-Escape has to come from the raw read.
        // Other sub-modals don't flip text-capture, so the
        // standard gated read still hits them.
        let escape_pressed = ui
            .input()
            .key_just_pressed(rift_engine::ui::im::ImKey::Escape)
            || (pre_talents_open
                && ui
                    .input()
                    .key_just_pressed_raw(rift_engine::ui::im::ImKey::Escape));

        // Escape is a single state-transition this frame —
        // *either* it opens the menu, *or* it closes one of
        // the open sub-modals, never both. Handled here (not
        // in the widgets) so the open-edge isn't immediately
        // re-consumed by the widget that just rendered for
        // the first time, which would slam the menu shut on
        // the same frame it opened.
        //
        // Priority: settings → pause menu → spellbook →
        // inventory → (otherwise) open the pause menu. The
        // sub-modal flags (`pre_spellbook_open` /
        // `pre_inventory_open`) were captured at the very
        // top of this phase so a widget's self-close doesn't
        // leak through.
        if escape_pressed {
            if state.pause.settings_open {
                state.pause.settings_open = false;
                state.pause.menu_open = true;
            } else if state.pause.menu_open {
                state.pause.menu_open = false;
            } else if pre_talents_open {
                state.talents_panel.close();
            } else if pre_spellbook_open {
                state.spellbook.close();
            } else if pre_inventory_open {
                state.inventory_ui.open = false;
            } else if state.selection.selected().is_some() {
                state.selection.clear_selected();
            } else if !escape_busy {
                state.pause.menu_open = true;
            }
        }

        if state.pause.settings_open {
            let view = rift_ui_types::settings::SettingsView {
                master_volume: state.pause.master_volume,
                shadows_enabled: state.pause.shadows_enabled,
                height_shadows_enabled: state.pause.height_shadows_enabled,
                bloom_enabled: state.pause.bloom_enabled,
                ssao_enabled: state.pause.ssao_enabled,
                volumetrics_enabled: state.pause.volumetrics_enabled,
                vsync_enabled: state.pause.vsync_enabled,
                display_resolutions: &display_resolutions_snapshot,
                selected_resolution: selected_resolution_snapshot,
            };
            for action in rift_ui::settings::frame_settings(&mut ui, &view) {
                use rift_ui_types::settings::SettingsAction;
                match action {
                    SettingsAction::SetMasterVolume(v) => {
                        state.pause.master_volume = v.clamp(0.0, 1.0);
                        if let Some(audio) = state.audio.as_mut() {
                            audio.set_master_volume(state.pause.master_volume);
                        }
                    }
                    SettingsAction::SetShadowsEnabled(enabled) => {
                        state.pause.shadows_enabled = enabled;
                        renderer.shadows_enabled = enabled;
                    }
                    SettingsAction::SetHeightShadowsEnabled(enabled) => {
                        state.pause.height_shadows_enabled = enabled;
                        renderer.height_shadows_enabled = enabled;
                    }
                    SettingsAction::SetBloomEnabled(enabled) => {
                        state.pause.bloom_enabled = enabled;
                        pending_bloom = Some(enabled);
                    }
                    SettingsAction::SetSsaoEnabled(enabled) => {
                        state.pause.ssao_enabled = enabled;
                        pending_ssao = Some(enabled);
                    }
                    SettingsAction::SetVolumetricsEnabled(enabled) => {
                        state.pause.volumetrics_enabled = enabled;
                        pending_volumetrics = Some(enabled);
                    }
                    SettingsAction::SetVsyncEnabled(enabled) => {
                        state.pause.vsync_enabled = enabled;
                        pending_vsync = Some(enabled);
                    }
                    SettingsAction::SetDisplayResolution(resolution) => {
                        state.pause.display_resolution = resolution;
                        pending_resolution = Some(resolution);
                    }
                    SettingsAction::Close => {
                        state.pause.settings_open = false;
                        state.pause.menu_open = true;
                    }
                }
            }
        } else if state.pause.menu_open {
            if let Some(action) =
                rift_ui::pause_menu::frame_pause_menu(&mut ui, !state.floor.in_hub)
            {
                use rift_ui_types::pause_menu::PauseMenuAction;
                match action {
                    PauseMenuAction::Resume => state.pause.menu_open = false,
                    PauseMenuAction::OpenSettings => {
                        state.pause.menu_open = false;
                        state.pause.settings_open = true;
                    }
                    PauseMenuAction::ExitToHub => {
                        state.pause.menu_open = false;
                        state.net.transition = Some(crate::game::NetTransitionRequest::ReturnToHub);
                    }
                    PauseMenuAction::ExitToCharacterSelect => {
                        state.pause.menu_open = false;
                        state.pause.request_character_select = true;
                    }
                    PauseMenuAction::ExitGame => {
                        state.pause.request_quit = true;
                    }
                }
            }
        }
    }

    let _ = ui.end();

    if let Some(enabled) = pending_bloom {
        renderer.set_bloom_enabled(enabled);
    }
    if let Some(enabled) = pending_ssao {
        renderer.set_ssao_enabled(enabled);
    }
    if let Some(enabled) = pending_volumetrics {
        renderer.set_volumetrics_enabled(enabled);
    }
    if let Some(enabled) = pending_vsync {
        if let Err(e) = renderer.set_vsync_enabled(enabled) {
            log::warn!("Failed to apply VSync setting: {e}");
        }
    }
    if let Some(resolution) = pending_resolution {
        renderer.request_display_resolution(rift_engine::renderer::forward::DisplayResolution {
            width: resolution.width,
            height: resolution.height,
        });
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MinimapToolIcon {
    ZoomOut,
    ZoomIn,
    Talents,
    Spellbook,
    Inventory,
    Settings,
}

fn render_minimap_controls(
    ui: &mut rift_engine::ui::im::Ui<'_>,
    minimap_rect: rift_engine::ui::im::Rect,
    minimap_zoom: &mut f32,
    unspent_talent_points: u32,
    talents_panel: &mut rift_ui_types::talents::TalentPanelState,
    spellbook: &mut spellbook::SpellbookUi,
    inventory_open: &mut bool,
    pause: &mut crate::game::pause::PauseState,
    stash_session: bool,
    exit_vote_active: bool,
    consume_rects: &mut Vec<rift_engine::ui::im::Rect>,
) {
    use rift_engine::ui::im::{Color, Id, Rect};

    let theme = *ui.theme();
    let s = theme.scale;
    let btn = 31.0 * s;
    let gap = 5.0 * s;
    let pad = 6.0 * s;
    let count = 6.0_f32;
    let toolbar_w = count * btn + (count - 1.0) * gap + pad * 2.0;
    let toolbar_h = btn + pad * 2.0;
    let toolbar = Rect::from_xywh(
        minimap_rect.max.x - toolbar_w,
        minimap_rect.max.y + 6.0 * s,
        toolbar_w,
        toolbar_h,
    );

    ui.draw_rounded_radial_rect_noisy(
        toolbar,
        0.0,
        Color::rgba(0.015, 0.017, 0.022, 0.88),
        Color::rgba(0.070, 0.058, 0.045, 0.88),
    );
    ui.draw_outline(toolbar, 1.0, theme.colors.border_stone);
    consume_rects.push(toolbar);

    let mut x = toolbar.x() + pad;
    let y = toolbar.y() + pad;
    let mut rect_for = || {
        let rect = Rect::from_xywh(x, y, btn, btn);
        x += btn + gap;
        rect
    };

    let zoom_out = rect_for();
    if icon_button(
        ui,
        Id::root("rift::minimap::zoom_out"),
        zoom_out,
        MinimapToolIcon::ZoomOut,
        false,
        false,
        *minimap_zoom > 0.66,
    )
    .clicked
    {
        *minimap_zoom = (*minimap_zoom - 0.15).clamp(0.65, 1.75);
    }
    if zoom_out.contains(ui.mouse_pos()) {
        tooltip(ui, zoom_out, "Zoom out minimap", "-");
    }

    let zoom_in = rect_for();
    if icon_button(
        ui,
        Id::root("rift::minimap::zoom_in"),
        zoom_in,
        MinimapToolIcon::ZoomIn,
        false,
        false,
        *minimap_zoom < 1.74,
    )
    .clicked
    {
        *minimap_zoom = (*minimap_zoom + 0.15).clamp(0.65, 1.75);
    }
    if zoom_in.contains(ui.mouse_pos()) {
        tooltip(ui, zoom_in, "Zoom in minimap", "+");
    }

    let talents_rect = rect_for();
    let talents_enabled = !stash_session && !exit_vote_active;
    if icon_button(
        ui,
        Id::root("rift::hud::tool_talents"),
        talents_rect,
        MinimapToolIcon::Talents,
        talents_panel.open,
        unspent_talent_points > 0,
        talents_enabled,
    )
    .clicked
    {
        talents_panel.toggle();
    }
    if talents_rect.contains(ui.mouse_pos()) {
        tooltip(ui, talents_rect, "Talents", "N");
    }

    let spellbook_rect = rect_for();
    if icon_button(
        ui,
        Id::root("rift::hud::tool_spellbook"),
        spellbook_rect,
        MinimapToolIcon::Spellbook,
        spellbook.open(),
        false,
        !stash_session,
    )
    .clicked
    {
        spellbook.toggle();
    }
    if spellbook_rect.contains(ui.mouse_pos()) {
        tooltip(ui, spellbook_rect, "Spellbook", "B");
    }

    let inventory_rect = rect_for();
    if icon_button(
        ui,
        Id::root("rift::hud::tool_inventory"),
        inventory_rect,
        MinimapToolIcon::Inventory,
        *inventory_open && !stash_session,
        false,
        !stash_session,
    )
    .clicked
    {
        *inventory_open = !*inventory_open;
    }
    if inventory_rect.contains(ui.mouse_pos()) {
        tooltip(ui, inventory_rect, "Inventory", "I");
    }

    let settings_rect = rect_for();
    if icon_button(
        ui,
        Id::root("rift::hud::tool_settings"),
        settings_rect,
        MinimapToolIcon::Settings,
        pause.settings_open,
        false,
        true,
    )
    .clicked
    {
        pause.menu_open = false;
        pause.settings_open = true;
    }
    if settings_rect.contains(ui.mouse_pos()) {
        tooltip(ui, settings_rect, "Settings", "Esc");
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct IconButtonResponse {
    clicked: bool,
}

fn icon_button(
    ui: &mut rift_engine::ui::im::Ui<'_>,
    id: rift_engine::ui::im::Id,
    rect: rift_engine::ui::im::Rect,
    icon: MinimapToolIcon,
    active: bool,
    attention: bool,
    enabled: bool,
) -> IconButtonResponse {
    use rift_engine::ui::im::Color;

    let theme = *ui.theme();
    let hovered = ui.interact_hover(id, rect);
    let clicked = enabled && hovered && ui.input().left_clicked();
    let base = if !enabled {
        Color::rgba(0.045, 0.045, 0.050, 0.74)
    } else if active {
        Color::rgba(0.30, 0.18, 0.10, 0.96)
    } else if hovered {
        Color::rgba(0.18, 0.145, 0.095, 0.94)
    } else {
        Color::rgba(0.095, 0.082, 0.065, 0.90)
    };
    ui.draw_rounded_rect(rect, 3.0 * theme.scale, base);
    ui.draw_rounded_outline(rect, 3.0 * theme.scale, 1.0, theme.colors.border_stone);
    if active || attention {
        ui.draw_rounded_outline(
            rect,
            3.0 * theme.scale,
            if attention { 2.0 } else { 1.4 },
            Color::rgba(0.95, 0.72, 0.28, if attention { 0.80 } else { 0.55 }),
        );
    }
    draw_tool_icon(ui, rect, icon, enabled, attention);
    IconButtonResponse { clicked }
}

fn tooltip(
    ui: &mut rift_engine::ui::im::Ui<'_>,
    anchor: rift_engine::ui::im::Rect,
    title: &str,
    shortcut: &str,
) {
    use rift_engine::ui::im::{Pos2, Tooltip, TooltipLine};

    let theme = *ui.theme();
    let lines = [TooltipLine::new(
        shortcut,
        theme.fonts.size_sm,
        theme.colors.text_dim,
    )];
    Tooltip::new()
        .header(title)
        .min_width(116.0)
        .anchor_to(anchor)
        .prefer_left(true)
        .show(ui, Pos2::new(0.0, 0.0), &lines);
}

fn draw_tool_icon(
    ui: &mut rift_engine::ui::im::Ui<'_>,
    rect: rift_engine::ui::im::Rect,
    icon: MinimapToolIcon,
    enabled: bool,
    attention: bool,
) {
    use rift_engine::ui::im::{Color, Pos2, Rect};

    let theme = *ui.theme();
    let s = theme.scale;
    let c = rect.center();
    let color = if enabled {
        Color::rgba(0.93, 0.84, 0.64, 1.0)
    } else {
        theme.colors.text_dim
    };
    let accent = if attention {
        Color::rgba(1.0, 0.74, 0.22, 1.0)
    } else {
        color
    };
    let line = (1.7 * s).max(1.0);
    match icon {
        MinimapToolIcon::ZoomOut => {
            let w = 9.0 * s;
            ui.draw_line(
                Pos2::new(c.x - w, c.y),
                Pos2::new(c.x + w, c.y),
                line,
                color,
            );
        }
        MinimapToolIcon::ZoomIn => {
            let w = 9.0 * s;
            ui.draw_line(
                Pos2::new(c.x - w, c.y),
                Pos2::new(c.x + w, c.y),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x, c.y - w),
                Pos2::new(c.x, c.y + w),
                line,
                color,
            );
        }
        MinimapToolIcon::Talents => {
            let top = Pos2::new(c.x, c.y - 8.0 * s);
            let mid = Pos2::new(c.x, c.y - 1.0 * s);
            let left = Pos2::new(c.x - 7.0 * s, c.y + 7.0 * s);
            let right = Pos2::new(c.x + 7.0 * s, c.y + 7.0 * s);
            ui.draw_line(top, mid, line, accent);
            ui.draw_line(mid, left, line, accent);
            ui.draw_line(mid, right, line, accent);
            ui.draw_circle(top, 2.3 * s, accent);
            ui.draw_circle(left, 2.3 * s, accent);
            ui.draw_circle(right, 2.3 * s, accent);
            if attention {
                ui.draw_circle(
                    Pos2::new(rect.max.x - 6.0 * s, rect.y() + 6.0 * s),
                    3.0 * s,
                    accent,
                );
            }
        }
        MinimapToolIcon::Spellbook => {
            let book = Rect::from_xywh(c.x - 10.0 * s, c.y - 8.0 * s, 20.0 * s, 16.0 * s);
            ui.draw_outline(book, line, color);
            ui.draw_line(
                Pos2::new(c.x, book.y()),
                Pos2::new(c.x, book.max.y),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(book.x() + 4.0 * s, book.y() + 5.0 * s),
                Pos2::new(c.x - 2.0 * s, book.y() + 5.0 * s),
                1.0,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x + 3.0 * s, book.y() + 5.0 * s),
                Pos2::new(book.max.x - 4.0 * s, book.y() + 5.0 * s),
                1.0,
                color,
            );
        }
        MinimapToolIcon::Inventory => {
            let bag = Rect::from_xywh(c.x - 9.0 * s, c.y - 4.0 * s, 18.0 * s, 13.0 * s);
            ui.draw_outline(bag, line, color);
            ui.draw_line(
                Pos2::new(c.x - 5.0 * s, c.y - 4.0 * s),
                Pos2::new(c.x - 3.0 * s, c.y - 9.0 * s),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x + 5.0 * s, c.y - 4.0 * s),
                Pos2::new(c.x + 3.0 * s, c.y - 9.0 * s),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - 3.0 * s, c.y - 9.0 * s),
                Pos2::new(c.x + 3.0 * s, c.y - 9.0 * s),
                line,
                color,
            );
        }
        MinimapToolIcon::Settings => {
            ui.draw_circle(c, 7.0 * s, Color::rgba(0.0, 0.0, 0.0, 0.0));
            ui.draw_circle(c, 3.0 * s, color);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                let inner = Pos2::new(c.x + a.cos() * 6.0 * s, c.y + a.sin() * 6.0 * s);
                let outer = Pos2::new(c.x + a.cos() * 9.0 * s, c.y + a.sin() * 9.0 * s);
                ui.draw_line(inner, outer, line, color);
            }
        }
    }
}
