//! Steam ticket verification.
//!
//! Phase 2 ships a stub: the `steam-auth` cargo feature gates
//! the call site, but neither path actually talks to the Steam
//! Web API yet. The real implementation will POST the
//! hex-encoded ticket to
//! `ISteamUserAuth/AuthenticateUserTicket` and trust the
//! returned `steamid` over the client-supplied hint.
//!
//! Until then both code paths return a deliberate
//! [`AuthError::SteamRejected`] so the production server can
//! be built end-to-end without accidentally accepting unsigned
//! Steam credentials.

use super::{AccountKey, AuthError};

/// Validate a Steam session ticket. Currently a stub — see
/// the module docs.
pub fn verify_ticket(_steam_id: u64, _ticket: &[u8]) -> Result<AccountKey, AuthError> {
    #[cfg(feature = "steam-auth")]
    {
        // TODO(steam): POST `_ticket` to
        // ISteamUserAuth/AuthenticateUserTicket using
        // STEAM_WEBAPI_KEY, parse the response, and return
        // `AccountKey::Steam(returned_steam_id)`. Until then
        // we fall through to the same stub error so an
        // operator who flips the feature on without finishing
        // the integration gets a loud failure rather than a
        // silently-bypassed auth.
        return Err(AuthError::SteamRejected(
            "Steam Web API integration is not yet implemented (stub).".to_string(),
        ));
    }
    #[cfg(not(feature = "steam-auth"))]
    Err(AuthError::SteamRejected(
        "Steam authentication is not enabled in this server build.".to_string(),
    ))
}
