//! Dev verifier — "local fake Steam".
//!
//! Parses the [`rift_net::auth_dev`] ticket layout, verifies the
//! HMAC against the shared key, enforces the replay window, and
//! returns an [`AccountKey::Dev`] on success. The dev path is
//! deliberately the same shape as the production Steam path:
//! one opaque-bytes `verify` entry point that yields either a
//! resolved account identity or an [`AuthError`].

use std::sync::Mutex;

use rift_net::auth_dev::{
    decode_dev_ticket, sign, DevTicketDecodeError, DEV_AUTH_REPLAY_WINDOW_SECS,
    MAX_DEV_IDENTITY_CHARS,
};
use subtle::ConstantTimeEq;

use super::{unix_now, AccountKey, AuthError, ReplayCache};

/// Stateful dev verifier. Owns the shared HMAC key plus the
/// nonce-replay cache. `Mutex` is fine here — the verify path
/// is a millisecond-scale handshake step, not a hot game-loop
/// path.
#[cfg_attr(feature = "steam-auth", allow(dead_code))]
pub struct DevVerifier {
    key: [u8; 32],
    replay_cache: Mutex<ReplayCache>,
}

impl Clone for DevVerifier {
    fn clone(&self) -> Self {
        // We never actually clone the verifier in practice
        // (it's behind `Arc` in `AuthConfig`), but the enum
        // wrapping it derives `Clone` and that requires this
        // impl to exist. Clone shares neither cache state nor
        // identity — callers shouldn't depend on it.
        Self {
            key: self.key,
            replay_cache: Mutex::new(ReplayCache::new()),
        }
    }
}

impl DevVerifier {
    /// Pull `RIFT_DEV_AUTH_KEY` (hex-encoded 32 bytes) from the
    /// environment and build a verifier. Returns the loud
    /// error string used in the startup log if the env var is
    /// missing or malformed.
    #[cfg_attr(feature = "steam-auth", allow(dead_code))]
    pub fn from_env() -> Result<Self, String> {
        let raw = std::env::var("RIFT_DEV_AUTH_KEY")
            .map_err(|_| "RIFT_DEV_AUTH_KEY is not set; dev auth verifier disabled".to_string())?;
        let key = decode_key(&raw)
            .ok_or_else(|| "RIFT_DEV_AUTH_KEY must be 64 hex chars (32 bytes)".to_string())?;
        Ok(Self {
            key,
            replay_cache: Mutex::new(ReplayCache::new()),
        })
    }

    /// Validate a wire ticket and (on success) return the
    /// resolved dev identity.
    pub fn verify(&self, ticket: &[u8]) -> Result<AccountKey, AuthError> {
        let parsed = decode_dev_ticket(ticket).map_err(|e| match e {
            DevTicketDecodeError::Truncated => {
                AuthError::MalformedTicket("dev ticket truncated".to_string())
            }
            DevTicketDecodeError::UnknownVersion(v) => {
                AuthError::MalformedTicket(format!("dev ticket version {v} not supported"))
            }
            DevTicketDecodeError::BadIdentityUtf8 => {
                AuthError::MalformedTicket("dev ticket identity not UTF-8".to_string())
            }
            DevTicketDecodeError::BadIdentityLength => {
                AuthError::MalformedTicket("dev ticket identity length out of range".to_string())
            }
        })?;

        validate_identity(&parsed.identity)?;

        // Replay window: timestamp must be within
        // `DEV_AUTH_REPLAY_WINDOW_SECS` of server clock, on
        // either side, to survive sloppy NTP.
        let now = unix_now();
        let delta = now.abs_diff(parsed.timestamp_unix);
        if delta > DEV_AUTH_REPLAY_WINDOW_SECS {
            return Err(AuthError::StaleTimestamp);
        }

        // HMAC compare in constant time. Mismatched secrets
        // are the most common dev cause of `BadSignature`.
        let expected = sign(
            &self.key,
            &parsed.identity,
            parsed.nonce,
            parsed.timestamp_unix,
        );
        if expected.ct_eq(&parsed.signature).unwrap_u8() != 1 {
            return Err(AuthError::BadSignature);
        }

        // Single-packet replay: lock + check + record.
        // Holding the lock across the whole check is fine —
        // verify is not on a hot loop.
        let mut cache = self
            .replay_cache
            .lock()
            .expect("dev replay cache mutex poisoned");
        cache.check_and_record(&parsed.identity, parsed.nonce, now)?;

        Ok(AccountKey::Dev(parsed.identity))
    }
}

/// Reject identity strings that would either collide with the
/// `issuer:identity` storage convention or sneak control
/// characters past the player-name UI.
fn validate_identity(identity: &str) -> Result<(), AuthError> {
    if identity.is_empty() {
        return Err(AuthError::BadIdentity("identity is empty".to_string()));
    }
    if identity.chars().count() > MAX_DEV_IDENTITY_CHARS {
        return Err(AuthError::BadIdentity(format!(
            "identity exceeds {MAX_DEV_IDENTITY_CHARS} chars"
        )));
    }
    if identity.contains(':') {
        return Err(AuthError::BadIdentity(
            "identity contains reserved character ':'".to_string(),
        ));
    }
    if identity.chars().any(char::is_control) {
        return Err(AuthError::BadIdentity(
            "identity contains control characters".to_string(),
        ));
    }
    Ok(())
}

/// Decode a 64-char hex string into a 32-byte key. Accepts
/// upper or lower case; ignores nothing else.
#[cfg_attr(feature = "steam-auth", allow(dead_code))]
fn decode_key(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        let hi = hex_digit(chunk[0])?;
        let lo = hex_digit(chunk[1])?;
        out[i] = (hi << 4) | lo;
    }
    Some(out)
}

#[cfg_attr(feature = "steam-auth", allow(dead_code))]
fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rift_net::auth_dev::{encode_dev_ticket, sign};

    fn mint(key: &[u8; 32], identity: &str, nonce: u64, ts: u64) -> Vec<u8> {
        let sig = sign(key, identity, nonce, ts);
        encode_dev_ticket(identity, nonce, ts, &sig)
    }

    fn make_verifier(key: [u8; 32]) -> DevVerifier {
        DevVerifier {
            key,
            replay_cache: Mutex::new(ReplayCache::new()),
        }
    }

    #[test]
    fn accepts_valid_ticket() {
        let key = [9u8; 32];
        let v = make_verifier(key);
        let ticket = mint(&key, "alice", 1, unix_now());
        let acct = v.verify(&ticket).expect("valid ticket accepted");
        assert_eq!(acct, AccountKey::Dev("alice".to_string()));
    }

    #[test]
    fn rejects_bad_signature() {
        let v = make_verifier([1u8; 32]);
        let ticket = mint(&[2u8; 32], "alice", 1, unix_now());
        assert!(matches!(v.verify(&ticket), Err(AuthError::BadSignature)));
    }

    #[test]
    fn rejects_stale_timestamp() {
        let key = [3u8; 32];
        let v = make_verifier(key);
        let ticket = mint(&key, "alice", 1, unix_now() - 10_000);
        assert!(matches!(v.verify(&ticket), Err(AuthError::StaleTimestamp)));
    }

    #[test]
    fn rejects_replay_within_window() {
        let key = [4u8; 32];
        let v = make_verifier(key);
        let ticket = mint(&key, "alice", 7, unix_now());
        v.verify(&ticket).expect("first accept");
        assert!(matches!(v.verify(&ticket), Err(AuthError::ReplayDetected)));
    }

    #[test]
    fn rejects_issuer_prefix_identity() {
        let key = [5u8; 32];
        let v = make_verifier(key);
        let ticket = mint(&key, "steam:bob", 1, unix_now());
        assert!(matches!(v.verify(&ticket), Err(AuthError::BadIdentity(_))));
    }

    #[test]
    fn rejects_empty_identity() {
        let key = [6u8; 32];
        let v = make_verifier(key);
        let ticket = mint(&key, "", 1, unix_now());
        assert!(matches!(v.verify(&ticket), Err(AuthError::BadIdentity(_))));
    }

    #[test]
    fn rejects_oversized_identity() {
        let key = [7u8; 32];
        let v = make_verifier(key);
        let too_long: String = "a".repeat(MAX_DEV_IDENTITY_CHARS + 1);
        let ticket = mint(&key, &too_long, 1, unix_now());
        assert!(matches!(v.verify(&ticket), Err(AuthError::BadIdentity(_))));
    }

    #[test]
    fn rejects_malformed_ticket() {
        let v = make_verifier([0u8; 32]);
        let mut ticket = mint(&[0u8; 32], "x", 1, unix_now());
        ticket.truncate(10);
        assert!(matches!(
            v.verify(&ticket),
            Err(AuthError::MalformedTicket(_))
        ));
    }

    #[test]
    fn decode_key_round_trip() {
        let expected = [0xab; 32];
        let hex = "ab".repeat(32);
        assert_eq!(decode_key(&hex), Some(expected));
    }

    #[test]
    fn decode_key_rejects_wrong_length() {
        assert!(decode_key("abcd").is_none());
        assert!(decode_key(&"a".repeat(63)).is_none());
    }
}
