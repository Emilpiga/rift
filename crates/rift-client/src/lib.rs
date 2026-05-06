//! Rift networked game client.
//!
//! This crate is the *only* binary players actually run. It owns the
//! network session (renet client + handshake + snapshot ingestion +
//! input shipping) and drives the renderer / animation / UI layers
//! that live in `rift-engine` and (transitionally) `rift-game`.
//!
//! Architecture:
//! - `net` — the renet session, split into transport+dispatch
//!   (`net::mod`), snapshot ingestion + reconciliation
//!   (`net::snapshot`), outbound commands + local prediction
//!   (`net::commands`), and snapshot→ECS reconciliation
//!   (`net::world_sync`).
//! - The binary entry point lives in `main.rs` and wires the
//!   network session into a [`rift_engine::App`] together with
//!   `rift_game::GameState` (which currently still owns the bulk
//!   of the visual state — that ownership migrates over here in
//!   subsequent refactor stages).

pub mod game;
pub mod net;

/// Compatibility re-export. `main.rs` and downstream code still
/// import from `rift_client::net_client::*`; keep that path live
/// so the split is internal-only.
pub mod net_client {
    pub use super::net::{ClientProfile, NetClient, PendingFloor, RemoteEntity, RemoteProfile};
}
