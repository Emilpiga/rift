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
