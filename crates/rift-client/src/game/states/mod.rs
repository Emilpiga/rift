//! State buckets owned by [`crate::game::state::GameState`].
//!
//! - [`sub_state`]: server-mirrored buckets (net, loot, channel,
//!   shrines) and the per-frame loading state machine.
//! - [`frame_state`]: transient HUD timers + edge detectors,
//!   wiped on every floor regen.
//! - [`floor_state`]: walls, portals, hub flag — rebuilt on
//!   every floor regen.
//! - [`player_state`]: cross-floor character state (loadout,
//!   experience, profile metadata).
//! - [`rift_state`]: per-rift-run progress (floor index,
//!   timer, boss-room flags).
//!
//! All five are re-exported flat at the [`crate::game`] root
//! so existing `crate::game::sub_state::Foo` paths still resolve.

pub mod floor_state;
pub mod frame_state;
pub mod player_state;
pub mod rift_state;
pub mod sub_state;
