//! Server-side authentication.
//!
//! Exactly one verifier is configured at startup. In a
//! production build (`--features steam-auth` + `STEAM_WEBAPI_KEY`
//! set) that's the Steam verifier, which validates the opaque
//! ticket against `ISteamUserAuth/AuthenticateUserTicket`. In a
//! dev build (`RIFT_DEV_AUTH_KEY` set) it's the Dev verifier,
//! which parses the local HMAC-signed ticket format owned by
//! [`rift_net::auth_dev`]. Dev mode is conceptually "local fake
//! Steam": same opaque-bytes wire shape, different verifier.
//!
//! Both verifiers consume `Hello.auth_ticket: Vec<u8>` and
//! return a single [`AccountKey`] type. The rest of the server
//! never branches on which verifier produced the result.

use std::sync::Arc;

pub mod dev;
#[cfg(feature = "steam-auth")]
pub mod steam;

/// Issuer-tagged account identity. The string form
/// (`steam:76561198…`, `dev:alice`) is what the persistence
/// layer stores in `accounts.account_key`, so the issuer
/// prefix guarantees a Steam player and a dev player who
/// happened to pick the same identity string can never
/// collide on the same account row.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AccountKey {
    /// Steam SteamID64 returned by the validated ticket.
    /// Currently unreachable (the Steam path is a stub); kept
    /// in the enum so dispatch + persistence are ready for the
    /// real HTTP integration.
    #[allow(dead_code)]
    Steam(u64),
    /// Dev identity string (free-form, post-validation).
    Dev(String),
}

impl AccountKey {
    /// Encode as the `issuer:identity` string the persistence
    /// layer stores in `accounts.account_key`. Stable wire /
    /// disk format — never change without a migration.
    pub fn as_storage_string(&self) -> String {
        match self {
            AccountKey::Steam(id) => format!("steam:{id}"),
            AccountKey::Dev(name) => format!("dev:{name}"),
        }
    }

    /// Human-readable display form. For Steam this is the
    /// SteamID64 today; once the Web API call lands the persona
    /// name will be threaded through here instead. For Dev it's
    /// just the identity string.
    pub fn display_name(&self) -> String {
        match self {
            AccountKey::Steam(id) => format!("steam:{id}"),
            AccountKey::Dev(name) => name.clone(),
        }
    }
}

/// Reasons a ticket verification can fail. The string payloads
/// are only logged; the user-visible message routed through
/// `Reject` lives on [`AuthError::user_message`] and is kept
/// deliberately non-leaky.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum AuthError {
    /// Ticket bytes were the wrong shape for the configured
    /// verifier (truncated, bad version byte, bad UTF-8, …).
    MalformedTicket(String),
    /// Dev: HMAC signature didn't match.
    BadSignature,
    /// Dev: `timestamp_unix` fell outside the replay window.
    StaleTimestamp,
    /// Dev: identity string failed validation (empty, too
    /// long, reserved `:` separator, control chars).
    BadIdentity(String),
    /// Dev: `(identity, nonce)` pair was already accepted
    /// inside the replay window — captured packet replay.
    ReplayDetected,
    /// Steam: Web API call rejected the ticket (or, today, the
    /// stub returns this verbatim because the HTTP integration
    /// hasn't landed).
    SteamRejected(String),
}

impl AuthError {
    /// User-visible string the server ships back in
    /// [`rift_net::ServerMsg::Reject`]. Kept short and
    /// non-leaky — we don't want to tell a malicious client
    /// *which* part of their ticket was wrong.
    pub fn user_message(&self) -> String {
        match self {
            AuthError::SteamRejected(_) => "Steam authentication failed.".to_string(),
            _ => "Authentication failed.".to_string(),
        }
    }
}

/// One configured verifier. Selected once at startup; the rest
/// of the server holds a [`Verifier`] inside [`AuthConfig`] and
/// calls [`Verifier::verify`] on every `Hello`.
///
/// `None` means the server refused to enable any verifier at
/// startup (e.g. no `RIFT_DEV_AUTH_KEY` set and the
/// `steam-auth` feature wasn't compiled in). In that state
/// every incoming `Hello` is rejected, which is the right
/// behaviour: silently accepting unverified clients would
/// undo the whole point of the auth flow.
#[derive(Clone)]
pub enum Verifier {
    /// Production path. Validates tickets against the Steam
    /// Game Server SDK (`ISteamGameServer::BeginAuthSession`).
    /// Only available when the `steam-auth` cargo feature is
    /// enabled — default builds get a dev-only `Verifier`.
    #[cfg(feature = "steam-auth")]
    Steam(steam::SteamVerifier),
    /// Local-iteration path. Parses the `rift_net::auth_dev`
    /// ticket layout, verifies the HMAC against the shared
    /// key, and tracks accepted nonces to defang single-packet
    /// replay.
    ///
    /// Compiled in all builds (the verifier's tests rely on
    /// it) but unreachable from `AuthConfig::from_env` when
    /// `steam-auth` is on — a production server forces Steam.
    #[cfg_attr(feature = "steam-auth", allow(dead_code))]
    Dev(dev::DevVerifier),
}

impl Verifier {
    /// Validate a wire ticket. Returns the resolved account
    /// identity on success.
    pub fn verify(&self, ticket: &[u8]) -> Result<AccountKey, AuthError> {
        match self {
            #[cfg(feature = "steam-auth")]
            Verifier::Steam(v) => v.verify(ticket),
            Verifier::Dev(v) => v.verify(ticket),
        }
    }

    /// Short label for log lines. Helps `cargo run` output
    /// make it obvious which verifier is active.
    pub fn label(&self) -> &'static str {
        match self {
            #[cfg(feature = "steam-auth")]
            Verifier::Steam(_) => "steam",
            Verifier::Dev(_) => "dev",
        }
    }
}

/// Server-side auth configuration. Built once at startup; held
/// on `Server` and threaded into every `Hello` resolution. The
/// inner verifier is `Arc`-wrapped so cloning the config (e.g.
/// for an async worker promotion later) is cheap.
#[derive(Clone, Default)]
pub struct AuthConfig {
    /// `None` disables auth entirely — every `Hello` is
    /// rejected with `AuthError::SteamRejected("...")`. This
    /// is the safe-by-default state when neither
    /// `RIFT_DEV_AUTH_KEY` nor the `steam-auth` feature is
    /// configured. Production servers run with `Steam`; dev
    /// loop runs with `Dev`.
    pub verifier: Option<Arc<Verifier>>,
}

impl AuthConfig {
    /// Pick a verifier based on cargo features + environment
    /// variables, logging loudly so an operator can tell which
    /// path is active.
    ///
    /// Precedence:
    /// 1. `--features steam-auth` → Steam verifier (production).
    /// 2. `RIFT_DEV_AUTH_KEY` set → Dev verifier.
    /// 3. Neither → disabled; every `Hello` is rejected.
    ///
    /// Mixing — running with both the feature on and the dev
    /// key set — picks Steam and logs a warning so a
    /// misconfigured prod box can't accept HMAC tickets by
    /// accident.
    pub fn from_env() -> Self {
        #[cfg(feature = "steam-auth")]
        {
            if std::env::var("RIFT_DEV_AUTH_KEY").is_ok() {
                log::warn!(
                    "auth: RIFT_DEV_AUTH_KEY is set but `steam-auth` feature is enabled \
                     — ignoring dev key, using Steam verifier"
                );
            }
            match steam::SteamVerifier::from_env() {
                Ok(v) => {
                    log::info!("auth: Steam verifier active");
                    return Self {
                        verifier: Some(Arc::new(Verifier::Steam(v))),
                    };
                }
                Err(e) => {
                    log::error!("auth: Steam verifier failed to initialize ({e}); auth DISABLED");
                    return Self::default();
                }
            }
        }

        #[cfg(not(feature = "steam-auth"))]
        {
            match dev::DevVerifier::from_env() {
                Ok(v) => {
                    log::warn!(
                        "auth: DEV verifier active (RIFT_DEV_AUTH_KEY set) — \
                         do not run this build against a production server"
                    );
                    Self {
                        verifier: Some(Arc::new(Verifier::Dev(v))),
                    }
                }
                Err(reason) => {
                    log::error!("auth: no verifier configured ({reason}); auth DISABLED");
                    Self::default()
                }
            }
        }
    }
}

/// Resolve a wire ticket into an account key. The single entry
/// point the dispatcher calls on every `Hello`. Returns an
/// explicit `SteamRejected` when no verifier is configured so
/// the rejection reason in the user-facing `Reject` is honest
/// rather than misleading the player into thinking their
/// credential was bad.
pub fn resolve(cfg: &AuthConfig, ticket: &[u8]) -> Result<AccountKey, AuthError> {
    let Some(verifier) = cfg.verifier.as_ref() else {
        return Err(AuthError::SteamRejected(
            "This server has no authentication verifier configured.".to_string(),
        ));
    };
    verifier.verify(ticket)
}

/// One-shot replay defence shared by every verifier that
/// accepts retryable tickets. Tracks `(identity, nonce)` pairs
/// for the duration of [`rift_net::auth_dev::DEV_AUTH_REPLAY_WINDOW_SECS`]
/// and rejects exact retries. GC on every check keeps the map
/// from growing unbounded.
#[derive(Debug, Default)]
pub struct ReplayCache {
    entries: std::collections::HashMap<(String, u64), u64>,
}

impl ReplayCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reject the credential if `(identity, nonce)` was already
    /// accepted inside the replay window; otherwise record it
    /// and return `Ok(())`.
    pub fn check_and_record(
        &mut self,
        identity: &str,
        nonce: u64,
        now: u64,
    ) -> Result<(), AuthError> {
        self.gc(now);
        if self.entries.contains_key(&(identity.to_string(), nonce)) {
            return Err(AuthError::ReplayDetected);
        }
        self.entries.insert((identity.to_string(), nonce), now);
        Ok(())
    }

    fn gc(&mut self, now: u64) {
        use rift_net::auth_dev::DEV_AUTH_REPLAY_WINDOW_SECS;
        self.entries
            .retain(|_, recorded| now.saturating_sub(*recorded) <= DEV_AUTH_REPLAY_WINDOW_SECS);
    }
}

/// Wall-clock seconds since the Unix epoch; pre-epoch clocks
/// (which would indicate a wildly broken host) become `0` so
/// the replay-window math stays sane.
pub(crate) fn unix_now() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// Manual Debug so an accidental `{cfg:?}` doesn't dump the HMAC key.
impl std::fmt::Debug for AuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthConfig")
            .field(
                "verifier",
                &self.verifier.as_ref().map(|v| v.label()).unwrap_or("none"),
            )
            .finish()
    }
}

impl std::fmt::Debug for Verifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple(self.label()).finish()
    }
}
