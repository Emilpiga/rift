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
    let smoothed_fps = FPS_LAST.with(|cell| {
        let last = cell.replace(Some(now));
        match last {
            Some(prev) => {
                let dt = now.duration_since(prev).as_secs_f32().max(1e-4);
                let inst_fps = 1.0 / dt;
                FPS_EMA.with(|e| {
                    // Exponential moving average with ~0.5 s
                    // time constant — fast enough to react to
                    // hitches, slow enough not to flicker.
                    let prev_ema = e.get();
                    let alpha = (dt / 0.5).clamp(0.0, 1.0);
                    let new_ema = prev_ema * (1.0 - alpha) + inst_fps * alpha;
                    e.set(new_ema);
                    new_ema
                })
            }
            None => 60.0,
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

    use rift_engine::ui::im::{Color, Ui, DEFAULT_THEME};
    let mut ui = Ui::begin(
        &mut renderer.overlay_batch,
        input,
        &mut state.ui_state,
        &DEFAULT_THEME,
        sw,
        sh,
    );
    if state.frame.damage_flash > 0.001 {
        hud::render_damage_flash(&mut ui, state.frame.damage_flash);
    }
    hud::render_hud(
        &mut ui,
        &state.world,
        &state.rift,
        &state.player_state,
        state.frame.level_up_flash,
        state.floor.in_hub,
    );
    if let Some(slot_idx) = hud::render_ability_bar(
        &mut ui,
        &state.player_state.abilities,
        state.player_state.experience.level,
        state.player_state.resource_pct * state.player_state.stats().max_resource,
        state.player_state.stats(),
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
    hud::render_enemy_health_bars(&mut ui, &state.world, view_proj);
    if !state.floor.in_hub {
        hud::render_boss_arrow(&mut ui, &state.world, view_proj);
        hud::render_remote_player_health_bars(&mut ui, &state.world, view_proj);
    }
    hud::render_minimap(
        &mut ui,
        &state.world,
        &state.floor_mgr.nav_grid,
        player_facing,
        hub_portal_pos,
    );
    state.combat_text.render(&mut ui, view_proj);
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
    ) {
        match action {
            spellbook::SpellbookAction::AssignSlot {
                slot_index,
                ability_id,
            } => {
                state
                    .net
                    .pending_loadout_changes
                    .push((slot_index, ability_id));
            }
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

    // Combat-meter panel (bottom-right). Only renders inside
    // a rift — the hub is meter-free.
    let in_rift = !state.floor.in_hub;
    state.meters.frame(&mut ui, in_rift);

    let _ = ui.end();
}
