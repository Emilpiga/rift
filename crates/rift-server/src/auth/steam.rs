//! Steam Game Server SDK verifier.
//!
//! Enabled by the `steam-auth` cargo feature. Links the
//! Steamworks Game Server SDK and routes ticket validation
//! through `ISteamGameServer::BeginAuthSession`, exactly like
//! a published Source-engine server would.
//!
//! ### Wire format
//!
//! The client wraps every Steam ticket as:
//!
//! ```text
//!   offset  size  field
//!   ------  ----  -------------------------------------------
//!   0       8     SteamID64 (claimed), little-endian u64
//!   8       N     raw GetAuthSessionTicket bytes (untouched)
//! ```
//!
//! `BeginAuthSession` takes both the ticket and the SteamID, so
//! we need the claimed ID prefixed. The SDK validates that the
//! ticket actually belongs to that ID — a client can't elevate
//! by lying about which SteamID it owns.
//!
//! ### Async model
//!
//! Steamworks delivers ticket validation results via the
//! `ValidateAuthTicketResponse` callback rather than as a sync
//! return value. To bridge that into our sync `verify()` entry
//! point we:
//!
//! 1. Spawn a background thread that pumps `SingleClient::run_callbacks`
//!    every ~50 ms for the lifetime of the verifier.
//! 2. Register one global callback that looks up the pending
//!    SteamID in a shared `HashMap<SteamId, oneshot::Sender>` and
//!    forwards the result.
//! 3. `verify()` inserts a sender, calls `BeginAuthSession`,
//!    and blocks on the receiver with a 5-second timeout.
//!
//! This keeps `Verifier::verify` synchronous for the `Hello`
//! dispatcher — auth is a handshake step, not a hot path, so
//! the block is fine.
//!
//! ### Setup
//!
//! For pre-launch testing we run against **Spacewar (appid 480)**,
//! Valve's public sandbox. Anyone with a Steam account can sign
//! tickets against 480 without owning a real product. To run:
//!
//! 1. Set `RIFT_STEAM_APPID=480` (or your real appid once
//!    you've onboarded with Valve).
//! 2. Make sure Steam is running on the host (or, for a
//!    headless server, that the host can reach Valve's auth
//!    backend; we log on anonymously which works without
//!    an explicit GSLT against Spacewar).
//! 3. Place a `steam_appid.txt` containing the appid in the
//!    binary's working directory — `Server::init` reads this
//!    when no Steam client is present.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use steamworks::{
    Server, ServerMode, SingleClient, SteamId, SteamServerConnectFailure, SteamServersConnected,
    SteamServersDisconnected, ValidateAuthTicketResponse,
};

use super::{AccountKey, AuthError};

/// How long `verify()` waits for the
/// `ValidateAuthTicketResponse` callback before giving up. 5 s
/// is well beyond a healthy Steam round-trip (typically tens of
/// ms) but short enough that a wedged session blocks the
/// handshake briefly rather than indefinitely.
const VERIFY_TIMEOUT: Duration = Duration::from_secs(5);

/// Cadence of the callback-pump thread. Steam's docs recommend
/// "frequently" — 20 Hz is plenty for an auth-only server and
/// won't pin a core.
const PUMP_INTERVAL: Duration = Duration::from_millis(50);

/// Sender side of the oneshot we hand each pending verify call.
/// `Ok` carries the resolved owner SteamID (Family Sharing:
/// distinct from the playing user if the game was shared);
/// `Err` carries Steam's reason string.
type PendingSender = mpsc::SyncSender<Result<u64, String>>;

/// Steamworks-backed verifier. Owns the `Server` handle and a
/// background callback-pump thread. Cloned cheaply (the
/// shared map + the `Server` handle are both `Arc`-backed).
#[derive(Clone)]
pub struct SteamVerifier {
    server: Server,
    pending: Arc<Mutex<HashMap<u64, PendingSender>>>,
    /// Flipped to `true` by the `SteamServersConnected`
    /// callback once the anonymous logon completes. `verify()`
    /// briefly blocks on this so the first `Hello` doesn't
    /// race the logon round-trip.
    connected: Arc<std::sync::atomic::AtomicBool>,
    // Held to keep the pump thread alive; not introspected.
    #[allow(dead_code)]
    pump: Arc<PumpThread>,
}

/// RAII wrapper around the callback-pump background thread.
/// Setting `shutdown` to `true` causes the thread to exit on
/// its next tick; `Drop` joins so the verifier going out of
/// scope cleanly tears it down.
struct PumpThread {
    shutdown: Arc<std::sync::atomic::AtomicBool>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for PumpThread {
    fn drop(&mut self) {
        self.shutdown
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(h) = self.handle.lock().expect("pump mutex").take() {
            let _ = h.join();
        }
    }
}

impl SteamVerifier {
    /// Initialise the Steamworks game-server, log on anonymously,
    /// register the `ValidateAuthTicketResponse` callback, and
    /// start the background callback pump. Failure modes are all
    /// captured in the returned `String` so the startup log can
    /// say exactly what went wrong.
    pub fn from_env() -> Result<Self, String> {
        let app_id_raw = std::env::var("RIFT_STEAM_APPID")
            .map_err(|_| "RIFT_STEAM_APPID is not set".to_string())?;
        let app_id: u32 = app_id_raw
            .parse()
            .map_err(|_| format!("RIFT_STEAM_APPID ({app_id_raw:?}) is not a u32"))?;

        // Write `steam_appid.txt` so `Server::init` can find
        // the appid even without a logged-in Steam client on
        // the host. Best-effort: if the working directory is
        // read-only the init will simply fail below and we'll
        // surface that error instead.
        let _ = std::fs::write("steam_appid.txt", format!("{app_id}\n"));

        // Choose two unused ports for the (unused) server
        // browser interface. 0 lets the OS pick a free port;
        // we don't run the master-server heartbeat path so the
        // exact value doesn't matter for our use case.
        let (server, single) = Server::init(
            Ipv4Addr::new(0, 0, 0, 0),
            0, // game port
            0, // query port
            ServerMode::Authentication,
            env!("CARGO_PKG_VERSION"),
        )
        .map_err(|e| format!("Steam Server::init failed: {e:?}"))?;
        log::info!(
            "steam: Server::init OK (appid={app_id}, mode=Authentication, server_steam_id={})",
            server.steam_id().raw()
        );

        // Identify the server to the master-server backend
        // even though we're in Authentication-only mode. The
        // SDK requires `set_product` + `set_game_description`
        // BEFORE the logon call; otherwise the logon is
        // silently rejected. We use the appid as the product
        // identifier (Valve's recommended default).
        server.set_product(&format!("{app_id}"));
        server.set_game_description("Rift Crawler");
        server.set_dedicated_server(true);

        // Anonymous logon is required for ticket validation
        // (the SDK needs to talk to Steam's auth servers).
        // Even `ServerMode::Authentication` mode — which skips
        // the public server browser — still requires this
        // logon for `BeginAuthSession` callbacks to fire.
        server.log_on_anonymous();

        let pending: Arc<Mutex<HashMap<u64, PendingSender>>> = Arc::new(Mutex::new(HashMap::new()));

        // Shared "are we logged on" flag, flipped by the
        // SteamServersConnected callback. `verify()` blocks on
        // this with a short timeout so the first incoming
        // `Hello` doesn't race the anonymous-logon round-trip
        // (the original bug: BeginAuthSession returns Ok but
        // the validation callback never fires because logon
        // hasn't completed).
        let connected = Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Diagnostic callbacks. These don't affect auth flow
        // but give an operator a clear log line when Steam's
        // backend is reachable / unreachable. Critical for
        // figuring out why a ticket validation timed out:
        // "connected" + timeout means real validation failure;
        // no "connected" + timeout means the server can't talk
        // to Steam at all.
        let connected_for_cb = Arc::clone(&connected);
        std::mem::forget(server.register_callback(move |c: SteamServersConnected| {
            connected_for_cb.store(true, std::sync::atomic::Ordering::Relaxed);
            log::info!("steam: SteamServersConnected ({c:?}) — auth ready");
        }));
        let connected_for_cb = Arc::clone(&connected);
        std::mem::forget(
            server.register_callback(move |c: SteamServerConnectFailure| {
                connected_for_cb.store(false, std::sync::atomic::Ordering::Relaxed);
                log::warn!("steam: SteamServerConnectFailure ({c:?})");
            }),
        );
        let connected_for_cb = Arc::clone(&connected);
        std::mem::forget(
            server.register_callback(move |c: SteamServersDisconnected| {
                connected_for_cb.store(false, std::sync::atomic::Ordering::Relaxed);
                log::warn!("steam: SteamServersDisconnected ({c:?})");
            }),
        );

        // Register the ValidateAuthTicketResponse callback.
        // It fires on the pump thread; we just look up the
        // pending sender and forward the verdict. Hold the
        // returned handle for the lifetime of the verifier so
        // the registration isn't dropped.
        let pending_for_cb = Arc::clone(&pending);
        let _handle = server.register_callback(move |r: ValidateAuthTicketResponse| {
            let steam_id_u64 = r.steam_id.raw();
            log::debug!(
                "steam: ValidateAuthTicketResponse steam_id={steam_id_u64} owner={} response={:?}",
                r.owner_steam_id.raw(),
                r.response
            );
            let mut map = pending_for_cb.lock().expect("steam pending map");
            if let Some(tx) = map.remove(&steam_id_u64) {
                let result = match r.response {
                    Ok(()) => Ok(r.owner_steam_id.raw()),
                    Err(err) => Err(format!("{err:?}")),
                };
                // SyncSender::send can only fail if the
                // receiver already dropped (verify timed out
                // and walked away). That's fine — the answer
                // arrived too late to matter.
                let _ = tx.try_send(result);
            } else {
                log::warn!("steam: ValidateAuthTicketResponse for unknown steam_id {steam_id_u64}");
            }
        });
        // Intentionally leak the CallbackHandle: we want the
        // registration to live as long as the process so any
        // late callbacks land in the map (or are dropped on
        // the floor if the matching sender is gone).
        std::mem::forget(_handle);

        // Background pump. SingleClient is !Send across some
        // platforms; capture it by move into a thread we own,
        // and use the atomic flag to shut it down on drop.
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let shutdown_thread = Arc::clone(&shutdown);
        let handle = thread::Builder::new()
            .name("steam-cb-pump".to_string())
            .spawn(move || {
                let single: SingleClient<_> = single;
                while !shutdown_thread.load(std::sync::atomic::Ordering::Relaxed) {
                    single.run_callbacks();
                    thread::sleep(PUMP_INTERVAL);
                }
            })
            .map_err(|e| format!("failed to spawn steam-cb-pump: {e}"))?;

        Ok(Self {
            server,
            pending,
            connected,
            pump: Arc::new(PumpThread {
                shutdown,
                handle: Mutex::new(Some(handle)),
            }),
        })
    }

    /// Wait up to `timeout` for the anonymous logon to
    /// complete. Returns `true` if `SteamServersConnected`
    /// fired (so ticket validation will work), `false`
    /// otherwise. Polled at 50 ms granularity to match the
    /// callback pump cadence.
    fn wait_for_connected(&self, timeout: Duration) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            if self.connected.load(std::sync::atomic::Ordering::Relaxed) {
                return true;
            }
            thread::sleep(Duration::from_millis(50));
        }
        false
    }

    /// Validate a wire ticket and return the resolved Steam
    /// account on success.
    pub fn verify(&self, ticket: &[u8]) -> Result<AccountKey, AuthError> {
        // Block briefly on the logon. First handshake after a
        // cold start can land here a couple hundred ms before
        // SteamServersConnected fires; subsequent handshakes
        // skip the wait because the flag is already set.
        if !self.connected.load(std::sync::atomic::Ordering::Relaxed)
            && !self.wait_for_connected(Duration::from_secs(10))
        {
            return Err(AuthError::SteamRejected(
                "Steam game-server logon has not completed; ticket validation unavailable"
                    .to_string(),
            ));
        }

        // Strip the 8-byte SteamID prefix our client added.
        if ticket.len() < 8 {
            return Err(AuthError::MalformedTicket(
                "Steam ticket missing SteamID prefix".to_string(),
            ));
        }
        let mut id_bytes = [0u8; 8];
        id_bytes.copy_from_slice(&ticket[..8]);
        let claimed_id_raw = u64::from_le_bytes(id_bytes);
        let claimed_id = SteamId::from_raw(claimed_id_raw);
        let raw_ticket = &ticket[8..];

        if raw_ticket.is_empty() {
            return Err(AuthError::MalformedTicket(
                "Steam ticket body is empty".to_string(),
            ));
        }

        // Register the pending sender BEFORE calling
        // BeginAuthSession so the callback (which can fire
        // before BeginAuthSession returns on some platforms)
        // always finds an entry.
        let (tx, rx) = mpsc::sync_channel::<Result<u64, String>>(1);
        {
            let mut map = self.pending.lock().expect("steam pending map");
            // Two simultaneous handshakes for the same SteamID
            // shouldn't happen in practice (one player, one
            // session), but if they do we evict the older
            // pending sender so it doesn't hold the slot.
            if let Some(_old) = map.insert(claimed_id_raw, tx) {
                log::warn!("steam: replacing duplicate pending validation for {claimed_id_raw}");
            }
        }

        log::debug!(
            "steam: BeginAuthSession claimed_id={claimed_id_raw} ticket_len={}",
            raw_ticket.len()
        );
        if let Err(e) = self
            .server
            .begin_authentication_session(claimed_id, raw_ticket)
        {
            self.pending
                .lock()
                .expect("steam pending map")
                .remove(&claimed_id_raw);
            return Err(AuthError::SteamRejected(format!(
                "BeginAuthSession failed: {e:?}"
            )));
        }

        let outcome = rx.recv_timeout(VERIFY_TIMEOUT);

        // Whatever happened, end the auth session so the SDK
        // doesn't accumulate orphaned validations.
        self.server.end_authentication_session(claimed_id);

        match outcome {
            Ok(Ok(owner_id_raw)) => {
                if owner_id_raw != claimed_id_raw {
                    // Family Sharing: the player isn't the
                    // license owner. Currently allowed —
                    // we key the account on the *playing*
                    // user — but logged so an operator can
                    // see it happening if they later care.
                    log::info!(
                        "steam: family-shared session, playing {claimed_id_raw} owner {owner_id_raw}"
                    );
                }
                Ok(AccountKey::Steam(claimed_id_raw))
            }
            Ok(Err(reason)) => Err(AuthError::SteamRejected(reason)),
            Err(RecvTimeoutError::Timeout) => {
                self.pending
                    .lock()
                    .expect("steam pending map")
                    .remove(&claimed_id_raw);
                Err(AuthError::SteamRejected(
                    "Steam validation timed out".to_string(),
                ))
            }
            Err(RecvTimeoutError::Disconnected) => Err(AuthError::SteamRejected(
                "Steam validation channel closed".to_string(),
            )),
        }
    }
}
