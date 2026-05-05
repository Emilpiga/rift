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
        self.clips.get(&name.to_ascii_lowercase()).cloned()
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
    pub pending_ability: Option<crate::combat::Ability>,
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
        }
    }
    pub fn is_active(&self) -> bool { self.phase != SpellPhase::Idle }

    /// Begin a new cast. Captures the ability + aim + damage so the
    /// projectile can be spawned later when we reach the Shoot phase.
    pub fn begin(&mut self, ability: crate::combat::Ability, aim_dir: glam::Vec3, damage: f32) {
        self.phase = SpellPhase::Entering;
        self.pending_ability = Some(ability);
        self.pending_aim_dir = aim_dir;
        self.pending_damage = damage;
        self.fired = false;
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

/// Current full-body action driving the player's locomotion / animation
/// override. `None` means normal locomotion (Idle/Walk/Jog/Sprint).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlayerAction {
    #[default]
    None,
    /// Liftoff windup; `Jump_Start` clip is playing.
    JumpStart,
    /// Airborne body loop; `Jump` clip is playing on a loop.
    JumpAir,
    /// Recovery on landing; `Jump_Land` clip is playing.
    JumpLand,
    /// Evasive dodge roll; `Roll` clip is playing and game-side code
    /// is driving forward velocity for the duration.
    Roll,
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
