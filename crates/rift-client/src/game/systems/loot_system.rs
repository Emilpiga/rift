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
use rift_engine::{Input, Mesh, Renderer};

use crate::game::loot_models::LootModelCache;
use crate::game::sub_state::LootClientState;

/// Walk-to-pickup range for ground loot drops. Mirrored on the
/// server as `rift_server::sim::PICKUP_RANGE`; we keep them
/// roughly in sync to avoid client-side prompts that the server
/// would reject.
pub const PICKUP_RADIUS: f32 = 1.8;

/// Target on-ground footprint of a dropped item along its
/// longest bind-pose axis. Authored loot meshes range from
/// character-height torsos down to tiny accessories; rescaling
/// to a constant footprint keeps the silhouette legible at
/// gameplay distance regardless of source size.
///
/// Sized to roughly match the equipped-on-character scale —
/// authored armour pieces are torso-sized in bind pose, so a
/// ~0.18 m footprint reads like the same item worn rather
/// than a giant prop on the floor.
pub const GROUND_MODEL_FOOTPRINT: f32 = 0.18;

/// Height the popped item peaks at above the spawn point
/// during the toss phase of the drop animation.
pub const POP_PEAK_HEIGHT: f32 = 0.9;

/// Vertical offset the model rests at after the pop settles.
/// Lifts the mesh just off the floor so it doesn't z-fight the
/// ground decals / pillar base.
pub const REST_HEIGHT: f32 = 0.05;

/// Duration of the pop-in tween (toss arc + scale grow). After
/// this the model holds its rest pose with a slow ambient bob.
pub const POP_DURATION: f32 = 0.45;

/// Drop every visual / VFX side-effect tracked by [`LootClientState`].
///
/// Must be called whenever the renderer's object list is wiped
/// via `Renderer::clear_objects` (i.e. floor regen / hub
/// (re)generation). Each [`LootDropVisual`] holds a renderer
/// `object_index` for its 3D ground model and a set of VFX
/// `EffectId`s for its pillar / base / anchored halo
/// emitters. After `clear_objects` those object indices are
/// stale — the next mesh added to the renderer will land at
/// index 0 (typically the new floor's ground platform), and
/// the per-frame `tick_drop_animation` would then stomp the
/// platform's model matrix with a `lay_flat * yaw`
/// transformation, rotating the ground 90° into a vertical
/// wall.
///
/// Despawning the VFX emitters here too prevents leaking
/// particle slots when a player picks up loot in a previous
/// floor and immediately portals back to the hub before the
/// `LootClaimed` ack lands.
pub fn clear_world_visuals(loot: &mut LootClientState, renderer: &mut Renderer) {
    for drop in loot.drops.drain(..) {
        renderer.vfx_system.despawn(drop.pillar_emitter);
        renderer.vfx_system.despawn(drop.base_emitter);
        if let Some(halo) = drop.anchored_emitter {
            renderer.vfx_system.despawn(halo);
        }
        // No need to zero `ground_mesh.object_index` — the
        // entire `renderer.objects` vector has just been
        // cleared, so the index is invalid by definition.
    }
    // Reset the pickup queues — they refer to net ids of drops
    // that no longer exist on this floor; the server will
    // re-broadcast any still-live drops via the next
    // `LootDropped` events.
    loot.pending_pickups.clear();
    loot.claimed_ids.clear();
}

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

/// Surface a "Not your loot" warning above the local player.
/// The server enforces a per-drop share window on player-dropped
/// items; pickup attempts from outside the eligibility snapshot
/// during the window land here. The window is silent on
/// monster drops.
pub fn warn_not_eligible(world: &hecs::World, combat_text: &mut CombatTextSystem) {
    let pos = world
        .query::<(&Transform, &Player, &LocalPlayer)>()
        .iter()
        .map(|(_, (t, _, _))| t.position)
        .next();
    if let Some(pos) = pos {
        combat_text.spawn_notice(pos, "Not your loot", [1.0, 0.55, 0.30, 1.0]);
    }
    log::info!("loot: pickup rejected — outside share window");
}

/// Closest loot drop inside [`PICKUP_RADIUS`] of the local player.
/// Returns the drop's `NetId` and the squared distance.
pub fn nearest_drop(world: &hecs::World, loot: &LootClientState) -> Option<(rift_net::NetId, f32)> {
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
    if let Some(halo) = drop.anchored_emitter {
        renderer.vfx_system.despawn(halo);
    }
    // Hide the 3D ground mesh by zeroing its model matrix —
    // the renderer treats `Mat4::ZERO` as a draw-skip sentinel.
    // We leak the dynamic-mesh slot rather than freeing it; per
    // session the volume is bounded and the slots are reused
    // implicitly when a future drop happens to land in the same
    // animation phase. (A free-list pool keyed on mesh path is
    // a follow-up if this becomes a memory concern.)
    if let Some(ground) = drop.ground_mesh {
        if let Some(obj) = renderer.objects.get_mut(ground.object_index) {
            obj.model_matrix = glam::Mat4::ZERO;
        }
    }
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
/// records a [`LootDropVisual`] entry. If the rolled item's
/// [`rift_game::loot::BaseItem::models`] points at a glTF/GLB,
/// also lazily decodes the bind-pose mesh through `models` and
/// adds a dynamic renderer object so the drop has a
/// recognisable 3D silhouette in addition to the beam VFX.
/// Idempotent on `loot_id` so receiving both the reliable event
/// and the next snapshot's `EntityKind::Loot` row doesn't
/// double-spawn the emitter.
pub fn on_loot_dropped(
    loot: &mut LootClientState,
    renderer: &mut Renderer,
    models: &mut LootModelCache,
    loot_id: rift_net::NetId,
    position: glam::Vec3,
    blob: rift_net::messages::ItemBlob,
    local_gender: Option<rift_game::character::Gender>,
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
        blob.anchored,
        blob.provenance
            .clone()
            .map(|v| rift_game::loot::LootProvenance::from_ids(v)),
        blob.unique_id
            .as_deref()
            .and_then(|s| rift_game::loot::uniques::find(s).map(|u| u.id)),
        blob.unique_pick,
    )
    .map(|mut it| {
        it.unstable = blob.unstable;
        it.rift_touched = rift_game::loot::Item::rift_touched_from_wire(blob.rift_touched);
        it.enchanted_affix_index = blob.enchanted_affix_index;
        it
    }) else {
        log::warn!(
            "loot drop {loot_id:?} has unknown indices base={} affixes={:?}; skipping visual",
            blob.base_id,
            blob.affixes
        );
        return;
    };

    let rarity = item.rarity;
    let pillar = renderer.vfx_system.spawn(
        rift_engine::renderer::vfx::presets::loot_beam(rarity),
        position,
    );
    let base = renderer.vfx_system.spawn(
        rift_engine::renderer::vfx::presets::loot_beam_base(rarity),
        position,
    );
    // Anchored drops get an extra orbital halo so the rare
    // trait reads at gameplay distance independent of rarity.
    let anchored_emitter = if item.anchored {
        Some(renderer.vfx_system.spawn(
            rift_engine::renderer::vfx::presets::loot_anchored_halo(),
            position,
        ))
    } else {
        None
    };
    log::info!(
        "loot dropped: {} (item-level {}) at {:?}",
        item.display_name(),
        item.ilvl,
        position
    );
    let ground_mesh = spawn_ground_mesh(renderer, models, &item, position, loot_id, local_gender);
    loot.drops.push(LootDropVisual {
        net_id: loot_id,
        position,
        item,
        pillar_emitter: pillar,
        base_emitter: base,
        anchored_emitter,
        ground_mesh,
    });
}

/// Try to add a 3D bind-pose mesh for `item` to the renderer.
/// Picks any available gender variant (loot on the ground is
/// gender-agnostic), goes through the [`LootModelCache`] for
/// load + cache, and inserts a dynamic mesh slot at
/// `Mat4::ZERO` so the first frame of
/// [`tick_drop_animation`] places it. Returns `None` when the
/// base item has no model or the load failed.
fn spawn_ground_mesh(
    renderer: &mut Renderer,
    models: &mut LootModelCache,
    item: &rift_game::loot::Item,
    position: glam::Vec3,
    loot_id: rift_net::NetId,
    local_gender: Option<rift_game::character::Gender>,
) -> Option<crate::game::sub_state::LootGroundMesh> {
    let path = pick_model_path(item.base.models.as_ref()?, local_gender)?;
    let model = models.fetch(path)?;
    // Build a transient `Mesh` view onto the cached vertices
    // (the renderer copies the data into its own buffers).
    let mesh_for_upload = Mesh {
        vertices: model.mesh.vertices.clone(),
        indices: model.mesh.indices.clone(),
    };
    let object_index = match renderer.add_dynamic_mesh(&mesh_for_upload, glam::Mat4::ZERO) {
        Ok(i) => i,
        Err(e) => {
            log::warn!("loot ground mesh upload failed for {:?}: {}", path, e);
            return None;
        }
    };
    // Tint by item rarity so the silhouette reads at a glance
    // even before a tooltip is hovered. The renderer's default
    // material multiplies by this in the fragment shader.
    let [r, g, b] = item.rarity.color();
    if let Some(obj) = renderer.objects.get_mut(object_index) {
        // Slight HDR boost so common drops still pop against the
        // ground; the rarity colour palette is authored in LDR.
        obj.tint = [r * 1.4, g * 1.4, b * 1.4, 1.0];
    }
    // Normalise so every drop lands at the same on-floor
    // footprint regardless of how the artist authored the
    // source asset (a 1.5 m sword and a tiny 4 cm ring should
    // both read as a recognisable "thing on the ground" of
    // the same visual weight). The wide clamp here protects
    // against truly degenerate AABBs (zero / NaN / camera-
    // facing decals) while still letting small accessories
    // scale up enough to match weapons + armour silhouettes.
    let base_scale = (GROUND_MODEL_FOOTPRINT / model.bounds_max_extent).clamp(0.1, 6.0);
    // Per-drop yaw randomisation: deterministic on `loot_id` so
    // the orientation is stable across late-join snapshot
    // reconciliations and re-spawns from the same id.
    let rest_yaw = ((loot_id.0 as u64).wrapping_mul(0x9E37_79B9) as u32 as f32) / (u32::MAX as f32)
        * std::f32::consts::TAU;
    let rest_position = position + glam::Vec3::new(0.0, REST_HEIGHT, 0.0);
    Some(crate::game::sub_state::LootGroundMesh {
        object_index,
        rest_position,
        base_scale,
        rest_yaw,
        bounds_min: model.bounds_min,
        bounds_max: model.bounds_max,
        anim_t: 0.0,
    })
}

fn pick_model_path(
    models: &rift_game::loot::GenderedModel,
    local_gender: Option<rift_game::character::Gender>,
) -> Option<&'static str> {
    // Prefer the variant matching the local player's authored
    // gender so the silhouette on the ground matches what the
    // avatar would equip. Fall back to whichever variant the
    // artist did author when the preferred side is missing —
    // a one-sided asset is still recognisable on the floor.
    let (preferred, fallback) = match local_gender {
        Some(rift_game::character::Gender::Female) => (models.female, models.male),
        // Default to male when the local profile hasn't loaded
        // yet (rare: a snapshot row arriving before
        // `set_profile` finishes). The drop is still valid;
        // we just pick a deterministic side.
        Some(rift_game::character::Gender::Male) | None => (models.male, models.female),
    };
    preferred.or(fallback)
}

/// Per-frame animation pass for ground-loot 3D models. Each
/// drop pops up off the spawn point with an arc + scale-in
/// tween for the first [`POP_DURATION`] seconds, then settles
/// into a slow ambient bob + spin so the silhouette stays
/// alive in the player's peripheral vision. Idempotent on
/// the per-drop `anim_t`.
pub fn tick_drop_animation(loot: &mut LootClientState, renderer: &mut Renderer, dt: f32) {
    for drop in loot.drops.iter_mut() {
        let Some(ground) = drop.ground_mesh.as_mut() else {
            continue;
        };
        ground.anim_t += dt;
        let matrix = ground_matrix(ground);
        if let Some(obj) = renderer.objects.get_mut(ground.object_index) {
            obj.model_matrix = matrix;
        }
    }
}

fn ground_matrix(ground: &crate::game::sub_state::LootGroundMesh) -> glam::Mat4 {
    let t = ground.anim_t;
    // Pop tween: 0..1 over POP_DURATION, clamped at 1 so the
    // settled rest pose is just `t = 1`.
    let pop = (t / POP_DURATION).clamp(0.0, 1.0);
    // Arc height: parabola peaking mid-tween, returning to 0
    // at settle. `4 * x * (1 - x)` is the standard normalised
    // arc, peaks at 1.0 when x = 0.5.
    let arc = 4.0 * pop * (1.0 - pop) * POP_PEAK_HEIGHT;
    // Scale-in: ease-out cubic so the model snaps to full
    // size in the first half of the tween, then holds.
    let scale_in = {
        let inv = 1.0 - pop;
        1.0 - inv * inv * inv
    };
    let scale = ground.base_scale * scale_in;
    // Tumble during the pop only; decays smoothly to zero by
    // the time the item lands so it rests at `rest_yaw`. No
    // perpetual ambient spin — and no rest-pose bob: the
    // settled drop should sit perfectly still on the floor
    // like a real item, not float like a pickup token.
    let tumble_decay = (1.0 - pop) * (1.0 - pop);
    let tumble = pop * std::f32::consts::TAU * 1.25 * tumble_decay;
    let yaw = ground.rest_yaw + tumble;

    // Lay the mesh flat on the floor. Models are authored
    // standing upright (worn pose), so a -90° rotation around
    // X tips them onto their back: local +Y (up the body)
    // becomes world +Z (away along the ground). Combined with
    // the per-drop yaw this gives a horizontal pose that
    // reads as "dropped on the floor" rather than "standing
    // on a stand".
    let lay_flat = glam::Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    let total_rot = glam::Quat::from_rotation_y(yaw) * lay_flat;

    // Recover the lift + horizontal centroid in *world*
    // space by transforming the mesh-local AABB centre
    // through the same rotation. After lay-flat the world-Y
    // span is the local-Z span (with sign flip), so the
    // lowest world-Y point is at `bounds_min.z * scale` — lift
    // by its negation to plant the lowest point at
    // `rest_position.y`. The horizontal centre we want under
    // the loot beam is the rotated XZ of the local centroid.
    let local_min = ground.bounds_min;
    let local_max = ground.bounds_max;
    let local_centre = (local_min + local_max) * 0.5;
    let world_centre = total_rot * (local_centre * scale);
    let lift = -local_min.z * scale;
    let pos = ground.rest_position + glam::Vec3::new(0.0, arc + lift, 0.0)
        - glam::Vec3::new(world_centre.x, 0.0, world_centre.z);
    glam::Mat4::from_scale_rotation_translation(glam::Vec3::splat(scale), total_rot, pos)
}
