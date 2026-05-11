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
pub fn apply_input(
    k: &mut Kinematic,
    move_dir: [f32; 2],
    aim_dir: [f32; 2],
    buttons: u16,
    move_speed: f32,
) {
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
        if buttons & button_bits::MOVE_FORWARD != 0 {
            bz -= 1.0;
        }
        if buttons & button_bits::MOVE_BACK != 0 {
            bz += 1.0;
        }
        if buttons & button_bits::MOVE_LEFT != 0 {
            bx -= 1.0;
        }
        if buttons & button_bits::MOVE_RIGHT != 0 {
            bx += 1.0;
        }
        wish = Vec3::new(bx, 0.0, bz);
    }
    if wish.length_squared() > 1.0 {
        wish = wish.normalize();
    }
    let air_factor = if k.airborne { 0.85 } else { 1.0 };
    k.velocity.x = wish.x * move_speed * air_factor;
    k.velocity.z = wish.z * move_speed * air_factor;

    // Jumping is disabled — this is an ARPG with grid-based
    // locomotion. The JUMP button bit and `JUMP_VELOCITY`
    // constant are kept so the network wire format and any
    // legacy save data don't break, but the input is ignored.
    let _ = buttons & button_bits::JUMP;

    let horiz_speed_sq = k.velocity.x * k.velocity.x + k.velocity.z * k.velocity.z;
    if horiz_speed_sq > 1.0e-4 {
        // Body yaw is *not* snapped to velocity here. Instantly
        // rotating the body whenever the move direction changes
        // (e.g. switching W → A) reads as a hard pose pop
        // because all locomotion clips are forward-running and
        // the renderer has nothing else to soften the
        // transition. Instead we record the desired yaw and let
        // `integrate` exponentially chase it with `dt`, which
        // produces a tight but visibly smooth pivot.
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
/// against the floor's per-tile elevation, including stair-tile slope
/// interpolation, so the player walks up daises and down pits at the
/// expected world Y.
pub fn integrate(k: &mut Kinematic, floor: &Floor, dt: f32) {
    let step = k.velocity * dt;

    let mut new_pos = k.position;

    // Tile / wall axis-separated step. Tiles are rigid grid
    // cells with the entire neighbouring cell either fully
    // walkable or fully blocking, so the cheap "would the
    // candidate point be inside a wall tile?" predicate is
    // a fine collision test for them — there's no
    // sub-tile-precision sliding to do.
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

    // Prop depenetration. Unlike tiles, prop AABBs are
    // sub-tile-sized and can graze the player's capsule at
    // *any* angle, so the binary "blocks?" test we use for
    // tiles introduces ULP-scale jitter when sliding along
    // a face: tiny float noise flips the predicate each
    // frame and the position oscillates within a sub-mm
    // band, which the camera and the footstep system
    // amplify visibly.
    //
    // Instead we accept the candidate position and then
    // push it back out of any overlapping prop AABB along
    // the minimum-translation vector. That gives:
    //
    //   * **Smooth slide** — moving tangent to a face
    //     produces a purely-normal push, leaving the
    //     velocity untouched along the face.
    //   * **No float-edge jitter** — the push magnitude is
    //     exactly the penetration depth, so a single
    //     application clears the overlap to within ULP, and
    //     the next frame's overlap is again exactly zero.
    //   * **Self-correction** — if the player ever lands
    //     inside a prop (spawn, teleport, two overlapping
    //     props pinching the capsule), they slide out the
    //     nearest face in one frame instead of jamming.
    //
    // Two iterations cover the case where pushing out of
    // one prop pushes the capsule into another (corner
    // pinches between adjacent wall props). More than that
    // is wasted work: if the capsule still overlaps after
    // two passes, the props themselves are placed too tight
    // and the placement code is the right thing to fix.
    for _ in 0..2 {
        let mut total_dx = 0.0_f32;
        let mut total_dz = 0.0_f32;
        let mut hits = 0u32;
        for p in floor.props.iter() {
            if let Some((dx, dz)) = p.depenetrate_capsule_xz(new_pos.x, new_pos.z, PLAYER_RADIUS) {
                total_dx += dx;
                total_dz += dz;
                hits += 1;
            }
        }
        if hits == 0 {
            break;
        }
        new_pos.x += total_dx;
        new_pos.z += total_dz;

        // Bounce-back guard: a depenetration push could
        // shove the capsule into a wall tile (especially for
        // wall-aligned props whose face sits right against a
        // wall). Re-clamp to the pre-push position on each
        // axis if the new spot is inside a wall, mirroring
        // the tile axis-separated rejection above.
        if tile_at(floor, new_pos.x, k.position.z) == Tile::Wall
            || tile_blocks_capsule(floor, new_pos.x, k.position.z, PLAYER_RADIUS)
        {
            new_pos.x -= total_dx;
        }
        if tile_at(floor, new_pos.x, new_pos.z) == Tile::Wall
            || tile_blocks_capsule(floor, new_pos.x, new_pos.z, PLAYER_RADIUS)
        {
            new_pos.z -= total_dz;
        }
    }

    // Ground Y under the *tile centre* under the capsule.
    //
    // Earlier this took the max over centre+4 cardinal taps at
    // the capsule radius, intended as a cheap step-up
    // primitive. That was wrong for this tile grid:
    // `tile_floor_y_at` returns 0.0 for walls/OOB, so any tap
    // grazing a wall yanks the result up to 0 — when standing
    // in a sunken pit the body would wedge half a metre above
    // the pit floor and the mesh would render half-buried in
    // the floor it's supposedly standing on.
    //
    // Vertical transitions between connected tiles are already
    // handled by stair tiles (which interpolate their own
    // floor height across the tile span). Single-tap centre
    // sampling is the correct primitive here.
    let ground_y = floor.tile_floor_y_at(new_pos.x, new_pos.z);

    if k.airborne || k.vy.abs() > 1.0e-4 || (new_pos.y - ground_y) > 1.0e-4 {
        k.vy -= GRAVITY * dt;
        new_pos.y += k.vy * dt;
        if new_pos.y <= ground_y {
            new_pos.y = ground_y;
            k.vy = 0.0;
            k.airborne = false;
        } else {
            k.airborne = true;
        }
    } else {
        // Glued to ground: snap to ground_y so walking onto a
        // raised dais lifts us instantly without leaving a
        // floating-then-falling artefact, and walking off the
        // dais into a pit drops us immediately. Vertical step
        // limit isn't enforced here yet — Phase 2's elevation
        // generator only ever produces ±1 step neighbours.
        new_pos.y = ground_y;
        k.vy = 0.0;
        k.airborne = false;
    }

    k.position = new_pos;

    // Stationary body-follow: when not running, exponentially
    // pull body yaw toward aim yaw so the spine twist (clamped
    // ±120° on the renderer) never has to hard-snap when the
    // cursor sweeps past the back of the character. While
    // running, body yaw is owned by velocity. Uses real `dt` so
    // server (fixed tick) and client (per-frame extrapolation +
    // per-snapshot replay) converge to the same yaw.
    if k.locomotion == loco::IDLE && k.roll_remaining <= 0.0 {
        const FOLLOW_RATE: f32 = 6.0; // 1/τ; τ ≈ 0.17 s
        let mut delta = k.aim_yaw - k.yaw;
        while delta > std::f32::consts::PI {
            delta -= std::f32::consts::TAU;
        }
        while delta < -std::f32::consts::PI {
            delta += std::f32::consts::TAU;
        }
        let alpha = 1.0 - (-FOLLOW_RATE * dt).exp();
        k.yaw += delta * alpha;
        if k.yaw > std::f32::consts::PI {
            k.yaw -= std::f32::consts::TAU;
        }
        if k.yaw < -std::f32::consts::PI {
            k.yaw += std::f32::consts::TAU;
        }
    } else if k.locomotion == loco::RUN && k.roll_remaining <= 0.0 {
        // Running body-follow: exponentially chase the
        // velocity-derived yaw instead of snapping to it. The
        // animation set is forward-only (no strafe / back-pedal
        // clips), so a hard yaw flip when the player switches
        // WASD direction reads as a pose pop — the entire mesh
        // teleports through 90°+ inside one frame to keep the
        // forward-run cycle pointing along the new velocity.
        // Chasing instead pivots the body across ~0.10 s, which
        // is fast enough to feel responsive but slow enough that
        // the eye perceives a turn rather than a teleport.
        //
        // Rate chosen empirically: τ ≈ 0.10 s lands at the
        // boundary where 90° direction changes still feel
        // immediate to the player but the visible mesh no
        // longer skips frames. Real `dt` keeps server and
        // client in sync (server fixed tick + client
        // prediction replay both run integrate with the same
        // `dt`).
        const RUN_TURN_RATE: f32 = 10.0; // 1/τ; τ ≈ 0.10 s
        let target = k.velocity.x.atan2(k.velocity.z);
        let mut delta = target - k.yaw;
        while delta > std::f32::consts::PI {
            delta -= std::f32::consts::TAU;
        }
        while delta < -std::f32::consts::PI {
            delta += std::f32::consts::TAU;
        }
        let alpha = 1.0 - (-RUN_TURN_RATE * dt).exp();
        k.yaw += delta * alpha;
        if k.yaw > std::f32::consts::PI {
            k.yaw -= std::f32::consts::TAU;
        }
        if k.yaw < -std::f32::consts::PI {
            k.yaw += std::f32::consts::TAU;
        }
    }

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
    // Rendering convention: a tile at grid (i, j) is rendered with
    // its centre at world (i, j), so it covers world space
    // [i - 0.5, i + 0.5] × [j - 0.5, j + 0.5]. Map world → grid
    // by snapping to the nearest centre, otherwise the collision
    // grid sits half a tile off the visible geometry and the
    // player merges into walls on one side while stopping short
    // on the other.
    let gx = (x + 0.5).floor();
    let gz = (z + 0.5).floor();
    if gx < 0.0 || gz < 0.0 {
        return Tile::Wall;
    }
    floor.get(gx as usize, gz as usize)
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
