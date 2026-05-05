//! Skeletal animation: clip loading, sampling, and bone palette generation.
//!
//! A `Clip` is a flat list of `Channel`s, each driving one transform component
//! (T, R, or S) of one skeleton joint. Clips are loaded from glTF files (often
//! separate "animation library" .glb files that share a skeleton with a
//! character mesh).
//!
//! At runtime an `Animator` advances a `time` cursor through one (or two,
//! when blending) clips and produces a `bone_palette: Vec<Mat4>` suitable for
//! `SkinnedMesh::skin_to`.

use glam::{Mat4, Quat, Vec3};
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Path3 { Translation, Rotation, Scale }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interp { Linear, Step }

/// One animation channel, targeting one component of one joint.
///
/// `joint` is an index into the consumer's `SkinnedMesh::joints`. Channels
/// whose target node doesn't correspond to any joint of the bound skin are
/// dropped at clip-binding time.
#[derive(Clone, Debug)]
pub struct Channel {
    pub joint: u16,
    pub path: Path3,
    pub interp: Interp,
    /// Keyframe times, monotonically increasing.
    pub times: Vec<f32>,
    /// Translation/scale: 3 floats per key. Rotation: 4 floats per key (quat xyzw).
    pub values: Vec<f32>,
}

impl Channel {
    fn key_indices_for(&self, t: f32) -> (usize, usize, f32) {
        let n = self.times.len();
        if n == 0 {
            return (0, 0, 0.0);
        }
        if t <= self.times[0] {
            return (0, 0, 0.0);
        }
        if t >= self.times[n - 1] {
            return (n - 1, n - 1, 0.0);
        }
        // Binary search for the upper bound.
        let mut lo = 0usize;
        let mut hi = n - 1;
        while lo + 1 < hi {
            let mid = (lo + hi) / 2;
            if self.times[mid] <= t { lo = mid; } else { hi = mid; }
        }
        let span = (self.times[hi] - self.times[lo]).max(1e-6);
        let f = ((t - self.times[lo]) / span).clamp(0.0, 1.0);
        (lo, hi, f)
    }

    fn sample_vec3(&self, t: f32) -> Vec3 {
        let (i0, i1, f) = self.key_indices_for(t);
        let a = Vec3::new(self.values[i0 * 3], self.values[i0 * 3 + 1], self.values[i0 * 3 + 2]);
        let b = Vec3::new(self.values[i1 * 3], self.values[i1 * 3 + 1], self.values[i1 * 3 + 2]);
        match self.interp {
            Interp::Step => a,
            Interp::Linear => a.lerp(b, f),
        }
    }

    fn sample_quat(&self, t: f32) -> Quat {
        let (i0, i1, f) = self.key_indices_for(t);
        let a = Quat::from_xyzw(
            self.values[i0 * 4],
            self.values[i0 * 4 + 1],
            self.values[i0 * 4 + 2],
            self.values[i0 * 4 + 3],
        ).normalize();
        let b = Quat::from_xyzw(
            self.values[i1 * 4],
            self.values[i1 * 4 + 1],
            self.values[i1 * 4 + 2],
            self.values[i1 * 4 + 3],
        ).normalize();
        match self.interp {
            Interp::Step => a,
            Interp::Linear => a.slerp(b, f),
        }
    }
}

/// One animation clip.
///
/// A clip is *not* tied to a specific skeleton at load time — channels
/// reference glTF node indices from the source file. To play a clip on a
/// skeleton, call `Clip::bind_to_skeleton` which remaps channels onto joint
/// indices using joint *names* (glTF node names).
#[derive(Clone, Debug)]
pub struct Clip {
    pub name: String,
    pub duration: f32,
    /// Channels indexed by source glTF node *name* — bind to a skeleton to
    /// produce a `BoundClip` whose channels reference joint indices.
    pub channels_by_node_name: Vec<RawChannel>,
}

/// Channel as parsed from glTF, before binding to a target skeleton.
#[derive(Clone, Debug)]
pub struct RawChannel {
    pub node_name: String,
    pub path: Path3,
    pub interp: Interp,
    pub times: Vec<f32>,
    pub values: Vec<f32>,
}

/// A clip whose channels have been remapped to a specific skeleton.
#[derive(Clone, Debug)]
pub struct BoundClip {
    pub name: String,
    pub duration: f32,
    pub channels: Vec<Channel>,
    /// Number of joints expected in the skeleton this clip is bound to.
    pub joint_count: usize,
}

impl Clip {
    /// Load every animation in a glTF / .glb file as a separate `Clip`.
    /// Buffers are loaded but images are skipped (animation files don't
    /// reference textures, but the helper is defensive anyway).
    pub fn load_all<P: AsRef<Path>>(path: P) -> anyhow::Result<Vec<Clip>> {
        let original = path.as_ref().to_path_buf();
        let candidates = [
            original.clone(),
            std::path::PathBuf::from("..").join(&original),
            std::path::PathBuf::from("../..").join(&original),
            std::path::PathBuf::from("../../..").join(&original),
        ];
        let resolved = candidates.iter().find(|p| p.exists()).cloned()
            .ok_or_else(|| anyhow::anyhow!(
                "animation gltf file not found in any candidate path (cwd={:?}): {:?}",
                std::env::current_dir().ok(), original
            ))?;
        log::info!("Loading animations from {:?}", resolved);

        let gltf = gltf::Gltf::open(&resolved)
            .map_err(|e| anyhow::anyhow!("gltf open failed for {:?}: {}", resolved, e))?;
        let base_dir = resolved.parent().unwrap_or_else(|| std::path::Path::new("."));
        let buffers = gltf::import_buffers(&gltf.document, Some(base_dir), gltf.blob.clone())
            .map_err(|e| anyhow::anyhow!("gltf buffer load failed for {:?}: {}", resolved, e))?;
        let doc = gltf.document;

        let mut clips = Vec::with_capacity(doc.animations().count());
        for anim in doc.animations() {
            let name = anim.name().unwrap_or("anim").to_string();
            let mut raw_channels: Vec<RawChannel> = Vec::new();
            let mut max_t: f32 = 0.0;
            for ch in anim.channels() {
                let target_node = ch.target().node();
                let node_name = target_node.name().unwrap_or("").to_string();
                if node_name.is_empty() {
                    continue;
                }
                let path = match ch.target().property() {
                    gltf::animation::Property::Translation => Path3::Translation,
                    gltf::animation::Property::Rotation => Path3::Rotation,
                    gltf::animation::Property::Scale => Path3::Scale,
                    _ => continue, // weights not supported yet
                };
                let sampler = ch.sampler();
                let interp = match sampler.interpolation() {
                    gltf::animation::Interpolation::Step => Interp::Step,
                    // CubicSpline is rare in our packs — fall back to linear
                    // for now (visually fine; correct cubic math added later).
                    _ => Interp::Linear,
                };
                let reader = ch.reader(|b| Some(&buffers[b.index()]));
                let times: Vec<f32> = match reader.read_inputs() {
                    Some(it) => it.collect(),
                    None => continue,
                };
                let values: Vec<f32> = match reader.read_outputs() {
                    Some(out) => match out {
                        gltf::animation::util::ReadOutputs::Translations(it) =>
                            it.flat_map(|v| v.into_iter()).collect(),
                        gltf::animation::util::ReadOutputs::Scales(it) =>
                            it.flat_map(|v| v.into_iter()).collect(),
                        gltf::animation::util::ReadOutputs::Rotations(rot) =>
                            rot.into_f32().flat_map(|v| v.into_iter()).collect(),
                        _ => continue,
                    },
                    None => continue,
                };
                if let Some(&t) = times.last() { max_t = max_t.max(t); }
                raw_channels.push(RawChannel { node_name, path, interp, times, values });
            }
            if raw_channels.is_empty() {
                continue;
            }
            clips.push(Clip {
                name,
                duration: max_t.max(0.001),
                channels_by_node_name: raw_channels,
            });
        }

        log::info!("  Found {} animation clip(s)", clips.len());
        Ok(clips)
    }

    /// Remap this clip's node-name-keyed channels onto a skeleton, dropping
    /// any channel whose target name isn't a joint of that skeleton.
    pub fn bind_to_skeleton(&self, joint_index_by_name: &HashMap<String, u16>, joint_count: usize) -> BoundClip {
        let mut channels = Vec::with_capacity(self.channels_by_node_name.len());
        for raw in &self.channels_by_node_name {
            if let Some(&j) = joint_index_by_name.get(&raw.node_name) {
                channels.push(Channel {
                    joint: j,
                    path: raw.path,
                    interp: raw.interp,
                    times: raw.times.clone(),
                    values: raw.values.clone(),
                });
            }
        }
        BoundClip {
            name: self.name.clone(),
            duration: self.duration,
            channels,
            joint_count,
        }
    }
}

/// Per-character animation playback state with optional cross-fade.
///
/// When `switch_to` is called the previous clip is retained as `prev` and a
/// `blend` factor ramps from 0 → 1 over `transition_duration` seconds. While
/// the blend is active, both clips are sampled and their poses are mixed at
/// the joint-TRS level (lerp T/S, slerp R) for clean transitions.
#[derive(Clone)]
pub struct Animator {
    pub clip: std::sync::Arc<BoundClip>,
    /// Current playback time in seconds, modulo `clip.duration`.
    pub time: f32,
    /// Time multiplier (1.0 = normal speed).
    pub speed: f32,
    /// If true, time wraps at clip end. If false, it clamps and the clip
    /// stays on its last pose (useful for one-shots like Death).
    pub looping: bool,

    /// Outgoing clip during a cross-fade. None when no transition is active.
    pub prev: Option<std::sync::Arc<BoundClip>>,
    /// Time cursor for the outgoing clip. Continues to advance during the fade.
    pub prev_time: f32,
    /// Speed multiplier captured at switch time, used for the outgoing clip.
    pub prev_speed: f32,
    /// 0.0 = fully on prev, 1.0 = fully on `clip`. Always 1.0 outside fades.
    pub blend: f32,
    /// Seconds remaining in the current cross-fade. 0 = no fade in progress.
    pub blend_time_remaining: f32,
    /// Total duration the most recent fade was scheduled for (used to recompute
    /// `blend` from time remaining).
    pub blend_total: f32,
}

impl Animator {
    pub fn new(clip: std::sync::Arc<BoundClip>) -> Self {
        Self {
            clip, time: 0.0, speed: 1.0, looping: true,
            prev: None, prev_time: 0.0, prev_speed: 1.0,
            blend: 1.0, blend_time_remaining: 0.0, blend_total: 0.0,
        }
    }

    pub fn advance(&mut self, dt: f32) {
        self.time += dt * self.speed;
        if self.looping {
            if self.clip.duration > 0.0 {
                self.time = self.time.rem_euclid(self.clip.duration);
            }
        } else {
            self.time = self.time.clamp(0.0, self.clip.duration);
        }
        if self.blend_time_remaining > 0.0 {
            // Advance the outgoing clip too so its motion stays alive.
            self.prev_time += dt * self.prev_speed;
            if let Some(prev) = &self.prev {
                if prev.duration > 0.0 {
                    self.prev_time = self.prev_time.rem_euclid(prev.duration);
                }
            }
            self.blend_time_remaining = (self.blend_time_remaining - dt).max(0.0);
            self.blend = if self.blend_total > 0.0 {
                1.0 - (self.blend_time_remaining / self.blend_total)
            } else { 1.0 };
            // Smoothstep for nicer feel.
            self.blend = self.blend.clamp(0.0, 1.0);
            let b = self.blend;
            self.blend = b * b * (3.0 - 2.0 * b);
            if self.blend_time_remaining <= 0.0 {
                self.prev = None;
                self.blend = 1.0;
                self.blend_total = 0.0;
            }
        }
    }

    /// Begin a cross-fade to `clip` over `duration` seconds. If `clip` is the
    /// same Arc as the current one this is a no-op. Pass `duration = 0.0`
    /// for an instant switch.
    pub fn cross_fade(&mut self, clip: std::sync::Arc<BoundClip>, looping: bool, duration: f32) {
        if std::sync::Arc::ptr_eq(&self.clip, &clip) {
            return;
        }
        if duration <= 0.0 {
            self.clip = clip;
            self.time = 0.0;
            self.looping = looping;
            self.prev = None;
            self.blend = 1.0;
            self.blend_time_remaining = 0.0;
            return;
        }
        // Move current clip to prev.
        let new_prev = std::mem::replace(&mut self.clip, clip);
        let new_prev_time = self.time;
        let new_prev_speed = self.speed;
        self.prev = Some(new_prev);
        self.prev_time = new_prev_time;
        self.prev_speed = new_prev_speed;
        // Reset incoming clip cursor (foot phases will mismatch initially but
        // the blend smooths that out for short fades).
        self.time = 0.0;
        self.looping = looping;
        self.blend = 0.0;
        self.blend_time_remaining = duration;
        self.blend_total = duration;
    }

    /// Backwards-compatible instant switch.
    pub fn switch_to(&mut self, clip: std::sync::Arc<BoundClip>, looping: bool) {
        self.cross_fade(clip, looping, 0.0);
    }
}

/// Sample `clip` at `time` and write per-joint TRS into `t`, `r`, `s`.
/// Joints with no animation channel keep the bind-pose TRS already there.
fn sample_into_trs(
    clip: &BoundClip,
    time: f32,
    t: &mut [Vec3],
    r: &mut [Quat],
    s: &mut [Vec3],
) {
    for ch in &clip.channels {
        let i = ch.joint as usize;
        if i >= t.len() { continue }
        match ch.path {
            Path3::Translation => t[i] = ch.sample_vec3(time),
            Path3::Rotation => r[i] = ch.sample_quat(time),
            Path3::Scale => s[i] = ch.sample_vec3(time),
        }
    }
}

/// Sample an `Animator` (handling optional cross-fade with `prev`) and produce
/// a bone palette for the given skeleton. Joints with no animation channel in
/// either active clip stay in their bind-pose local transform.
pub fn build_bone_palette(
    animator: &Animator,
    joints: &[crate::renderer::mesh::Joint],
    out: &mut Vec<Mat4>,
) {
    let n = joints.len();
    out.clear();
    out.resize(n, Mat4::IDENTITY);

    // Bind-pose TRS as the baseline for the active clip.
    let mut t = vec![Vec3::ZERO; n];
    let mut r = vec![Quat::IDENTITY; n];
    let mut s = vec![Vec3::ONE; n];
    for (i, j) in joints.iter().enumerate() {
        let (scl, rot, tr) = j.local_bind.to_scale_rotation_translation();
        t[i] = tr; r[i] = rot; s[i] = scl;
    }
    sample_into_trs(&animator.clip, animator.time, &mut t, &mut r, &mut s);

    // If a cross-fade is active, sample the previous clip into a parallel
    // TRS array (also seeded with bind pose so joints absent from the prev
    // clip blend with their bind transform) and blend at the TRS level.
    if let Some(prev) = animator.prev.as_ref() {
        if animator.blend < 1.0 {
            let mut tp = vec![Vec3::ZERO; n];
            let mut rp = vec![Quat::IDENTITY; n];
            let mut sp = vec![Vec3::ONE; n];
            for (i, j) in joints.iter().enumerate() {
                let (scl, rot, tr) = j.local_bind.to_scale_rotation_translation();
                tp[i] = tr; rp[i] = rot; sp[i] = scl;
            }
            sample_into_trs(prev, animator.prev_time, &mut tp, &mut rp, &mut sp);

            let b = animator.blend;
            for i in 0..n {
                t[i] = tp[i].lerp(t[i], b);
                s[i] = sp[i].lerp(s[i], b);
                // Slerp picks the shorter great-circle path; if the two
                // quats are on opposite hemispheres, glam handles this.
                r[i] = rp[i].slerp(r[i], b);
            }
        }
    }

    // Compose local matrices, then world matrices via parent chain.
    let mut world = vec![Mat4::IDENTITY; n];
    for i in 0..n {
        let local = Mat4::from_scale_rotation_translation(s[i], r[i], t[i]);
        let parent_world = match joints[i].parent {
            Some(p) => world[p as usize],
            None => Mat4::IDENTITY,
        };
        world[i] = parent_world * local;
    }

    // Skinning matrix: world(joint) * inverse_bind(joint).
    for i in 0..n {
        out[i] = world[i] * joints[i].inverse_bind;
    }
}

/// Sample a base animator and a layered animator together, where the layer
/// only contributes to joints with non-zero `mask` weight. The layer's
/// global `weight` is applied on top of the per-joint mask.
///
/// Per-joint blend factor = `mask[i] * layer_weight`. Layer joints not
/// authored by the layer's clip fall back to their bind pose, which is
/// what we want for clean "upper body cast over walking legs" results.
///
/// `twist_joint` + `twist_yaw` apply an extra rotation around the world
/// Y axis to a single joint's local transform — used to point the torso
/// at the cursor while the legs continue running in the body's facing
/// direction. Pass `None` to skip twisting.
pub fn build_bone_palette_layered(
    base: &Animator,
    layer: Option<&Animator>,
    layer_mask: &[f32],
    layer_weight: f32,
    twist: Option<(usize, f32)>,
    joints: &[crate::renderer::mesh::Joint],
    out: &mut Vec<Mat4>,
) {
    let n = joints.len();
    out.clear();
    out.resize(n, Mat4::IDENTITY);

    // Bind-pose TRS as the baseline.
    let mut t = vec![Vec3::ZERO; n];
    let mut r = vec![Quat::IDENTITY; n];
    let mut s = vec![Vec3::ONE; n];
    for (i, j) in joints.iter().enumerate() {
        let (scl, rot, tr) = j.local_bind.to_scale_rotation_translation();
        t[i] = tr; r[i] = rot; s[i] = scl;
    }

    // Sample base clip (and its cross-fade prev, if any).
    sample_into_trs(&base.clip, base.time, &mut t, &mut r, &mut s);
    if let Some(prev) = base.prev.as_ref() {
        if base.blend < 1.0 {
            let mut tp = vec![Vec3::ZERO; n];
            let mut rp = vec![Quat::IDENTITY; n];
            let mut sp = vec![Vec3::ONE; n];
            for (i, j) in joints.iter().enumerate() {
                let (scl, rot, tr) = j.local_bind.to_scale_rotation_translation();
                tp[i] = tr; rp[i] = rot; sp[i] = scl;
            }
            sample_into_trs(prev, base.prev_time, &mut tp, &mut rp, &mut sp);
            let b = base.blend;
            for i in 0..n {
                t[i] = tp[i].lerp(t[i], b);
                s[i] = sp[i].lerp(s[i], b);
                r[i] = rp[i].slerp(r[i], b);
            }
        }
    }

    // Apply layer if active. Sample its clip into a fresh TRS array (seeded
    // with bind pose) and blend per joint by mask*weight.
    if let (Some(layer), true) = (layer, layer_weight > 0.001) {
        let mut tl = vec![Vec3::ZERO; n];
        let mut rl = vec![Quat::IDENTITY; n];
        let mut sl = vec![Vec3::ONE; n];
        for (i, j) in joints.iter().enumerate() {
            let (scl, rot, tr) = j.local_bind.to_scale_rotation_translation();
            tl[i] = tr; rl[i] = rot; sl[i] = scl;
        }
        sample_into_trs(&layer.clip, layer.time, &mut tl, &mut rl, &mut sl);
        // Cross-fade inside the layer too (e.g. Enter→Shoot transition).
        if let Some(prev) = layer.prev.as_ref() {
            if layer.blend < 1.0 {
                let mut tp = vec![Vec3::ZERO; n];
                let mut rp = vec![Quat::IDENTITY; n];
                let mut sp = vec![Vec3::ONE; n];
                for (i, j) in joints.iter().enumerate() {
                    let (scl, rot, tr) = j.local_bind.to_scale_rotation_translation();
                    tp[i] = tr; rp[i] = rot; sp[i] = scl;
                }
                sample_into_trs(prev, layer.prev_time, &mut tp, &mut rp, &mut sp);
                let b = layer.blend;
                for i in 0..n {
                    tl[i] = tp[i].lerp(tl[i], b);
                    sl[i] = sp[i].lerp(sl[i], b);
                    rl[i] = rp[i].slerp(rl[i], b);
                }
            }
        }
        for i in 0..n {
            let m = layer_mask.get(i).copied().unwrap_or(0.0) * layer_weight;
            if m <= 0.001 { continue }
            t[i] = t[i].lerp(tl[i], m);
            s[i] = s[i].lerp(sl[i], m);
            r[i] = r[i].slerp(rl[i], m);
        }
    }

    // Apply torso twist.  We want the hips to *visually* rotate
    // slightly with the upper body, but in a standard humanoid rig
    // the pelvis is the root joint — rotating it would also rotate
    // the legs, which makes the entire character pivot in place.
    // To avoid that we:
    //   1. apply 25% of the yaw to the pelvis (spine's parent),
    //   2. apply the remaining 75% to the spine itself,
    //   3. counter-rotate the *upper-leg* joints by the same 25% so
    //      the legs cancel out the pelvis rotation and stay planted.
    // The visual result: chest tracks the cursor exactly (full yaw),
    // pelvis follows by ¼ of the yaw, feet/legs hold their ground.
    if let Some((spine_idx, yaw)) = twist {
        if spine_idx < n && yaw.abs() > 1e-4 {
            const HIP_FRACTION: f32 = 0.25;
            let parent = joints[spine_idx].parent.map(|p| p as usize);
            if let Some(hip_idx) = parent.filter(|&p| p < n) {
                let hip_yaw = yaw * HIP_FRACTION;
                r[hip_idx] = r[hip_idx] * Quat::from_rotation_y(hip_yaw);
                r[spine_idx] = r[spine_idx]
                    * Quat::from_rotation_y(yaw * (1.0 - HIP_FRACTION));
                // Counter-rotate the legs so the hip rotation doesn't
                // drag them along.  Any joint whose parent is the
                // pelvis and whose name looks like a leg gets the
                // inverse hip yaw applied to its local rotation.
                let counter = Quat::from_rotation_y(-hip_yaw);
                for i in 0..n {
                    if joints[i].parent.map(|p| p as usize) != Some(hip_idx) {
                        continue;
                    }
                    let lname = joints[i].name.to_ascii_lowercase();
                    let is_leg = lname.contains("leg")
                        || lname.contains("thigh")
                        || lname.contains("upleg")
                        || lname.contains("hip_l")
                        || lname.contains("hip_r");
                    // Skip the spine itself (also a child of the pelvis).
                    if i == spine_idx || lname.contains("spine") { continue; }
                    if is_leg {
                        r[i] = counter * r[i];
                    }
                }
            } else {
                r[spine_idx] = r[spine_idx] * Quat::from_rotation_y(yaw);
            }
        }
    }

    // Compose locals → worlds.
    let mut world = vec![Mat4::IDENTITY; n];
    for i in 0..n {
        let local = Mat4::from_scale_rotation_translation(s[i], r[i], t[i]);
        let parent_world = match joints[i].parent {
            Some(p) => world[p as usize],
            None => Mat4::IDENTITY,
        };
        world[i] = parent_world * local;
    }
    for i in 0..n {
        out[i] = world[i] * joints[i].inverse_bind;
    }
}
