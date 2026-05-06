//! Rift networked game client.
//!
//! This crate is the *only* binary players actually run. It owns the
//! network session (renet client + handshake + snapshot ingestion +
//! input shipping) and drives the renderer / animation / UI layers
//! that live in `rift-engine` and (transitionally) `rift-game`.
//!
//! Architecture:
//! - `net_client` — the renet session. Drives prediction, applies
//!   server snapshots into the ECS world, ships casts/inputs back.
//! - The binary entry point lives in `main.rs` and wires the
//!   network session into a [`rift_engine::App`] together with
//!   `rift_game::GameState` (which currently still owns the bulk
//!   of the visual state — that ownership migrates over here in
//!   subsequent refactor stages).

pub mod game;
pub mod net_client;
