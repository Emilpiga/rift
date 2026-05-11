//! Steam-issuer signer.
//!
//! Initialises Steamworks in client mode, fetches a fresh
//! `ISteamUser::GetAuthSessionTicket` on every `mint()`, and
//! prefixes the resulting bytes with the local user's
//! SteamID64 so the server's `BeginAuthSession` call has both
//! pieces it needs.
//!
//! Background callback pump: similar story to the server side
//! — Steamworks delivers events on whichever thread calls
//! `SingleClient::run_callbacks`, so we spawn one pump thread
//! at signer construction to keep the SDK happy for the
//! process lifetime.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use steamworks::networking_types::NetworkingIdentity;
use steamworks::{Client, ClientManager, SingleClient};

/// Cadence of the callback-pump thread. Mirrors the server
/// side; auth is the only Steam interaction we care about
/// today so we don't need a tighter pump.
const PUMP_INTERVAL: Duration = Duration::from_millis(50);

/// Steamworks-backed signer. Holds the `Client` handle for
/// `GetAuthSessionTicket` calls; the pump thread keeps the
/// SDK callback queue draining.
#[derive(Clone)]
pub struct SteamSigner {
    client: Client<ClientManager>,
    // RAII handle: dropping the signer cleanly shuts down the
    // callback pump.
    #[allow(dead_code)]
    pump: Arc<PumpThread>,
}

struct PumpThread {
    shutdown: Arc<std::sync::atomic::AtomicBool>,
    handle: std::sync::Mutex<Option<thread::JoinHandle<()>>>,
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

impl std::fmt::Debug for SteamSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SteamSigner")
            .field("steam_id", &self.client.user().steam_id().raw())
            .finish()
    }
}

impl SteamSigner {
    /// Initialise Steamworks in client mode against the appid
    /// in `RIFT_STEAM_APPID` (or `480` / Spacewar when unset,
    /// for sandbox testing). Returns a user-facing reason on
    /// failure so the binary can surface it before exiting.
    pub fn from_env() -> Result<Self, String> {
        let app_id: u32 = std::env::var("RIFT_STEAM_APPID")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(480);

        // Write `steam_appid.txt` so Steamworks finds the
        // appid even when the binary wasn't launched via the
        // Steam client. Best-effort; if it fails the init
        // below will surface the underlying error.
        let _ = std::fs::write("steam_appid.txt", format!("{app_id}\n"));

        let (client, single) = Client::init_app(app_id)
            .map_err(|e| format!("Steam Client::init_app({app_id}) failed: {e:?}"))?;

        // Spawn the callback pump. SingleClient is moved into
        // the thread; the atomic flag drives shutdown.
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let shutdown_thread = Arc::clone(&shutdown);
        let handle = thread::Builder::new()
            .name("steam-client-cb-pump".to_string())
            .spawn(move || {
                let single: SingleClient<_> = single;
                while !shutdown_thread.load(std::sync::atomic::Ordering::Relaxed) {
                    single.run_callbacks();
                    thread::sleep(PUMP_INTERVAL);
                }
            })
            .map_err(|e| format!("failed to spawn steam-client-cb-pump: {e}"))?;

        log::info!(
            "auth: Steam signer active (appid={app_id}, steam_id={})",
            client.user().steam_id().raw()
        );

        Ok(Self {
            client,
            pump: Arc::new(PumpThread {
                shutdown,
                handle: std::sync::Mutex::new(Some(handle)),
            }),
        })
    }

    /// Best-effort identity hint for "logged in as …" UI.
    pub fn identity(&self) -> String {
        format!("steam:{}", self.client.user().steam_id().raw())
    }

    /// Mint a fresh wire ticket: 8-byte SteamID64 prefix +
    /// the raw `GetAuthSessionTicket` bytes the server feeds
    /// straight into `BeginAuthSession`.
    pub fn mint(&self) -> Vec<u8> {
        let steam_id = self.client.user().steam_id();
        // `authentication_session_ticket` takes a
        // `NetworkingIdentity` identifying the validator. For
        // a dedicated game server we don't have a stable
        // identity to bind to (servers come and go), so we
        // pass the default empty identity — matches the
        // legacy `GetAuthSessionTicket` semantics the server's
        // `BeginAuthSession` validates against.
        let identity = NetworkingIdentity::new();
        let (_ticket_handle, raw) = self.client.user().authentication_session_ticket(identity);
        let mut buf = Vec::with_capacity(8 + raw.len());
        buf.extend_from_slice(&steam_id.raw().to_le_bytes());
        buf.extend_from_slice(&raw);
        buf
    }
}
