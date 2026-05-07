//! Phase 4 of the per-frame `GameState::update` pipeline.
//!
//! HUD + inventory immediate-mode UI pass. One `Ui::begin/end`
//! scope so layer order and `OverlayBatch` ownership stay
//! coherent across every widget.

use glam::Vec3;
use rift_engine::ecs::components::{LocalPlayer, Player, Transform};
use rift_engine::{Input, Renderer};

use crate::game::hud;
use crate::game::spellbook;
use crate::game::state::GameState;

pub fn tick(state: &mut GameState, renderer: &mut Renderer, input: &Input) {
    renderer.overlay_batch.clear();
    let (sw, sh) = renderer.screen_size();

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
    state.mp_inventory_ui.frame(
        &mut ui,
        &state.loot.items,
        &state.loot.equipment,
        &mut state.loot.pending_equip_requests,
        state.loot.stash_session,
        &state.loot.stash_items,
        &mut state.loot.pending_stash_requests,
        &state.player_state,
    );

    // Spellbook toggle (B) — open / close the loadout editor.
    // Suppressed while a stash session is active so B doesn't
    // double-bind alongside the inventory drag context.
    if !state.loot.stash_session
        && ui.input().key_just_pressed(winit::keyboard::KeyCode::KeyB)
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
    let vote_active = state
        .exit_vote
        .as_ref()
        .map(|v| v.active)
        .unwrap_or(false);
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
    let _ = ui.end();
}
