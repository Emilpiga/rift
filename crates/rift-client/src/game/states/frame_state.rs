//! Per-frame transient state. Every field is recomputed each
//! tick or driven by an edge event; nothing here survives a
//! floor regen. See [`FrameState::reset`] for the contract.
//!
//! Owned by [`super::state::GameState::frame`]; phase modules
//! mutate these fields directly through the struct.

use std::collections::HashMap;

use crate::game::combat_system::{EntityTargeting, PlacedTargeting};
use glam::Vec3;
use rift_engine::ui::im::Rect;
use rift_net::NetId;

#[derive(Default)]
pub struct FrameState {
    /// Active placed-ability targeting (if any). Pure visual /
    /// input state — the actual cast is sent to the server.
    pub targeting: Option<PlacedTargeting>,
    /// Active entity-target picking for friendly single-target
    /// abilities (heals). Mutually exclusive with
    /// [`Self::targeting`] in practice — the keybind dispatch
    /// only enters one mode at a time.
    pub entity_targeting: Option<EntityTargeting>,
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
    /// Edge-triggered "user clicked party member with this
    /// character name" intent. Set by [`crate::game::party::PartyUi`]
    /// when a left-click lands on a party frame; consumed by
    /// the entity-targeting tick (resolves to a `NetId` via
    /// [`crate::net::NetClient::net_id_for_name`] and confirms
    /// the cast as if the player had clicked the avatar).
    pub party_click_target_name: Option<String>,
    /// Resolved counterpart to `party_click_target_name`. The
    /// binary fills this each frame by running the name
    /// through `NetClient::net_id_for_name`; the combat tick
    /// then consumes it as a confirmed entity-target click.
    /// Split from the name field so the lookup can sit in
    /// the binary (which holds the net session) without
    /// teaching the combat module about `NetClient`.
    pub party_click_target_net_id: Option<rift_net::NetId>,
    /// Last-seen plant counters from the local player's
    /// `FootIkState`. The foot IK pass increments those
    /// counters every time a foot transitions from airborne
    /// to planted (animation-driven, not velocity-derived,
    /// so rolls and other movement effects don't desynchronise
    /// the audio from the visible step). Each render frame we
    /// compare against the current counters and fire one
    /// footstep one-shot per delta.
    ///
    /// `step_rotation` advances 0..N each plant so two
    /// consecutive plants never use the same surface sample
    /// (the bank length is per-surface; modulo is taken at
    /// the call site).
    pub last_left_plant_seq: u32,
    pub last_right_plant_seq: u32,
    pub step_rotation: u8,
    /// HUD widget rects that should swallow gameplay LMB clicks.
    /// Pushed by HUD render passes during `ui_phase` (ability
    /// bar plaque, alt-hold loot labels, etc.); read on the
    /// **next** frame by `combat_phase` to decide whether the
    /// click was a UI interaction rather than a basic-attack
    /// cast. Cleared at the top of every `ui_phase` tick so
    /// rects only ever survive one frame.
    pub hud_consume_rects: Vec<Rect>,
    /// Wraith scream wind-up aim per caster. Impact VFX must
    /// reuse this so the release matches the telegraph even if
    /// the wire `dir` on the impact event differs.
    pub wraith_scream_telegraph_aim: HashMap<NetId, Vec3>,
}

impl FrameState {
    /// Wipe transient frame state on a floor regen. Equivalent
    /// to assigning `FrameState::default()`, written out so a
    /// future reader can see exactly which fields participate.
    pub fn reset(&mut self) {
        self.targeting = None;
        self.entity_targeting = None;
        self.damage_flash = 0.0;
        self.level_up_flash = 0.0;
        self.last_left_plant_seq = 0;
        self.last_right_plant_seq = 0;
        self.step_rotation = 0;
        // `transition_fade` is intentionally NOT cleared here:
        // `apply_net_transition` pins it to 1.0 immediately
        // after calling reset, and clearing it would race with
        // that pin in a way that's only correct by accident.
        self.prev_player_hp = None;
        self.prev_local_ghost = false;
        self.hud_prompt = None;
        self.descend_prompt = false;
        self.party_click_target_name = None;
        self.party_click_target_net_id = None;
        self.hud_consume_rects.clear();
        self.wraith_scream_telegraph_aim.clear();
    }
}
