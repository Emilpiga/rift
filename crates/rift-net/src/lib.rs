//! Networking layer for Rift Crawler.
//!
//! ## Scope (Phase 1)
//!
//! This crate defines the *wire* — message types, channel layout,
//! protocol version — and provides thin helpers for opening a renet
//! [`RenetServer`] / [`RenetClient`]. It deliberately knows nothing
//! about the ECS, the renderer, or any game-specific systems. The
//! game crate consumes these types and translates them to/from world
//! state.
//!
//! ## Authority model
//!
//! Server-authoritative. Clients send [`ClientMsg`]s (mostly inputs);
//! the server runs the simulation and broadcasts [`ServerMsg`]s
//! (snapshots + events).
//!
//! ## Channels
//!
//! Two reliable channels and one unreliable:
//!
//! | Channel             | Reliability  | Used for |
//! |---------------------|--------------|----------|
//! | `Channel::Snapshot` | Unreliable   | Per-tick world snapshots (lossy, idempotent) |
//! | `Channel::Event`    | Reliable-ord | Damage events, cast events, deaths, loot |
//! | `Channel::Control`  | Reliable-ord | Handshake, lobby, floor transitions, errors |

pub mod auth_dev;
pub mod channel;
pub mod codec;
pub mod ids;
pub mod messages;
pub mod protocol;
pub mod transport;

pub use channel::{channel_config, Channel};
pub use codec::{decode, encode, NetCodecError};
pub use ids::{ClientId, NetId, NetTick};
pub use messages::{AuthCredential, ClientMsg, Gender, ServerMsg};
pub use protocol::{NetSettings, MAX_CLIENTS, PROTOCOL_ID, PROTOCOL_VERSION, SNAPSHOT_HZ, TICK_HZ};
pub use transport::{open_client, open_server, ClientHandle, ServerHandle};

// Re-export the renet types our consumers (rift-server, rift-game)
// need to interact with after construction. Keeping rift-net the
// only crate that names `renet` directly avoids version drift.
pub use renet;
