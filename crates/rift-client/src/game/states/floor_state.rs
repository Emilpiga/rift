//! Per-floor state. Rebuilt from scratch on every floor
//! transition (hub ↔ rift, rift floor advance). Distinct from
//! [`super::frame_state::FrameState`] (which resets *every*
//! frame's transient timers) and from cross-floor state
//! (inventory, level, account) which lives on `GameState`
//! directly.

use glam::Vec3;
use rift_engine::ecs::components::Collider;
use rift_engine::physics::Aabb;

use crate::game::portal_system::HubPortal;

pub struct FloorState {
    /// Cached wall colliders for physics (rebuilt on floor change).
    pub wall_colliders: Vec<(Vec3, Collider)>,
    /// Cached wall AABBs for raycasting (rebuilt on floor change).
    pub wall_aabbs: Vec<Aabb>,
    /// True while the player is in the safe hub zone.
    pub in_hub: bool,
    /// Glowing entry portal placed in the hub.
    pub hub_portal: Option<HubPortal>,
    /// Glowing exit portal that appears in the boss room after
    /// the floor's boss dies. Same chrome as `hub_portal` but
    /// triggers `NetTransitionRequest::EnterRift` (which the
    /// server interprets as "advance one floor" once we're not
    /// in the hub).
    pub exit_portal: Option<HubPortal>,
    /// Always-present portal at the rift floor's spawn point.
    /// Pressing F here opens the rift exit vote (or, solo,
    /// instantly transitions to the hub). Spawned lazily when a
    /// rift floor is generated; cleared on hub return.
    pub rift_spawn_portal: Option<HubPortal>,
}

impl Default for FloorState {
    fn default() -> Self {
        Self {
            wall_colliders: Vec::new(),
            wall_aabbs: Vec::new(),
            // Default to "in hub" because that's the post-character-select
            // entry state; the rift transition path explicitly flips this
            // to false.
            in_hub: true,
            hub_portal: None,
            exit_portal: None,
            rift_spawn_portal: None,
        }
    }
}

impl FloorState {
    /// Wipe per-floor visuals. `in_hub` and the wall caches are
    /// NOT reset here — the caller (the transition pipeline)
    /// sets them explicitly to the new floor's values
    /// immediately afterwards, and a stale-then-correct pair of
    /// writes would only make ordering bugs harder to spot.
    ///
    /// Audio cleanup is the caller's responsibility: call
    /// [`detach_portal_audio`] BEFORE this so the looping hum
    /// emitters are despawned before the `HubPortal` structs
    /// holding their ids are dropped.
    pub fn reset_portals(&mut self) {
        self.hub_portal = None;
        self.exit_portal = None;
        self.rift_spawn_portal = None;
    }

    /// Despawn every portal's looping audio emitter. Pair with
    /// [`Self::reset_portals`] on the floor-teardown path so
    /// the kira tracks free up before the slot ids are
    /// dropped.
    pub fn detach_portal_audio(&mut self, audio: &mut rift_audio::AudioSystem) {
        for slot in [
            self.hub_portal.as_mut(),
            self.exit_portal.as_mut(),
            self.rift_spawn_portal.as_mut(),
        ]
        .into_iter()
        .flatten()
        {
            crate::game::portal_system::detach_audio(slot, audio);
        }
    }
}
