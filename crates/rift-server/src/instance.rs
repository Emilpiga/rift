//! Per-party / per-matchmaking-lobby rift instances.
//!
//! Replaces the old singleton `rift: Sim` on [`crate::Server`]
//! with a `HashMap<RiftInstanceId, RiftInstance>` so multiple
//! rifts can run side by side. Each [`RiftInstance`] owns its
//! own [`Sim`] and a tiny envelope of metadata describing who
//! it belongs to (a private party, or a matchmaking lobby) and
//! how many members it can hold.
//!
//! Lifecycle:
//!
//! 1. A player (or party leader) accepts the portal modal →
//!    [`InstanceManager::create_private`] or
//!    [`InstanceManager::find_or_create_matchmade`] is called.
//! 2. Server moves players into the instance via
//!    `move_client_to_instance` (in `main.rs`), routing each
//!    one through the existing `Sim::extract_player` /
//!    `Sim::inject_player` pair.
//! 3. When the last client exits (return-to-hub vote, wipe,
//!    or disconnect), the instance is dropped via
//!    [`InstanceManager::dissolve`]. The `Sim` is destructed
//!    along with it — every fresh run starts from a clean
//!    state regardless of previous descents.

use std::collections::HashMap;

use crate::sim::Sim;

/// Stable rift-instance identity. Allocated monotonically
/// inside [`InstanceManager`]; never reused for the lifetime
/// of the server process so a stale `client_instance` lookup
/// doesn't collide with a freshly-spawned instance.
#[derive(Copy, Clone, Hash, Eq, PartialEq, Debug, Default)]
pub struct RiftInstanceId(pub u64);

/// What kind of run an instance is hosting. Drives both the
/// capacity check (Solo / Party are capped at the proposing
/// party's size; Matchmade is always capped at
/// [`rift_net::messages::MAX_PARTY`]) and the matchmaking
/// search path (Matchmade instances with capacity remaining
/// are joinable by other matchmakers; Private ones are not).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum InstanceMode {
    /// Solo or Party: bound to the proposer's party (or the
    /// lone proposer for solo runs). Capacity = the number of
    /// confirmed members at portal time. Closed to outside
    /// joins.
    Private,
    /// Open to matchmaking. The proposing party (after opt-in)
    /// fills part of the cap; remaining slots are filled by
    /// any other Matchmade proposer who picks the same start
    /// floor.
    Matchmade,
}

/// One running rift run. Owns the simulation world and a tiny
/// envelope of metadata used by the lobby / chat / portal
/// flows.
pub struct RiftInstance {
    pub sim: Sim,
    pub mode: InstanceMode,
    /// Hard cap on concurrent members. Server refuses
    /// `move_client_to_instance` once this many clients have
    /// `client_instance == self.id`.
    pub capacity: u8,
    /// Floor index the proposer asked to start on. Stored
    /// here (rather than read off `sim.floor_index`) so
    /// matchmaking can match instances by their *configured*
    /// start floor even after the party has descended past
    /// it. Future quality-of-life: refuse late-join into an
    /// instance that has already advanced past the joiner's
    /// `deepest_cleared_floor + 1`.
    pub start_floor: u32,
    /// Last-seen `boss_killed` flag for the rising-edge
    /// detector that fires the GLOBAL "boss slain" toast and
    /// bumps each member's `deepest_cleared_floor`. Per-
    /// instance so two instances on the same floor index
    /// don't share state.
    pub prev_boss_killed: bool,
}

/// Top-level rift-instance state. Owns every running
/// [`RiftInstance`] and a monotonic id allocator.
#[derive(Default)]
pub struct InstanceManager {
    instances: HashMap<RiftInstanceId, RiftInstance>,
    next_id: u64,
}

impl InstanceManager {
    pub fn new() -> Self {
        Self {
            instances: HashMap::new(),
            next_id: 1,
        }
    }

    pub fn get(&self, id: RiftInstanceId) -> Option<&RiftInstance> {
        self.instances.get(&id)
    }

    pub fn get_mut(&mut self, id: RiftInstanceId) -> Option<&mut RiftInstance> {
        self.instances.get_mut(&id)
    }

    /// Iterator over every running instance, immutable. Used
    /// by the snapshot / event drain loops in `main.rs`.
    pub fn iter(&self) -> impl Iterator<Item = (&RiftInstanceId, &RiftInstance)> {
        self.instances.iter()
    }

    /// Spin up a fresh private instance. `start_floor` must be
    /// a positive floor index — the caller is responsible for
    /// clamping against the party-wide
    /// `deepest_cleared_floor + 1`.
    pub fn create_private(&mut self, start_floor: u32, capacity: u8, seed: u64) -> RiftInstanceId {
        let id = RiftInstanceId(self.next_id);
        self.next_id += 1;
        let sim = Sim::new(seed, start_floor.max(1));
        self.instances.insert(
            id,
            RiftInstance {
                sim,
                mode: InstanceMode::Private,
                capacity,
                start_floor,
                prev_boss_killed: false,
            },
        );
        id
    }

    /// Look up a Matchmade instance with capacity remaining at
    /// `start_floor`. Returns the first match found (HashMap
    /// iteration order — deterministic enough for one server
    /// process). `None` when no match is available; the caller
    /// then opens a fresh one with [`Self::create_matchmade`].
    ///
    /// `member_count` is a closure so the caller can ask the
    /// server for the *live* member count without taking a
    /// borrow on this manager.
    pub fn find_open_matchmade<F>(
        &self,
        start_floor: u32,
        member_count: F,
    ) -> Option<RiftInstanceId>
    where
        F: Fn(RiftInstanceId) -> u8,
    {
        for (id, inst) in &self.instances {
            if inst.mode != InstanceMode::Matchmade {
                continue;
            }
            if inst.start_floor != start_floor {
                continue;
            }
            if member_count(*id) >= inst.capacity {
                continue;
            }
            return Some(*id);
        }
        None
    }

    /// Spin up a fresh Matchmade instance. Capacity always
    /// equals [`rift_net::messages::MAX_PARTY`] — the whole
    /// point of matchmade is to fill the lobby.
    pub fn create_matchmade(&mut self, start_floor: u32, seed: u64) -> RiftInstanceId {
        let id = RiftInstanceId(self.next_id);
        self.next_id += 1;
        let sim = Sim::new(seed, start_floor.max(1));
        self.instances.insert(
            id,
            RiftInstance {
                sim,
                mode: InstanceMode::Matchmade,
                capacity: rift_net::messages::MAX_PARTY,
                start_floor,
                prev_boss_killed: false,
            },
        );
        id
    }

    /// Drop an instance. Caller must have already extracted /
    /// re-homed every player that was in it (or the next
    /// snapshot will dangle their net ids). Returns the
    /// dropped instance for any final teardown the server
    /// needs to do.
    pub fn dissolve(&mut self, id: RiftInstanceId) -> Option<RiftInstance> {
        self.instances.remove(&id)
    }
}
