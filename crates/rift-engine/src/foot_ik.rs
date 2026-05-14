//! Terrain-aware foot IK pass.
//!
//! Industry-standard pipeline:
//!
//! ```text
//! Animation
//!     ↓ build_bone_palette  →  joint_worlds + palette
//!     ↓ apply_foot_ik       →  patches palette in place
//!     ↓ GPU skinning        →  vertex deformation
//! ```
//!
//! The IK *corrects* the authored animation; it doesn't replace
//! it. Per frame, per foot:
//!
//! 1. Read the animated foot world position.
//! 2. Sample the dungeon floor's analytic height + normal at the
//!    foot's XZ.
//! 3. Compute a swing-phase weight: 1.0 when the foot is at /
//!    below the ground, smoothly tapering to 0.0 at
//!    `SWING_THRESHOLD` above the ground. This is the critical
//!    piece that prevents IK from clamping the foot down during
//!    its airborne phase (the bug that broke the gait the first
//!    time we tried this).
//! 4. Smooth the desired correction toward its current value
//!    with an exponential filter so feet glide rather than pop.
//! 5. After both feet are computed, lower the pelvis by the
//!    most-negative correction (clamped ≤ 0) so when one foot
//!    steps down into a pit the upper leg doesn't have to
//!    over-extend.
//! 6. Two-bone analytical IK (hip → knee → ankle) drives each
//!    leg to its corrected target. Foot orientation is partially
//!    aligned to the surface normal so feet roll along ramp
//!    surfaces instead of staying flat.
//! 7. Carry foot descendants (toes / ball joints) along with
//!    the foot's rigid-body delta — they live in foot-local
//!    space so this preserves their relative pose.
//!
//! The IK runs in **skel-local space** (the same frame as
//! `joint_worlds`). The host `Transform` is yaw-only on bipeds,
//! so XZ in skel-local matches world after a yaw rotation. We
//! still need the host yaw to convert world XZ samples back
//! into skel-local XZ for the ground sampler.

use glam::{Mat4, Quat, Vec3};

use crate::ecs::components::FootIkState;
use crate::renderer::mesh::Joint;

/// Maximum foot height above ground at which any IK correction
/// is applied. Above this height the foot is considered fully
/// in swing phase and gets weight 0; the baked clip is
/// authoritative. Tuned to keep a planted-foot ankle (which
/// sits ~0.1 m above the sole, that's just where ankles are)
/// fully weighted while still tapering off well before the
/// peak of a typical run-cycle's swing arc (~0.35 m). The
/// fade band runs from `SWING_PLANTED` (full weight) up to
/// `SWING_PLANTED + SWING_FADE` (zero weight) on a smoothstep.
const SWING_PLANTED: f32 = 0.18;
const SWING_FADE: f32 = 0.20;

/// Plant-detection hysteresis (foot height above the avatar's
/// grounded plane). A foot becomes "planted" when it crosses
/// **below** [`PLANT_DOWN`] and stays planted until it rises
/// above [`PLANT_UP`]. The gap prevents chatter mid-swing for
/// rigs whose ankle bone bobs slightly while the sole is still
/// in contact, and keeps the detector stable when foot IK is
/// actively dragging the ankle toward the ground (the IK
/// correction can sit just below `PLANT_DOWN` for a frame or
/// two after touchdown). Tuned against the run cycle's
/// per-frame ankle-Y trace: the lowest mid-stance sample is
/// ~0.10 m, the swing peak is ~0.32 m.
const PLANT_DOWN: f32 = 0.12;
const PLANT_UP: f32 = 0.22;

/// Maximum vertical correction, in either direction.
///
/// The body's grounded Y is now sampled across the capsule
/// footprint (see `movement_system` in `ecs/systems.rs`), so
/// when the player straddles a ledge the body sits on the
/// **higher** tile. That means the trailing foot — over the
/// lower tile — needs to reach **down** by a full
/// `ELEVATION_STEP` (0.5 m) to plant. We size the cap to one
/// full step plus a touch of headroom for slope grading and
/// sub-tile noise. Going higher than this would be the IK
/// trying to do locomotion's job (climbing multi-step
/// terrain), which the body-Y sampler should be solving
/// instead — so a cap of ~0.6 m is the right "polish + one
/// ledge" budget.
const MAX_CORRECTION: f32 = 0.6;

/// Exponential-smoothing time constant. With a typical 60 Hz
/// frame this gives ~95 % convergence in ~0.25 s, which is
/// short enough that stair transitions read as snappy and long
/// enough that a single tile boundary doesn't pop the foot.
const SMOOTH_TAU: f32 = 0.08;

/// Pelvis recovery should be much quicker than foot placement
/// smoothing. Foot corrections need a little glide so planted feet
/// don't pop on stair edges, but the hips staying dropped after the
/// avatar has reached stable ground reads as an unwanted crouch.
const PELVIS_RECOVERY_TAU: f32 = 0.025;

/// Pelvis lowering is for asymmetric terrain contact: one foot down
/// in a pit / off a platform while the other is still near the body.
/// When both feet ask for roughly the same downward correction, that
/// is usually visible-body Y smoothing after stepping down, and the
/// legs should absorb it without also dragging the hips into a crouch.
const PELVIS_UNEVEN_START: f32 = 0.08;
const PELVIS_UNEVEN_FULL: f32 = 0.35;

/// Blend factor for foot rotation alignment with the surface
/// normal. Currently unused — see the comment in `solve_leg`
/// where the alignment was disabled because it requires a
/// per-rig "foot up" axis we don't yet have. Kept here as a
/// hook for the proper implementation.
#[allow(dead_code)]
const NORMAL_ALIGN_BLEND: f32 = 0.5;

/// Trait abstraction over "what's the ground at world (x, z)?"
/// so the IK pass can stay engine-side while the dungeon-grid
/// implementation lives in `rift_dungeon`. Returning normals
/// alongside heights costs nothing here and saves a second
/// dispatch per foot.
pub trait GroundSampler {
    /// Returns `(height_y, surface_normal)` at world `(x, z)`.
    /// Out-of-bounds / wall samples should return a sane upright
    /// value (height = 0, normal = +Y) — never panic.
    fn sample(&self, x: f32, z: f32) -> (f32, Vec3);
}

impl GroundSampler for &rift_dungeon::Floor {
    fn sample(&self, x: f32, z: f32) -> (f32, Vec3) {
        (self.tile_floor_y_at(x, z), self.tile_floor_normal_at(x, z))
    }
}

/// Identifies the (thigh, shin, foot) chain for one leg.
#[derive(Clone, Copy)]
struct LegChain {
    thigh: usize,
    shin: usize,
    foot: usize,
}

impl LegChain {
    /// Walk parents twice from `foot_idx` to discover the shin
    /// and thigh joints. Returns `None` if the chain is shorter
    /// than expected (e.g. foot's parent is the root) — IK is
    /// skipped in that case.
    fn from_foot(joints: &[Joint], foot_idx: usize) -> Option<Self> {
        if foot_idx >= joints.len() {
            return None;
        }
        let shin = joints[foot_idx].parent? as usize;
        if shin >= joints.len() {
            return None;
        }
        let thigh = joints[shin].parent? as usize;
        if thigh >= joints.len() {
            return None;
        }
        Some(Self {
            thigh,
            shin,
            foot: foot_idx,
        })
    }
}

/// Apply terrain-aware foot IK to the bone palette. `palette`
/// and `joint_worlds` are mutated in place: every patched joint
/// gets its `joint_worlds[i]` updated and the corresponding
/// `palette[i] = joint_worlds[i] * inverse_bind` rebuilt.
///
/// `state` is the persistent smoothing state — pass the same
/// instance across frames for one entity. `dt` drives temporal
/// smoothing of the corrections.
pub fn apply_foot_ik<G: GroundSampler>(
    joints: &[Joint],
    joint_worlds: &mut [Mat4],
    palette: &mut [Mat4],
    host_xform: &Mat4,
    grounded_y: f32,
    foot_l_idx: Option<usize>,
    foot_r_idx: Option<usize>,
    ground: &G,
    state: &mut FootIkState,
    dt: f32,
) {
    let n = joints.len();
    if joint_worlds.len() != n || palette.len() != n {
        return;
    }

    let host_pos = host_xform.col(3).truncate();
    // Yaw-only rotation: extract from the host matrix once and
    // reuse for both feet's world↔skel transforms. Composing
    // with `Mat4::from_quat(host_rot)` would also work but
    // pulling the column directly is allocation-free.
    let host_rot_mat = {
        let (_, r, _) = host_xform.to_scale_rotation_translation();
        r
    };

    // ── Per-foot pass 1: sample ground & compute desired correction ──
    let left_chain = foot_l_idx.and_then(|i| LegChain::from_foot(joints, i));
    let right_chain = foot_r_idx.and_then(|i| LegChain::from_foot(joints, i));

    let alpha = 1.0 - (-(dt.max(0.0) / SMOOTH_TAU)).exp();

    // Reference plane: the kinematic's authoritative grounded
    // Y. This used to be cached as `ground_at_center` and
    // subtracted from per-foot `gy` to produce a "delta from
    // centre" correction. That's been replaced by the
    // body-origin-relative formulation in `sample_foot` which
    // works correctly during body-Y smoothing too. `grounded_y`
    // is still consumed below by the swing-phase gate as the
    // reference for "how high is the foot above the floor",
    // because that test must stay stable across body-lift
    // smoothing windows.

    let sample_foot =
        |chain: LegChain, ground: &G, joint_worlds: &[Mat4]| -> (f32, Vec3, Vec3, f32) {
            let foot_skel = joint_worlds[chain.foot].col(3).truncate();
            let foot_world = host_pos + host_rot_mat * foot_skel;
            let (gy, gn) = ground.sample(foot_world.x, foot_world.z);
            // Correction = ground-under-foot minus body-origin Y.
            //
            // The foot's animated world Y is `host_pos.y +
            // foot_skel.y`. The IK target ends up at
            //   target = foot_world.y + correction
            //          = host_pos.y + foot_skel.y + (gy - host_pos.y)
            //          = gy + foot_skel.y
            // so the foot's sole lands on the ground under the
            // foot regardless of where the body origin currently
            // sits, and the animation's foot-Y curve (sole offset,
            // swing arc) is preserved through `foot_skel.y`.
            //
            // Earlier this used `gy - grounded_y`, which assumes
            // `host_pos.y == grounded_y`. The locomotion layer
            // exponentially smooths `host_pos.y` toward the
            // resolved ground over ~0.36 s to avoid snap-teleports
            // when stepping onto raised tiles, so during a lift
            // the two values diverge — and that's precisely when
            // the IK most needs to plant feet correctly. Using
            // `gy - host_pos.y` makes the IK absolute against the
            // real ground; body-Y smoothing is then a pure
            // *visual* parameter that doesn't affect plant
            // accuracy.
            let elevation_delta = gy - host_pos.y;
            // Swing-phase gate: how high is the ankle above the
            // *avatar's grounded plane* (the kinematic's resolved
            // support height — same frame the clip was authored
            // in). A planted ankle reads ~0.1 m (ankle-to-sole
            // offset), a swinging ankle reads 0.3+ m at peak.
            // Using `grounded_y` rather than the visible
            // `host_pos.y` is critical: during a sudden body
            // lift onto a raised tile, `host_pos.y` lags behind
            // by up to half a metre (it's mid-lerp), and a gate
            // computed against a stale body Y misclassifies a
            // planted foot as a swinging foot, dropping the IK
            // weight to zero just when we need it most.
            //
            // Two-stage gate: full weight up to `SWING_PLANTED`,
            // smoothstep down to zero across `SWING_FADE` metres.
            let foot_above_host = foot_world.y - grounded_y;
            let t = ((foot_above_host - SWING_PLANTED) / SWING_FADE).clamp(0.0, 1.0);
            let w = 1.0 - (t * t * (3.0 - 2.0 * t));
            let desired = elevation_delta.clamp(-MAX_CORRECTION, MAX_CORRECTION) * w;
            (desired, gn, foot_world, foot_above_host)
        };

    let (desired_l_y, desired_l_n, foot_l_world, foot_l_above) = match left_chain {
        Some(c) => sample_foot(c, ground, joint_worlds),
        None => (0.0, Vec3::Y, Vec3::ZERO, f32::INFINITY),
    };
    let (desired_r_y, desired_r_n, foot_r_world, foot_r_above) = match right_chain {
        Some(c) => sample_foot(c, ground, joint_worlds),
        None => (0.0, Vec3::Y, Vec3::ZERO, f32::INFINITY),
    };

    // Plant detection. Hysteresis on `foot_above_host` (the
    // ankle's height above the avatar's grounded plane, the
    // same signal the swing-phase gate uses): airborne→planted
    // bumps the per-foot sequence counter and stamps the
    // plant world position so audio / VFX consumers can fire
    // exactly once per real foot contact. This replaces the
    // velocity-derived gait synthesis that broke during
    // rolls and other movement effects — the animation itself
    // is now the source of truth for when feet hit the floor.
    update_plant(
        &mut state.left_planted,
        &mut state.left_plant_seq,
        &mut state.left_plant_pos,
        foot_l_above,
        foot_l_world,
        left_chain.is_some(),
    );
    update_plant(
        &mut state.right_planted,
        &mut state.right_plant_seq,
        &mut state.right_plant_pos,
        foot_r_above,
        foot_r_world,
        right_chain.is_some(),
    );

    // Smooth toward desired. Decay toward zero when no chain
    // exists / IK is disabled so any leftover correction from a
    // previous frame fades cleanly instead of locking in.
    state.left_correction_y += (desired_l_y - state.left_correction_y) * alpha;
    state.right_correction_y += (desired_r_y - state.right_correction_y) * alpha;
    state.left_normal = lerp_normal(state.left_normal, desired_l_n, alpha);
    state.right_normal = lerp_normal(state.right_normal, desired_r_n, alpha);

    // ── Pelvis adjustment ──
    //
    // Pelvis IK is **conservative** by design. It only ever
    // *lowers* the pelvis (never raises it), and only by a
    // fraction of the foot correction. Rationale:
    //
    //   * Raising the pelvis to follow a foot lifted onto a
    //     ledge causes the whole skeleton to float — the
    //     other foot leaves the ground because pelvis went
    //     up, and the IK then has to chase that, and the
    //     animation reads as a hover. Step-up motion belongs
    //     to the locomotion layer (kinematic snap +
    //     `visual_y` smoothing in `world_sync.rs`), not to
    //     foot IK.
    //   * Lowering the pelvis when one foot drops into a
    //     small dip is genuinely useful — it keeps the
    //     opposite leg from over-extending. But scaling
    //     by 0.6 means the foot does most of the work and
    //     the body stays mostly stable.
    let lower_foot = state.left_correction_y.min(state.right_correction_y);
    let higher_foot = state.left_correction_y.max(state.right_correction_y);
    let foot_unevenness = (higher_foot - lower_foot).max(0.0);
    let uneven_t = ((foot_unevenness - PELVIS_UNEVEN_START)
        / (PELVIS_UNEVEN_FULL - PELVIS_UNEVEN_START))
        .clamp(0.0, 1.0);
    let uneven_weight = uneven_t * uneven_t * (3.0 - 2.0 * uneven_t);
    let desired_pelvis = lower_foot.min(0.0) * 0.6 * uneven_weight;

    let pelvis_tau = if desired_pelvis > state.pelvis_offset_y {
        PELVIS_RECOVERY_TAU
    } else {
        SMOOTH_TAU
    };
    let pelvis_alpha = 1.0 - (-(dt.max(0.0) / pelvis_tau)).exp();
    state.pelvis_offset_y += (desired_pelvis - state.pelvis_offset_y) * pelvis_alpha;
    if state.pelvis_offset_y.abs() < 0.001 && desired_pelvis.abs() < 0.001 {
        state.pelvis_offset_y = 0.0;
    }

    if state.pelvis_offset_y.abs() > 1.0e-4 {
        translate_root_chain(joints, joint_worlds, palette, state.pelvis_offset_y);
    }

    // ── Per-foot pass 2: 2-bone IK to corrected target ──
    if let Some(c) = left_chain {
        solve_leg(
            joints,
            joint_worlds,
            palette,
            c,
            // Target = animated foot Y + correction - pelvis lift
            // (because we already moved the pelvis, the foot now
            // sits at anim_y + pelvis; subtracting recovers the
            // intended absolute target).
            state.left_correction_y - state.pelvis_offset_y,
            state.left_normal,
            &host_rot_mat,
        );
    }
    if let Some(c) = right_chain {
        solve_leg(
            joints,
            joint_worlds,
            palette,
            c,
            state.right_correction_y - state.pelvis_offset_y,
            state.right_normal,
            &host_rot_mat,
        );
    }
}

/// Translate the entire skeleton vertically by `offset_y`. For
/// a pure translation `lift = T(0, dy, 0)`, propagating it
/// through the parent chain is equivalent to applying it to
/// every joint's world matrix uniformly:
///   `child.world_new = lift * (parent.world_old * child_local)`
///                    `= lift * child.world_old`
/// So we just bump each joint's translation column by dy and
/// rebuild the palette entry. This is exactly what we want for
/// pelvis adjustment in foot IK — the whole upper body drops
/// with the pelvis, both hip joints follow, and the subsequent
/// per-leg IK pass restores foot positions.
fn translate_root_chain(
    joints: &[Joint],
    joint_worlds: &mut [Mat4],
    palette: &mut [Mat4],
    offset_y: f32,
) {
    for i in 0..joints.len() {
        joint_worlds[i].w_axis.y += offset_y;
        palette[i] = joint_worlds[i] * joints[i].inverse_bind;
    }
}

/// Slerp between two unit-ish surface normals with a linear
/// interpolation + renormalisation. Good enough for foot IK
/// where the inputs are nearly colinear (Y axis dominant).
fn lerp_normal(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    let n = a + (b - a) * t.clamp(0.0, 1.0);
    n.try_normalize().unwrap_or(Vec3::Y)
}

/// Update the plant state for one foot. Bumps `seq` and stamps
/// `plant_pos` on the airborne→planted transition. `chain_ok`
/// is `false` when the leg chain wasn't resolvable (no foot
/// joint, malformed skeleton); we then force the foot to the
/// "lifted" state without emitting events so a future frame
/// that resolves the chain will fire a clean first plant.
fn update_plant(
    planted: &mut bool,
    seq: &mut u32,
    plant_pos: &mut Vec3,
    foot_above_host: f32,
    foot_world: Vec3,
    chain_ok: bool,
) {
    if !chain_ok {
        *planted = false;
        return;
    }
    if *planted {
        if foot_above_host > PLANT_UP {
            *planted = false;
        }
    } else if foot_above_host < PLANT_DOWN {
        *planted = true;
        *seq = seq.wrapping_add(1);
        *plant_pos = foot_world;
    }
}

/// Analytical 2-bone IK for one leg (hip → knee → ankle).
///
/// `correction_y` is the **skel-local** Y offset to apply to
/// the foot relative to its current animated position. The
/// foot's XZ stays where the animation put it — only Y is
/// driven by terrain.
fn solve_leg(
    joints: &[Joint],
    joint_worlds: &mut [Mat4],
    palette: &mut [Mat4],
    chain: LegChain,
    correction_y: f32,
    surface_normal_world: Vec3,
    host_rot: &Quat,
) {
    if correction_y.abs() < 1.0e-4 {
        return;
    }

    let hip = joint_worlds[chain.thigh].col(3).truncate();
    let knee = joint_worlds[chain.shin].col(3).truncate();
    let ankle = joint_worlds[chain.foot].col(3).truncate();

    let target = Vec3::new(ankle.x, ankle.y + correction_y, ankle.z);

    let l1 = (knee - hip).length();
    let l2 = (ankle - knee).length();
    if l1 < 1.0e-4 || l2 < 1.0e-4 {
        return;
    }

    let to_target = target - hip;
    let d = to_target.length();
    if d < 1.0e-4 {
        return;
    }
    let d_min = (l1 - l2).abs() + 1.0e-3;
    let d_max = l1 + l2 - 1.0e-3;
    let d_clamped = d.clamp(d_min, d_max);
    let to_target_dir = to_target / d;

    // Bend direction: knees bend **forward** in the avatar's
    // local frame. For a biped this is invariant — humans
    // don't have hyperextending knees, and animations never
    // author knees bending sideways or backward. Using a
    // fixed forward direction is dramatically more stable
    // than trying to recover bend direction from the current
    // pose, because in the default standing pose the leg is
    // nearly straight and the knee's perpendicular component
    // (relative to the leg axis) is tiny and dominated by
    // rig noise — which is what was pushing knees sideways
    // and backward.
    //
    // Skel-local +Z is the avatar's forward (the host
    // transform applies a yaw to align skel-local with the
    // facing direction). We then take the component of +Z
    // that is perpendicular to the new leg axis — that's
    // the direction the knee should poke out toward.
    let forward_skel = Vec3::Z;
    let proj = forward_skel - to_target_dir * forward_skel.dot(to_target_dir);
    let perp = proj.try_normalize().unwrap_or(Vec3::Z);
    let _ = host_rot;

    let cos_alpha =
        ((l1 * l1 + d_clamped * d_clamped - l2 * l2) / (2.0 * l1 * d_clamped)).clamp(-1.0, 1.0);
    let alpha = cos_alpha.acos();

    let new_knee = hip + l1 * (alpha.cos() * to_target_dir + alpha.sin() * perp);
    let new_ankle = if d > d_max {
        hip + d_max * to_target_dir
    } else {
        target
    };

    // World-space rotation deltas.
    let old_thigh_dir = (knee - hip) / l1;
    let new_thigh_dir = (new_knee - hip).normalize();
    let rot_thigh = Quat::from_rotation_arc(old_thigh_dir, new_thigh_dir);

    let old_shin_dir = (ankle - knee) / l2;
    let intermediate = (rot_thigh * old_shin_dir).normalize();
    let new_shin_dir = (new_ankle - new_knee).normalize();
    let rot_shin = Quat::from_rotation_arc(intermediate, new_shin_dir);

    // Snapshot foot world before patching so we can carry toes.
    let old_foot_world = joint_worlds[chain.foot];

    // Patch thigh: rotation += rot_thigh, translation unchanged
    // (hip pivot).
    {
        let (s, r, _) = joint_worlds[chain.thigh].to_scale_rotation_translation();
        joint_worlds[chain.thigh] = Mat4::from_scale_rotation_translation(s, rot_thigh * r, hip);
        palette[chain.thigh] = joint_worlds[chain.thigh] * joints[chain.thigh].inverse_bind;
    }

    // Patch shin: rotation = rot_shin * rot_thigh * old, pivot
    // at new_knee.
    {
        let (s, r, _) = joint_worlds[chain.shin].to_scale_rotation_translation();
        joint_worlds[chain.shin] =
            Mat4::from_scale_rotation_translation(s, rot_shin * rot_thigh * r, new_knee);
        palette[chain.shin] = joint_worlds[chain.shin] * joints[chain.shin].inverse_bind;
    }

    // Patch foot: rotation follows the shin (preserving the
    // baked foot orientation relative to the lower leg). We
    // intentionally do **not** rotate the foot toward the
    // surface normal here — that requires knowing which of
    // the foot bone's local axes is "up out of the sole",
    // which varies per rig (some authoring tools use +Y,
    // others +Z, and a few have it flipped). Picking the
    // wrong axis applies a constant ~90° rotation even on
    // flat ground, making the foot point skyward. The shin's
    // IK rotation already tilts the foot along the slope's
    // travel axis (because the foot inherits `rot_shin *
    // rot_thigh * r`), which sells "the leg is climbing"
    // even without sole-to-surface conformance. If we want
    // proper foot-roll later it should be derived from a
    // per-rig "foot up" axis stored alongside the joint
    // index, not from a hardcoded `Vec3::Y`.
    let _ = surface_normal_world;
    {
        let (s, r, _) = joint_worlds[chain.foot].to_scale_rotation_translation();
        let new_foot_rot = rot_shin * rot_thigh * r;
        joint_worlds[chain.foot] =
            Mat4::from_scale_rotation_translation(s, new_foot_rot, new_ankle);
        palette[chain.foot] = joint_worlds[chain.foot] * joints[chain.foot].inverse_bind;
    }

    // Carry foot descendants (toe / ball joints) along by the
    // rigid-body delta old_foot_world → new_foot_world.
    let new_foot_world = joint_worlds[chain.foot];
    let foot_delta = new_foot_world * old_foot_world.inverse();
    let n = joints.len();
    let mut in_subtree = vec![false; n];
    in_subtree[chain.foot] = true;
    for i in 0..n {
        if i == chain.foot {
            continue;
        }
        let parent_in = joints[i]
            .parent
            .map(|p| {
                let pi = p as usize;
                pi < n && in_subtree[pi]
            })
            .unwrap_or(false);
        if !parent_in {
            continue;
        }
        in_subtree[i] = true;
        if i == chain.shin || i == chain.thigh {
            continue;
        }
        joint_worlds[i] = foot_delta * joint_worlds[i];
        palette[i] = joint_worlds[i] * joints[i].inverse_bind;
    }
}
