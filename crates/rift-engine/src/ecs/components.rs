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
        Self { position, ..Default::default() }
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

impl AnimationSet {
    pub fn get(&self, name: &str) -> Option<std::sync::Arc<crate::animation::BoundClip>> {
        // Avoid allocating a lowercased key string every frame by
        // doing a case-insensitive linear scan. Clip counts per set
        // are small (typically < 30) so this is faster in practice
        // than the lowercased-key hash lookup it replaces.
        for (k, v) in &self.clips {
            if k.eq_ignore_ascii_case(name) {
                return Some(v.clone());
            }
        }
        None
    }
    /// Look up the first clip whose name matches any of `candidates` (case-insensitive).
    pub fn find_any(&self, candidates: &[&str]) -> Option<std::sync::Arc<crate::animation::BoundClip>> {
        for c in candidates {
            if let Some(clip) = self.get(c) { return Some(clip); }
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
}

impl SpellCast {
    pub fn new(mask: Vec<f32>) -> Self {
        Self {
            phase: SpellPhase::Idle,
            layer_animator: None,
            mask,
            weight: 0.0,
            pending_ability: None,
            pending_aim_dir: glam::Vec3::Z,
            pending_damage: 0.0,
            fired: false,
            pending_oneshot: None,
            hit_cooldown: 0.0,
            oneshot_is_hit: false,
            channeling: false,
        }
    }
    pub fn is_active(&self) -> bool { self.phase != SpellPhase::Idle }

    /// Begin a new cast. Captures the ability + aim + damage so the
    /// projectile can be spawned later when we reach the Shoot phase.
    ///
    /// Skips the `Entering` wind-up phase and jumps straight to
    /// `Shooting`. The local visual fire is suppressed (`fired = true`)
    /// because the server owns projectile spawn under multiplayer
    /// authority — the client just needs the Shoot clip to play for
    /// upper-body feedback. This makes LMB / Multishot feel snappy:
    /// the cast pose appears the same frame the click lands instead
    /// of after the wind-up clip's full duration.
    pub fn begin(&mut self, ability: rift_game::abilities::Ability, aim_dir: glam::Vec3, damage: f32) {
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
        if self.phase != SpellPhase::Idle { return; }
        self.phase = SpellPhase::OneShot;
        self.pending_oneshot = Some(clip);
        self.fired = false;
        self.oneshot_is_hit = false;
    }

    /// Trigger a hit-reaction OneShot. Unlike [`Self::play_oneshot`],
    /// this preempts any currently-active cast or one-shot so the
    /// flinch always reads. A short [`Self::hit_cooldown`] gate
    /// prevents the animation from re-triggering every frame while
    /// the player is in melee contact.
    pub fn play_hit(&mut self, clip: std::sync::Arc<crate::animation::BoundClip>) {
        if self.hit_cooldown > 0.0 { return; }
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
        if self.phase == SpellPhase::Idle { return; }
        self.phase = SpellPhase::Exiting;
        self.fired = true;
        self.pending_ability = None;
        self.channeling = false;
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
}

// `PlayerAction` lives in `rift_game::components`; re-exported here
// so existing `crate::ecs::components::PlayerAction` paths in engine
// systems still resolve.
pub use rift_game::components::PlayerAction;

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

/// Active-debuff bitmask on an entity. One bit per
/// `rift_game::debuffs::id::*`. The HUD reads this to paint
/// indicator pips above the entity. Owned by network sync on the
/// client, by the simulation on the server.
#[derive(Clone, Copy, Debug, Default)]
pub struct Debuffs {
    pub mask: u32,
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
        Self { damage, range, cooldown, timer: 0.0 }
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
