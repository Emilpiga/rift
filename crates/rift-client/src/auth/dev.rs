//! Dev-issuer signer. Builds a fresh
//! [`AuthCredential::Dev`] on demand from a shared HMAC key
//! plus a per-process identity.
//!
//! Identity selection rule (first match wins):
//!
//! 1. `$RIFT_DEV_USER` if set and non-empty (after trimming).
//!    Lets a developer pin a stable identity for save-data
//!    continuity across runs.
//! 2. A randomized `dev-XXXXXX` (six lowercase hex chars)
//!    chosen once at signer construction. Two clients launched
//!    on the same machine without `RIFT_DEV_USER` therefore
//!    end up on different accounts, which is the whole point
//!    of dev auth.

use std::time::{SystemTime, UNIX_EPOCH};

use rift_net::auth_dev::{sign, MAX_DEV_IDENTITY_CHARS};
use rift_net::AuthCredential;

/// Read the shared HMAC key + identity from environment and
/// build a dev signer. Returns a user-facing reason on failure
/// so the binary can print it before exiting.
#[derive(Clone)]
pub struct DevSigner {
    /// Shared HMAC-SHA256 key (32 bytes). Decoded once from
    /// `RIFT_DEV_AUTH_KEY`.
    key: [u8; 32],
    /// Identity this client logs in as. Either
    /// `RIFT_DEV_USER` or a randomized `dev-XXXXXX`.
    identity: String,
}

impl std::fmt::Debug for DevSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never log the key — it's a credential.
        f.debug_struct("DevSigner")
            .field("identity", &self.identity)
            .field("key", &"<redacted 32 bytes>")
            .finish()
    }
}

impl DevSigner {
    /// Try to construct from environment variables. On failure
    /// returns a short user-facing reason (the binary surfaces
    /// it verbatim before exiting).
    pub fn from_env() -> Result<Self, String> {
        let raw = std::env::var("RIFT_DEV_AUTH_KEY").map_err(|_| {
            "RIFT_DEV_AUTH_KEY is not set (no auth issuer is enabled in this build)".to_string()
        })?;
        let key = decode_key(raw.trim())?;
        let identity = pick_identity();
        Ok(Self { key, identity })
    }

    /// Identity this signer logs in as.
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// Build a fresh credential to ship in the next `Hello`.
    /// Stamps the current wall-clock + a random nonce so back-
    /// to-back `Hello`s never produce a byte-identical
    /// payload (which would make replay-detection harder for
    /// any future intermediary).
    pub fn mint(&self) -> AuthCredential {
        let nonce = random_u64();
        let timestamp_unix = unix_now();
        let signature = sign(&self.key, &self.identity, nonce, timestamp_unix);
        AuthCredential::Dev {
            identity: self.identity.clone(),
            nonce,
            timestamp_unix,
            signature,
        }
    }
}

/// Decode a 64-char hex string into the 32-byte key. Mirrors
/// the server-side decoder so a bad key fails the same way on
/// either end.
fn decode_key(hex_str: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(hex_str).map_err(|e| {
        format!("RIFT_DEV_AUTH_KEY is not valid hex ({e}); expected 64 hex chars")
    })?;
    if bytes.len() != 32 {
        return Err(format!(
            "RIFT_DEV_AUTH_KEY decoded to {} bytes; expected 32 (64 hex chars)",
            bytes.len()
        ));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// Pick the identity string this signer will use. Prefers
/// `RIFT_DEV_USER` when set; otherwise mints a `dev-XXXXXX`
/// once per process. Caps to [`MAX_DEV_IDENTITY_CHARS`] so
/// the server-side validator doesn't reject us.
fn pick_identity() -> String {
    if let Ok(raw) = std::env::var("RIFT_DEV_USER") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            // Truncate to the server's max so the round-trip
            // doesn't reject us. Iterate by char (Unicode
            // scalar) rather than byte to avoid splitting
            // multibyte characters in the middle.
            let truncated: String = trimmed.chars().take(MAX_DEV_IDENTITY_CHARS).collect();
            return truncated;
        }
    }
    // Six hex chars = 16.7 M possibilities — collision-free
    // for any realistic single-machine playtest. The `dev-`
    // prefix makes randomly-minted identities easy to spot in
    // logs vs. an explicit `RIFT_DEV_USER`.
    let suffix = random_u64() & 0x00FF_FFFF; // 24 bits → exactly 6 hex chars
    format!("dev-{suffix:06x}")
}

/// Wall-clock seconds since the Unix epoch.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Cheap non-cryptographic u64 from process state. Used for
/// the dev identity suffix and the per-Hello nonce — both
/// have a "must differ between processes / between calls"
/// requirement, not a cryptographic-strength one.
///
/// We avoid pulling `rand` for one helper. The mix combines a
/// strictly-monotonic-per-call counter with sub-microsecond
/// time and the process id, then runs SplitMix64 over the
/// concatenation.
fn random_u64() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let pid = std::process::id() as u64;
    splitmix64(nanos ^ counter.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ pid.rotate_left(31))
}

/// SplitMix64 — the constant-time mixer used to seed PRNGs in
/// the standard library. Strong enough for our identity /
/// nonce needs and dependency-free.
fn splitmix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_key_round_trips() {
        let raw: [u8; 32] = std::array::from_fn(|i| i as u8);
        let s = hex::encode(raw);
        assert_eq!(decode_key(&s).unwrap(), raw);
    }

    #[test]
    fn random_identity_is_six_hex_chars() {
        // Force the no-RIFT_DEV_USER path. We can't reliably
        // unset env vars in a test (parallel runs share the
        // env), so just assert pick_identity always returns
        // either an explicit override or our `dev-XXXXXX`
        // shape.
        let id = pick_identity();
        if !id.starts_with("dev-") {
            // RIFT_DEV_USER is set in this test run; nothing
            // to assert about its value beyond it being non-
            // empty.
            assert!(!id.is_empty());
            return;
        }
        assert_eq!(id.len(), 4 + 6);
        assert!(id["dev-".len()..]
            .chars()
            .all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn random_u64_varies() {
        // Two consecutive calls should differ — the counter
        // alone guarantees this even when wall clock has the
        // same nanosecond reading.
        let a = random_u64();
        let b = random_u64();
        assert_ne!(a, b);
    }
}
