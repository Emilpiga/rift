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
    if let Some(slot_idx) = hud::render_ability_bar(
        &mut ui,
        &state.player_state.abilities,
        state.player_state.experience.level,
        state.player_state.resource_pct * state.player_state.stats().max_resource,
        state.player_state.stats(),
        state.player_state.ability_mods(),
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
    // Small HUD button next to the ability bar that opens the
    // talent panel. Sits to the LEFT of the plaque so it doesn't
    // compete with the standard 1..6 / Space layout. Tinted
    // gold when the player has unspent points so the affordance
    // is visible without an explicit notification.
    {
        use rift_engine::ui::im::{Button, Color, Id, Rect};
        let theme = *ui.theme();
        let s = theme.scale;
        let screen = ui.screen_size();
        let plaque_w = rift_ui::hud::PLAQUE_W_BASE * s;
        let plaque_h = rift_ui::hud::PLAQUE_H_BASE * s;
        let plaque_x = (screen.x - plaque_w) * 0.5;
        let plaque_y = screen.y - plaque_h - rift_ui::hud::BOTTOM_GAP_BASE * s;
        let btn_w = 96.0 * s;
        let btn_h = 32.0 * s;
        let btn_rect = Rect::from_xywh(
            plaque_x - btn_w - 8.0 * s,
            plaque_y + (plaque_h - btn_h) * 0.5,
            btn_w,
            btn_h,
        );
        let has_unspent = state.player_state.talents.unspent_points > 0;
        let label = if has_unspent {
            format!("Talents ({})", state.player_state.talents.unspent_points)
        } else {
            "Talents".to_string()
        };
        let resp = if has_unspent {
            Button::primary(&label)
        } else {
            Button::new(&label)
        }
        .show_with_id(&mut ui, Id::root("rift::hud::talents_btn"), btn_rect);
        if resp.clicked {
            state.talents_panel.toggle();
        }
        // Suppress the next-frame basic-attack cast when the
        // cursor sits on the button.
        state.frame.hud_consume_rects.push(btn_rect);
        // Faint gold pulse when unspent points exist — helps the
        // player notice. Painted as a translucent rim.
        if has_unspent {
            ui.draw_rounded_outline(btn_rect, 6.0, 2.0, Color::rgba(0.92, 0.78, 0.32, 0.55));
        }
    }
    hud::render_enemy_health_bars(&mut ui, &state.world, view_proj, ui_dt);
    if !state.floor.in_hub {
        hud::render_boss_arrow(&mut ui, &state.world, view_proj);
        if let Some(portal_pos) = portal_compass_pos {
            hud::render_portal_compass(&mut ui, &state.world, view_proj, portal_pos);
        }
        hud::render_remote_player_health_bars(&mut ui, &state.world, view_proj, ui_dt);
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
    hud::render_minimap(
        &mut ui,
        &state.world,
        &state.floor_mgr.nav_grid,
        state.floor_mgr.dungeon.as_ref(),
        &mut state.floor_mgr.minimap_seen,
        minimap_zone_title,
        &minimap_zone_detail,
        state.floor.in_hub,
        player_facing,
        hub_portal_pos,
    );
    state.combat_text.render(&mut ui, view_proj);

    // Combat-meter panel (bottom-right). Only renders inside
    // a rift — the hub is meter-free. Drawn *before* the
    // inventory so the inventory's bag/equip panels sit on
    // top of the meter when the player opens the bag (the
    // panels overlap in the bottom-right of the screen).
    let in_rift = !state.floor.in_hub;
    state.meters.frame(&mut ui, in_rift);

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
            } else if !escape_busy {
                state.pause.menu_open = true;
            }
        }

        if state.pause.settings_open {
            let view = rift_ui_types::settings::SettingsView {
                master_volume: state.pause.master_volume,
                shadows_enabled: state.pause.shadows_enabled,
                height_shadows_enabled: state.pause.height_shadows_enabled,
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
}
