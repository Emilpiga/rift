//! Tiny serde wrappers so callers don't import bincode directly.
//!
//! We pick the standard bincode 1.x config (little-endian, fixint
//! length encoding) because it's deterministic across platforms and
//! produces a tight binary format. If we ever need a smaller wire we
//! can swap in `bincode::options().with_varint_encoding()`; the
//! choice is centralized here so it's a one-line change.

use serde::{de::DeserializeOwned, Serialize};
use thiserror::Error;

/// Errors raised by [`encode`] / [`decode`].
#[derive(Debug, Error)]
pub enum NetCodecError {
    #[error("serialize: {0}")]
    Serialize(bincode::Error),
    #[error("deserialize: {0}")]
    Deserialize(bincode::Error),
}

/// Serialize a message to bytes ready to hand to renet.
pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, NetCodecError> {
    bincode::serialize(msg).map_err(NetCodecError::Serialize)
}

/// Deserialize a message from bytes received from renet.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, NetCodecError> {
    bincode::deserialize(bytes).map_err(NetCodecError::Deserialize)
}
