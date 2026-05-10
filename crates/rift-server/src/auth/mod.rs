//! Authentication for the dedicated server.
//!
//! Two issuers are supported: **Steam** (production) and **Dev**
//! (local iteration / playtest). Both pipe through a single
//! [`resolve`] entry point that turns a wire-level
//! [`AuthCredential`] into an [`AccountKey`] — the issuer-tagged
//! identifier the rest of the server uses for persistence keys,
//! party invites, log lines, etc.
//!
//! The Phase-2 implementation lands the real verification logic
//! (HMAC-SHA256 for dev, Steam Web API stub for steam) and
//! keeps the call site synchronous. The Phase-3 promotion onto
//! a background task / mpsc reply channel is **deferred** until
//! a concrete async issuer lands: HMAC verification is
//! microseconds and the Steam path is a stub today, so spinning
//! up a worker thread now would add complexity without payoff.
//! When the real Steam Web API integration arrives (`reqwest` /
//! `ureq` HTTP call out of `verify_ticket`), promote `resolve`
//! to push the request onto a `mpsc::Sender<AuthRequest>` and
//! poll the result channel from `Server::step` — the existing
//! `handle_hello` callsite is structured so the swap is local.

use rift_net::AuthCredential;

pub mod dev;
pub mod steam;

/// Issuer-tagged account identity. The string form (`steam:1234`,
/// `dev:alice`) is what the persistence layer keys character
/// rows on, so the issuer prefix means a Steam player and a
/// dev player who happened to pick the same identity string
/// can never collide on the same account row.
///
/// `Steam` is constructed only from the steam stub today;
/// silenced until the Steam Web API integration lands.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[allow(dead_code)]
pub enum AccountKey {
    /// Steam SteamID64 as returned by `AuthenticateUserTicket`.
    Steam(u64),
    /// Dev identity string (free-form, post-validation).
    Dev(String),
}

impl AccountKey {
    /// Encode as the `issuer:identity` string the persistence
    /// layer stores in `accounts.account_key`. Stable wire /
    /// disk format — never change without a migration. Used
    /// by the Phase-4 migration; allowed to be unused until
    /// then.
    #[allow(dead_code)]
    pub fn as_storage_string(&self) -> String {
        match self {
            AccountKey::Steam(id) => format!("steam:{id}"),
            AccountKey::Dev(name) => format!("dev:{name}"),
        }
    }

    /// Human-readable display form. For Steam this is
    /// currently the SteamID64; once Steam Web API integration
    /// lands the persona name will be threaded through here
    /// instead. For Dev it's just the identity string.
    pub fn display_name(&self) -> String {
        match self {
            AccountKey::Steam(id) => format!("steam:{id}"),
            AccountKey::Dev(name) => name.clone(),
        }
    }

    /// The bare identity slice the legacy `account_name`-keyed
    /// persistence path expects. Dropped once Phase 4 migrates
    /// the storage layer to `account_key`.
    pub fn legacy_account_name(&self) -> String {
        self.display_name()
    }
}

/// Reasons an auth resolution can fail. Each variant carries a
/// short user-facing message because the client surfaces the
/// rejection verbatim and exits.
///
/// The string payloads on `BadIdentity` and `SteamRejected`
/// only flow through `Debug` log lines today, which dead-code
/// analysis can't see — silenced explicitly so the warning
/// doesn't drown out real ones.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum AuthError {
    /// Server has no `RIFT_DEV_AUTH_KEY` configured but the
    /// client presented a [`AuthCredential::Dev`]. Production
    /// servers should always hit this for dev credentials.
    DevAuthDisabled,
    /// Dev credential's HMAC signature did not match. Either a
    /// wrong key, a tampered payload, or a key rotation the
    /// client hasn't picked up yet.
    BadSignature,
    /// Dev credential's `timestamp_unix` was outside the
    /// `±DEV_AUTH_REPLAY_WINDOW_SECS` window. Stops captured
    /// credentials from being replayed indefinitely.
    StaleTimestamp,
    /// Identity string was empty, too long, or contained
    /// control characters.
    BadIdentity(String),
    /// Steam ticket validation failed (network error, invalid
    /// ticket, or the Phase-2 stub refusing because the
    /// `steam-auth` feature isn't compiled in).
    SteamRejected(String),
}

impl AuthError {
    /// User-visible string the server ships back in
    /// [`rift_net::ServerMsg::Reject`]. Kept short and
    /// non-leaky — we don't want to tell a malicious client
    /// *which* part of their credential was wrong.
    pub fn user_message(&self) -> String {
        match self {
            AuthError::DevAuthDisabled => {
                "This server is not configured to accept dev credentials.".to_string()
            }
            AuthError::BadSignature | AuthError::StaleTimestamp | AuthError::BadIdentity(_) => {
                "Authentication failed.".to_string()
            }
            AuthError::SteamRejected(_) => "Steam authentication failed.".to_string(),
        }
    }
}

/// Server-side auth configuration. Built once at startup from
/// environment variables; threaded into [`resolve`] on every
/// `Hello`. Cheap to clone — the dev key is 32 bytes.
#[derive(Clone, Debug, Default)]
pub struct AuthConfig {
    /// Optional shared HMAC key for dev auth, hex-decoded from
    /// `RIFT_DEV_AUTH_KEY`. `None` disables the entire dev
    /// auth path: any `AuthCredential::Dev` is rejected with
    /// [`AuthError::DevAuthDisabled`]. Production servers
    /// should always leave this unset.
    pub dev_key: Option<[u8; 32]>,
}

impl AuthConfig {
    /// Read configuration from environment variables. Logs
    /// loudly when dev auth is enabled — operators should
    /// never see this on a production server.
    pub fn from_env() -> Self {
        let dev_key = match std::env::var("RIFT_DEV_AUTH_KEY") {
            Ok(hex) => match dev::decode_key(&hex) {
                Ok(k) => {
                    log::warn!(
                        "auth: DEV credential issuer ENABLED via RIFT_DEV_AUTH_KEY \
                         (do not set this on a production server)"
                    );
                    Some(k)
                }
                Err(e) => {
                    log::error!(
                        "auth: RIFT_DEV_AUTH_KEY is set but malformed ({e}); \
                         dev auth remains DISABLED"
                    );
                    None
                }
            },
            Err(_) => None,
        };
        Self { dev_key }
    }
}

/// Resolve a wire credential into an account key. Synchronous
/// today; see the module docs for the deferred Phase-3
/// promotion onto a background task once Steam HTTP integration
/// makes async actually pay for itself.
pub fn resolve(cfg: &AuthConfig, credential: &AuthCredential) -> Result<AccountKey, AuthError> {
    match credential {
        AuthCredential::Dev {
            identity,
            nonce,
            timestamp_unix,
            signature,
        } => {
            let Some(key) = cfg.dev_key.as_ref() else {
                return Err(AuthError::DevAuthDisabled);
            };
            dev::verify(key, identity, *nonce, *timestamp_unix, signature)
        }
        AuthCredential::Steam { steam_id, ticket } => steam::verify_ticket(*steam_id, ticket),
    }
}
