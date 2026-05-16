//! Hub anvil interaction.

use rift_engine::ecs::components::{LocalPlayer, Player, Transform};
use rift_engine::Input;
use rift_ui_types::inventory::InventoryUiState;

use crate::game::floor::FloorManager;
use crate::game::sub_state::{LootClientState, NetState};

const INTERACT_RADIUS: f32 = super::stash_system::INTERACT_RADIUS;

pub fn tick(
    world: &hecs::World,
    floor_mgr: &FloorManager,
    input: &Input,
    mp_inventory_ui: &mut InventoryUiState,
    net: &mut NetState,
    loot: &mut LootClientState,
    hud_prompt: &mut Option<&'static str>,
) {
    use winit::keyboard::KeyCode;

    let Some(anvil_pos) = floor_mgr.anvil_pos else {
        if loot.anvil_session {
            loot.anvil_session = false;
            mp_inventory_ui.open = false;
            net.anvil_session_requests.push(false);
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
    let in_range = player_pos.distance(anvil_pos) <= INTERACT_RADIUS;

    if in_range {
        *hud_prompt = Some(if loot.anvil_session {
            "PRESS [F] TO CLOSE ANVIL"
        } else {
            "PRESS [F] TO USE ANVIL"
        });
        if input.key_just_pressed(KeyCode::KeyF) {
            loot.anvil_session = !loot.anvil_session;
            mp_inventory_ui.open = loot.anvil_session;
            net.anvil_session_requests.push(loot.anvil_session);
        }
    } else if loot.anvil_session {
        loot.anvil_session = false;
        mp_inventory_ui.open = false;
        net.anvil_session_requests.push(false);
    }
}
