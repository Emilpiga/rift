//! Dev authentication ticket format.
//!
//! Dev mode is conceptually "local fake Steam": the client mints
//! an opaque `Vec<u8>` ticket and ships it on the wire, exactly
//! as the production Steam path would. The server's installed
//! verifier knows how to validate the bytes.
//!
//! This module owns the dev ticket layout end-to-end so client
//! and server are byte-compatible from one source of truth —
//! drift here is the most common cause of `BadSignature`
//! rejections after schema changes.
//!
//! ### Ticket layout
//!
//! ```text
//!   offset  size  field
//!   ------  ----  ------------------------------------------------
//!   0       1     version byte (currently DEV_TICKET_VERSION = 1)
//!   1       1     identity length in bytes (≤ 255)
//!   2       N     identity (UTF-8, ≤ MAX_DEV_IDENTITY_CHARS chars)
//!   2+N     8     nonce, little-endian u64
//!  10+N     8     timestamp_unix, little-endian u64
//!  18+N     32    HMAC-SHA256 signature
//! ```
//!
//! Signing input is the concatenation of `identity || nonce_le
//! || timestamp_le` — i.e. exactly the prefix bytes minus the
//! version + length header, which means a passive observer
//! can't tweak the length byte without invalidating the
//! signature.
//!
//! The version byte lets us evolve the dev format independently
//! of `PROTOCOL_VERSION`: a future revision bumps `DEV_TICKET_VERSION`
//! and the server's decoder rejects unknown versions.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Current dev ticket layout version. Bump on any breaking
/// change to the byte layout above.
pub const DEV_TICKET_VERSION: u8 = 1;

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

/// Parsed dev ticket fields. The server's verifier consumes
/// these after [`decode_dev_ticket`] has range-checked the
/// version byte + length header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DevTicket {
    pub identity: String,
    pub nonce: u64,
    pub timestamp_unix: u64,
    pub signature: [u8; 32],
}

/// Reasons a dev ticket failed to decode. Returned by
/// [`decode_dev_ticket`]; the server's verifier maps these
/// onto its own `AuthError` variants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DevTicketDecodeError {
    /// Buffer was shorter than the minimum fixed header
    /// (version + length + nonce + timestamp + signature).
    Truncated,
    /// Version byte didn't match [`DEV_TICKET_VERSION`].
    /// Either a stale client or a corrupt ticket.
    UnknownVersion(u8),
    /// Identity bytes weren't valid UTF-8.
    BadIdentityUtf8,
    /// Identity length byte pointed past the end of the buffer.
    BadIdentityLength,
}

/// Canonical signing payload: `identity` UTF-8 bytes, then
/// little-endian `nonce`, then little-endian `timestamp`. The
/// version + length header are not part of the signed payload
/// so a tampered header invalidates the signature on its own.
pub fn signing_payload(identity: &str, nonce: u64, timestamp_unix: u64) -> Vec<u8> {
    let id_bytes = identity.as_bytes();
    let mut buf = Vec::with_capacity(id_bytes.len() + 16);
    buf.extend_from_slice(id_bytes);
    buf.extend_from_slice(&nonce.to_le_bytes());
    buf.extend_from_slice(&timestamp_unix.to_le_bytes());
    buf
}

/// Sign a dev credential. Used by the client when minting a
/// ticket and by the server's verifier to compute the
/// expected value for constant-time compare.
pub fn sign(key: &[u8; 32], identity: &str, nonce: u64, timestamp_unix: u64) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC-SHA256 accepts any key length");
    mac.update(&signing_payload(identity, nonce, timestamp_unix));
    let bytes = mac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    out
}

/// Encode the full dev ticket byte string the client ships in
/// `Hello.auth_ticket`. Panics in debug builds if `identity` is
/// longer than 255 bytes (the wire format reserves a single
/// length byte); release builds truncate to 255 to stay safe.
///
/// Most callers run the identity through their own validator
/// first (the client's `pick_identity` caps at
/// `MAX_DEV_IDENTITY_CHARS = 18` Unicode scalars, which is well
/// under 255 bytes).
pub fn encode_dev_ticket(
    identity: &str,
    nonce: u64,
    timestamp_unix: u64,
    signature: &[u8; 32],
) -> Vec<u8> {
    let id_bytes = identity.as_bytes();
    debug_assert!(
        id_bytes.len() <= u8::MAX as usize,
        "identity must fit in one length byte (got {})",
        id_bytes.len()
    );
    let id_len = id_bytes.len().min(u8::MAX as usize) as u8;
    let mut buf = Vec::with_capacity(2 + id_len as usize + 8 + 8 + 32);
    buf.push(DEV_TICKET_VERSION);
    buf.push(id_len);
    buf.extend_from_slice(&id_bytes[..id_len as usize]);
    buf.extend_from_slice(&nonce.to_le_bytes());
    buf.extend_from_slice(&timestamp_unix.to_le_bytes());
    buf.extend_from_slice(signature);
    buf
}

/// Parse a ticket the server received in `Hello.auth_ticket`.
/// Range-checks the version byte + length header; the actual
/// HMAC compare + replay-window check happens in the server's
/// verifier after this returns.
pub fn decode_dev_ticket(bytes: &[u8]) -> Result<DevTicket, DevTicketDecodeError> {
    // Minimum size: 1 (version) + 1 (id_len) + 0 (identity) + 8 (nonce)
    // + 8 (ts) + 32 (sig) = 50 bytes.
    if bytes.len() < 50 {
        return Err(DevTicketDecodeError::Truncated);
    }
    let version = bytes[0];
    if version != DEV_TICKET_VERSION {
        return Err(DevTicketDecodeError::UnknownVersion(version));
    }
    let id_len = bytes[1] as usize;
    let id_end = 2 + id_len;
    if bytes.len() < id_end + 8 + 8 + 32 {
        return Err(DevTicketDecodeError::BadIdentityLength);
    }
    let identity = std::str::from_utf8(&bytes[2..id_end])
        .map_err(|_| DevTicketDecodeError::BadIdentityUtf8)?
        .to_string();
    let nonce = u64::from_le_bytes(bytes[id_end..id_end + 8].try_into().unwrap());
    let ts = u64::from_le_bytes(bytes[id_end + 8..id_end + 16].try_into().unwrap());
    let mut sig = [0u8; 32];
    sig.copy_from_slice(&bytes[id_end + 16..id_end + 48]);
    Ok(DevTicket {
        identity,
        nonce,
        timestamp_unix: ts,
        signature: sig,
    })
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

    #[test]
    fn ticket_round_trip() {
        let sig = sign(&[3u8; 32], "alice", 42, 1_000_000);
        let buf = encode_dev_ticket("alice", 42, 1_000_000, &sig);
        let decoded = decode_dev_ticket(&buf).expect("round-trip decodes");
        assert_eq!(decoded.identity, "alice");
        assert_eq!(decoded.nonce, 42);
        assert_eq!(decoded.timestamp_unix, 1_000_000);
        assert_eq!(decoded.signature, sig);
    }

    #[test]
    fn ticket_rejects_unknown_version() {
        let mut buf = encode_dev_ticket("x", 0, 0, &[0u8; 32]);
        buf[0] = 99;
        assert_eq!(
            decode_dev_ticket(&buf),
            Err(DevTicketDecodeError::UnknownVersion(99))
        );
    }

    #[test]
    fn ticket_rejects_truncation() {
        let buf = encode_dev_ticket("alice", 1, 1, &[0u8; 32]);
        assert_eq!(
            decode_dev_ticket(&buf[..20]),
            Err(DevTicketDecodeError::Truncated)
        );
    }
}
