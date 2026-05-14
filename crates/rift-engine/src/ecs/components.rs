use glam::{Mat4, Quat, Vec3};

/// 3D transform component.
#[derive(Clone, Copy, Debug)]
pub struct Transform {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }
}

impl Transform {
    pub fn from_position(position: Vec3) -> Self {
        Self {
            position,
            ..Default::default()
        }
    }

    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.position)
    }
}

/// Marks an entity as renderable with a specific mesh index in the renderer's object list.
#[derive(Clone, Copy, Debug)]
pub struct Renderable {
    pub object_index: usize,
}

/// Attached to entities whose render mesh is driven by a skinned glTF rig.
/// The entity's `Renderable.object_index` points at the renderer slot whose
/// vertices we re-skin every frame.
///
/// Phase 2b uses this purely to keep the source SkinnedMesh alive next to
/// the entity (and to identify which renderer objects need re-skinning).
/// Phase 3 will add an `Animator` next to it driving the bone palette.
#[derive(Clone)]
pub struct Skinned {
    pub mesh: std::sync::Arc<crate::renderer::mesh::SkinnedMesh>,
    /// Reusable scratch buffer for skinned vertices, sized to mesh.bind_vertices.len().
    /// Stored here to avoid per-frame allocation. (Empty until first skin pass.)
    pub scratch: Vec<crate::renderer::mesh::Vertex>,
    /// Per-joint world transforms in **mesh-local** space, refreshed
    /// every frame the entity is skinned. Other systems (e.g. beam
    /// VFX, weapon attachments) read this to anchor world-space
    /// effects to bones. Multiply by the entity's `Transform.matrix()`
    /// to get a true world-space pose.
    pub joint_worlds: Vec<glam::Mat4>,
}

/// One outfit piece riding on top of a `Skinned` entity. The piece's
/// `vertex_skin.joints` have already been remapped to refer into the
/// host skeleton's palette (see `SkinnedMesh::remap_joint_indices_to`),
/// so the skinning system can re-skin it with the host's bone palette.
pub struct AttachmentPiece {
    pub mesh: std::sync::Arc<crate::renderer::mesh::SkinnedMesh>,
    /// Renderer dynamic-mesh slot for this piece.
    pub object_index: usize,
    /// Reusable per-frame skinned-vertex buffer.
    pub scratch: Vec<crate::renderer::mesh::Vertex>,
    /// Whether this piece should be rendered this frame. Toggled by
    /// gameplay code (e.g. equipment changes); when false, the skinning
    /// system collapses the renderer object so it disappears.
    pub visible: bool,
    /// Opaque, gameplay-defined identifier for this piece. The engine
    /// never inspects it; client code uses it to find and replace the
    /// piece occupying a logical "slot" (e.g. equipment slot byte) so
    /// re-equipping doesn't accumulate orphan attachments. Default 0.
    pub tag: u32,
    /// When true, the skinning system inflates this piece's vertices
    /// outward along their normals to keep clothing from z-fighting
    /// the base body. Set to false for cosmetic geometry whose shape
    /// matters (eyeballs, hair-card strands) — those would otherwise
    /// puff outward / split along card normals. Defaults to true so
    /// existing equipment call sites keep their old behavior.
    pub inflate: bool,
}

/// Set of outfit / armor pieces attached to a skinned character. All
/// pieces share the host entity's bone palette.
#[derive(Default)]
pub struct SkinnedAttachments {
    pub pieces: Vec<AttachmentPiece>,
    /// When true, the host's base skinned mesh is collapsed to a zero
    /// matrix so it doesn't render through the outfit. Set by gameplay
    /// code whenever a body-covering attachment is visible.
    pub hide_base: bool,
}

/// Library of pre-bound animation clips keyed by lowercase name (e.g.
/// "idle_loop", "walk_loop"). Stored alongside an entity so gameplay
/// systems can swap the active clip on its `Animator` without reloading.
#[derive(Clone, Default)]
pub struct AnimationSet {
    pub clips: std::collections::HashMap<String, std::sync::Arc<crate::animation::BoundClip>>,
}

#[derive(Clone, Default)]
pub struct LocomotionClips {
    pub sprint: Option<std::sync::Arc<crate::animation::BoundClip>>,
    pub jog: Option<std::sync::Arc<crate::animation::BoundClip>>,
    pub walk: Option<std::sync::Arc<crate::animation::BoundClip>>,
    pub idle: Option<std::sync::Arc<crate::animation::BoundClip>>,
}

impl AnimationSet {
    pub fn get(&self, name: &str) -> Option<std::sync::Arc<crate::animation::BoundClip>> {
        if let Some(clip) = self.clips.get(name) {
            return Some(clip.clone());
        }
        self.get_case_insensitive(name)
    }

    fn get_case_insensitive(
        &self,
        name: &str,
    ) -> Option<std::sync::Arc<crate::animation::BoundClip>> {
        for (k, v) in &self.clips {
            if k.eq_ignore_ascii_case(name) {
                return Some(v.clone());
            }
        }
        None
    }
    /// Look up the first clip whose name matches any of `candidates` (case-insensitive).
    pub fn find_any(
        &self,
        candidates: &[&str],
    ) -> Option<std::sync::Arc<crate::animation::BoundClip>> {
        for c in candidates {
            if let Some(clip) = self.clips.get(*c) {
                return Some(clip.clone());
            }
        }
        for c in candidates {
            if let Some(clip) = self.get_case_insensitive(c) {
                return Some(clip);
            }
        }
        None
    }
}

/// Phase of a layered spell cast. Drives clip selection on the cast layer
/// and the moment at which the projectile is spawned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpellPhase {
    /// No cast active; layer weight should be 0.
    Idle,
    /// Wind-up. Plays Spell_Simple_Enter once.
    Entering,
    /// Release. Plays Spell_Simple_Shoot once. Projectile is spawned on
    /// entry to this phase.
    Shooting,
    /// Recovery. Plays Spell_Simple_Exit once, then returns to Idle.
    Exiting,
    /// One-shot upper-body action (e.g. PickUp). Plays `pending_oneshot`
    /// once on the cast layer, ramps weight in/out, no projectile fire.
    OneShot,
}

/// Layered spell-cast state attached to a player entity.
///
/// While `phase != Idle`, the skinning system blends `layer_animator` over
/// the base locomotion animator using `mask` (1.0 on upper-body bones,
/// 0.0 on legs/pelvis). `weight` ramps in/out for clean fades.
///
/// `pending_aim_dir` and `pending_damage` capture the original click so the
/// projectile can be deferred to the start of `Shooting` while still using
/// the aim and stats from when the player pressed the button.
pub struct SpellCast {
    pub phase: SpellPhase,
    pub layer_animator: Option<crate::animation::Animator>,
    pub mask: Vec<f32>,
    /// Per-joint flag (`1.0` = yes, `0.0` = no) marking joints
    /// whose layered rotation should be **yaw-projected** before
    /// being mixed with the base pose. Set for spine / chest so
    /// a forward-pitched cast pose (Punch) doesn't tip the
    /// running torso into the ground; only the lateral twist
    /// transfers onto the locomotion clip.
    pub yaw_only_mask: Vec<f32>,
    pub weight: f32,
    /// Ability captured at trigger time. Cloned so cooldown state on the
    /// `Abilities` resource has already been advanced and we don't need to
    /// re-acquire it when the projectile actually spawns.
    pub pending_ability: Option<rift_game::abilities::Ability>,
    /// Aim direction captured at trigger time (xz horizontal).
    pub pending_aim_dir: glam::Vec3,
    /// Damage captured at trigger time. Spent at the moment of fire.
    pub pending_damage: f32,
    /// Set true once the projectile has been spawned for the current cast,
    /// so Shooting doesn't fire repeatedly per frame.
    pub fired: bool,
    /// Clip to play in `OneShot` phase. Cleared when the action ends.
    pub pending_oneshot: Option<std::sync::Arc<crate::animation::BoundClip>>,
    /// Cooldown (s) before another hit-reaction can play, so being in
    /// melee contact doesn't replay the flinch every frame.
    pub hit_cooldown: f32,
    /// `true` when the current `OneShot` is a hit reaction (so it's
    /// allowed to interrupt itself / resists being interrupted by
    /// pickups but yields to spell casts).
    pub oneshot_is_hit: bool,
    /// `true` while the player is channeling. Tells the cast
    /// system to loop the Enter clip and not auto-advance to
    /// `Shooting` / `Exiting` when it ends. Cleared by
    /// [`SpellCast::cancel`] and on `Idle`.
    pub channeling: bool,
    /// Playback speed multiplier applied to the next one-shot
    /// when the system spins up a fresh `layer_animator`. Reset
    /// to `1.0` whenever the one-shot ends so subsequent
    /// vanilla one-shots play at natural speed. Used by rapid-
    /// fire melee (Punch) to keep a longer wind-up / impact /
    /// recovery clip near the cooldown window while preserving
    /// enough anticipation for the swing to read.
    pub pending_oneshot_speed: f32,
    /// Real-time seconds until a scheduled cast-layer freeze
    /// (hit-stop) engages. While `> 0` it counts down each
    /// tick and the layer animator advances normally. Once it
    /// hits `0`, [`Self::oneshot_freeze_for`] takes over and
    /// the layer animator's `dt` is gated to zero for the
    /// freeze window. Used by Punch to inject a ~50 ms hold
    /// at the contact frame so each swing reads as a hit
    /// rather than a continuous wind-mill.
    pub oneshot_freeze_in: f32,
    /// Remaining real-time seconds of the active hit-stop. Once
    /// [`Self::oneshot_freeze_in`] reaches `0`, this counter
    /// ticks down while the layer animator is held in place.
    /// Cleared on cast end so a fresh one-shot doesn't inherit
    /// the previous swing's stop window.
    pub oneshot_freeze_for: f32,
}

impl SpellCast {
    pub fn new(mask: Vec<f32>) -> Self {
        let n = mask.len();
        Self::new_with_axis(mask, vec![0.0; n])
    }

    /// Construct with explicit yaw-only flags per joint.
    /// Caller obtains both vectors from
    /// `MeshData::upper_body_mask_with_axis`.
    pub fn new_with_axis(mask: Vec<f32>, yaw_only_mask: Vec<f32>) -> Self {
        Self {
            phase: SpellPhase::Idle,
            layer_animator: None,
            mask,
            yaw_only_mask,
            weight: 0.0,
            pending_ability: None,
            pending_aim_dir: glam::Vec3::Z,
            pending_damage: 0.0,
            fired: false,
            pending_oneshot: None,
            hit_cooldown: 0.0,
            oneshot_is_hit: false,
            channeling: false,
            pending_oneshot_speed: 1.0,
            oneshot_freeze_in: 0.0,
            oneshot_freeze_for: 0.0,
        }
    }
    pub fn is_active(&self) -> bool {
        self.phase != SpellPhase::Idle
    }

    /// Begin a new cast. Captures the ability + aim + damage so the
    /// projectile can be spawned later when we reach the Shoot phase.
    ///
    /// Skips the `Entering` wind-up phase and jumps straight to
    /// `Shooting`. The local visual fire is suppressed (`fired = true`)
    /// because the server owns projectile spawn under multiplayer
    /// authority — the client just needs the Shoot clip to play for
    /// upper-body feedback. This makes LMB / Fireball Volley feel snappy:
    /// the cast pose appears the same frame the click lands instead
    /// of after the wind-up clip's full duration.
    pub fn begin(
        &mut self,
        ability: rift_game::abilities::Ability,
        aim_dir: glam::Vec3,
        damage: f32,
    ) {
        self.phase = SpellPhase::Shooting;
        self.pending_ability = Some(ability);
        self.pending_aim_dir = aim_dir;
        self.pending_damage = damage;
        self.fired = true;
        self.channeling = false;
    }

    /// Begin a *channeled* cast. Like [`Self::begin`] but instructs
    /// the cast system to loop the Enter clip and stay in the
    /// channeling pose until [`Self::cancel`] is called. Used for
    /// abilities like Frost Ray / Whirlwind whose duration is
    /// driven by the player holding the action button rather than
    /// a fixed clip length.
    pub fn begin_channel(&mut self, ability: rift_game::abilities::Ability, aim_dir: glam::Vec3) {
        self.phase = SpellPhase::Entering;
        self.pending_ability = Some(ability);
        self.pending_aim_dir = aim_dir;
        self.pending_damage = 0.0;
        self.fired = true; // channels never fire a single projectile
        self.channeling = true;
    }

    /// Begin a one-shot upper-body action (e.g. picking loot off the
    /// ground). Plays `clip` once on the cast layer with the upper-body
    /// mask, no projectile fire. If the upper body is already busy with
    /// another cast/action, the request is ignored so we don't interrupt
    /// it mid-swing.
    pub fn play_oneshot(&mut self, clip: std::sync::Arc<crate::animation::BoundClip>) {
        if self.phase != SpellPhase::Idle {
            return;
        }
        self.phase = SpellPhase::OneShot;
        self.pending_oneshot = Some(clip);
        self.fired = false;
        self.oneshot_is_hit = false;
    }

    /// Begin a one-shot upper-body action, **preempting** any
    /// currently-playing one-shot. Unlike [`Self::play_oneshot`]
    /// this restarts the cast layer even if a previous one-shot
    /// is still in flight — used by rapid-fire melee (Punch)
    /// whose cooldown is shorter than the swing clip, so a fresh
    /// click should snap into a fresh swing rather than be
    /// silently dropped while the previous Jab plays out.
    ///
    /// We refuse to interrupt a *projectile / channel* cast
    /// (Entering / Shooting / Exiting) so a Punch tap can't
    /// cancel a Fireball wind-up. Hit-reactions take priority
    /// over us in the other direction via [`Self::play_hit`].
    pub fn play_oneshot_preempt(&mut self, clip: std::sync::Arc<crate::animation::BoundClip>) {
        self.play_oneshot_preempt_scaled(clip, 1.0);
    }

    /// Same as [`Self::play_oneshot_preempt`] but with an explicit
    /// playback `speed` (1.0 = natural). Used by rapid-fire melee
    /// to keep a longer swing clip responsive without erasing its
    /// wind-up.
    pub fn play_oneshot_preempt_scaled(
        &mut self,
        clip: std::sync::Arc<crate::animation::BoundClip>,
        speed: f32,
    ) {
        match self.phase {
            SpellPhase::Idle | SpellPhase::OneShot => {}
            _ => return,
        }
        self.phase = SpellPhase::OneShot;
        // Drop the animator so the next tick spins up a fresh
        // one starting at t=0 instead of cross-fading from the
        // previous swing's tail pose.
        self.layer_animator = None;
        self.pending_oneshot = Some(clip);
        self.pending_oneshot_speed = speed.max(0.01);
        self.fired = false;
        self.oneshot_is_hit = false;
    }

    /// Trigger a hit-reaction OneShot. Unlike [`Self::play_oneshot`],
    /// this preempts any currently-active cast or one-shot so the
    /// flinch always reads. A short [`Self::hit_cooldown`] gate
    /// prevents the animation from re-triggering every frame while
    /// the player is in melee contact.
    pub fn play_hit(&mut self, clip: std::sync::Arc<crate::animation::BoundClip>) {
        if self.hit_cooldown > 0.0 {
            return;
        }
        self.phase = SpellPhase::OneShot;
        // Force a fresh animator so cross_fade doesn't blend back from
        // the spell-cast pose if we're interrupting one mid-swing.
        self.layer_animator = None;
        self.pending_oneshot = Some(clip);
        self.fired = false;
        self.oneshot_is_hit = true;
        self.hit_cooldown = 0.55;
    }

    /// Force the cast layer back to `Exiting` so the player drops
    /// out of the channeled pose immediately. Used when a channel
    /// is cancelled (button release / movement / server end).
    pub fn cancel(&mut self) {
        if self.phase == SpellPhase::Idle {
            return;
        }
        self.phase = SpellPhase::Exiting;
        self.fired = true;
        self.pending_ability = None;
        self.channeling = false;
        self.oneshot_freeze_in = 0.0;
        self.oneshot_freeze_for = 0.0;
    }
}

/// Player marker component.
#[derive(Clone, Copy, Debug, Default)]
pub struct Player {
    pub speed: f32,
    /// Direction the player is currently aiming (cursor direction in
    /// world space, projected on the XZ plane). The body itself still
    /// turns to face *movement* — this only drives the upper-body twist
    /// and the locomotion direction sign so that running while aiming
    /// elsewhere doesn't suddenly snap the character around.
    pub aim_dir: Vec3,
    /// Index of the spine-root joint to apply the torso twist to, or
    /// `usize::MAX` if unset / not a skinned player.
    pub spine_joint: u32,
    /// Index of the right-hand joint, used as the spawn anchor for
    /// hand-held VFX (Frost Ray beam, etc.). `u32::MAX` if the
    /// rig has no detectable hand joint.
    pub hand_joint: u32,
    /// Indices of the left and right foot joints for terrain-aware
    /// foot IK. Set at character spawn by name-matching against
    /// the rig (`*foot_l*` / `*foot_r*`). The skinning system
    /// uses these to pin each foot to the dungeon floor's per-tile
    /// elevation when standing on stair / dais / pit tiles, so
    /// running up a ramp doesn't leave feet floating mid-air or
    /// punching through the geometry. `u32::MAX` if the rig has
    /// no detectable foot joint.
    pub foot_l_joint: u32,
    pub foot_r_joint: u32,
    /// Active full-body action (jump / roll). Drives whether `player_input_system`
    /// and `locomotion_anim_system` should leave the entity alone, and lets
    /// game-side code know which clip to play.
    pub action: PlayerAction,
    /// Time remaining in the current action's phase (seconds). Game code
    /// owns the state machine and decrements / transitions this.
    pub action_timer: f32,
    /// Vertical velocity in m/s. Used by `movement_system` to integrate
    /// jump arcs against gravity for the player.
    pub vy: f32,
    /// True while the player is off the ground (above y=0). Set by
    /// `movement_system` after integrating `vy`.
    pub airborne: bool,
    /// Leftover frame time carried into the next vertical-integration
    /// step. The jump arc is integrated at a fixed 120 Hz substep
    /// inside `movement_system` so a wobbly frame dt (vsync hiccups,
    /// streaming spikes) doesn't translate into visible stutter
    /// during the ~0.4 s airborne window. Anything below the
    /// fixed step gets banked here and consumed next frame.
    pub vy_accum: f32,
    /// Authoritative ground-tile Y under the player (post step-up
    /// resolution). Used as the foot-IK reference plane so it
    /// keeps planting feet against the *real* ground even while
    /// `transform.position.y` is mid-lerp catching up after a
    /// sudden lift onto a raised tile. Set every frame by
    /// `movement_system`.
    pub grounded_y: f32,
}

// `PlayerAction` lives in `rift_game::components`; re-exported here
// so existing `crate::ecs::components::PlayerAction` paths in engine
// systems still resolve.
pub use rift_game::components::PlayerAction;

/// Persistent per-foot smoothing state for terrain-aware foot IK.
/// Attach to any skinned entity whose feet should plant on the
/// dungeon's per-tile elevation (raised daises, sunken pits,
/// stair-tile slopes). Without persistent state the IK pass would
/// snap the correction every frame, producing the classic
/// "vibrating feet" / stair-popping artefact.
///
/// Stored in skel-local space because the IK solver runs there.
/// `correction_y` eases toward the per-frame raycast result;
/// `surface_normal` smooths the foot orientation across tile
/// boundaries so a foot crossing from flat onto a ramp doesn't
/// pop instantly upright.
#[derive(Clone, Copy, Debug)]
pub struct FootIkState {
    pub left_correction_y: f32,
    pub right_correction_y: f32,
    pub left_normal: glam::Vec3,
    pub right_normal: glam::Vec3,
    /// Smoothed pelvis lowering (skel-local Y, always ≤ 0). Lets
    /// the hips drop when a foot is stepping into a pit so the
    /// upper leg doesn't have to over-stretch.
    pub pelvis_offset_y: f32,
    /// True while the corresponding foot is in contact with the
    /// ground (animated foot Y is below the plant threshold).
    /// Hysteresis is applied in [`crate::foot_ik::apply_foot_ik`]
    /// so a foot mid-swing doesn't chatter between planted/lifted.
    pub left_planted: bool,
    pub right_planted: bool,
    /// Monotonically increasing counter that bumps every time
    /// the corresponding foot transitions from airborne to
    /// planted. Consumers (footstep audio, dust VFX) cache the
    /// last-seen value and fire one event per delta. Survives
    /// `u32` wraparound by virtue of equality comparison.
    pub left_plant_seq: u32,
    pub right_plant_seq: u32,
    /// World-space position the foot was at the moment of its
    /// most recent plant. Sampled once on the airborne→planted
    /// transition and held until the next plant, so consumers
    /// can spatialise the audio at the foot rather than the
    /// body origin.
    pub left_plant_pos: glam::Vec3,
    pub right_plant_pos: glam::Vec3,
}

impl Default for FootIkState {
    fn default() -> Self {
        Self {
            left_correction_y: 0.0,
            right_correction_y: 0.0,
            left_normal: glam::Vec3::Y,
            right_normal: glam::Vec3::Y,
            pelvis_offset_y: 0.0,
            left_planted: false,
            right_planted: false,
            left_plant_seq: 0,
            right_plant_seq: 0,
            left_plant_pos: glam::Vec3::ZERO,
            right_plant_pos: glam::Vec3::ZERO,
        }
    }
}

/// Marker indicating this entity's kinematics are owned by an
/// external authority (e.g. the multiplayer client's prediction
/// state, which is reconciled against the server). SP systems
/// like `player_input_system`, `movement_system`, and
/// `collision_system` skip entities carrying this marker so they
/// don't fight whoever is driving the Transform.
#[derive(Clone, Copy, Debug, Default)]
pub struct NetControlled;

/// Marker for the player entity owned by THIS client. SP-only
/// concerns (camera follow, HUD readouts, input dispatch, ability
/// casting, pickup) filter on this component so that remote
/// player entities (which carry `Player + NetControlled` but NOT
/// `LocalPlayer`) don't accidentally drive the local UI/camera.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalPlayer;

/// Marker for player entities owned by another client. Drives
/// "remote-player" specific behavior (snapshot-driven kinematics,
/// no input dispatch) and lets SP systems skip them where needed.
#[derive(Clone, Copy, Debug, Default)]
pub struct RemotePlayer {
    /// The remote's stable network id, for snapshot lookup.
    pub net_id: u32,
}

/// Marker tag set on a player entity that's currently in
/// risen-but-dead "ghost" spectator mode (HP still 0, but the
/// server permits movement and the avatar is renderered with
/// the translucent ghost tint). Engine systems that gate on
/// `Health::is_dead()` (locomotion, etc.) treat Ghost as alive
/// so the spectator can walk around. Removed on respawn.
#[derive(Clone, Copy, Debug, Default)]
pub struct Ghost;

/// Transient marker placed on the local player while the
/// `LayToIdle` (get-up-from-corpse) animation is still
/// playing. Engine input / locomotion systems do **not**
/// treat `GhostRising` as alive — the avatar stays frozen on
/// the floor pose until the rise anim finishes, at which
/// point the client swaps `GhostRising` for [`Ghost`] and
/// movement unlocks. `remaining` is seeded from the rise
/// clip's duration and counted down by the client's
/// gameplay tick.
#[derive(Clone, Copy, Debug)]
pub struct GhostRising {
    pub remaining: f32,
}

/// Velocity component for movement.
#[derive(Clone, Copy, Debug, Default)]
pub struct Velocity {
    pub linear: Vec3,
}

/// What archetype an enemy is — drives behavior and visuals.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EnemyKind {
    /// Heavy melee bruiser: slow, high HP, charges and slams.
    #[default]
    Brute,
    /// Fast flanker: circle-strafes then lunges.
    Stalker,
    /// Ranged caster: kites at distance, throws bolts.
    Caster,
}

/// Marks an entity as an enemy.
#[derive(Clone, Copy, Debug)]
pub struct Enemy {
    pub speed: f32,
    /// How much rift progress this enemy gives when killed.
    pub progress_value: f32,
    pub kind: EnemyKind,
}

/// Marker for enemy entities driven by server snapshots. Stores the
/// stable network id so clients can detect stale `hecs::Entity` handles
/// after `World::new()` reuses ids across hub/rift transitions.
#[derive(Clone, Copy, Debug, Default)]
pub struct RemoteEnemy {
    pub net_id: u32,
}

/// Marker for player-owned minions driven by server snapshots.
/// Kept distinct from [`RemoteEnemy`] so HUD, targeting, and
/// client-only combat helpers do not mistake friendly summons for
/// hostile monsters.
#[derive(Clone, Copy, Debug, Default)]
pub struct RemoteMinion {
    pub net_id: u32,
    pub owner_net_id: u32,
}

/// Marker for entities whose visual transform should float above
/// the navigation/collision plane. Grounded summons should not
/// carry this component, so they continue through normal movement
/// ground-follow logic.
#[derive(Clone, Copy, Debug)]
pub struct FloatingVisual {
    pub hover_height: f32,
}

/// Engine-side mirror of `rift_net::messages::ActiveEffect`.
/// Duplicated here so this crate doesn't need to depend on
/// `rift-net`; the client converts at sync time.
#[derive(Clone, Copy, Debug)]
pub struct ActiveEffect {
    pub id: u8,
    pub remaining: f32,
    pub duration: f32,
}

/// Active buffs / debuffs replicated onto an entity. One entry
/// per running effect, in no particular order. Replaces the
/// older bit-mask representation so the HUD can render duration
/// rings and individual icons. Owned by network sync on the
/// client, by the simulation on the server.
#[derive(Clone, Debug, Default)]
pub struct Effects {
    pub effects: Vec<ActiveEffect>,
}

/// Marks an entity as a rift boss.
#[derive(Clone, Copy, Debug, Default)]
pub struct Boss;

/// Marks an enemy as an elite pack leader (larger, tougher, more loot).
#[derive(Clone, Copy, Debug, Default)]
pub struct Elite;

/// Health component.
#[derive(Clone, Copy, Debug)]
pub struct Health {
    pub current: f32,
    pub max: f32,
}

impl Health {
    pub fn new(max: f32) -> Self {
        Self { current: max, max }
    }

    pub fn is_dead(&self) -> bool {
        self.current <= 0.0
    }
}

/// Essence / mana component — the universal ability resource.
///
/// Shaped like [`Health`] so HUD widgets can read `current /
/// max` the same way for both. The local player's authoritative
/// pool is the server-side `PlayerState.resource`; client code
/// receives a fraction in `[0, 1]` per snapshot. For remote
/// avatars we mirror that fraction by setting `current = max *
/// resource_pct`, with `max` kept at a placeholder (1.0 by
/// default) — the bar code only ever looks at the ratio.
#[derive(Clone, Copy, Debug)]
pub struct Resource {
    pub current: f32,
    pub max: f32,
}

impl Resource {
    pub fn new(max: f32) -> Self {
        Self { current: max, max }
    }
}

/// Axis-aligned bounding box collider (world-space).
#[derive(Clone, Copy, Debug)]
pub struct Collider {
    pub half_extents: Vec3,
}

impl Collider {
    pub fn new(half_x: f32, half_y: f32, half_z: f32) -> Self {
        Self {
            half_extents: Vec3::new(half_x, half_y, half_z),
        }
    }

    /// Get world-space min/max given a position.
    pub fn bounds(&self, position: Vec3) -> (Vec3, Vec3) {
        (position - self.half_extents, position + self.half_extents)
    }
}

/// Marker for static geometry (walls, floors) — won't be moved by collision.
#[derive(Clone, Copy, Debug, Default)]
pub struct Static;

/// Attack state for the player.
#[derive(Clone, Copy, Debug)]
pub struct Attack {
    pub damage: f32,
    pub range: f32,
    pub cooldown: f32,
    pub timer: f32,
}

impl Attack {
    pub fn new(damage: f32, range: f32, cooldown: f32) -> Self {
        Self {
            damage,
            range,
            cooldown,
            timer: 0.0,
        }
    }

    pub fn ready(&self) -> bool {
        self.timer <= 0.0
    }
}

/// Marks an entity as dead (to be despawned).
#[derive(Clone, Copy, Debug, Default)]
pub struct Dead;

/// Enemy death animation state — shrinks and collapses over time.
pub struct Dying {
    pub timer: f32,
    pub duration: f32,
    pub original_scale: f32,
}

/// Per-enemy animation state for triggering one-shot reactions
/// (HitRecieve, Bite_Front, Death) on top of the locomotion clip.
/// Attached alongside `AnimationSet` + `Animator` for skinned monsters.
#[derive(Clone, Copy, Debug, Default)]
pub struct EnemyAnim {
    /// Health value seen on the previous frame; if the current value
    /// drops below this we trigger `HitRecieve`.
    pub last_hp: f32,
    /// Set to `true` by gameplay systems when this enemy is currently
    /// in melee contact with the player (used to play `Bite_Front`).
    /// The animation system clears this flag every frame.
    pub attacking: bool,
    /// Seconds remaining in a one-shot animation lock. While > 0,
    /// `locomotion_anim_system` should not change the clip and
    /// `enemy_anim_system` will not start a new one-shot. The `Death`
    /// animation sets this to a large value.
    pub lock_remaining: f32,
}
