//! Steam-issuer signer. Stub today: returns a placeholder
//! credential the server's stub will reject. The real
//! implementation will fetch a session ticket from Steamworks
//! at signer construction (or per-mint, depending on ticket
//! lifetime) and ship the bytes untouched.

use rift_net::AuthCredential;

/// Build a placeholder Steam credential. Compiled regardless
/// of the `steam-auth` feature so non-Steam test paths can
/// still talk about the variant; gated on the feature so
/// production builds can reject the path with a build error
/// once the integration lands.
pub(super) fn mint_stub() -> AuthCredential {
    AuthCredential::Steam {
        steam_id: 0,
        ticket: Vec::new(),
    }
}
