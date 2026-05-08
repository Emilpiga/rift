//! Hub stash chest interaction.
//!
//! Fires the press-F prompt when the local player walks within
//! [`INTERACT_RADIUS`] of the hub chest, toggles the local
//! `stash_open` flag on F, and queues `OpenStash` / `CloseStash`
//! for the binary to forward to the server. Auto-closes the
//! panel if the player walks out of range while it's open.

use rift_engine::ecs::components::{LocalPlayer, Player, Transform};
use rift_engine::Input;

use crate::game::floor::FloorManager;
use crate::game::inventory::MpInventoryUI;
use crate::game::sub_state::{LootClientState, NetState};

/// Walk-to-interact range for the hub stash chest. Slightly
/// tighter than the portal radius so the prompt only fires
/// when the player is unmistakably standing in front of the
/// chest, not just passing nearby.
pub const INTERACT_RADIUS: f32 = 1.8;

/// Per-frame stash chest tick. Reads / writes
/// `loot.stash_session` (the server-mirrored flag), pushes
/// open / close requests onto `net.stash_session_requests`,
/// and forces `mp_inventory_ui.open` while a session is
/// active.
#[allow(clippy::too_many_arguments)]
pub fn tick(
    world: &hecs::World,
    floor_mgr: &FloorManager,
    input: &Input,
    mp_inventory_ui: &mut MpInventoryUI,
    net: &mut NetState,
    loot: &mut LootClientState,
    hud_prompt: &mut Option<&'static str>,
) {
    use winit::keyboard::KeyCode;

    let Some(chest_pos) = floor_mgr.stash_chest_pos else {
        // Not in the hub (or chest hasn't spawned yet). Force-
        // close any lingering session.
        if loot.stash_session {
            loot.stash_session = false;
            mp_inventory_ui.open = false;
            net.stash_session_requests.push(false);
        }
        return;
    };
    let Some(player_pos) = world
        .query::<(&Transform, &Player, &LocalPlayer)>()
        .iter()
        .map(|(_, (t, _, _))| t.position)
        .next()
    else {
        return;
    };
    let in_range = player_pos.distance(chest_pos) <= INTERACT_RADIUS;

    if in_range {
        *hud_prompt = Some(if loot.stash_session {
            "PRESS [F] TO CLOSE STASH"
        } else {
            "PRESS [F] TO OPEN STASH"
        });
        if input.key_just_pressed(KeyCode::KeyF) {
            loot.stash_session = !loot.stash_session;
            mp_inventory_ui.open = loot.stash_session;
            net.stash_session_requests.push(loot.stash_session);
            if loot.stash_session {
                log::info!("stash: opening");
            } else {
                log::info!("stash: closing");
                // Stale stash mirror is harmless but tidier
                // to drop on close.
                loot.stash_tabs.clear();
            }
        }
    } else if loot.stash_session {
        // Walked away — auto close.
        log::info!("stash: out of range, auto-closing");
        loot.stash_session = false;
        mp_inventory_ui.open = false;
        loot.stash_tabs.clear();
        net.stash_session_requests.push(false);
    }
}
