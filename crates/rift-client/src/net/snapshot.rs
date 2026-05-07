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
    messages::{EntityKind, Snapshot},
    ClientId, NetId, NetTick,
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
    pub airborne: bool,
    /// Latest full-body action id from the snapshot
    /// (`rift_game::kinematic::action`). Drives the dodge-roll
    /// animation on remote avatars.
    pub action: u8,
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
        self.last_server_tick = snap.tick;

        // Find our own row before draining `remote` so we have
        // both the authoritative state and the new ack seq in hand.
        let our_id = self.our_net_id;
        let our_auth = our_id.and_then(|nid| {
            snap.entities
                .iter()
                .find(|e| e.net_id == nid)
                .map(|e| (Vec3::from_array(e.position), Vec3::from_array(e.velocity), e.yaw))
        });
        // Track our own DEAD flag so input + prediction can stop
        // running on a corpse. The flag flips back to false once
        // the server respawns us via `LoadFloor` (handled there).
        if let Some(nid) = our_id {
            if let Some(e) = snap.entities.iter().find(|e| e.net_id == nid) {
                let now_dead =
                    e.flags & rift_net::messages::entity_flags::DEAD != 0;
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
                    airborne: e.flags & rift_net::messages::entity_flags::AIRBORNE != 0,
                    action,
                },
            );
            self.last_positions.insert(net_id, position);
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
                    // Shift current → previous, slot the new one in
                    // as current. `curr_arrival = now` resets the
                    // ramp so the next frame starts at alpha=0.
                    b.prev = b.curr;
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
        if let Some((auth_pos, auth_vel, auth_yaw)) = our_auth {
            self.input_history
                .retain(|(seq, _, _)| seq.wrapping_sub(snap.ack_seq) as i32 > 0);

            let prev = self.predicted.position;
            self.predicted.position = auth_pos;
            self.predicted.velocity = auth_vel;
            self.predicted.yaw = auth_yaw;

            if let Some(floor) = self.predict_floor.as_ref() {
                let history: Vec<(u32, f32, rift_net::messages::InputCmd)> =
                    self.input_history.iter().cloned().collect();
                for (_, dt, cmd) in history {
                    rift_game::kinematic::apply_input(
                        &mut self.predicted,
                        cmd.move_dir,
                        cmd.aim_dir,
                        cmd.buttons,
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
        // Interp window first: blend prev → curr over one period.
        let alpha = (elapsed / period).clamp(0.0, 1.0);
        let mut position = b.prev.position.lerp(b.curr.position, alpha);
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
        Some(DisplaySample { position, yaw, aim_yaw })
    }
}
