//! Client-side authentication.
//!
//! Mirrors the server's `auth` module: there are two issuers
//! (Steam and Dev), both produce a wire-level
//! [`rift_net::AuthCredential`] the net client ships in
//! `Hello`. The client picks an issuer at startup based on the
//! environment + cargo features:
//!
//! * **Dev** — when `RIFT_DEV_AUTH_KEY` is set and the
//!   `steam-auth` cargo feature is **off**. Reads
//!   `RIFT_DEV_USER` for an explicit identity, falling back to
//!   a per-process random `dev-XXXXXX` so several local
//!   clients can be "logged in" simultaneously.
//! * **Steam** — when the `steam-auth` cargo feature is on.
//!   Currently a stub; future work fetches the session ticket
//!   from Steamworks and ships it untouched.
//! * **Disabled** — neither path produces credentials. The net
//!   client cannot sign `Hello`; the binary should exit with a
//!   "no auth issuer configured" message instead of starting
//!   netcode.
//!
//! All variants are wire-compatible — the server cares only
//! about the resolved [`AuthCredential`], not which build
//! produced it.

pub mod dev;
pub mod steam;

use rift_net::AuthCredential;

/// Resolver capable of minting fresh
/// [`AuthCredential`]s for outgoing `Hello` messages.
///
/// One instance per process, built at startup from
/// [`Config::from_env`]. The net client clones the contained
/// signer cheaply (the dev key is 32 bytes, the identity is a
/// short string).
#[derive(Clone, Debug)]
pub enum Signer {
    /// Dev signer with the shared HMAC key + the identity this
    /// client logs in as. `mint()` produces a fresh credential
    /// with the current wall-clock timestamp + a random nonce
    /// every call so two `Hello`s from the same client are
    /// never byte-identical.
    Dev(dev::DevSigner),
    /// Steam path — currently a stub that returns a
    /// placeholder credential the server's stub will reject.
    /// Real implementation calls into Steamworks.
    Steam,
}

impl Signer {
    /// Mint a credential to ship in the next `Hello`.
    pub fn mint(&self) -> AuthCredential {
        match self {
            Signer::Dev(d) => d.mint(),
            Signer::Steam => steam::mint_stub(),
        }
    }

    /// Best-effort human-readable identity this signer logs
    /// in as. Used for "logged in as …" UI strings; the server
    /// is the authority on the post-auth display name.
    pub fn identity_hint(&self) -> String {
        match self {
            Signer::Dev(d) => d.identity().to_string(),
            Signer::Steam => "steam".to_string(),
        }
    }
}

/// Process-wide auth configuration. Built once at startup from
/// environment + cargo features. Cheap to clone.
#[derive(Clone, Debug)]
pub struct Config {
    /// `Some` if the build has a working auth issuer; the net
    /// client uses this to sign every outgoing `Hello`. `None`
    /// disables network play entirely (the binary should fail
    /// loud at startup).
    pub signer: Option<Signer>,
    /// User-facing reason the signer is disabled, when
    /// `signer.is_none()`. Surfaced in the startup error so a
    /// player knows whether to set an env var or rebuild with
    /// the steam feature.
    pub disabled_reason: Option<String>,
}

impl Config {
    /// Pick an issuer based on environment + cargo features.
    /// Logs the choice (and the random dev identity, when
    /// applicable) so launching multiple clients is easy to
    /// trace in the same terminal.
    pub fn from_env() -> Self {
        // Steam takes precedence when its feature is compiled
        // in — production clients should never accidentally
        // fall back to dev auth.
        #[cfg(feature = "steam-auth")]
        {
            log::info!("auth: Steam issuer selected (cargo feature `steam-auth` enabled)");
            return Config {
                signer: Some(Signer::Steam),
                disabled_reason: None,
            };
        }

        #[cfg(not(feature = "steam-auth"))]
        {
            match dev::DevSigner::from_env() {
                Ok(signer) => {
                    log::warn!(
                        "auth: DEV issuer selected (RIFT_DEV_AUTH_KEY set, identity={:?}); \
                         do not use this build against a production server",
                        signer.identity()
                    );
                    Config {
                        signer: Some(Signer::Dev(signer)),
                        disabled_reason: None,
                    }
                }
                Err(reason) => {
                    log::error!("auth: no issuer configured ({reason})");
                    Config {
                        signer: None,
                        disabled_reason: Some(reason),
                    }
                }
            }
        }
    }
}
