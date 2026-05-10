//! Dev auth: HMAC-SHA256 over `(identity, nonce, timestamp)`
//! with a shared 32-byte key supplied via `RIFT_DEV_AUTH_KEY`.
//!
//! The signing payload + `sign` helper live in
//! [`rift_net::auth_dev`] so the client and server compute
//! byte-identical signatures from one source of truth. This
//! module owns verification: replay-window enforcement,
//! identity validation, constant-time signature compare, and
//! key decoding from environment hex.
//!
//! Threat model is intentionally narrow — this exists so devs
//! and playtesters can spin up multiple "logged in" clients
//! without standing up Steam. It is not a substitute for a
//! real identity provider:
//!
//! * Anyone with the shared key can mint credentials for any
//!   identity. Treat the key like a password.
//! * The replay window is short but not zero; an attacker on
//!   the wire can resubmit a fresh credential within
//!   [`rift_net::auth_dev::DEV_AUTH_REPLAY_WINDOW_SECS`].
//! * There is no per-identity revocation. Rotating the key
//!   revokes everyone.
//!
//! All three caveats are acceptable for a closed-network dev
//! environment and unacceptable for production — which is why
//! the production server should leave `RIFT_DEV_AUTH_KEY`
//! unset and rely solely on Steam.

use rift_net::auth_dev::{sign, DEV_AUTH_REPLAY_WINDOW_SECS, MAX_DEV_IDENTITY_CHARS};
use subtle::ConstantTimeEq;

use super::AuthError;
use crate::auth::AccountKey;

/// Decode a 64-character hex string into the 32-byte HMAC key.
/// Whitespace around the value is trimmed so users can paste
/// without worrying about trailing newlines.
pub fn decode_key(hex_str: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(hex_str.trim()).map_err(|e| format!("not valid hex: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!(
            "expected 32 bytes (64 hex chars), got {}",
            bytes.len()
        ));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// Verify a dev credential. Constant-time compare on the
/// signature; replay-window check on the timestamp; basic
/// shape check on the identity.
pub fn verify(
    key: &[u8; 32],
    identity: &str,
    nonce: u64,
    timestamp_unix: u64,
    signature: &[u8; 32],
) -> Result<AccountKey, AuthError> {
    let trimmed = identity.trim();
    if trimmed.is_empty() {
        return Err(AuthError::BadIdentity("identity is empty".to_string()));
    }
    let len = trimmed.chars().count();
    if len > MAX_DEV_IDENTITY_CHARS {
        return Err(AuthError::BadIdentity(format!(
            "identity too long ({len} chars)"
        )));
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err(AuthError::BadIdentity(
            "identity contains control characters".to_string(),
        ));
    }

    // Replay window: signed-distance from "now". `saturating_sub`
    // on both sides handles the edge case where the system clock
    // jumped (e.g. a container start before NTP sync) so we
    // reject loudly instead of underflowing into "fresh".
    let now = unix_now();
    let drift = now.max(timestamp_unix) - now.min(timestamp_unix);
    if drift > DEV_AUTH_REPLAY_WINDOW_SECS {
        return Err(AuthError::StaleTimestamp);
    }

    let expected = sign(key, trimmed, nonce, timestamp_unix);
    if expected.ct_eq(signature).into() {
        Ok(AccountKey::Dev(trimmed.to_string()))
    } else {
        Err(AuthError::BadSignature)
    }
}

/// Wall-clock seconds since the Unix epoch, monotonically
/// best-effort: we treat any pre-epoch clock (which would
/// indicate a wildly broken host) as `0` so the replay-window
/// math stays sane instead of panicking.
fn unix_now() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; 32] {
        // Deterministic non-zero key so each test byte differs.
        let mut k = [0u8; 32];
        for (i, b) in k.iter_mut().enumerate() {
            *b = i as u8;
        }
        k
    }

    #[test]
    fn round_trip_accepts() {
        let key = test_key();
        let now = unix_now();
        let sig = sign(&key, "alice", 42, now);
        let resolved = verify(&key, "alice", 42, now, &sig).expect("valid credential");
        assert_eq!(resolved, AccountKey::Dev("alice".to_string()));
    }

    #[test]
    fn rejects_bad_signature() {
        let key = test_key();
        let now = unix_now();
        let sig = sign(&key, "alice", 1, now);
        let mut tampered = sig;
        tampered[0] ^= 0x01;
        assert!(matches!(
            verify(&key, "alice", 1, now, &tampered),
            Err(AuthError::BadSignature)
        ));
    }

    #[test]
    fn rejects_wrong_identity() {
        let key = test_key();
        let now = unix_now();
        let sig = sign(&key, "alice", 1, now);
        assert!(matches!(
            verify(&key, "bob", 1, now, &sig),
            Err(AuthError::BadSignature)
        ));
    }

    #[test]
    fn rejects_stale_timestamp() {
        let key = test_key();
        let stale = unix_now().saturating_sub(DEV_AUTH_REPLAY_WINDOW_SECS + 5);
        let sig = sign(&key, "alice", 1, stale);
        assert!(matches!(
            verify(&key, "alice", 1, stale, &sig),
            Err(AuthError::StaleTimestamp)
        ));
    }

    #[test]
    fn rejects_empty_identity() {
        let key = test_key();
        let now = unix_now();
        let sig = sign(&key, "", 0, now);
        assert!(matches!(
            verify(&key, "", 0, now, &sig),
            Err(AuthError::BadIdentity(_))
        ));
    }

    #[test]
    fn rejects_long_identity() {
        let key = test_key();
        let long = "a".repeat(MAX_DEV_IDENTITY_CHARS + 1);
        let now = unix_now();
        let sig = sign(&key, &long, 0, now);
        assert!(matches!(
            verify(&key, &long, 0, now, &sig),
            Err(AuthError::BadIdentity(_))
        ));
    }

    #[test]
    fn decode_key_round_trips() {
        let raw = test_key();
        let s = hex::encode(raw);
        assert_eq!(decode_key(&s).unwrap(), raw);
        assert_eq!(decode_key(&format!("  {s}\n")).unwrap(), raw);
    }

    #[test]
    fn decode_key_rejects_wrong_length() {
        assert!(decode_key("deadbeef").is_err());
    }
}
