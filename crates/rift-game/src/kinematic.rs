//! Shared deterministic player movement.
//!
//! The *single* implementation of player input processing + collision
//! integration. The server runs it authoritatively in `rift-server::sim`;
//! the client runs the exact same code on its predicted local-player
//! state in `rift-client::net_client` so prediction can't drift from the
//! server result.
//!
//! Determinism: every input is `f32` math against a deterministic tile
//! grid (`rift_dungeon::Floor`). Same `(state, cmd, dt, floor)` triple
//! yields the same `state` output on either side, tick after tick,
//! regardless of host frame rate.

use glam::Vec3;
use rift_dungeon::{Floor, Tile};

/// Horizontal player movement speed, in world units / second.
pub const PLAYER_SPEED: f32 = 6.0;

/// Approximate player capsule radius for wall-vs-player collision.
pub const PLAYER_RADIUS: f32 = 0.35;

/// Initial upward velocity on a fresh jump (m/s).
pub const JUMP_VELOCITY: f32 = 9.5;

/// Gravitational acceleration applied to airborne players (m/s²).
pub const GRAVITY: f32 = 22.0;

/// Locomotion bucket ids on the wire. Both sides agree on these.
pub mod loco {
    pub const IDLE: u8 = 0;
    pub const RUN: u8 = 1;
    /// Active dodge-roll. Clients pick the roll clip when they see
    /// this bucket on a snapshot row instead of the usual
    /// Walk/Run/Idle blend.
    pub const ROLL: u8 = 2;
}

/// Wire-side full-body action ids. Mirror of the engine’s
/// `PlayerAction` enum but flattened to a `u8` so it can be sent
/// in `EntityKind::Player::action`. Server and client read this
/// directly off the snapshot to keep the dodge-roll animation in
/// sync with the authoritative kinematic state.
pub mod action {
    pub const NONE: u8 = 0;
    pub const ROLL: u8 = 1;
}

/// Total dodge-roll duration in seconds. Includes the trailing
/// landing window during which the roll decelerates.
pub const ROLL_DURATION: f32 = 0.95;

/// Length (in seconds) of the trailing slow-down phase at the end
/// of a roll. While `roll_remaining > ROLL_LANDING` the roll runs
/// at peak speed; below it we ease the speed down so the roll
/// settles cleanly into the recovery animation.
pub const ROLL_LANDING: f32 = 0.25;

/// Peak roll speed during the active phase, in world units / second.
/// Tuned by feel against the roll clip length.
pub const ROLL_PEAK_SPEED: f32 = 14.0;

/// Floor speed at the very end of the landing phase. Keeping a
/// small forward bleed avoids a hard stop that visibly clashes
/// with the recovery animation.
pub const ROLL_END_SPEED: f32 = 1.5;

/// Bit positions inside the input command's button bitfield. These
/// MUST stay in sync with `rift_net::messages::button_bits` — they are
/// the same wire constants, duplicated here so the kinematic
/// integrator (which lives in rift-game) doesn't need to depend on
/// rift-net.
pub mod button_bits {
    pub const MOVE_FORWARD: u16 = 1 << 0;
    pub const MOVE_BACK: u16 = 1 << 1;
    pub const MOVE_LEFT: u16 = 1 << 2;
    pub const MOVE_RIGHT: u16 = 1 << 3;
    pub const ROLL: u16 = 1 << 4;
    pub const JUMP: u16 = 1 << 5;
    pub const INTERACT: u16 = 1 << 6;
    pub const ABILITY_1: u16 = 1 << 7;
    pub const ABILITY_2: u16 = 1 << 8;
    pub const ABILITY_3: u16 = 1 << 9;
    pub const ABILITY_4: u16 = 1 << 10;
    pub const ABILITY_5: u16 = 1 << 11;
    pub const ABILITY_6: u16 = 1 << 12;
    pub const ATTACK: u16 = 1 << 13;
}

/// The minimum slice of player state that participates in
/// movement + collision. Both server and client embed one of
/// these inside their own richer player struct (`ServerPlayer`,
/// `PredictedPlayer`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Kinematic {
    pub position: Vec3,
    pub velocity: Vec3,
    /// Body yaw in radians (0 = facing +Z).
    pub yaw: f32,
    /// Independently-controlled aim yaw, drives spine twist + ranged
    /// targeting. Equals `yaw` until the client sends a distinct aim.
    pub aim_yaw: f32,
    /// Bucketised locomotion id; clients use it to pick a clip.
    pub locomotion: u8,
    /// Vertical velocity (m/s). Non-zero only while airborne (jump).
    pub vy: f32,
    /// True while the player is mid-jump.
    pub airborne: bool,
    /// Time remaining on the active dodge-roll, in seconds. While
    /// non-zero `apply_input` overrides horizontal velocity with the
    /// rolling speed curve and ignores movement input. Decremented
    /// every `integrate` step.
    pub roll_remaining: f32,
    /// Unit XZ direction the active dodge-roll is travelling in.
    /// Captured at `start_roll` time so input changes mid-roll
    /// don't curve the path.
    pub roll_dir: [f32; 2],
    /// Active full-body action id (see [`action`] module). Mirrored
    /// into the snapshot so clients can drive the matching
    /// animation on remote avatars.
    pub action: u8,
}

/// Apply a fresh input command to the kinematic state. Mirrors
/// `ServerPlayer::apply_input` exactly — same RNG-free math.
///
/// Falls back to button bits if `move_dir` is empty, so a hand-crafted
/// test client without aim/move axes can still drive movement.
pub fn apply_input(k: &mut Kinematic, move_dir: [f32; 2], aim_dir: [f32; 2], buttons: u16) {
    // Aim updates always apply, even mid-roll, so the spine twist
    // tracks the cursor while rolling.
    if aim_dir[0] != 0.0 || aim_dir[1] != 0.0 {
        k.aim_yaw = aim_dir[0].atan2(aim_dir[1]);
    }

    // Active dodge-roll: lock movement to the captured roll vector
    // with a speed curve that runs at peak until the last
    // `ROLL_LANDING` seconds, then eases out to `ROLL_END_SPEED`.
    // Movement input + jump are ignored for the duration.
    if k.roll_remaining > 0.0 {
        let scale = if k.roll_remaining >= ROLL_LANDING {
            1.0
        } else {
            let t = (k.roll_remaining / ROLL_LANDING).clamp(0.0, 1.0);
            // ease-out cubic: full speed early in the landing
            // window, settles smoothly into the floor speed.
            let eased = 1.0 - (1.0 - t).powi(3);
            let min_scale = ROLL_END_SPEED / ROLL_PEAK_SPEED;
            min_scale + (1.0 - min_scale) * eased
        };
        let speed = ROLL_PEAK_SPEED * scale;
        k.velocity.x = k.roll_dir[0] * speed;
        k.velocity.z = k.roll_dir[1] * speed;
        k.locomotion = loco::ROLL;
        return;
    }

    let mut wish = Vec3::new(move_dir[0], 0.0, move_dir[1]);
    if wish.length_squared() < 1.0e-4 {
        let mut bx = 0.0;
        let mut bz = 0.0;
        if buttons & button_bits::MOVE_FORWARD != 0 { bz -= 1.0; }
        if buttons & button_bits::MOVE_BACK != 0 { bz += 1.0; }
        if buttons & button_bits::MOVE_LEFT != 0 { bx -= 1.0; }
        if buttons & button_bits::MOVE_RIGHT != 0 { bx += 1.0; }
        wish = Vec3::new(bx, 0.0, bz);
    }
    if wish.length_squared() > 1.0 {
        wish = wish.normalize();
    }
    let air_factor = if k.airborne { 0.85 } else { 1.0 };
    k.velocity.x = wish.x * PLAYER_SPEED * air_factor;
    k.velocity.z = wish.z * PLAYER_SPEED * air_factor;

    if buttons & button_bits::JUMP != 0 && !k.airborne {
        k.vy = JUMP_VELOCITY;
        k.airborne = true;
    }

    let horiz_speed_sq = k.velocity.x * k.velocity.x + k.velocity.z * k.velocity.z;
    if horiz_speed_sq > 1.0e-4 {
        k.yaw = k.velocity.x.atan2(k.velocity.z);
        k.locomotion = loco::RUN;
    } else {
        k.locomotion = loco::IDLE;
    }
}

/// Begin a dodge-roll in the given XZ direction. Sets the roll
/// timer + locked direction so subsequent `apply_input` calls
/// override movement with the roll speed curve.
///
/// `dir` does not need to be normalised; we project to XZ and
/// renormalise. If `dir` is degenerate we fall back to the
/// player’s current body yaw so a stationary roll still moves
/// forward instead of pinning the player in place.
pub fn start_roll(k: &mut Kinematic, dir: Vec3) {
    let xz = Vec3::new(dir.x, 0.0, dir.z);
    let n = if xz.length_squared() > 1.0e-4 {
        xz.normalize()
    } else {
        Vec3::new(k.yaw.sin(), 0.0, k.yaw.cos())
    };
    k.roll_remaining = ROLL_DURATION;
    k.roll_dir = [n.x, n.z];
    k.action = action::ROLL;
    // Snap body yaw to the roll direction so the rolling capsule
    // visibly travels forward, not sideways.
    k.yaw = n.x.atan2(n.z);
    // Cancel any in-flight jump so a roll on the ramp-up frame
    // doesn't preserve vertical velocity.
    if k.airborne {
        k.vy = 0.0;
    }
}

/// Integrate `velocity * dt` into `position` while resisting wall
/// tiles. X and Z resolved separately so sliding along a wall keeps
/// the orthogonal component. Vertical motion (jump arc) integrates
/// against a flat ground plane at y=0.
pub fn integrate(k: &mut Kinematic, floor: &Floor, dt: f32) {
    let step = k.velocity * dt;

    let mut new_pos = k.position;
    new_pos.x += step.x;
    if tile_at(floor, new_pos.x, k.position.z) == Tile::Wall
        || tile_blocks_capsule(floor, new_pos.x, k.position.z, PLAYER_RADIUS)
    {
        new_pos.x = k.position.x;
    }
    new_pos.z += step.z;
    if tile_at(floor, new_pos.x, new_pos.z) == Tile::Wall
        || tile_blocks_capsule(floor, new_pos.x, new_pos.z, PLAYER_RADIUS)
    {
        new_pos.z = k.position.z;
    }

    if k.airborne || k.vy.abs() > 1.0e-4 || new_pos.y > 1.0e-4 {
        k.vy -= GRAVITY * dt;
        new_pos.y += k.vy * dt;
        if new_pos.y <= 0.0 {
            new_pos.y = 0.0;
            k.vy = 0.0;
            k.airborne = false;
        } else {
            k.airborne = true;
        }
    } else {
        new_pos.y = 0.0;
        k.vy = 0.0;
        k.airborne = false;
    }

    k.position = new_pos;

    // Tick the dodge-roll timer down. Cleared `action` flips back
    // to `NONE` so the snapshot pipeline puts the body back into
    // its locomotion blend on the very next tick.
    if k.roll_remaining > 0.0 {
        k.roll_remaining = (k.roll_remaining - dt).max(0.0);
        if k.roll_remaining == 0.0 {
            k.action = action::NONE;
        }
    }
}

fn tile_at(floor: &Floor, x: f32, z: f32) -> Tile {
    if x < 0.0 || z < 0.0 {
        return Tile::Wall;
    }
    floor.get(x.floor() as usize, z.floor() as usize)
}

fn tile_blocks_capsule(floor: &Floor, x: f32, z: f32, radius: f32) -> bool {
    [
        (x + radius, z),
        (x - radius, z),
        (x, z + radius),
        (x, z - radius),
    ]
    .iter()
    .any(|&(px, pz)| tile_at(floor, px, pz) == Tile::Wall)
}
