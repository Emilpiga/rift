//! Server-side revive shrines.
//!
//! A revive shrine is a rare interactable that spawns at most
//! once per rift floor (>= floor 2), in a random arena room. To
//! activate it every **living** player on the floor must stand
//! within [`SHRINE_INTERACT_RADIUS`] of the shrine and hold the
//! channel intent ([`ClientMsg::ToggleShrineChannel`]) for
//! [`SHRINE_CHANNEL_DURATION`] real-time seconds.
//!
//! On completion the shrine revives every ghost / down-pose
//! player on the floor (HP back to max, `is_ghost` cleared,
//! [`WorldEvent::PlayersRevived`] broadcast) and despawns
//! itself. Floor changes also despawn it via [`despawn_all`].
//!
//! Spawn probability scales with floor depth — see
//! [`spawn_chance`]. Floor 2 is "astronomically rare" by design:
//! the shrine is a panic button, not a routine resource.

use glam::Vec3;
use hecs::Entity;
use rift_dungeon::Floor;
use rift_net::messages::{WorldEvent, SHRINE_CHANNEL_DURATION, SHRINE_INTERACT_RADIUS};
use rift_net::NetId;

use super::player::ServerPlayer;

/// One revive shrine sitting on the floor. Only the channel-tick
/// path mutates `progress` / `channelers` — spawn is one-shot per
/// floor and despawn happens via `despawn_all` or revive
/// completion.
#[derive(Clone, Debug)]
pub struct ServerReviveShrine {
    pub net_id: NetId,
    pub position: Vec3,
    /// Channel progress in seconds, 0.0..=`SHRINE_CHANNEL_DURATION`.
    pub progress: f32,
    /// Number of living players currently channeling. Cached on
    /// the row each tick so the snapshot encoder can read it
    /// without re-walking the player query.
    pub channelers: u8,
    /// Total living players on the floor at the last tick.
    /// Mirrors the channel target count for the HUD readout.
    pub required: u8,
}

/// Per-floor spawn probability for a shrine. Returns `0.0` for
/// the hub and floor 1 so shrines never appear there. Floor 2
/// is intentionally near-zero — players who push through deeper
/// floors are more likely to find one.
pub fn spawn_chance(floor_index: u32) -> f32 {
    match floor_index {
        0 | 1 => 0.0,
        2 => 0.005,
        3 => 0.02,
        4 => 0.05,
        5 => 0.10,
        _ => 0.15,
    }
}

/// Roll the shrine spawn for a freshly-loaded floor. Called from
/// [`super::Sim::change_floor`] after enemies are spawned.
///
/// Determinism: seeds the local xorshift with `floor_seed`,
/// `floor_index`, and a salt so the result is stable per floor
/// but disjoint from the dungeon's enemy / loot rolls.
pub fn maybe_spawn(
    world: &mut hecs::World,
    floor: &Floor,
    floor_seed: u64,
    floor_index: u32,
    next_misc_net_id: &mut u32,
) {
    let chance = spawn_chance(floor_index);
    if chance <= 0.0 {
        return;
    }
    // Local xorshift64 — `rift_dungeon::SimpleRng` is `pub(crate)`
    // so we can't reuse it. The salt keeps shrine rolls
    // independent of enemy/loot streams on the same floor.
    let mut state = floor_seed
        ^ ((floor_index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
        ^ 0x5F1D_4E2B_AA55_C0DEu64;
    let mut next_u64 = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    let roll = (next_u64() as f64 / u64::MAX as f64) as f32;
    if roll > chance {
        return;
    }

    let arenas = floor.arena_rooms();
    if arenas.is_empty() {
        return;
    }
    let room_idx = (next_u64() as usize) % arenas.len();
    let room = arenas[room_idx];
    let positions = room.spawn_positions(1, next_u64());
    let position = match positions.into_iter().next() {
        Some(p) => Vec3::new(p.x, 0.0, p.z),
        None => return,
    };

    let net_id = NetId(*next_misc_net_id);
    *next_misc_net_id = next_misc_net_id.wrapping_add(1);
    if *next_misc_net_id >= 0x8000_0000 {
        *next_misc_net_id = 0x6000_0000;
    }

    let shrine = ServerReviveShrine {
        net_id,
        position,
        progress: 0.0,
        channelers: 0,
        required: 0,
    };
    log::info!(
        "shrine: spawned revive shrine {net_id:?} on floor {floor_index} at {position:?}"
    );
    let _ = world.spawn((shrine,));
}

/// Despawn every shrine in the world. Called on floor change.
pub fn despawn_all(world: &mut hecs::World) {
    let stale: Vec<Entity> = world
        .query::<&ServerReviveShrine>()
        .iter()
        .map(|(e, _)| e)
        .collect();
    for e in stale {
        let _ = world.despawn(e);
    }
}

/// Look up a shrine entity + position by net id. Used by the
/// dispatcher to validate `ToggleShrineChannel` requests.
pub fn find(world: &hecs::World, shrine: NetId) -> Option<(Entity, Vec3)> {
    world
        .query::<&ServerReviveShrine>()
        .iter()
        .find(|(_, s)| s.net_id == shrine)
        .map(|(e, s)| (e, s.position))
}

/// Per-tick channel logic. Walks every shrine, counts living
/// players + active channelers within range, advances or resets
/// progress, and on completion revives every ghost and pushes
/// [`WorldEvent::PlayersRevived`] for the caller to broadcast.
///
/// Cancel rules:
/// - Walking out of range clears the channel intent for that
///   player (writes `channeling_shrine = None`).
/// - Players who die mid-channel drop out implicitly — we only
///   count living channelers. The `required` denominator
///   updates on the next tick so the channel can still complete
///   if the survivor finishes solo.
/// - If `channelers < required` (or `required == 0`) progress
///   resets to 0; partial progress doesn't bank.
pub fn tick(
    world: &mut hecs::World,
    events: &mut Vec<WorldEvent>,
    dt: f32,
) {
    if dt <= 0.0 {
        return;
    }

    // Snapshot all shrines (their NetId + position) so we can
    // mutate ServerPlayer rows in a separate query without
    // tripping hecs's borrow checker.
    let shrines: Vec<(Entity, NetId, Vec3)> = world
        .query::<&ServerReviveShrine>()
        .iter()
        .map(|(e, s)| (e, s.net_id, s.position))
        .collect();
    if shrines.is_empty() {
        return;
    }

    // First pass: auto-cancel intents for players outside any
    // valid range, count living players + per-shrine channelers.
    // We collect (entity, channeling_shrine, position, living)
    // up-front because we need to mutate `channeling_shrine`
    // and re-query after.
    #[derive(Clone, Copy)]
    struct PlayerInfo {
        entity: Entity,
        position: Vec3,
        channeling: Option<NetId>,
        living: bool,
    }
    let players: Vec<PlayerInfo> = world
        .query::<&ServerPlayer>()
        .iter()
        .map(|(e, p)| PlayerInfo {
            entity: e,
            position: p.k.position,
            channeling: p.channeling_shrine,
            living: !p.is_dead_or_ghosting(),
        })
        .collect();

    // Auto-cancel: any channeler whose target shrine doesn't
    // exist or whom is now out of range loses their intent.
    let radius_sq = SHRINE_INTERACT_RADIUS * SHRINE_INTERACT_RADIUS;
    for info in &players {
        let Some(target) = info.channeling else {
            continue;
        };
        let still_valid = shrines.iter().any(|(_, id, pos)| {
            *id == target
                && info.living
                && (info.position - *pos).length_squared() <= radius_sq
        });
        if !still_valid {
            if let Ok(mut p) = world.get::<&mut ServerPlayer>(info.entity) {
                p.channeling_shrine = None;
            }
        }
    }

    // Re-snapshot after auto-cancel so per-shrine counts use
    // up-to-date intents.
    let players: Vec<PlayerInfo> = world
        .query::<&ServerPlayer>()
        .iter()
        .map(|(e, p)| PlayerInfo {
            entity: e,
            position: p.k.position,
            channeling: p.channeling_shrine,
            living: !p.is_dead_or_ghosting(),
        })
        .collect();

    let living_count = players.iter().filter(|p| p.living).count() as u8;

    // Advance / reset progress for each shrine; collect those
    // that completed so we can revive + despawn after the
    // borrows drop.
    let mut completed: Vec<(Entity, NetId)> = Vec::new();
    for (entity, net_id, _pos) in &shrines {
        let channelers = players
            .iter()
            .filter(|p| p.living && p.channeling == Some(*net_id))
            .count() as u8;

        let mut shrine_ref = match world.get::<&mut ServerReviveShrine>(*entity) {
            Ok(s) => s,
            Err(_) => continue,
        };
        shrine_ref.channelers = channelers;
        shrine_ref.required = living_count;

        if living_count > 0 && channelers >= living_count {
            shrine_ref.progress = (shrine_ref.progress + dt).min(SHRINE_CHANNEL_DURATION);
            if shrine_ref.progress >= SHRINE_CHANNEL_DURATION {
                completed.push((*entity, *net_id));
            }
        } else {
            // Lost the unanimous condition — drop progress so
            // players can't bank a half-channel.
            shrine_ref.progress = 0.0;
        }
    }

    if completed.is_empty() {
        return;
    }

    // Revive every ghost / down-pose player on the floor. The
    // revived NetIds ride out on a single PlayersRevived event
    // per completion so clients can clear ghost tint + spawn
    // VFX in one place.
    let revive_targets: Vec<NetId> = world
        .query::<&ServerPlayer>()
        .iter()
        .filter(|(_, p)| p.is_dead_or_ghosting())
        .map(|(_, p)| p.net_id)
        .collect();

    for (_, net_id) in &completed {
        if !revive_targets.is_empty() {
            // Apply the revive: clear ghost flags + restore HP.
            let player_ents: Vec<Entity> = world
                .query::<&ServerPlayer>()
                .iter()
                .filter(|(_, p)| p.is_dead_or_ghosting())
                .map(|(e, _)| e)
                .collect();
            for e in player_ents {
                if let Ok(mut p) = world.get::<&mut ServerPlayer>(e) {
                    p.hp = p.hp_max;
                    p.is_ghost = false;
                    p.ghost_rise_timer = None;
                    p.channeling_shrine = None;
                }
            }
            log::info!(
                "shrine: {net_id:?} channel complete — revived {} player(s)",
                revive_targets.len()
            );
            events.push(WorldEvent::PlayersRevived {
                entities: revive_targets.clone(),
            });
        }
        // Despawn the shrine even if there were no ghosts to
        // revive — once channel completes, the shrine has done
        // its job (and players burned their channel intent
        // expecting a result).
    }

    // Also clear surviving channelers' intent so they don't
    // immediately try to re-channel a despawned shrine.
    let to_clear: Vec<Entity> = world
        .query::<&ServerPlayer>()
        .iter()
        .filter(|(_, p)| {
            p.channeling_shrine
                .map(|id| completed.iter().any(|(_, c)| *c == id))
                .unwrap_or(false)
        })
        .map(|(e, _)| e)
        .collect();
    for e in to_clear {
        if let Ok(mut p) = world.get::<&mut ServerPlayer>(e) {
            p.channeling_shrine = None;
        }
    }

    for (entity, _) in completed {
        let _ = world.despawn(entity);
    }
}
