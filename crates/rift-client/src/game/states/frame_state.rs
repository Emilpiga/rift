//! Per-frame transient state. Every field is recomputed each
//! tick or driven by an edge event; nothing here survives a
//! floor regen. See [`FrameState::reset`] for the contract.
//!
//! Owned by [`super::state::GameState::frame`]; phase modules
//! mutate these fields directly through the struct.

use crate::game::combat_system::PlacedTargeting;

#[derive(Default)]
pub struct FrameState {
    /// Active placed-ability targeting (if any). Pure visual /
    /// input state — the actual cast is sent to the server.
    pub targeting: Option<PlacedTargeting>,
    /// Eases from 1 -> 0 over ~0.5 s after the player takes damage.
    pub damage_flash: f32,
    /// Eases from 1 -> 0 over ~2.5 s after a level-up. Drives a
    /// HUD banner overlay.
    pub level_up_flash: f32,
    /// Black-screen alpha used for hub ↔ rift transitions and
    /// for the post-death respawn fade. Pinned to 1.0 by
    /// [`crate::game::transition::apply_net_transition`] (and
    /// locally when a death kicks in) and decayed back to 0
    /// over ~0.6 s each frame so the world fades in cleanly
    /// after the regeneration stall.
    pub transition_fade: f32,
    /// Local player's HP last frame, used to detect damage
    /// events (for the hit-react one-shot) and the alive→dead
    /// edge that triggers the death animation. `None` until
    /// the first frame the local player exists in the world.
    pub prev_player_hp: Option<f32>,
    /// Edge-detector mirror of `local_ghost_cached`, used to
    /// fire `trigger_player_rise` exactly once on the down-pose
    /// → ghost transition. Cleared on regen / respawn.
    pub prev_local_ghost: bool,
    /// `Some(text)` if the local player is standing in an
    /// interaction range this frame and the HUD should show a
    /// press-F prompt. Set during `tick_*_portal` and the stash
    /// chest tick, consumed and cleared during the HUD pass.
    pub hud_prompt: Option<&'static str>,
    /// `true` whenever the local player is standing inside the
    /// boss-room exit portal's interaction radius this frame
    /// (and a transition isn't already pending). Drives the
    /// difficulty step-up tooltip in the HUD pass so the
    /// player can read what they're walking into before
    /// pressing F. Set in `portal_system::tick_exit`,
    /// consumed in the HUD pass.
    pub descend_prompt: bool,
}

impl FrameState {
    /// Wipe transient frame state on a floor regen. Equivalent
    /// to assigning `FrameState::default()`, written out so a
    /// future reader can see exactly which fields participate.
    pub fn reset(&mut self) {
        self.targeting = None;
        self.damage_flash = 0.0;
        self.level_up_flash = 0.0;
        // `transition_fade` is intentionally NOT cleared here:
        // `apply_net_transition` pins it to 1.0 immediately
        // after calling reset, and clearing it would race with
        // that pin in a way that's only correct by accident.
        self.prev_player_hp = None;
        self.prev_local_ghost = false;
        self.hud_prompt = None;
        self.descend_prompt = false;
    }
}
