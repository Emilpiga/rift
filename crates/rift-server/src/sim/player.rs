//! Connected-player state, input ingestion, and movement integration.
//!
//! Players are pure data: no AI, no scripted state machines. Every
//! tick the latest coalesced `InputCmd` is fed through the shared
//! `rift-game` integrator (which the client mirrors verbatim for
//! prediction).

use std::collections::HashMap;

use hecs::Entity;
use rift_dungeon::Floor;
use rift_net::{
    messages::{button_bits, InputCmd},
    ClientId, NetId,
};
use rift_game::kinematic::{self, loco, Kinematic};

/// Default per-player starting health. Will become class-driven once
/// the class config table is wired through.
pub const DEFAULT_HP: f32 = 100.0;

/// Component bundle for a connected player.
#[derive(Clone, Debug)]
pub struct ServerPlayer {
    pub client_id: ClientId,
    pub net_id: NetId,
    pub k: Kinematic,
    pub hp_max: f32,
    pub hp: f32,
    /// Last input `seq` we successfully applied. Echoed back in
    /// snapshots so the client can prune its prediction buffer.
    pub last_input_seq: u32,
    /// In-memory inventory of items the player has picked up this
    /// session. Authoritative on the server. Persisted to the DB
    /// is TODO (no `inventory_items` schema yet) — for now the
    /// items live for the lifetime of the connection and ride
    /// across floor transitions.
    pub inventory: Vec<rift_game::loot::Item>,
}

impl ServerPlayer {
    pub fn fresh(client_id: ClientId, net_id: NetId, spawn: glam::Vec3) -> Self {
        Self {
            client_id,
            net_id,
            k: Kinematic {
                position: spawn,
                velocity: glam::Vec3::ZERO,
                yaw: 0.0,
                aim_yaw: 0.0,
                locomotion: loco::IDLE,
                vy: 0.0,
                airborne: false,
                ..Default::default()
            },
            hp_max: DEFAULT_HP,
            hp: DEFAULT_HP,
            last_input_seq: 0,
            inventory: Vec::new(),
        }
    }
}

/// Edge-triggered button bits we forward across input coalescing so a
/// brief press never gets dropped between server ticks.
const STICKY_BUTTONS: u16 = button_bits::JUMP
    | button_bits::ROLL
    | button_bits::INTERACT
    | button_bits::ATTACK
    | button_bits::ABILITY_1
    | button_bits::ABILITY_2
    | button_bits::ABILITY_3
    | button_bits::ABILITY_4
    | button_bits::ABILITY_5
    | button_bits::ABILITY_6;

/// Merge a fresh input into a possibly-already-pending one for the
/// same client. Drops out-of-order packets and OR-folds sticky
/// buttons forward.
pub fn merge_pending(pending: &mut HashMap<ClientId, InputCmd>, client_id: ClientId, cmd: InputCmd) {
    if let Some(existing) = pending.get(&client_id) {
        if cmd.seq.wrapping_sub(existing.seq) as i32 <= 0 {
            return;
        }
    }
    let mut merged = cmd;
    if let Some(existing) = pending.get(&client_id) {
        merged.buttons |= existing.buttons & STICKY_BUTTONS;
    }
    pending.insert(client_id, merged);
}

/// Apply the latest pending input for each connected player. Drains
/// `pending`. Records the applied `seq` on each `ServerPlayer` so
/// the next snapshot's `ack_seq` is correct.
pub fn apply_inputs(
    world: &mut hecs::World,
    sessions: &HashMap<ClientId, Entity>,
    pending: &mut HashMap<ClientId, InputCmd>,
) {
    let inputs: Vec<(ClientId, InputCmd)> = pending.drain().collect();
    for (client_id, cmd) in inputs {
        if let Some(&entity) = sessions.get(&client_id) {
            if let Ok(mut p) = world.get::<&mut ServerPlayer>(entity) {
                p.last_input_seq = cmd.seq;
                kinematic::apply_input(&mut p.k, cmd.move_dir, cmd.aim_dir, cmd.buttons);
            }
        }
    }
}

/// Integrate every player's velocity against the floor's wall grid.
pub fn integrate_motion(world: &mut hecs::World, floor: &Floor, dt: f32) {
    for (_e, p) in world.query_mut::<&mut ServerPlayer>() {
        kinematic::integrate(&mut p.k, floor, dt);
    }
}

/// Snapshot every player's `(entity, position)` into a Vec, suitable
/// for use as the AI target list during the enemy tick.
pub fn target_positions(world: &hecs::World) -> Vec<(Entity, glam::Vec3)> {
    world
        .query::<&ServerPlayer>()
        .iter()
        .map(|(e, p)| (e, p.k.position))
        .collect()
}

/// Reset every player's kinematic state to a fresh spawn pose.
/// Called from the floor-change path so a held key doesn't slide
/// the freshly-loaded floor's start position.
pub fn snap_all_to(world: &mut hecs::World, spawn: glam::Vec3) {
    for (_e, p) in world.query_mut::<&mut ServerPlayer>() {
        p.k.position = spawn;
        p.k.velocity = glam::Vec3::ZERO;
        p.k.vy = 0.0;
        p.k.airborne = false;
        p.k.locomotion = loco::IDLE;
    }
}
