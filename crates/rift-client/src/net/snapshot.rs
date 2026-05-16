//! Snapshot ingestion + interpolation buffers + reconciliation of
//! the locally-predicted player against the latest authoritative
//! state.
//!
//! `apply_snapshot` is the only writer for the `interp` map and
//! the prediction state (`predicted`, `correction_error`,
//! `input_history`). Render-time consumers live in
//! [`super::world_sync`].

use std::time::{Duration, Instant};

use glam::{Quat, Vec3};
use rift_net::{
    messages::{EntityKind, SnapshotDelta},
    ClientId, NetId, NetTick, Snapshot,
};

use super::NetClient;

/// Snapshot of one remote-controlled entity, derived from the
/// latest server snapshot. Drives this frame's per-remote ECS
/// Transform/Velocity write.
#[derive(Clone, Debug)]
pub struct RemoteEntity {
    pub net_id: NetId,
    pub kind: EntityKind,
    pub position: Vec3,
    pub yaw: f32,
    pub velocity: Vec3,
    pub health_pct: f32,
    /// Current target/focus replicated by the server when known.
    pub target_net_id: Option<NetId>,
    /// Essence (universal ability resource) 0..=1. Meaningful
    /// only on the local player's own row — server fills 1.0
    /// for everyone else. HUD reads it via
    /// `NetClient::take_resource_pct` once the local row arrives.
    pub resource_pct: f32,
    /// Snapshot row carried `entity_flags::DEAD` this frame.
    pub dead: bool,
    pub airborne: bool,
    /// Latest full-body action id from the snapshot
    /// (`rift_game::kinematic::action`). Drives the dodge-roll
    /// animation on remote avatars.
    pub action: u8,
    /// Active buff / debuff list mirrored from the snapshot
    /// row. Empty for entities without a server-side stack.
    pub effects: Vec<rift_net::messages::ActiveEffect>,
}

/// Two-sample interpolation buffer for one remote entity. The
/// renderer always reads `prev → curr` blended by an alpha derived
/// from wall-clock time, with a one-snapshot-period render delay so
/// `curr` is always in the future relative to what we display. This
/// turns 20 Hz snapshot delivery into 60+ Hz visually smooth motion.
///
/// We also stash the latest snapshot velocity so [`NetClient::interp_sample`]
/// can dead-reckon a short distance past `curr` when snapshots are
/// late or dropped — without it, a jittered or lost snapshot freezes
/// the entity in place client-side even though the server kept it
/// moving (and may already be inside attack range dealing damage).
#[derive(Clone, Copy, Debug)]
pub(super) struct InterpSample {
    pub(super) position: Vec3,
    pub(super) yaw: f32,
    /// Aim yaw (spine-twist / cursor direction). Interpolated with
    /// the rest so spell beams and aim cones don't jitter at the
    /// snapshot rate.
    pub(super) aim_yaw: f32,
    /// Most recent snapshot velocity (world-space, m/s). Used by
    /// `interp_sample` to extrapolate when wall-clock has advanced
    /// past `curr_arrival + snapshot_period`.
    pub(super) velocity: Vec3,
}

#[derive(Clone, Debug)]
pub(crate) struct RemoteInterp {
    pub(super) prev: InterpSample,
    pub(super) curr: InterpSample,
    /// Wall-clock time at which `curr` arrived. Display alpha ramps
    /// from 0 to 1 over the next snapshot period after this.
    pub(super) curr_arrival: Instant,
}

/// A floor transition the server has told us about and we haven't
/// yet applied to the visual world. Drained once per frame by the
/// binary, which reruns the equivalent of the SP `perform_transition`
/// path with the server-provided seed/index. Until drained, snapshot
/// processing is paused (see `apply_snapshot`) so we don't try to
/// place avatars in an old-floor world.
#[derive(Clone, Debug)]
pub struct PendingFloor {
    pub seed: u64,
    pub index: u32,
    pub is_hub: bool,
    pub spawn_pos: Vec3,
    pub tick: NetTick,
}

/// Cosmetic identity for a remote player. Received via
/// `ServerMsg::PlayerJoined` and consumed when we spawn that
/// player's avatar entity into the local world.
#[derive(Clone, Debug)]
pub struct RemoteProfile {
    pub net_id: NetId,
    pub client_id: ClientId,
    pub character_name: String,
    pub class_id: String,
    pub gender: rift_net::messages::Gender,
    pub appearance: rift_net::messages::Appearance,
}

impl NetClient {
    /// Rebuild `remote` from the latest server snapshot, and
    /// reconcile our local prediction with the authoritative
    /// position the server reports for our own player.
    pub(super) fn apply_snapshot(&mut self, snap: Snapshot) {
        // Drop stale snapshots — UDP can deliver out of order.
        if snap.tick.diff(self.last_server_tick) <= 0 {
            return;
        }
        let latest_snapshot = snap.clone();
        self.last_server_tick = snap.tick;

        // Find our own row before draining `remote` so we have
        // both the authoritative state and the new ack seq in hand.
        let our_id = self.our_net_id;
        let our_auth = our_id.and_then(|nid| {
            snap.entities.iter().find(|e| e.net_id == nid).map(|e| {
                // Pull action + action_start from the Player
                // payload so the reconcile path below can
                // re-derive `roll_remaining` from the server's
                // clock instead of letting the local timer
                // drift by ~RTT/2 every dodge.
                let (action, action_start) = match &e.kind {
                    EntityKind::Player {
                        action,
                        action_start,
                        ..
                    } => (*action, *action_start),
                    _ => (0u8, NetTick::default()),
                };
                (
                    Vec3::from_array(e.position),
                    Vec3::from_array(e.velocity),
                    e.yaw,
                    action,
                    action_start,
                )
            })
        });
        // Track our own DEAD flag so input + prediction can stop
        // running on a corpse. The flag flips back to false once
        // the server respawns us via `LoadFloor` (handled there).
        if let Some(nid) = our_id {
            if let Some(e) = snap.entities.iter().find(|e| e.net_id == nid) {
                let now_dead = e.flags & rift_net::messages::entity_flags::DEAD != 0;
                if now_dead && !self.local_dead {
                    // First snapshot of death: drop any unacked
                    // pre-death movement inputs so the reconcile
                    // path below doesn't replay WASD on top of
                    // the corpse position every snapshot, which
                    // would oscillate `predicted` and visibly
                    // shake the death-animation avatar around
                    // (#jitter).
                    self.input_history.clear();
                    self.correction_error = Vec3::ZERO;
                }
                self.local_dead = now_dead;
                self.local_ghost = e.flags & rift_net::messages::entity_flags::GHOST != 0;
            }
        }

        self.remote.clear();
        let now = Instant::now();
        let our_id_for_interp = self.our_net_id;
        for e in snap.entities {
            let net_id = e.net_id;
            let position = Vec3::from_array(e.position);
            let yaw = e.yaw;
            let action = match &e.kind {
                EntityKind::Player { action, .. } => *action,
                _ => 0,
            };
            let aim_yaw = match &e.kind {
                EntityKind::Player { aim_yaw, .. } => *aim_yaw,
                _ => yaw,
            };
            self.remote.insert(
                net_id,
                RemoteEntity {
                    net_id,
                    kind: e.kind,
                    position,
                    yaw,
                    velocity: Vec3::from_array(e.velocity),
                    health_pct: e.health_pct,
                    target_net_id: e.target_net_id,
                    resource_pct: e.resource_pct,
                    dead: e.flags & rift_net::messages::entity_flags::DEAD != 0,
                    airborne: e.flags & rift_net::messages::entity_flags::AIRBORNE != 0,
                    action,
                    effects: e.effects,
                },
            );
            self.last_positions.insert(net_id, position);
            // Stash velocity alongside position so the death-event
            // handler can reconstruct an impact direction even
            // when the row has already been culled by the time
            // the reliable Death packet arrives. Used by the
            // layered blood decal system to orient the corpse
            // pool / spray fan / wall arc along the kill axis.
            self.last_velocities
                .insert(net_id, Vec3::from_array(e.velocity));
            // Skip our own row — we own the local player's transform
            // through prediction, not through the interp buffer.
            if Some(net_id) == our_id_for_interp {
                continue;
            }
            let new_sample = InterpSample {
                position,
                yaw,
                aim_yaw,
                velocity: Vec3::from_array(e.velocity),
            };
            self.interp
                .entry(net_id)
                .and_modify(|b| {
                    // When a new snapshot lands, what we display
                    // *right now* is the Hermite interp from
                    // `b.prev → b.curr` plus possibly some dead-
                    // reckoning past `b.curr`. The naive update
                    // (`prev = b.curr`) discards that extrapolation,
                    // which at dash speed pops the model backward
                    // ~0.1–0.2 m every snapshot boundary. Carry
                    // the currently-displayed sample forward as the
                    // new `prev` so the next interp window starts
                    // exactly where this frame's render ends.
                    let snapshot_period = 1.0 / rift_net::SNAPSHOT_HZ as f32;
                    let elapsed = now.saturating_duration_since(b.curr_arrival).as_secs_f32();
                    let alpha = (elapsed / snapshot_period.max(1e-4)).clamp(0.0, 1.0);
                    let mut display_pos = hermite_position(
                        b.prev.position,
                        b.curr.position,
                        b.prev.velocity,
                        b.curr.velocity,
                        alpha,
                        snapshot_period,
                    );
                    if elapsed > snapshot_period {
                        let extrap = (elapsed - snapshot_period).min(snapshot_period * 4.0);
                        display_pos += b.curr.velocity * extrap;
                    }
                    let q_prev = Quat::from_rotation_y(b.prev.yaw);
                    let q_curr = Quat::from_rotation_y(b.curr.yaw);
                    let (display_yaw, _, _) =
                        q_prev.slerp(q_curr, alpha).to_euler(glam::EulerRot::YXZ);
                    let qa_prev = Quat::from_rotation_y(b.prev.aim_yaw);
                    let qa_curr = Quat::from_rotation_y(b.curr.aim_yaw);
                    let (display_aim_yaw, _, _) =
                        qa_prev.slerp(qa_curr, alpha).to_euler(glam::EulerRot::YXZ);
                    b.prev = InterpSample {
                        position: display_pos,
                        yaw: display_yaw,
                        aim_yaw: display_aim_yaw,
                        // Use the previous snapshot's velocity as
                        // the prev-tangent. Mixing the displayed
                        // position with the old endpoint velocity
                        // keeps Hermite's first derivative
                        // continuous across the boundary.
                        velocity: b.curr.velocity,
                    };
                    b.curr = new_sample;
                    b.curr_arrival = now;
                })
                .or_insert_with(|| RemoteInterp {
                    prev: new_sample,
                    curr: new_sample,
                    curr_arrival: now,
                });
        }

        // Reconcile: drop acked inputs from history, snap to the
        // server's authoritative state, then replay everything
        // still in flight to recover the predicted "now".
        if let Some((auth_pos, auth_vel, auth_yaw, auth_action, auth_action_start)) = our_auth {
            self.input_history
                .retain(|(seq, _, _)| seq.wrapping_sub(snap.ack_seq) as i32 > 0);

            let prev = self.predicted.position;
            self.predicted.position = auth_pos;
            self.predicted.velocity = auth_vel;
            self.predicted.yaw = auth_yaw;

            // Reconcile dodge-roll state from the authoritative
            // snapshot. Without this the local `roll_remaining`
            // clock ticks independently of the server's: locally
            // we start the roll on the input frame, the server
            // can't start until our cast packet has flown
            // one-way (~RTT/2 later), so the client's roll ends
            // earlier and the next few snapshots keep snapping
            // the predicted position back into the still-rolling
            // server pose. Replay-on-reconcile alone can't fix
            // this either — buffered inputs don't carry the
            // one-shot ROLL trigger (that lives in commands.rs's
            // `start_roll` call, not in button bits).
            if auth_action == rift_game::kinematic::action::ROLL {
                let elapsed_ticks = snap.tick.diff(auth_action_start).max(0) as f32;
                let elapsed_s = elapsed_ticks / rift_net::TICK_HZ as f32;
                let remaining = (rift_game::kinematic::ROLL_DURATION - elapsed_s).max(0.0);
                self.predicted.roll_remaining = remaining;
                self.predicted.action = rift_game::kinematic::action::ROLL;
                // Reconstruct the locked roll direction from the
                // server's reported XZ velocity (during an active
                // roll the kinematic writes `velocity = roll_dir *
                // speed_curve`, so normalising it recovers the
                // direction the server chose). Fall back to the
                // existing local `roll_dir` if the snapshot vel
                // is degenerate (e.g. capsule pinned against a
                // wall mid-roll).
                let vx = auth_vel.x;
                let vz = auth_vel.z;
                let len_sq = vx * vx + vz * vz;
                if len_sq > 1.0e-4 {
                    let inv = len_sq.sqrt().recip();
                    self.predicted.roll_dir = [vx * inv, vz * inv];
                }
            } else if rift_game::kinematic::action::is_attack(auth_action) {
                // Same reconcile recipe as the roll branch
                // above, scaled to the swing window. Melee
                // is a single fixed-duration action; we just
                // mirror `attack_remaining` from the server's
                // clock so prediction expires in sync.
                let elapsed_ticks = snap.tick.diff(auth_action_start).max(0) as f32;
                let elapsed_s = elapsed_ticks / rift_net::TICK_HZ as f32;
                let total = rift_game::kinematic::MELEE_ATTACK.duration;
                let remaining = (total - elapsed_s).max(0.0);
                self.predicted.attack_remaining = remaining;
                self.predicted.action = auth_action;
                self.predicted.roll_remaining = 0.0;
            } else {
                // Server says the roll has ended (or never
                // started). Clear the local timer so apply_input
                // stops overriding velocity with the roll curve.
                self.predicted.roll_remaining = 0.0;
                self.predicted.attack_remaining = 0.0;
                self.predicted.action = rift_game::kinematic::action::NONE;
            }

            if let Some(floor) = self.predict_floor.as_ref() {
                let history: Vec<(u32, f32, rift_net::messages::InputCmd)> =
                    self.input_history.iter().cloned().collect();
                for (_, dt, cmd) in history {
                    rift_game::kinematic::apply_input(
                        &mut self.predicted,
                        cmd.move_dir,
                        cmd.aim_dir,
                        cmd.buttons,
                        self.predicted_move_speed,
                    );
                    rift_game::kinematic::integrate(&mut self.predicted, floor, dt);
                }
            }

            // Roll the visible position correction into
            // `correction_error` so we bleed it off smoothly
            // instead of teleporting on each snapshot.
            //
            // We deliberately keep `correction_error` 2D (XZ
            // only): `sync_local_player` writes only X/Z back to
            // the local Transform — Y is owned by the SP
            // `movement_system`'s gravity/jump path. Letting Y
            // accumulate here would cause the length cap below
            // to fire mid-jump (peak height ~2 m), wholesale
            // zeroing the XZ component too and producing a
            // visible micro-jolt while airborne.
            if self.predicted_ready {
                let mut delta = prev - self.predicted.position;
                delta.y = 0.0;
                self.correction_error += delta;
                // Cap the smoothing budget so a real teleport
                // (death respawn, floor transition) snaps cleanly.
                if self.correction_error.length() > 1.5 {
                    self.correction_error = Vec3::ZERO;
                }
            } else {
                self.predicted_ready = true;
            }
        }

        let enemy_count = self
            .remote
            .values()
            .filter(|re| matches!(re.kind, EntityKind::Enemy { .. }))
            .count();
        log::debug!(
            "net: snapshot tick={:?} entities={} (enemies={}) ack_seq={} pending_inputs={}",
            snap.tick,
            self.remote.len(),
            enemy_count,
            snap.ack_seq,
            self.input_history.len(),
        );
        self.latest_snapshot = Some(latest_snapshot);
    }

    pub(super) fn apply_snapshot_delta(&mut self, delta: SnapshotDelta) {
        if delta.tick.diff(self.last_server_tick) <= 0 {
            return;
        }
        let Some(base) = self.latest_snapshot.as_ref() else {
            log::debug!(
                "net: dropped snapshot delta tick={:?} base={:?}: no local baseline",
                delta.tick,
                delta.base_tick
            );
            return;
        };
        let delta_tick = delta.tick;
        let delta_base = delta.base_tick;
        match base.apply_delta(delta) {
            Some(snapshot) => self.apply_snapshot(snapshot),
            None => log::debug!(
                "net: dropped snapshot delta tick={delta_tick:?} base={delta_base:?}: baseline mismatch local={:?}",
                base.tick
            ),
        }
    }
}

/// Per-frame display sample used by the world-sync code: the
/// interpolated position / yaw / aim-yaw for one remote entity.
/// Returned by [`NetClient::interp_sample`] so the avatar / enemy
/// drivers don't have to re-implement the slerp + alpha math.
#[derive(Clone, Copy, Debug)]
pub(super) struct DisplaySample {
    pub position: Vec3,
    pub yaw: f32,
    pub aim_yaw: f32,
}

impl NetClient {
    /// Compute the smoothed `prev → curr` blend for one remote
    /// entity at wall-clock `now`. Returns `None` when no interp
    /// buffer exists for `net_id` yet — caller falls back to the
    /// raw snapshot value.
    pub(super) fn interp_sample(&self, net_id: NetId, now: Instant) -> Option<DisplaySample> {
        let b = self.interp.get(&net_id)?;
        let snapshot_period = Duration::from_secs_f32(1.0 / rift_net::SNAPSHOT_HZ as f32);
        let elapsed = now.saturating_duration_since(b.curr_arrival).as_secs_f32();
        let period = snapshot_period.as_secs_f32().max(1e-4);
        // Hermite cubic interpolation over the prev → curr window.
        // Linear `lerp` produces a constant visual velocity inside
        // each window, which then flips abruptly to the dead-
        // reckoning velocity (`curr.velocity`) at alpha = 1.0. For
        // slow-moving entities that bump is invisible, but at
        // 4.5× base speed (stalker dash) it shows up as choppy
        // staccato motion at the snapshot boundaries. Hermite
        // matches both endpoint velocities, so the visual derivative
        // stays C¹ across the boundary into the dead-reckon segment
        // — motion reads as one smooth curve instead of a chain
        // of straight chords.
        let alpha = (elapsed / period).clamp(0.0, 1.0);
        let mut position = hermite_position(
            b.prev.position,
            b.curr.position,
            b.prev.velocity,
            b.curr.velocity,
            alpha,
            period,
        );
        // If we've already consumed the whole interp window and the
        // next snapshot still hasn't landed, dead-reckon forward at
        // the last known velocity. Capped at 4 snapshot periods
        // (~0.2 s at 20 Hz) so a truly stale entity doesn't drift
        // arbitrarily far from its real position before reconciling.
        if elapsed > period {
            let extrap = (elapsed - period).min(period * 4.0);
            position += b.curr.velocity * extrap;
        }
        // Shortest-path yaw lerp via Quat slerp so a wraparound
        // from +π → -π doesn't spin the avatar.
        let q_prev = Quat::from_rotation_y(b.prev.yaw);
        let q_curr = Quat::from_rotation_y(b.curr.yaw);
        let q = q_prev.slerp(q_curr, alpha);
        let (yaw, _, _) = q.to_euler(glam::EulerRot::YXZ);
        let qa_prev = Quat::from_rotation_y(b.prev.aim_yaw);
        let qa_curr = Quat::from_rotation_y(b.curr.aim_yaw);
        let qa = qa_prev.slerp(qa_curr, alpha);
        let (aim_yaw, _, _) = qa.to_euler(glam::EulerRot::YXZ);
        Some(DisplaySample {
            position,
            yaw,
            aim_yaw,
        })
    }
}

/// Cubic Hermite interpolation between two position samples with
/// matching endpoint velocities. `period` is the snapshot interval
/// in seconds — used to convert m/s velocities into per-window
/// tangent vectors. At `t = 0` returns `p0` with derivative `v0`,
/// at `t = 1` returns `p1` with derivative `v1`.
#[inline]
fn hermite_position(p0: Vec3, p1: Vec3, v0: Vec3, v1: Vec3, t: f32, period: f32) -> Vec3 {
    let t2 = t * t;
    let t3 = t2 * t;
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;
    p0 * h00 + (v0 * period) * h10 + p1 * h01 + (v1 * period) * h11
}
