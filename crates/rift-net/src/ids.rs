//! Identifier types used on the wire.
//!
//! These are deliberately *not* hecs `Entity` handles: hecs IDs are
//! local to one world, but multiplayer needs identifiers that survive
//! the trip across the wire and resolve to (potentially different)
//! local entities on every peer. The mapping
//! [`NetId`]→`hecs::Entity` lives in the game crate.

use serde::{Deserialize, Serialize};

/// Stable, server-assigned identifier for a replicated entity.
///
/// Allocated by the server when an entity becomes networked (player
/// joins, monster spawns, projectile fires). `0` is reserved as a
/// sentinel meaning "no entity / not yet networked" so a default
/// `NetId` can be used as a tombstone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[repr(transparent)]
pub struct NetId(pub u32);

impl NetId {
    pub const NONE: NetId = NetId(0);

    pub fn is_some(self) -> bool {
        self.0 != 0
    }
}

/// Identifier for a connected client (server's view of a player).
/// Renet exposes this as `u64`; we wrap it for type clarity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct ClientId(pub u64);

/// Server simulation tick counter. Wraps after ~2.2 years at 30 Hz —
/// effectively never for our purposes, but we treat comparison via
/// wrapping arithmetic so a future fix is trivial.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[repr(transparent)]
pub struct NetTick(pub u32);

impl NetTick {
    pub fn next(self) -> NetTick {
        NetTick(self.0.wrapping_add(1))
    }

    /// Signed delta `self - rhs` using wrapping arithmetic. Useful for
    /// expressions like "is event.start_tick within `N` ticks of now".
    pub fn diff(self, rhs: NetTick) -> i32 {
        (self.0.wrapping_sub(rhs.0)) as i32
    }
}
