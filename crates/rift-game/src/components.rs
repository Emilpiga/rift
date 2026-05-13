//! ECS components owned by the gameplay layer.
//!
//! These are pure data — gameplay declares what they mean, the engine
//! consumes them. Currently this is just `PlayerAction`, the
//! state-machine variant attached to a player entity.

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
    /// Melee swing in flight; the `Sword_Attack` clip from
    /// `rift_game::kinematic::MELEE_ATTACK` is playing. The
    /// kinematic owns forward motion (`Kinematic::apply_input`
    /// reads `action::ATTACK` and applies the profile's
    /// `forward_step` along the locked `attack_dir`); the
    /// action gate exists so `locomotion_anim_system` doesn't
    /// override the swing pose with Walk/Idle.
    Attack,
}
