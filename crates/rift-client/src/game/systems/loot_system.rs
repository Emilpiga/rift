//! Ground loot interaction.
//!
//! Picks the closest drop inside [`PICKUP_RADIUS`] of the local
//! player every frame, queues `PickUpLoot` requests when F is
//! pressed, and tears down the visual + appends the rolled item
//! to the local mirror when the server confirms a claim.
//!
//! Free-standing functions taking explicit borrows of the
//! `GameState` slices they actually touch — no `&mut GameState`
//! so the systems can be called and tested in isolation.

use rift_engine::ecs::components::{LocalPlayer, Player, Transform};
use rift_engine::ui::CombatTextSystem;
use rift_engine::{Input, Renderer};

use crate::game::sub_state::LootClientState;

/// Walk-to-pickup range for ground loot drops. Mirrored on the
/// server as `rift_server::sim::PICKUP_RANGE`; we keep them
/// roughly in sync to avoid client-side prompts that the server
/// would reject.
pub const PICKUP_RADIUS: f32 = 1.8;

/// Per-frame ground-loot interaction. Picks the closest drop
/// inside [`PICKUP_RADIUS`] of the local player and, if the F key
/// was just pressed this frame, queues a `PickUpLoot` for the
/// binary to forward.
pub fn tick(
    world: &hecs::World,
    loot: &mut LootClientState,
    combat_text: &mut CombatTextSystem,
    input: &Input,
) {
    use winit::keyboard::KeyCode;

    let Some((net_id, _)) = nearest_drop(world, loot) else {
        return;
    };
    if !input.key_just_pressed(KeyCode::KeyF) {
        return;
    }
    // Local capacity check: if our mirror of the bag is already
    // full, don't even ship the request — show the same warning
    // the server would have sent. The server still enforces, so
    // a stale mirror only costs us one extra round-trip in the
    // worst case.
    if local_inventory_filled(loot) >= rift_net::messages::INVENTORY_CAPACITY {
        warn_inventory_full(world, combat_text);
        return;
    }
    // De-dupe: one in-flight request per drop.
    if !loot.pending_pickups.contains(&net_id) {
        loot.pending_pickups.push(net_id);
    }
}

/// Number of occupied bag slots in our local inventory mirror.
/// Matches the server's `count_filled` definition (`Some(_)`
/// slots only — sparse holes don't count).
pub fn local_inventory_filled(loot: &LootClientState) -> usize {
    loot.items.iter().filter(|s| s.is_some()).count()
}

/// Surface an "Inventory full" warning above the local player.
/// Called both proactively (client-side cap check before sending
/// `PickUpLoot`) and reactively (when the server replies with
/// `PickupRejected::InventoryFull`).
pub fn warn_inventory_full(world: &hecs::World, combat_text: &mut CombatTextSystem) {
    let pos = world
        .query::<(&Transform, &Player, &LocalPlayer)>()
        .iter()
        .map(|(_, (t, _, _))| t.position)
        .next();
    if let Some(pos) = pos {
        combat_text.spawn_notice(pos, "Inventory full", [1.0, 0.35, 0.25, 1.0]);
    }
    log::warn!("loot: inventory full — pickup blocked");
}

/// Closest loot drop inside [`PICKUP_RADIUS`] of the local player.
/// Returns the drop's `NetId` and the squared distance.
pub fn nearest_drop(
    world: &hecs::World,
    loot: &LootClientState,
) -> Option<(rift_net::NetId, f32)> {
    if loot.drops.is_empty() {
        return None;
    }
    let player_pos = world
        .query::<(&Transform, &Player, &LocalPlayer)>()
        .iter()
        .map(|(_, (t, _, _))| t.position)
        .next()?;
    let mut best: Option<(rift_net::NetId, f32)> = None;
    for drop in &loot.drops {
        let d2 = (drop.position - player_pos).length_squared();
        if d2 > PICKUP_RADIUS * PICKUP_RADIUS {
            continue;
        }
        if best.map_or(true, |(_, b)| d2 < b) {
            best = Some((drop.net_id, d2));
        }
    }
    best
}

/// Tear down the visual for a loot drop that was claimed (either
/// by the local player or another). If `add_to_local` is set, the
/// rolled item is also appended to our local inventory — the
/// server is the persistence authority, but the local mirror
/// lets the UI react instantly.
pub fn resolve_claim(
    loot: &mut LootClientState,
    renderer: &mut Renderer,
    loot_id: rift_net::NetId,
    add_to_local: bool,
) {
    let idx = loot.drops.iter().position(|d| d.net_id == loot_id);
    // Mark the id claimed unconditionally so the late-joiner
    // snapshot scan can't re-spawn the pillar from a stale
    // snapshot still in flight when the server despawned the
    // loot ECS row.
    loot.claimed_ids.insert(loot_id);
    let Some(idx) = idx else { return };
    let drop = loot.drops.swap_remove(idx);
    renderer.vfx_system.despawn(drop.pillar_emitter);
    renderer.vfx_system.despawn(drop.base_emitter);
    if add_to_local {
        log::info!(
            "loot picked up: {} (item-level {})",
            drop.item.display_name(),
            drop.item.ilvl
        );
        // Mirror the server's authoritative inventory so the UI
        // can react instantly. The server's `try_pickup_loot`
        // fills the *first empty slot* (so dropping an item then
        // picking another reuses the hole); duplicate that
        // placement here or the local UI flashes the item in the
        // wrong slot for one frame until the follow-up
        // `InventorySync` corrects it.
        if let Some(hole) = loot.items.iter_mut().find(|s| s.is_none()) {
            *hole = Some(drop.item);
        } else {
            loot.items.push(Some(drop.item));
        }
        log::debug!(
            "inventory: {} item(s) total",
            loot.items.iter().filter(|s| s.is_some()).count()
        );
    }
}

/// Handle a `WorldEvent::LootDropped` event (or the equivalent
/// snapshot-reconciliation row for a client that just joined).
///
/// Spawns the pillar + base loot beam VFX at `position` and
/// records a [`LootDropVisual`] entry. Idempotent on `loot_id`
/// so receiving both the reliable event and the next snapshot's
/// `EntityKind::Loot` row doesn't double-spawn the emitter.
pub fn on_loot_dropped(
    loot: &mut LootClientState,
    renderer: &mut Renderer,
    loot_id: rift_net::NetId,
    position: glam::Vec3,
    blob: rift_net::messages::ItemBlob,
) {
    use crate::game::sub_state::LootDropVisual;

    if loot.drops.iter().any(|d| d.net_id == loot_id) {
        return;
    }
    // Rehydrate the wire blob into a full Item. Mismatched indices
    // (e.g. server running a newer build) → drop the visual.
    let Some(item) = rift_game::loot::Item::from_wire(
        blob.base_id,
        blob.rarity,
        blob.ilvl,
        &blob.affixes,
    ) else {
        log::warn!(
            "loot drop {loot_id:?} has unknown indices base={} affixes={:?}; skipping visual",
            blob.base_id,
            blob.affixes
        );
        return;
    };

    let color = item.rarity.color();
    let pillar = renderer
        .vfx_system
        .spawn(rift_engine::renderer::vfx::presets::loot_beam(color), position);
    let base = renderer
        .vfx_system
        .spawn(rift_engine::renderer::vfx::presets::loot_beam_base(color), position);
    log::info!(
        "loot dropped: {} (item-level {}) at {:?}",
        item.display_name(),
        item.ilvl,
        position
    );
    loot.drops.push(LootDropVisual {
        net_id: loot_id,
        position,
        item,
        pillar_emitter: pillar,
        base_emitter: base,
    });
}
