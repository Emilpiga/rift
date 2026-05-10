//! Shared HMAC-SHA256 signing format for the dev auth issuer.
//!
//! Lives in `rift-net` (rather than `rift-server`) so the
//! client and server compute byte-identical signatures from
//! the same source — drift here is the most common cause of
//! `BadSignature` rejections after schema changes.
//!
//! See `rift_server::auth::dev` for verification + replay-window
//! enforcement; this module only owns the signing payload + the
//! `sign` helper both sides call into.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Maximum allowed length (Unicode scalars) of a dev `identity`
/// string. Mirrors the 18-char player-name cap the client UI
/// already enforces; re-checked by the server's verifier
/// because the wire is server-authoritative.
pub const MAX_DEV_IDENTITY_CHARS: usize = 18;

/// Acceptable drift between client and server clocks for a
/// dev credential, in seconds. Generous enough to survive
/// sloppy NTP without indefinitely accepting captured payloads.
/// Both sides must agree on this constant: the client uses it
/// implicitly (timestamps now), the server enforces it on
/// verify.
pub const DEV_AUTH_REPLAY_WINDOW_SECS: u64 = 60;

/// Canonical signing payload: `identity` UTF-8 bytes, then
/// little-endian `nonce`, then little-endian `timestamp`.
/// Pulled out so both client and server compute it the same
/// way — mismatch here is the most common cause of
/// `BadSignature` after schema changes.
pub fn signing_payload(identity: &str, nonce: u64, timestamp_unix: u64) -> Vec<u8> {
    let id_bytes = identity.as_bytes();
    let mut buf = Vec::with_capacity(id_bytes.len() + 16);
    buf.extend_from_slice(id_bytes);
    buf.extend_from_slice(&nonce.to_le_bytes());
    buf.extend_from_slice(&timestamp_unix.to_le_bytes());
    buf
}

/// Sign a dev credential. Used by the client to mint
/// `AuthCredential::Dev::signature` and by the server's
/// verifier to compute the expected value for constant-time
/// compare.
pub fn sign(key: &[u8; 32], identity: &str, nonce: u64, timestamp_unix: u64) -> [u8; 32] {
    let mut mac =
        HmacSha256::new_from_slice(key).expect("HMAC-SHA256 accepts any key length");
    mac.update(&signing_payload(identity, nonce, timestamp_unix));
    let bytes = mac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_layout_is_stable() {
        // Spot-check the byte layout so a refactor that
        // accidentally changes endianness or field order trips
        // a test instead of silently invalidating every
        // existing signature.
        let payload = signing_payload("ab", 0x0102_0304_0506_0708, 0x1112_1314_1516_1718);
        assert_eq!(
            payload,
            vec![
                b'a', b'b', // identity bytes
                0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, // nonce LE
                0x18, 0x17, 0x16, 0x15, 0x14, 0x13, 0x12, 0x11, // timestamp LE
            ]
        );
    }

    #[test]
    fn sign_is_deterministic() {
        let key = [7u8; 32];
        let a = sign(&key, "alice", 1, 1000);
        let b = sign(&key, "alice", 1, 1000);
        assert_eq!(a, b);
    }
}
